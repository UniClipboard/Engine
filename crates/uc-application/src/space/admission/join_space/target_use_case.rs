use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ports::SettingsPort;

use crate::space::membership::project_current_join;
use crate::space::membership::WakeSpaceMembershipMaintenancePort;
use crate::space::membership::{MembershipLedger, MembershipLedgerError};

use super::{JoinSpaceError, JoinSpaceInput, JoinSpaceResult};

pub struct PreparedJoinSpace {
    pub record: uc_core::membership::SpaceJoinRecord,
    pub expected_membership_history_v2: Option<Vec<u8>>,
    pub requires_session_transition: bool,
}

#[async_trait]
/// Prepares cryptographic and protocol material only.
///
/// Implementations must not persist, send, dial, or start recovery. The use
/// case commits the returned record before the membership runtime may deliver
/// its outbox.
pub trait PrepareJoinSpacePort: Send + Sync {
    async fn prepare(&self, input: &JoinSpaceInput) -> Result<PreparedJoinSpace, JoinSpaceError>;
}

pub(crate) struct JoinSpaceUseCase {
    settings: Arc<dyn SettingsPort>,
    preparation: Arc<dyn PrepareJoinSpacePort>,
    ledger: Arc<MembershipLedger>,
    maintenance: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    execution_lock: tokio::sync::Mutex<()>,
}

impl JoinSpaceUseCase {
    pub(crate) fn new(
        settings: Arc<dyn SettingsPort>,
        preparation: Arc<dyn PrepareJoinSpacePort>,
        ledger: Arc<MembershipLedger>,
        maintenance: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    ) -> Self {
        Self {
            settings,
            preparation,
            ledger,
            maintenance,
            execution_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) async fn execute(
        &self,
        input: JoinSpaceInput,
    ) -> Result<JoinSpaceResult, JoinSpaceError> {
        let _guard = self.execution_lock.lock().await;
        persist_device_name(self.settings.as_ref(), input.device_name.as_deref()).await?;
        let prepared = self.preparation.prepare(&input).await?;
        let requires_session_transition = prepared.requires_session_transition;
        self.ledger
            .create_admission_record(
                prepared.record,
                None,
                prepared.expected_membership_history_v2,
            )
            .await
            .map_err(map_ledger_error)?;
        self.maintenance.wake();
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        let status = project_current_join(snapshot.record())
            .map_err(|error| JoinSpaceError::SavedState(error.to_string()))?
            .ok_or_else(|| {
                JoinSpaceError::SavedState("saved join state is unavailable".to_owned())
            })?;
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

fn map_ledger_error(error: MembershipLedgerError) -> JoinSpaceError {
    JoinSpaceError::SavedState(error.to_string())
}
