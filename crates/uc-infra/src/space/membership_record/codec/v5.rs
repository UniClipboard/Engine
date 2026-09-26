//! `MembershipLedgerRecordV5`：成员记录的唯一最终格式。
//!
//! 状态部分（对端、效果、交换与分叉资料）全部以本模块或 [`super::common`] 的独立结构声明；只直接嵌入
//! 标识值与带自身版本的已签名协议对象（事件、决定、回执、分页、换组与恢复包）。成员历史保存为其归档
//! 编码，读取时重新验签。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uc_application::deps::{
    MembershipHistoryExchangeRecord, MembershipLedgerError, MembershipRecord, SpaceMembershipRecord,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    DepartingLinkSnapshot, HistoricalMembershipSignatureVerifier, MemberEffectKind,
    MemberEffectMaterial, MemberEffectPhase, MemberInstanceId, MemberLinkSnapshot,
    MembershipDecisionV2, MembershipEventId, MembershipEventV2, MembershipHistoryAckV3,
    MembershipLedger, MembershipLedgerSnapshot, PeerLinkSnapshot, PeerRelation,
    PeerSyncBackoffSnapshot, PeerSyncOutcome, UnfinishedMemberEffectSnapshot,
    VersionedMembershipHistory,
};

use super::common::{BranchRecoveryDto, InboundTransferDto, PositionDto};
use super::parse;

pub(super) const FORMAT_V5: u16 = 5;

#[derive(Serialize, Deserialize)]
struct MembershipLedgerRecordV5 {
    format_version: u16,
    profile_generation: [u8; 16],
    revision: u64,
    space: Option<SpaceRecordV5>,
}

#[derive(Serialize, Deserialize)]
struct SpaceRecordV5 {
    history: Vec<u8>,
    local_device_id: DeviceId,
    local_member: MemberInstanceId,
    peers: BTreeMap<DeviceId, PeerLinkV5>,
    effects: Vec<MemberEffectV5>,
    sync_cursor: Option<DeviceId>,
    inbound_transfers: BTreeMap<DeviceId, InboundTransferDto>,
    completed_inbound_transfers: BTreeMap<(DeviceId, [u8; 32]), MembershipHistoryAckV3>,
    branch_recovery: BranchRecoveryDto,
}

#[derive(Serialize, Deserialize)]
enum PeerLinkV5 {
    Member(MemberLinkV5),
    Departing(DepartingLinkV5),
}

#[derive(Serialize, Deserialize)]
struct MemberLinkV5 {
    relation: PeerRelationV5,
    confirmed_position: Option<PositionDto>,
    sync_pending_since_revision: Option<u64>,
    sync_retry_attempt: u32,
    sync_next_attempt_at_ms: i64,
    sync_last_outcome: PeerSyncOutcomeV5,
    outgoing_decision: Option<MembershipDecisionV2>,
}

#[derive(Serialize, Deserialize)]
enum PeerRelationV5 {
    Unconfirmed,
    Consistent,
    UpgradeRequired,
    AwaitingLocalDecision,
    Diverged,
    Invalid,
    AwaitingPeerDecision,
}

#[derive(Serialize, Deserialize)]
enum PeerSyncOutcomeV5 {
    Never,
    Deferred,
    Acked,
    StableRejected,
}

#[derive(Serialize, Deserialize)]
struct DepartingLinkV5 {
    notice: MembershipEventV2,
    since_ms: i64,
}

#[derive(Serialize, Deserialize)]
struct MemberEffectV5 {
    event_id: MembershipEventId,
    kind: MemberEffectKindV5,
    phase: MemberEffectPhaseV5,
    affected_device_ids: Vec<DeviceId>,
    material: MemberEffectMaterialV5,
}

#[derive(Serialize, Deserialize)]
enum MemberEffectKindV5 {
    AddDevice,
    RemoveDevice,
}

#[derive(Serialize, Deserialize)]
enum MemberEffectPhaseV5 {
    Prepared,
    MemberFactsApplied,
    SecurityApplied,
}

#[derive(Serialize, Deserialize)]
enum MemberEffectMaterialV5 {
    Event(MembershipEventV2),
    InitiatedRemoval {
        event: MembershipEventV2,
        retained_device_ids: Vec<DeviceId>,
    },
    Decision(MembershipDecisionV2),
}

pub(super) fn encode(
    record: &MembershipRecord,
    generation: [u8; 16],
) -> Result<Vec<u8>, MembershipLedgerError> {
    let space = match record {
        MembershipRecord::NoSpace { .. } => None,
        MembershipRecord::Space(space) => Some(SpaceRecordV5::from_record(space)?),
    };
    postcard::to_stdvec(&MembershipLedgerRecordV5 {
        format_version: FORMAT_V5,
        profile_generation: generation,
        revision: record.revision(),
        space,
    })
    .map_err(MembershipLedgerError::corrupt_from)
}

