pub(super) mod codec;
mod persisted;
pub(super) mod token;

use std::sync::Arc;

use crate::db::ports::DbExecutor;
use crate::security::{ActiveSpaceGenerationManifestStore, AdmissionKeyManager};

pub struct SqliteSpaceAdmissionState<E> {
    pub(super) executor: E,
    pub(super) keys: Arc<AdmissionKeyManager>,
    pub(super) manifests: Arc<ActiveSpaceGenerationManifestStore>,
}

impl<E> SqliteSpaceAdmissionState<E> {
    pub fn new(
        executor: E,
        keys: Arc<AdmissionKeyManager>,
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
    ) -> Self {
        Self {
            executor,
            keys,
            manifests,
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

impl<E: DbExecutor> SqliteSpaceAdmissionState<E> {}
