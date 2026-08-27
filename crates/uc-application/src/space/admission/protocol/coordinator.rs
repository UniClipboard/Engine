use std::sync::Arc;

use super::{JoinerStartMaterialPort, JoinerStartMutation, JoinerStartStatePort};
use crate::space::admission::{CurrentJoinStatus, JoinSpaceError, JoinSpaceInput, JoinSpaceResult};
use uc_core::membership::{
    AdmissionRetryState, PendingAdmissionExchange, SpaceAdmissionAggregate,
    SpaceAdmissionMessageKind,
};
use uc_core::ports::SettingsPort;

pub(crate) struct SpaceAdmissionProtocol {
    settings: Arc<dyn SettingsPort>,
    /// 生成初始加入材料
    joiner_start_material: Arc<dyn JoinerStartMaterialPort>,
    /// 读取完整开始视图并一次性提交变化
    joiner_start_state: Arc<dyn JoinerStartStatePort>,
    execution_lock: tokio::sync::Mutex<()>,
}

impl SpaceAdmissionProtocol {
    pub(crate) fn new(
        settings: Arc<dyn SettingsPort>,
        joiner_start_material: Arc<dyn JoinerStartMaterialPort>,
        joiner_start_state: Arc<dyn JoinerStartStatePort>,
    ) -> Self {
        Self {
            settings,
            joiner_start_material,
            joiner_start_state,
            execution_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) async fn start_join(
        &self,
        input: JoinSpaceInput,
    ) -> Result<JoinSpaceResult, JoinSpaceError> {
        let _guard = self.execution_lock.lock().await;
        persist_device_name(self.settings.as_ref(), input.device_name.as_deref()).await?;
        let loaded = self.joiner_start_state.load().await?;
        let (
            next_local_join_ordinal,
            source_snapshot,
            current_join,
            requires_session_transition,
            commit_token,
        ) = loaded.into_parts();

        let superseded = current_join
            .map(SpaceAdmissionAggregate::supersede)
            .transpose()
            .map_err(|_| JoinSpaceError::PreviousJoinCannotBeSuperseded)?;

        let material = self.joiner_start_material.create(&input).await?;
        let (admission_id, join_id, route, join_request, encrypted_password_equivalent) =
            material.into_parts();
        let pending_exchange = PendingAdmissionExchange::new(
            route,
            join_request,
            SpaceAdmissionMessageKind::Candidate,
            AdmissionRetryState::new(0, 0).map_err(|_| JoinSpaceError::InvalidStartMaterial)?,
        )
        .map_err(|_| JoinSpaceError::InvalidStartMaterial)?;
        let transition = SpaceAdmissionAggregate::start_join(
            admission_id,
            join_id,
            next_local_join_ordinal,
            source_snapshot,
            encrypted_password_equivalent,
            pending_exchange,
        )
        .map_err(|_| JoinSpaceError::InvalidStartMaterial)?;

        self.joiner_start_state
            .commit(
                commit_token,
                JoinerStartMutation::new(transition, superseded),
            )
            .await?;

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
