use uc_core::membership::AdmissionReplayDecision;

use super::super::{
    AuthenticatedSpaceAdmissionMessage, HandleAuthenticatedSpaceAdmissionMessageError,
    SpaceAdmissionMessageReply, SponsorAdmissionMutation, SponsorAdmissionState,
};
use crate::space::admission::protocol::SponsorAdmissionService;

impl SponsorAdmissionService {
    pub(in crate::space::admission::protocol::sponsor) async fn handle_applied(
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
        let (peer_binding, applied, canonical_digest, _) = message.into_parts();
        let evidence = applied.evidence(canonical_digest).ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::invalid(anyhow::anyhow!(
                "the Applied canonical digest is invalid"
            ))
        })?;
        let (state, commit_token) = loaded.into_parts();
        let SponsorAdmissionState::Existing(aggregate) = state else {
            return Err(HandleAuthenticatedSpaceAdmissionMessageError::out_of_order(
                anyhow::anyhow!("Applied cannot start a fresh Sponsor admission"),
            ));
        };
        let expected_peer_binding = aggregate.sponsor_peer_binding().ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!(
                "the Sponsor admission has no authenticated peer binding"
            ))
        })?;
        if peer_binding != expected_peer_binding {
            return Err(HandleAuthenticatedSpaceAdmissionMessageError::conflict(
                anyhow::anyhow!("the Applied channel peer binding differs from saved state"),
            ));
        }
        match aggregate.replay_or_reject(&evidence)? {
            AdmissionReplayDecision::ExactReply(_) => {
                return SpaceAdmissionMessageReply::new(aggregate).ok_or_else(|| {
                    HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(
                        anyhow::anyhow!("the saved Applied reply is unavailable"),
                    )
                });
            }
            AdmissionReplayDecision::Duplicate | AdmissionReplayDecision::New => {}
        }
        let preparation = aggregate.sponsor_complete_preparation().ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!(
                "the Sponsor Committed state has no Complete preparation"
            ))
        })?;
        let prepare_started = std::time::Instant::now();
        let material_result = self
            .prepare_complete
            .prepare(aggregate.admission_id(), preparation, &applied)
            .await;
        super::super::super::record_performance_phase(
            "sponsor_prepare_complete",
            prepare_started,
            material_result.is_ok(),
        );
        let material = material_result?;
        let (activated_security, complete_reply) = material.into_parts();
        let activation_started = std::time::Instant::now();
        let activation_result = self.activate_admission.activate(&activated_security).await;
        super::super::super::record_performance_phase(
            "sponsor_activate_admission",
            activation_started,
            activation_result.is_ok(),
        );
        activation_result?;
        let transition = aggregate.complete_applied(
            applied,
            canonical_digest,
            activated_security,
            complete_reply,
        )?;
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
                "the committed Sponsor Complete reply is unavailable"
            ))
        })
    }
}
