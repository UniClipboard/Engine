pub(super) mod codec;
mod persisted;
pub(super) mod token;

use std::sync::Arc;

use crate::db::ports::DbExecutor;
use crate::security::{ActiveSpaceGenerationManifestStore, AdmissionKeyManager};
use uc_application::deps::LoadMembershipLedgerPort;
use uc_core::membership::{AdmissionContinuationCredential, SpaceAdmissionId};

pub struct SqliteSpaceAdmissionState<E> {
    pub(super) executor: E,
    pub(super) keys: Arc<AdmissionKeyManager>,
    pub(super) manifests: Arc<ActiveSpaceGenerationManifestStore>,
    pub(super) membership: Arc<dyn LoadMembershipLedgerPort>,
}

impl<E> SqliteSpaceAdmissionState<E> {
    pub fn new(
        executor: E,
        keys: Arc<AdmissionKeyManager>,
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
        membership: Arc<dyn LoadMembershipLedgerPort>,
    ) -> Self {
        Self {
            executor,
            keys,
            manifests,
            membership,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(super) enum SpaceAdmissionStateStoreError {
    #[error("space admission state is locked")]
    Locked,
    #[error("space admission state is corrupt")]
    Corrupt,
    #[error("space admission state changed")]
    Conflict,
    #[error("space admission state storage is unavailable")]
    Unavailable,
}

impl<E: DbExecutor> SqliteSpaceAdmissionState<E> {
    pub(in crate::space::admission) fn load_continuation_credential(
        &self,
        admission_id: SpaceAdmissionId,
    ) -> Result<AdmissionContinuationCredential, SpaceAdmissionStateStoreError> {
        self.executor
            .run(|conn| {
                let state = self.load_state_on(conn).map_err(codec::into_anyhow)?;
                let stored = state
                    .records
                    .get(admission_id.as_bytes())
                    .ok_or_else(|| codec::into_anyhow(SpaceAdmissionStateStoreError::Conflict))?;
                let aggregate = self
                    .open_record(*admission_id.as_bytes(), stored)
                    .map_err(codec::into_anyhow)?;
                let continuation_credential = aggregate
                    .sponsor_continuation_credential()
                    .ok_or_else(|| codec::into_anyhow(SpaceAdmissionStateStoreError::Conflict))?;
                let credential = AdmissionContinuationCredential::from_bytes(
                    continuation_credential.as_bytes().to_vec(),
                )
                .map_err(|_| codec::into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                Ok(credential)
            })
            .map_err(codec::map_executor_error)
    }
}
