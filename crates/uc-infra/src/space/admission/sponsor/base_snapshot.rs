use serde::{Deserialize, Serialize};
use uc_application::deps::MembershipLedgerError;
use uc_core::membership::AdmissionBaseSnapshot;

use crate::db::ports::DbExecutor;

use super::super::repository::{SpaceAdmissionStateStoreError, SqliteSpaceAdmissionState};

const SPONSOR_BASE_SNAPSHOT_FORMAT_V1: u16 = 1;

#[derive(Serialize, Deserialize)]
pub(super) struct PersistedSponsorBaseSnapshotV1 {
    pub(super) format_version: u16,
    pub(super) ledger_revision: u64,
    pub(super) lineage_id: String,
    pub(super) membership_history: Vec<u8>,
}

pub(super) fn decode_sponsor_base_snapshot(
    snapshot: &AdmissionBaseSnapshot,
) -> Result<PersistedSponsorBaseSnapshotV1, SpaceAdmissionStateStoreError> {
    let decoded: PersistedSponsorBaseSnapshotV1 = postcard::from_bytes(snapshot.as_bytes())
        .map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
    if decoded.format_version != SPONSOR_BASE_SNAPSHOT_FORMAT_V1
        || decoded.lineage_id.is_empty()
        || decoded.membership_history.is_empty()
    {
        return Err(SpaceAdmissionStateStoreError::Corrupt);
    }
    Ok(decoded)
}

impl<E: DbExecutor> SqliteSpaceAdmissionState<E> {
    pub(super) async fn load_sponsor_base_snapshot(
        &self,
    ) -> Result<AdmissionBaseSnapshot, SpaceAdmissionStateStoreError> {
        let loaded = self.membership.load().await.map_err(map_membership_error)?;
        let lineage_id = loaded
            .lineage_id
            .filter(|value| !value.is_empty())
            .ok_or(SpaceAdmissionStateStoreError::Corrupt)?;
        let membership_history = loaded
            .membership_history
            .filter(|value| !value.is_empty())
            .ok_or(SpaceAdmissionStateStoreError::Corrupt)?;
        let encoded = postcard::to_stdvec(&PersistedSponsorBaseSnapshotV1 {
            format_version: SPONSOR_BASE_SNAPSHOT_FORMAT_V1,
            ledger_revision: loaded.revision,
            lineage_id,
            membership_history,
        })
        .map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
        AdmissionBaseSnapshot::from_bytes(encoded)
            .map_err(|_| SpaceAdmissionStateStoreError::Corrupt)
    }
}

fn map_membership_error(error: MembershipLedgerError) -> SpaceAdmissionStateStoreError {
    match error {
        MembershipLedgerError::Locked => SpaceAdmissionStateStoreError::Locked,
        MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
            SpaceAdmissionStateStoreError::Corrupt
        }
        MembershipLedgerError::Conflict | MembershipLedgerError::Unavailable => {
            SpaceAdmissionStateStoreError::Unavailable
        }
    }
}
