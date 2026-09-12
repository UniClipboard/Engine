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

#[derive(Debug, thiserror::Error)]
pub(super) enum CredentialLoadError {
    #[error("admission record is absent")]
    RecordMissing,
    #[error("continuation credential is absent")]
    CredentialMissing,
    #[error(transparent)]
    State(#[from] SpaceAdmissionStateStoreError),
    #[error("continuation credential is invalid")]
    Invalid {
        #[source]
        source: anyhow::Error,
    },
    #[error("continuation credential could not be read")]
    Storage {
        #[source]
        source: anyhow::Error,
    },
}

impl CredentialLoadError {
    pub(super) fn diagnostic_failure(
        &self,
    ) -> uc_observability_contract::diagnostics::connectivity::CredentialFailure {
        use uc_observability_contract::diagnostics::connectivity::CredentialFailure;
        match self {
            Self::RecordMissing => CredentialFailure::RecordMissing,
            Self::CredentialMissing => CredentialFailure::CredentialMissing,
            Self::State(SpaceAdmissionStateStoreError::Locked) => CredentialFailure::Locked,
            Self::State(SpaceAdmissionStateStoreError::Corrupt) | Self::Invalid { .. } => {
                CredentialFailure::Corrupt
            }
            Self::State(SpaceAdmissionStateStoreError::Conflict) => {
                CredentialFailure::RecoveryRequired
            }
            Self::State(SpaceAdmissionStateStoreError::Unavailable) | Self::Storage { .. } => {
                CredentialFailure::Unavailable
            }
        }
    }

    fn from_executor(error: anyhow::Error) -> Self {
        match error.downcast::<Self>() {
            Ok(error) => error,
            Err(error) => Self::Storage { source: error },
        }
    }
}

impl<E: DbExecutor> SqliteSpaceAdmissionState<E> {
    pub(in crate::space::admission) fn load_continuation_credential(
        &self,
        admission_id: SpaceAdmissionId,
    ) -> Result<AdmissionContinuationCredential, CredentialLoadError> {
        self.executor
            .run(|conn| {
                let state = self
                    .load_state_on(conn)
                    .map_err(CredentialLoadError::from)?;
                let stored = state
                    .records
                    .get(admission_id.as_bytes())
                    .ok_or(CredentialLoadError::RecordMissing)?;
                let aggregate = self
                    .open_record(*admission_id.as_bytes(), stored)
                    .map_err(CredentialLoadError::from)?;
                let credential = aggregate
                    .sponsor_continuation_credential()
                    .ok_or(CredentialLoadError::CredentialMissing)?;
                AdmissionContinuationCredential::from_bytes(credential.as_bytes().to_vec()).map_err(
                    |source| {
                        anyhow::Error::new(CredentialLoadError::Invalid {
                            source: anyhow::Error::new(source),
                        })
                    },
                )
            })
            .map_err(CredentialLoadError::from_executor)
    }
}
