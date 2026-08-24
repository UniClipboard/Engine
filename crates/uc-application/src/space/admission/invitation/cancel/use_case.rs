use std::sync::Arc;

use tracing::info;

use super::CancelInvitationError;
use crate::space::admission::invitation::InMemoryPairingInvitationHolder;

/// 清除当前全部待处理配对邀请；没有邀请时返回明确冲突。
pub(crate) struct CancelPairingInvitationUseCase {
    invitation_holder: Arc<InMemoryPairingInvitationHolder>,
}

impl CancelPairingInvitationUseCase {
    pub(crate) fn new(invitation_holder: Arc<InMemoryPairingInvitationHolder>) -> Self {
        Self { invitation_holder }
    }

    pub(crate) async fn execute(&self) -> Result<(), CancelInvitationError> {
        let removed = self.invitation_holder.cancel_all().await;
        if removed == 0 {
            return Err(CancelInvitationError::NotIssued);
        }
        info!(count = removed, "cancelled in-flight pairing invitations");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone, Utc};
    use uc_core::ids::DeviceId;
    use uc_core::pairing::invitation::PairingInvitation;
    use uc_core::pairing::InvitationCode;

    use super::*;

    fn pending_invitation(code: &str) -> PairingInvitation {
        let issued_at = Utc.with_ymd_and_hms(2026, 8, 23, 10, 0, 0).unwrap();
        let (invitation, _) = PairingInvitation::issue(
            InvitationCode::new(code),
            issued_at,
            issued_at + Duration::minutes(5),
            DeviceId::new("device-a"),
            0,
        );
        invitation
    }

    #[tokio::test]
    async fn no_pending_invitation_returns_not_issued() {
        let use_case =
            CancelPairingInvitationUseCase::new(Arc::new(InMemoryPairingInvitationHolder::new()));

        let error = use_case.execute().await.unwrap_err();

        assert!(matches!(error, CancelInvitationError::NotIssued));
    }

    #[tokio::test]
    async fn all_pending_invitations_are_cancelled() {
        let holder = Arc::new(InMemoryPairingInvitationHolder::new());
        holder.insert(pending_invitation("FIRST")).await;
        holder.insert(pending_invitation("SECOND")).await;
        let use_case = CancelPairingInvitationUseCase::new(Arc::clone(&holder));

        use_case.execute().await.unwrap();

        assert_eq!(holder.len().await, 0);
    }
}
