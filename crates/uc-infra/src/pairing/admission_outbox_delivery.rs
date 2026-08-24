use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::timeout;
use uc_application::deps::{
    AdmissionOutboxDeliveryError, AdmissionOutboxDeliveryPort, AdmissionOutboxDeliveryResultV1,
    AdmissionOutboxDeliveryRouteV1,
};
use uc_core::membership::{AdmissionAttemptId, AdmissionOutboxMessageV1, AdmissionOutboxPurposeV1};
use uc_core::pairing::{
    DurableAdmissionFrame, DurableAdmissionMessageKind, InvitationCode, PairingSessionMessage,
};
use uc_core::ports::pairing::PairingSessionPort;

pub struct PairingAdmissionOutboxDelivery {
    sessions: Arc<dyn PairingSessionPort>,
    response_timeout: Duration,
}

impl PairingAdmissionOutboxDelivery {
    pub fn new(sessions: Arc<dyn PairingSessionPort>, response_timeout: Duration) -> Self {
        Self {
            sessions,
            response_timeout,
        }
    }
}

#[async_trait]
impl AdmissionOutboxDeliveryPort for PairingAdmissionOutboxDelivery {
    async fn deliver(
        &self,
        attempt_id: AdmissionAttemptId,
        message: &AdmissionOutboxMessageV1,
        route: Option<&AdmissionOutboxDeliveryRouteV1>,
    ) -> Result<AdmissionOutboxDeliveryResultV1, AdmissionOutboxDeliveryError> {
        if message.purpose != AdmissionOutboxPurposeV1::CancelRequested {
            return Ok(AdmissionOutboxDeliveryResultV1::Deferred);
        }
        let session = match route {
            Some(AdmissionOutboxDeliveryRouteV1::Continuation(address)) => self
                .sessions
                .dial_admission_continuation(address)
                .await
                .map_err(|_| AdmissionOutboxDeliveryError)?,
            Some(AdmissionOutboxDeliveryRouteV1::Invitation(code)) => {
                let invitation = std::str::from_utf8(code)
                    .map(InvitationCode::new)
                    .map_err(|_| AdmissionOutboxDeliveryError)?;
                self.sessions
                    .dial_by_invitation(&invitation)
                    .await
                    .map_err(|_| AdmissionOutboxDeliveryError)?
                    .session_id
            }
            None => return Ok(AdmissionOutboxDeliveryResultV1::Deferred),
        };
        let delivery = async {
            let payload = postcard::to_stdvec(message).map_err(|_| AdmissionOutboxDeliveryError)?;
            self.sessions
                .send(
                    &session,
                    PairingSessionMessage::DurableAdmission(DurableAdmissionFrame {
                        attempt_id: *attempt_id.as_bytes(),
                        kind: DurableAdmissionMessageKind::CancelRequested,
                        message_id: message.message_id,
                        predecessor_message_id: message.predecessor_message_id,
                        payload,
                    }),
                )
                .await
                .map_err(|_| AdmissionOutboxDeliveryError)?;
            let response = timeout(self.response_timeout, self.sessions.recv_next(&session))
                .await
                .map_err(|_| AdmissionOutboxDeliveryError)?
                .map_err(|_| AdmissionOutboxDeliveryError)?
                .ok_or(AdmissionOutboxDeliveryError)?;
            let PairingSessionMessage::DurableAdmission(frame) = response else {
                return Err(AdmissionOutboxDeliveryError);
            };
            if frame.attempt_id != *attempt_id.as_bytes()
                || frame.kind != DurableAdmissionMessageKind::Rejected
                || frame.predecessor_message_id != Some(message.message_id)
            {
                return Err(AdmissionOutboxDeliveryError);
            }
            let rejected: AdmissionOutboxMessageV1 =
                postcard::from_bytes(&frame.payload).map_err(|_| AdmissionOutboxDeliveryError)?;
            if rejected.purpose != AdmissionOutboxPurposeV1::Rejected
                || rejected.message_id != frame.message_id
                || rejected.predecessor_message_id != frame.predecessor_message_id
            {
                return Err(AdmissionOutboxDeliveryError);
            }
            Ok(AdmissionOutboxDeliveryResultV1::Rejected(rejected))
        }
        .await;
        self.sessions.close(&session, None).await;
        delivery
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use uc_core::ports::pairing::{DialError, DialOutcome, PairingSessionId, SessionError};

    struct ScriptedSessions {
        response: Mutex<Option<PairingSessionMessage>>,
        sent: Mutex<Vec<PairingSessionMessage>>,
        closed: Mutex<Vec<PairingSessionId>>,
    }

    #[async_trait]
    impl PairingSessionPort for ScriptedSessions {
        async fn dial_by_invitation(
            &self,
            _code: &InvitationCode,
        ) -> Result<DialOutcome, DialError> {
            panic!("continuation delivery must not reuse the consumed invitation")
        }

        async fn dial_admission_continuation(
            &self,
            address_blob: &[u8],
        ) -> Result<PairingSessionId, DialError> {
            assert_eq!(address_blob, b"sponsor-address");
            Ok(PairingSessionId::new("cleanup-session"))
        }

        async fn send(
            &self,
            _session: &PairingSessionId,
            message: PairingSessionMessage,
        ) -> Result<(), SessionError> {
            self.sent.lock().unwrap().push(message);
            Ok(())
        }

        async fn recv_next(
            &self,
            _session: &PairingSessionId,
        ) -> Result<Option<PairingSessionMessage>, SessionError> {
            Ok(self.response.lock().unwrap().take())
        }

        async fn close(&self, session: &PairingSessionId, _reason: Option<String>) {
            self.closed.lock().unwrap().push(session.clone());
        }
    }

    #[tokio::test]
    async fn cancel_delivery_returns_the_remote_rejection() {
        let attempt_id = AdmissionAttemptId::from_bytes([0x11; 32]);
        let cancel = AdmissionOutboxMessageV1 {
            purpose: AdmissionOutboxPurposeV1::CancelRequested,
            recipient: b"invitation".to_vec(),
            message_id: [0x12; 32],
            predecessor_message_id: Some([0x13; 32]),
            payload: b"cancel_requested".to_vec(),
            superseded: false,
        };
        let rejected = AdmissionOutboxMessageV1 {
            purpose: AdmissionOutboxPurposeV1::Rejected,
            recipient: cancel.recipient.clone(),
            message_id: [0x14; 32],
            predecessor_message_id: Some(cancel.message_id),
            payload: b"cancelled".to_vec(),
            superseded: false,
        };
        let response = PairingSessionMessage::DurableAdmission(DurableAdmissionFrame {
            attempt_id: *attempt_id.as_bytes(),
            kind: DurableAdmissionMessageKind::Rejected,
            message_id: rejected.message_id,
            predecessor_message_id: rejected.predecessor_message_id,
            payload: postcard::to_stdvec(&rejected).unwrap(),
        });
        let sessions = Arc::new(ScriptedSessions {
            response: Mutex::new(Some(response)),
            sent: Mutex::new(Vec::new()),
            closed: Mutex::new(Vec::new()),
        });
        let delivery = PairingAdmissionOutboxDelivery::new(
            Arc::clone(&sessions) as Arc<dyn PairingSessionPort>,
            Duration::from_secs(1),
        );

        let result = delivery
            .deliver(
                attempt_id,
                &cancel,
                Some(&AdmissionOutboxDeliveryRouteV1::Continuation(
                    b"sponsor-address".to_vec(),
                )),
            )
            .await
            .unwrap();

        assert_eq!(result, AdmissionOutboxDeliveryResultV1::Rejected(rejected));
        assert!(matches!(
            sessions.sent.lock().unwrap().first(),
            Some(PairingSessionMessage::DurableAdmission(frame))
                if frame.kind == DurableAdmissionMessageKind::CancelRequested
        ));
        assert_eq!(
            *sessions.closed.lock().unwrap(),
            vec![PairingSessionId::new("cleanup-session")]
        );
    }
}
