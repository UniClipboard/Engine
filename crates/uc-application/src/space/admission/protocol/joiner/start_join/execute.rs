use super::super::JoinerAdmissionService;
use super::{JoinerStartMutation, PreparedJoinerInvitation};
use crate::space::admission::protocol::SpaceAdmissionProtocol;
use crate::space::admission::{CurrentJoinStatus, JoinSpaceError, JoinSpaceInput, JoinSpaceResult};
use std::time::Instant;
use uc_core::membership::{
    AdmissionRetryState, JoinerAdmission, PendingAdmissionExchange, SpaceAdmissionMessageKind,
};
use uc_core::ports::SettingsPort;

impl SpaceAdmissionProtocol {
    pub(crate) async fn start_join(
        &self,
        input: JoinSpaceInput,
    ) -> Result<JoinSpaceResult, JoinSpaceError> {
        self.execute_exclusively(self.joiner.start(input)).await
    }
}

impl JoinerAdmissionService {
    async fn start(&self, input: JoinSpaceInput) -> Result<JoinSpaceResult, JoinSpaceError> {
        let started = Instant::now();
        persist_device_name(self.settings.as_ref(), input.device_name.as_deref()).await?;
        let loaded = self.start_state.load().await?;
        let (
            next_local_join_ordinal,
            source_snapshot,
            current_join,
            requires_session_transition,
            commit_token,
        ) = loaded.into_parts();
        let superseded = current_join
            .map(JoinerAdmission::supersede)
            .transpose()
            .map_err(|_| JoinSpaceError::PreviousJoinCannotBeSuperseded)?;

        let prepared_invitation = self.prepare_invitation.prepare(&input).await?;
        let (join_id, transition) = match prepared_invitation {
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
                )
                .map_err(|_| JoinSpaceError::InvalidStartMaterial)?;
                (join_id, transition)
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
                )
                .map_err(|_| JoinSpaceError::InvalidStartMaterial)?;
                (join_id, transition)
            }
        };

        self.start_state
            .commit(
                commit_token,
                JoinerStartMutation::new(transition, superseded),
            )
            .await?;
        self.maintenance_wake.wake();

        tracing::info!(
            target: "admission.performance",
            phase = "joiner_start_persisted",
            elapsed_ms = started.elapsed().as_millis() as u64,
            "pairing phase completed"
        );

        Ok(JoinSpaceResult {
            status: CurrentJoinStatus::Pending {
                join_id: *join_id.as_bytes(),
                target_space_id: None,
                sponsor_device_id: None,
                sponsor_identity_fingerprint: None,
                cancel_requested: false,
            },
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