pub(super) fn decode(
    bytes: &[u8],
    generation: [u8; 16],
    verifier: &dyn HistoricalMembershipSignatureVerifier,
) -> Result<MembershipRecord, MembershipLedgerError> {
    let value: MembershipLedgerRecordV5 = parse(bytes)?;
    if value.format_version != FORMAT_V5 || value.profile_generation != generation {
        return Err(MembershipLedgerError::corrupt());
    }
    match value.space {
        None => Ok(MembershipRecord::NoSpace {
            revision: value.revision,
        }),
        Some(space) => Ok(MembershipRecord::Space(Box::new(
            space.into_record(value.revision, verifier)?,
        ))),
    }
}

impl SpaceRecordV5 {
    fn from_record(record: &SpaceMembershipRecord) -> Result<Self, MembershipLedgerError> {
        let ledger = &record.ledger;
        Ok(Self {
            history: ledger
                .history
                .encode_persisted_v2()
                .map_err(MembershipLedgerError::corrupt_from)?,
            local_device_id: ledger.local_device_id,
            local_member: ledger.local_member,
            peers: ledger
                .peers
                .iter()
                .map(|(device, link)| (*device, PeerLinkV5::from_snapshot(link)))
                .collect(),
            effects: ledger
                .effects
                .iter()
                .map(MemberEffectV5::from_snapshot)
                .collect(),
            sync_cursor: ledger.sync_cursor,
            inbound_transfers: record
                .history_exchange
                .inbound_transfers
                .iter()
                .map(|(device, transfer)| (*device, InboundTransferDto::from_transfer(transfer)))
                .collect(),
            completed_inbound_transfers: record
                .history_exchange
                .completed_inbound_transfers
                .clone(),
            branch_recovery: BranchRecoveryDto::from_record(&record.branch_recovery),
        })
    }

    fn into_record(
        self,
        revision: u64,
        verifier: &dyn HistoricalMembershipSignatureVerifier,
    ) -> Result<SpaceMembershipRecord, MembershipLedgerError> {
        let history = VersionedMembershipHistory::decode_persisted_v2(&self.history, verifier)
            .map_err(MembershipLedgerError::corrupt_from)?;
        let snapshot = MembershipLedgerSnapshot {
            revision,
            history,
            local_device_id: self.local_device_id,
            local_member: self.local_member,
            peers: self
                .peers
                .into_iter()
                .map(|(device, link)| (device, link.into_snapshot()))
                .collect(),
            effects: self
                .effects
                .into_iter()
                .map(MemberEffectV5::into_snapshot)
                .collect(),
            sync_cursor: self.sync_cursor,
        };
        // 不满足聚合不变量的记录视为损坏，不在读取时修复。
        let ledger = MembershipLedger::restore(snapshot)
            .map_err(MembershipLedgerError::corrupt_from)?
            .snapshot();
        Ok(SpaceMembershipRecord {
            ledger,
            history_exchange: MembershipHistoryExchangeRecord {
                inbound_transfers: self
                    .inbound_transfers
                    .into_iter()
                    .map(|(device, transfer)| (device, transfer.into_transfer()))
                    .collect(),
                completed_inbound_transfers: self.completed_inbound_transfers,
            },
            branch_recovery: self.branch_recovery.into_record()?,
        })
    }
}

impl PeerLinkV5 {
    fn from_snapshot(link: &PeerLinkSnapshot) -> Self {
        match link {
            PeerLinkSnapshot::Member(member) => Self::Member(MemberLinkV5 {
                relation: match member.relation {
                    PeerRelation::Unconfirmed => PeerRelationV5::Unconfirmed,
                    PeerRelation::Consistent => PeerRelationV5::Consistent,
                    PeerRelation::UpgradeRequired => PeerRelationV5::UpgradeRequired,
                    PeerRelation::AwaitingLocalDecision => PeerRelationV5::AwaitingLocalDecision,
                    PeerRelation::Diverged => PeerRelationV5::Diverged,
                    PeerRelation::Invalid => PeerRelationV5::Invalid,
                    PeerRelation::AwaitingPeerDecision => PeerRelationV5::AwaitingPeerDecision,
                },
                confirmed_position: member
                    .confirmed_position
                    .as_ref()
                    .map(PositionDto::from_position),
                sync_pending_since_revision: member.sync.pending_since_revision,
                sync_retry_attempt: member.sync.retry_attempt,
                sync_next_attempt_at_ms: member.sync.next_attempt_at_ms,
                sync_last_outcome: match member.sync.last_outcome {
                    PeerSyncOutcome::Never => PeerSyncOutcomeV5::Never,
                    PeerSyncOutcome::Deferred => PeerSyncOutcomeV5::Deferred,
                    PeerSyncOutcome::Acked => PeerSyncOutcomeV5::Acked,
                    PeerSyncOutcome::StableRejected => PeerSyncOutcomeV5::StableRejected,
                },
                outgoing_decision: member.outgoing_decision.as_deref().cloned(),
            }),
            PeerLinkSnapshot::Departing(departing) => Self::Departing(DepartingLinkV5 {
                notice: (*departing.notice).clone(),
                since_ms: departing.since_ms,
            }),
        }
    }

