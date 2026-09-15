use crate::space::admission::observation::message_action;
use async_trait::async_trait;
use uc_core::membership::{SpaceAdmissionBodyV1, SpaceAdmissionMessageKind};
use uc_observability_contract::diagnostics::connectivity::{
    scope_pairing_work, AdmissionExchangeSide,
};
use uc_observability_contract::diagnostics::describe_admission_request;

use super::super::{
    AuthenticatedSpaceAdmissionMessage, HandleAuthenticatedSpaceAdmissionMessageError,
    HandleAuthenticatedSpaceAdmissionMessagePort, SpaceAdmissionMessageReply,
};
use crate::space::admission::protocol::{SpaceAdmissionProtocol, SponsorAdmissionService};

#[async_trait]
impl HandleAuthenticatedSpaceAdmissionMessagePort for SpaceAdmissionProtocol {
    async fn handle(
        &self,
        message: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<SpaceAdmissionMessageReply, HandleAuthenticatedSpaceAdmissionMessageError> {
        let action = message_action(message.envelope().kind());
        if let Some(action) = action {
            describe_admission_request(action);
        }
        let message_kind = message.envelope().kind();
        if message_kind == SpaceAdmissionMessageKind::JoinRequest {
            let contract = message.attempt_contract().ok_or_else(|| {
                HandleAuthenticatedSpaceAdmissionMessageError::invalid(anyhow::anyhow!(
                    "a new JoinRequest requires the shared attempt timeline"
                ))
            })?;
            let SpaceAdmissionBodyV1::JoinRequest(request) = message.envelope().body() else {
                return Err(HandleAuthenticatedSpaceAdmissionMessageError::invalid(
                    anyhow::anyhow!("the JoinRequest body is invalid"),
                ));
            };
            let binding = message.peer_binding();
            if contract.admission_id() != message.envelope().header().admission_id()
                || contract.invitation_id() != request.invitation_id()
                || contract.joiner_peer_id() != binding.remote_peer_id()
                || contract.sponsor_peer_id() != binding.local_peer_id()
            {
                return Err(HandleAuthenticatedSpaceAdmissionMessageError::invalid(
                    anyhow::anyhow!("the shared attempt binding does not match the request"),
                ));
            }
            let timeline = contract.timeline();
            let now_ms = self.recovery.now_ms();
            if timeline.started_at_ms() > now_ms || timeline.is_expired(now_ms) {
                return Err(HandleAuthenticatedSpaceAdmissionMessageError::invalid(
                    anyhow::anyhow!("the shared attempt timeline is not currently valid"),
                ));
            }
        }
        let result = scope_pairing_work(
            AdmissionExchangeSide::Sponsor,
            action,
            self.execute_exclusively(
                self.sponsor
                    .handle_authenticated_message(message, self.recovery.now_ms()),
            ),
        )
        .await;
        if let Ok(reply) = &result {
            if message_kind == SpaceAdmissionMessageKind::Applied
                && reply.has_pairing_confirmation()
            {
                if let Some(expires_at_ms) = reply.expires_at_ms() {
                    let now_ms = self.recovery.now_ms();
                    if now_ms < expires_at_ms {
                        self.joiner
                            .maintenance_wake
                            .schedule_at(expires_at_ms, now_ms);
                    } else {
                        self.joiner.maintenance_wake.wake();
                    }
                }
                self.recovery.notify_admission_changed();
            } else if message_kind == SpaceAdmissionMessageKind::CompleteAck
                && reply.has_pairing_confirmation()
            {
                self.joiner.maintenance_wake.wake();
                self.recovery.notify_admission_changed();
            } else if message_kind == SpaceAdmissionMessageKind::Abandonment {
                self.joiner.maintenance_wake.wake();
                self.recovery.notify_admission_changed();
            }
        }
        result
    }
}

impl SponsorAdmissionService {
    async fn handle_authenticated_message(
        &self,
        message: AuthenticatedSpaceAdmissionMessage,
        now_ms: i64,
    ) -> Result<SpaceAdmissionMessageReply, HandleAuthenticatedSpaceAdmissionMessageError> {
        match message.envelope().kind() {
            SpaceAdmissionMessageKind::JoinRequest => self.handle_join_request(message).await,
            SpaceAdmissionMessageKind::Prepared => self.handle_prepared(message, now_ms).await,
            SpaceAdmissionMessageKind::Applied => self.handle_applied(message, now_ms).await,
            SpaceAdmissionMessageKind::CompleteAck => self.handle_complete_ack(message).await,
            SpaceAdmissionMessageKind::Abandonment => self.handle_abandonment(message).await,
            _ => Err(HandleAuthenticatedSpaceAdmissionMessageError::out_of_order(
                anyhow::anyhow!("the Sponsor cannot handle this admission message kind"),
            )),
        }
    }
}
