//! 当前唯一写入格式：原冻结成员资料与单一组更新恢复附录同存、同一修订。
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uc_application::deps::{GroupUpdateRecoveryRecord, MembershipLedgerError, MembershipRecord};
use uc_core::membership::HistoricalMembershipSignatureVerifier;

use super::common::PositionDto;
use super::parse;
use super::v5::SpaceRecordV5;

pub(super) const FORMAT_V6: u16 = 6;
#[derive(Serialize, Deserialize)]
struct RecordV6 {
    version: u16,
    generation: [u8; 16],
    revision: u64,
    space: Option<SpaceRecordV5>,
    group_update_recovery: Option<RecoveryDto>,
    prepared_group_updates: BTreeMap<[u8; 32], Vec<u8>>,
}
#[derive(Serialize, Deserialize)]
struct RecoveryDto {
    base: PositionDto,
    target_epoch: u64,
    payload_digest: [u8; 32],
}
pub(super) fn encode(
    record: &MembershipRecord,
    generation: [u8; 16],
) -> Result<Vec<u8>, MembershipLedgerError> {
    let (space, recovery) = match record {
        MembershipRecord::NoSpace { .. } => (None, None),
        MembershipRecord::Space(space) => (
            Some(SpaceRecordV5::from_record(space)?),
            space
                .group_update_recovery
                .as_ref()
                .map(|recovery| RecoveryDto {
                    base: PositionDto::from_position(&recovery.base_position),
                    target_epoch: recovery.target_epoch,
                    payload_digest: recovery.payload_digest,
                }),
        ),
    };
    postcard::to_stdvec(&RecordV6 {
        version: FORMAT_V6,
        generation,
        revision: record.revision(),
        space,
        group_update_recovery: recovery,
        prepared_group_updates: match record {
            MembershipRecord::Space(space) => space.prepared_group_updates.clone(),
            _ => BTreeMap::new(),
        },
    })
    .map_err(MembershipLedgerError::corrupt_from)
}
pub(super) fn decode(
    bytes: &[u8],
    generation: [u8; 16],
    verifier: &dyn HistoricalMembershipSignatureVerifier,
) -> Result<MembershipRecord, MembershipLedgerError> {
    let value: RecordV6 = parse(bytes)?;
    if value.version != FORMAT_V6 || value.generation != generation {
        return Err(MembershipLedgerError::corrupt());
    }
    match value.space {
        None if value.group_update_recovery.is_none()
            && value.prepared_group_updates.is_empty() =>
        {
            Ok(MembershipRecord::NoSpace {
                revision: value.revision,
            })
        }
        None => Err(MembershipLedgerError::corrupt()),
        Some(space) => {
            let mut record = space.into_record(value.revision, verifier)?;
            record.prepared_group_updates = value.prepared_group_updates;
            record.group_update_recovery =
                value
                    .group_update_recovery
                    .map(|recovery| GroupUpdateRecoveryRecord {
                        base_position: recovery.base.into_position(),
                        target_epoch: recovery.target_epoch,
                        payload_digest: recovery.payload_digest,
                    });
            Ok(MembershipRecord::Space(Box::new(record)))
        }
    }
}
