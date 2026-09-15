use super::super::JoinerAdmissionService;
use super::{JoinerStartMutation, PreparedJoinerInvitation};
use crate::space::admission::protocol::SpaceAdmissionProtocol;
use crate::space::admission::{
    CurrentJoinStatus, JoinSpaceError, JoinSpaceInput, JoinSpaceResult, JoinSpaceTerminationReason,
};
use uc_core::membership::{
    AdmissionRetryState, JoinerAdmission, PendingAdmissionExchange, SpaceAdmissionAggregateError,
    SpaceAdmissionMessageKind,
};
use uc_core::ports::SettingsPort;
use uc_observability_contract::diagnostics::connectivity::{
    scope_pairing_work, AdmissionExchangeSide,
};
use uc_observability_contract::diagnostics::{
    DiagnosticErrorType, SpaceAdmissionObservationOutcome,
};

impl SpaceAdmissionProtocol {
    pub(crate) async fn start_join_at(
        &self,
        input: JoinSpaceInput,
        started_at_ms: i64,
    ) -> Result<JoinSpaceResult, JoinSpaceError> {
        let result = scope_pairing_work(
            AdmissionExchangeSide::Joiner,
            None,
            self.execute_exclusively(self.joiner.start(input, started_at_ms)),
        )
        .await;
        if result.is_ok() {
            self.recovery.interrupt_current();
        }
        result
    }
}

impl JoinerAdmissionService {
    async fn start(
        &self,
        input: JoinSpaceInput,
        started_at_ms: i64,
    ) -> Result<JoinSpaceResult, JoinSpaceError> {
        persist_device_name(self.settings.as_ref(), input.device_name.as_deref()).await?;
        let loaded = self.start_state.load().await?;
        let (
            next_local_join_ordinal,
            source_snapshot,
            current_join,
            requires_session_transition,
            commit_token,
        ) = loaded.into_parts();
        let superseded_observation_material = current_join.as_ref().map(|admission| {
            (
                *admission.admission_id().as_bytes(),
                admission.is_expired_at(started_at_ms) == Some(true),
            )
        });
        let superseded = current_join
            .map(|admission| {
                if admission.is_expired_at(started_at_ms) == Some(true) {
                    admission
                        .terminate_if_expired(started_at_ms)?
                        .ok_or(SpaceAdmissionAggregateError::InvalidTransition)
                } else {
                    admission.supersede()
                }
            })
            .transpose()
            .map_err(|_| JoinSpaceError::PreviousJoinCannotBeSuperseded)?;

        let prepared_invitation = self.prepare_invitation.prepare(&input).await?;
        let (admission_id, join_id, mut transition) = match prepared_invitation {
            PreparedJoinerInvitation::Full => {
                let material = self.start_material.create(&input).await?;
                let (
                    admission_id,
                    join_id,
                    route,
                    join_request,
                    private_state,
                    encrypted_password_equivalent,
                ) = material.into_parts();
                let pending_exchange = PendingAdmissionExchange::new(
                    route,
                    join_request,
                    SpaceAdmissionMessageKind::Candidate,
                    AdmissionRetryState::new(0, 0)
                        .map_err(|_| JoinSpaceError::InvalidStartMaterial)?,
                )
                .map_err(|_| JoinSpaceError::InvalidStartMaterial)?;
                let transition = JoinerAdmission::start_join(
                    admission_id,
                    join_id,
                    next_local_join_ordinal,
                    source_snapshot,
                    private_state,
                    encrypted_password_equivalent,
                    pending_exchange,
                    started_at_ms,
                )
                .map_err(|_| JoinSpaceError::InvalidStartMaterial)?;
                (admission_id, join_id, transition)
            }
            PreparedJoinerInvitation::Short {
                admission_id,
                join_id,
                start_context,
                short_code,
            } => {
                let transition = JoinerAdmission::start_resolving_invitation(
                    admission_id,
                    join_id,
                    next_local_join_ordinal,
                    source_snapshot,
                    start_context,
                    short_code,
                    started_at_ms,
                )
                .map_err(|_| JoinSpaceError::InvalidStartMaterial)?;
                (admission_id, join_id, transition)
            }
        };
        let expires_at_ms = transition
            .replacement()
            .expires_at_ms()
            .ok_or(JoinSpaceError::InvalidStartMaterial)?;
        let now_ms = self.clock.now_ms();
        if now_ms >= expires_at_ms {
            transition = transition
                .into_replacement()
                .terminate_if_expired(now_ms)
                .map_err(|_| JoinSpaceError::InvalidStartMaterial)?
                .ok_or(JoinSpaceError::InvalidStartMaterial)?;
        }
        let terminated = transition.replacement().termination_reason();

        self.start_state
            .commit(
                commit_token,
                JoinerStartMutation::new(transition, superseded),
            )
            .await?;
        if terminated.is_none() {
            self.maintenance_wake.schedule_at(expires_at_ms, now_ms);
        }
        if let Some((material, expired)) = superseded_observation_material {
            let outcome = if expired {
                SpaceAdmissionObservationOutcome::Failed(DiagnosticErrorType::Timeout)
            } else {
                SpaceAdmissionObservationOutcome::Cancelled
            };
            self.observations.finish(material, outcome);
        }
        if terminated.is_none() {
            self.observations.begin(*admission_id.as_bytes());
        }
        self.observations
            .scope(*admission_id.as_bytes(), async {
                self.maintenance_wake.wake();
            })
            .await;

        let status = match terminated {
            Some(uc_core::membership::SpaceAdmissionTerminationReason::Expired) => {
                CurrentJoinStatus::Terminated {
                    join_id: *join_id.as_bytes(),
                    reason: JoinSpaceTerminationReason::Expired,
                }
            }
            Some(_) => return Err(JoinSpaceError::InvalidStartMaterial),
            None => CurrentJoinStatus::Pending {
                join_id: *join_id.as_bytes(),
                target_space_id: None,
                sponsor_device_id: None,
                sponsor_identity_fingerprint: None,
                cancel_requested: false,
                peer_upgrade_required: false,
            },
        };
        Ok(JoinSpaceResult {
            status,
            requires_session_transition,
        })
    }
}

async fn persist_device_name(
    settings: &dyn SettingsPort,
    device_name: Option<&str>,
) -> Result<(), JoinSpaceError> {
    let Some(device_name) = device_name else {
        return Ok(());
    };
    let device_name = device_name.trim();
    if device_name.is_empty() {
        return Err(JoinSpaceError::DeviceNameRequired);
    }
    let mut current = settings
        .load()
        .await
        .map_err(|error| JoinSpaceError::Settings(error.to_string()))?;
    if current.general.device_name.as_deref() == Some(device_name) {
        return Ok(());
    }
    current.general.device_name = Some(device_name.to_owned());
    settings
        .save(&current)
        .await
        .map_err(|error| JoinSpaceError::Settings(error.to_string()))
}
