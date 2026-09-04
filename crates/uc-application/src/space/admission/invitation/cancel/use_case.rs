use std::sync::Arc;

use tracing::info;
use uc_core::ports::{ConsumeInvitationError, PairingInvitationPort};

use super::CancelInvitationError;
use crate::space::admission::invitation::InMemoryPairingInvitationHolder;

/// 清除当前全部待处理配对邀请；没有邀请时返回明确冲突。
pub(crate) struct CancelPairingInvitationUseCase {
    invitation_holder: Arc<InMemoryPairingInvitationHolder>,
    pairing_invitation: Arc<dyn PairingInvitationPort>,
}

impl CancelPairingInvitationUseCase {
    pub(crate) fn new(
        invitation_holder: Arc<InMemoryPairingInvitationHolder>,
        pairing_invitation: Arc<dyn PairingInvitationPort>,
    ) -> Self {
        Self {
            invitation_holder,
            pairing_invitation,
        }
    }

    pub(crate) async fn execute(&self) -> Result<(), CancelInvitationError> {
        let codes = self.invitation_holder.pending_codes().await;
        if codes.is_empty() {
            return Err(CancelInvitationError::NotIssued);
        }
        for code in &codes {
            match self.pairing_invitation.consume_invitation(code).await {
                Ok(())
                | Err(ConsumeInvitationError::NotFound)
                | Err(ConsumeInvitationError::Expired) => {}
                Err(error @ ConsumeInvitationError::ServiceUnavailable) => {
                    return Err(CancelInvitationError::unavailable(error));
                }
                Err(error @ ConsumeInvitationError::Internal(_)) => {
                    return Err(CancelInvitationError::internal(error));
                }
            }
        }
        let removed = self.invitation_holder.cancel_all().await;
        info!(count = removed, "cancelled in-flight pairing invitations");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use chrono::{Duration, TimeZone, Utc};
    use uc_core::ids::DeviceId;
    use uc_core::pairing::invitation::PairingInvitation;
    use uc_core::pairing::InvitationCode;
    use uc_core::ports::{
        ConsumeInvitationError, InvitationError, IssuedInvitation, PairingInvitationPort,
    };

    use super::*;

    #[derive(Default)]
    struct RecordingInvitationPort {
        consumed: Mutex<Vec<String>>,
        next_error: Mutex<Option<ConsumeInvitationError>>,
    }

    #[async_trait]
    impl PairingInvitationPort for RecordingInvitationPort {
        async fn issue_invitation(&self) -> Result<IssuedInvitation, InvitationError> {
            unreachable!("cancel tests never issue invitations")
        }

        async fn consume_invitation(
            &self,
            code: &InvitationCode,
        ) -> Result<(), ConsumeInvitationError> {
            self.consumed.lock().unwrap().push(code.as_str().to_owned());
            match self.next_error.lock().unwrap().take() {
                Some(error) => Err(error),
                None => Ok(()),
            }
        }
    }

    fn pending_invitation(code: &str) -> PairingInvitation {
        let issued_at = Utc.with_ymd_and_hms(2026, 8, 23, 10, 0, 0).unwrap();
        let invitation_byte = code.as_bytes().first().copied().unwrap_or(1);
        let (invitation, _) = PairingInvitation::issue(
            uc_core::membership::InvitationId::from_bytes([invitation_byte; 32])
                .expect("valid invitation id"),
            InvitationCode::new(code),
            uc_core::pairing::invitation::FullInvitation::new(format!("ucspace1_{code}"))
                .expect("valid full invitation"),
            issued_at,
            issued_at + Duration::minutes(5),
            DeviceId::new("device-a"),
            0,
        );
        invitation
    }

    #[tokio::test]
    async fn no_pending_invitation_returns_not_issued() {
        let use_case = CancelPairingInvitationUseCase::new(
            Arc::new(InMemoryPairingInvitationHolder::new()),
            Arc::new(RecordingInvitationPort::default()),
        );

        let error = use_case.execute().await.unwrap_err();

        assert!(matches!(error, CancelInvitationError::NotIssued));
    }

    #[tokio::test]
    async fn all_pending_invitations_are_cancelled() {
        let holder = Arc::new(InMemoryPairingInvitationHolder::new());
        holder.insert(pending_invitation("FIRST")).await;
        holder.insert(pending_invitation("SECOND")).await;
        let port = Arc::new(RecordingInvitationPort::default());
        let use_case = CancelPairingInvitationUseCase::new(Arc::clone(&holder), port.clone());

        use_case.execute().await.unwrap();

        assert_eq!(holder.len().await, 0);
        let mut consumed = port.consumed.lock().unwrap().clone();
        consumed.sort();
        assert_eq!(consumed, ["FIRST", "SECOND"]);
    }

    #[tokio::test]
    async fn discovery_failure_keeps_the_invitation_available_for_retry() {
        let holder = Arc::new(InMemoryPairingInvitationHolder::new());
        holder.insert(pending_invitation("RETRY")).await;
        let port = Arc::new(RecordingInvitationPort {
            consumed: Mutex::new(Vec::new()),
            next_error: Mutex::new(Some(ConsumeInvitationError::ServiceUnavailable)),
        });
        let use_case = CancelPairingInvitationUseCase::new(Arc::clone(&holder), port);

        let error = use_case.execute().await.unwrap_err();

        assert!(matches!(error, CancelInvitationError::Unavailable { .. }));
        assert!(std::error::Error::source(&error).is_some());
        assert_eq!(holder.len().await, 1);
    }
}
