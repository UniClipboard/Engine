use serde::{Deserialize, Serialize};
use uc_core::membership::{ActiveSpaceGenerationManifestV2, AdmissionSourceSnapshot};

use crate::db::ports::DbExecutor;
use crate::security::ActiveSpaceGenerationManifestStoreError;

use super::super::repository::{SpaceAdmissionStateStoreError, SqliteSpaceAdmissionState};

const SOURCE_SNAPSHOT_FORMAT_V1: u16 = 1;

#[derive(Serialize, Deserialize)]
struct PersistedAdmissionSourceSnapshotV1 {
    format_version: u16,
    active_manifest: Option<ActiveSpaceGenerationManifestV2>,
}

impl<E: DbExecutor> SqliteSpaceAdmissionState<E> {
    pub(super) async fn load_source_snapshot(
        &self,
    ) -> Result<(AdmissionSourceSnapshot, bool), SpaceAdmissionStateStoreError> {
        let active_manifest = self.manifests.load().await.map_err(map_manifest_error)?;
        let requires_session_transition = active_manifest.is_some();
        let encoded = postcard::to_stdvec(&PersistedAdmissionSourceSnapshotV1 {
            format_version: SOURCE_SNAPSHOT_FORMAT_V1,
            active_manifest,
        })
        .map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
        let snapshot = AdmissionSourceSnapshot::from_bytes(encoded)
            .map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
        Ok((snapshot, requires_session_transition))
    }
}

fn map_manifest_error(
    error: ActiveSpaceGenerationManifestStoreError,
) -> SpaceAdmissionStateStoreError {
    match error {
        ActiveSpaceGenerationManifestStoreError::Storage => {
            SpaceAdmissionStateStoreError::Unavailable
        }
        ActiveSpaceGenerationManifestStoreError::Corrupt => SpaceAdmissionStateStoreError::Corrupt,
    }
}