    fn into_snapshot(self) -> PeerLinkSnapshot {
        match self {
            Self::Member(member) => PeerLinkSnapshot::Member(MemberLinkSnapshot {
                relation: match member.relation {
                    PeerRelationV5::Unconfirmed => PeerRelation::Unconfirmed,
                    PeerRelationV5::Consistent => PeerRelation::Consistent,
                    PeerRelationV5::UpgradeRequired => PeerRelation::UpgradeRequired,
                    PeerRelationV5::AwaitingLocalDecision => PeerRelation::AwaitingLocalDecision,
                    PeerRelationV5::Diverged => PeerRelation::Diverged,
                    PeerRelationV5::Invalid => PeerRelation::Invalid,
                    PeerRelationV5::AwaitingPeerDecision => PeerRelation::AwaitingPeerDecision,
                },
                confirmed_position: member.confirmed_position.map(PositionDto::into_position),
                sync: PeerSyncBackoffSnapshot {
                    pending_since_revision: member.sync_pending_since_revision,
                    retry_attempt: member.sync_retry_attempt,
                    next_attempt_at_ms: member.sync_next_attempt_at_ms,
                    last_outcome: match member.sync_last_outcome {
                        PeerSyncOutcomeV5::Never => PeerSyncOutcome::Never,
                        PeerSyncOutcomeV5::Deferred => PeerSyncOutcome::Deferred,
                        PeerSyncOutcomeV5::Acked => PeerSyncOutcome::Acked,
                        PeerSyncOutcomeV5::StableRejected => PeerSyncOutcome::StableRejected,
                    },
                },
                outgoing_decision: member.outgoing_decision.map(Box::new),
            }),
            Self::Departing(departing) => PeerLinkSnapshot::Departing(DepartingLinkSnapshot {
                notice: Box::new(departing.notice),
                since_ms: departing.since_ms,
            }),
        }
    }
}

impl MemberEffectV5 {
    fn from_snapshot(effect: &UnfinishedMemberEffectSnapshot) -> Self {
        Self {
            event_id: effect.event_id,
            kind: match effect.kind {
                MemberEffectKind::AddDevice => MemberEffectKindV5::AddDevice,
                MemberEffectKind::RemoveDevice => MemberEffectKindV5::RemoveDevice,
            },
            phase: match effect.phase {
                MemberEffectPhase::Prepared => MemberEffectPhaseV5::Prepared,
                MemberEffectPhase::MemberFactsApplied => MemberEffectPhaseV5::MemberFactsApplied,
                MemberEffectPhase::SecurityApplied => MemberEffectPhaseV5::SecurityApplied,
            },
            affected_device_ids: effect.affected_device_ids.clone(),
            material: match &effect.material {
                MemberEffectMaterial::Event(event) => MemberEffectMaterialV5::Event(event.clone()),
                MemberEffectMaterial::InitiatedRemoval {
                    event,
                    retained_device_ids,
                } => MemberEffectMaterialV5::InitiatedRemoval {
                    event: event.clone(),
                    retained_device_ids: retained_device_ids.clone(),
                },
                MemberEffectMaterial::Decision(decision) => {
                    MemberEffectMaterialV5::Decision(decision.clone())
                }
            },
        }
    }

    fn into_snapshot(self) -> UnfinishedMemberEffectSnapshot {
        UnfinishedMemberEffectSnapshot {
            event_id: self.event_id,
            kind: match self.kind {
                MemberEffectKindV5::AddDevice => MemberEffectKind::AddDevice,
                MemberEffectKindV5::RemoveDevice => MemberEffectKind::RemoveDevice,
            },
            phase: match self.phase {
                MemberEffectPhaseV5::Prepared => MemberEffectPhase::Prepared,
                MemberEffectPhaseV5::MemberFactsApplied => MemberEffectPhase::MemberFactsApplied,
                MemberEffectPhaseV5::SecurityApplied => MemberEffectPhase::SecurityApplied,
            },
            affected_device_ids: self.affected_device_ids,
            material: match self.material {
                MemberEffectMaterialV5::Event(event) => MemberEffectMaterial::Event(event),
                MemberEffectMaterialV5::InitiatedRemoval {
                    event,
                    retained_device_ids,
                } => MemberEffectMaterial::InitiatedRemoval {
                    event,
                    retained_device_ids,
                },
                MemberEffectMaterialV5::Decision(decision) => {
                    MemberEffectMaterial::Decision(decision)
                }
            },
        }
    }
}
