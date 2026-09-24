use serde::{Deserialize, Serialize};
use uc_application::deps::{MembershipLedgerError, MembershipRecord};
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
        .map_err(SpaceAdmissionStateStoreError::corrupt_from)?;
    if decoded.format_version != SPONSOR_BASE_SNAPSHOT_FORMAT_V1
        || decoded.lineage_id.is_empty()
        || decoded.membership_history.is_empty()
    {
        return Err(SpaceAdmissionStateStoreError::corrupt());
    }
    Ok(decoded)
}

impl<E: DbExecutor> SqliteSpaceAdmissionState<E> {
    pub(super) async fn load_sponsor_base_snapshot(
        &self,
    ) -> Result<AdmissionBaseSnapshot, SpaceAdmissionStateStoreError> {
        let loaded = self.membership.load().await.map_err(map_membership_error)?;
        let ledger_revision = loaded.revision();
        let MembershipRecord::Space(space) = loaded else {
            return Err(SpaceAdmissionStateStoreError::corrupt());
        };
        let lineage_id = space.ledger.history.lineage_id().to_owned();
        let membership_history = space
            .ledger
            .history
            .encode_persisted_v2()
            .map_err(SpaceAdmissionStateStoreError::corrupt_from)?;
        if lineage_id.is_empty() || membership_history.is_empty() {
            return Err(SpaceAdmissionStateStoreError::corrupt());
        }
        let encoded = postcard::to_stdvec(&PersistedSponsorBaseSnapshotV1 {
            format_version: SPONSOR_BASE_SNAPSHOT_FORMAT_V1,
            ledger_revision,
            lineage_id,
            membership_history,
        })
        .map_err(SpaceAdmissionStateStoreError::corrupt_from)?;
        AdmissionBaseSnapshot::from_bytes(encoded)
            .map_err(SpaceAdmissionStateStoreError::corrupt_from)
    }
}

fn map_membership_error(error: MembershipLedgerError) -> SpaceAdmissionStateStoreError {
    match error {
        MembershipLedgerError::Locked => SpaceAdmissionStateStoreError::Locked,
        MembershipLedgerError::Corrupt { .. } | MembershipLedgerError::RecoveryRequired => {
            SpaceAdmissionStateStoreError::corrupt()
        }
        MembershipLedgerError::Conflict | MembershipLedgerError::Unavailable { .. } => {
            SpaceAdmissionStateStoreError::unavailable()
        }
    }
}
