use uc_core::membership::AdmissionReplayDecision;

use super::super::{
    AuthenticatedSpaceAdmissionMessage, HandleAuthenticatedSpaceAdmissionMessageError,
    SpaceAdmissionMessageReply, SponsorAdmissionMutation, SponsorAdmissionState,
};
use crate::space::admission::protocol::SponsorAdmissionService;

impl SponsorAdmissionService {
    pub(in crate::space::admission::protocol::sponsor) async fn handle_complete_ack(
        &self,
        message: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<SpaceAdmissionMessageReply, HandleAuthenticatedSpaceAdmissionMessageError> {
        let load_started = std::time::Instant::now();
        let loaded_result = self.state.load(&message).await;
        super::super::super::record_performance_phase(
            "sponsor_state_load",
            load_started,
            loaded_result.is_ok(),
        );
        let loaded = loaded_result?;
        let (peer_binding, complete_ack, canonical_digest, _) = message.into_parts();
        let evidence = complete_ack.evidence(canonical_digest).ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::invalid(anyhow::anyhow!(
                "the CompleteAck canonical digest is invalid"
            ))
        })?;
        let (state, commit_token) = loaded.into_parts();
        let SponsorAdmissionState::Existing(aggregate) = state else {
            return Err(HandleAuthenticatedSpaceAdmissionMessageError::out_of_order(
                anyhow::anyhow!("CompleteAck cannot start a fresh Sponsor admission"),
            ));
        };
        let expected_peer_binding = aggregate.sponsor_peer_binding().ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!(
                "the Sponsor admission has no authenticated peer binding"
            ))
        })?;
        if peer_binding != expected_peer_binding {
            return Err(HandleAuthenticatedSpaceAdmissionMessageError::conflict(
                anyhow::anyhow!("the CompleteAck channel peer binding differs from saved state"),
            ));
        }
        match aggregate.replay_or_reject(&evidence)? {
            AdmissionReplayDecision::ExactReply(_) => {
                return SpaceAdmissionMessageReply::new(aggregate).ok_or_else(|| {
                    HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(
                        anyhow::anyhow!("the saved CompleteAck reply is unavailable"),
                    )
                });
            }
            AdmissionReplayDecision::Duplicate | AdmissionReplayDecision::New => {}
        }
        let preparation = aggregate.sponsor_settlement_preparation().ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!(
                "the Sponsor Applied state has no settlement preparation"
            ))
        })?;
        let prepare_started = std::time::Instant::now();
        let settled_result = self
            .prepare_settled
            .prepare(aggregate.admission_id(), preparation, &complete_ack)
            .await;
        super::super::super::record_performance_phase(
            "sponsor_prepare_settled",
            prepare_started,
            settled_result.is_ok(),
        );
        let settled = settled_result?;
        let transition =
            aggregate.settle_complete_ack(complete_ack, canonical_digest, settled.into_reply())?;
        let commit_started = std::time::Instant::now();
        let committed_result = self
            .state
            .commit(commit_token, SponsorAdmissionMutation::new(transition))
            .await;
        super::super::super::record_performance_phase(
            "sponsor_state_commit",
            commit_started,
            committed_result.is_ok(),
        );
        let committed = committed_result?;
        let (aggregate, _) = committed.into_parts();
        SpaceAdmissionMessageReply::new(aggregate).ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!(
                "the committed Sponsor Settled reply is unavailable"
            ))
        })
    }
}
