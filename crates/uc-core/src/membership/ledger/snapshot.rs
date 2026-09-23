use std::collections::BTreeMap;

use crate::ids::DeviceId;
use crate::membership::{
    BaseMembershipHistoryPosition, MemberInstanceId, MembershipDecisionV2, MembershipEventId,
    MembershipEventV2, VersionedMembershipHistory,
};

use super::{
    DepartingLink, LedgerTransitionError, MemberEffectKind, MemberEffectMaterial,
    MemberEffectPhase, MemberLink, MembershipLedger, PeerLink, PeerRelation, PeerSyncBackoff,
    PeerSyncOutcome, UnfinishedMemberEffect,
};

/// 与持久层交换的纯数据。不带格式版本；字节布局与版本演进由 Infra 负责。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipLedgerSnapshot {
    pub revision: u64,
    pub history: VersionedMembershipHistory,
    pub local_device_id: DeviceId,
    pub local_member: MemberInstanceId,
    pub peers: BTreeMap<DeviceId, PeerLinkSnapshot>,
    pub effects: Vec<UnfinishedMemberEffectSnapshot>,
    pub sync_cursor: Option<DeviceId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerLinkSnapshot {
    Member(MemberLinkSnapshot),
    Departing(DepartingLinkSnapshot),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberLinkSnapshot {
    pub relation: PeerRelation,
    pub confirmed_position: Option<BaseMembershipHistoryPosition>,
    pub sync: PeerSyncBackoffSnapshot,
    pub outgoing_decision: Option<Box<MembershipDecisionV2>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerSyncBackoffSnapshot {
    pub pending_since_revision: Option<u64>,
    pub retry_attempt: u32,
    pub next_attempt_at_ms: i64,
    pub last_outcome: PeerSyncOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepartingLinkSnapshot {
    pub notice: Box<MembershipEventV2>,
    pub since_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnfinishedMemberEffectSnapshot {
    pub event_id: MembershipEventId,
    pub kind: MemberEffectKind,
    pub phase: MemberEffectPhase,
    pub affected_device_ids: Vec<DeviceId>,
    pub material: MemberEffectMaterial,
}

impl MembershipLedger {
    pub fn snapshot(&self) -> MembershipLedgerSnapshot {
        MembershipLedgerSnapshot {
            revision: self.revision,
            history: self.history.clone(),
            local_device_id: self.local_device_id,
            local_member: self.local_member,
            peers: self
                .peers
                .iter()
                .map(|(device, link)| {
                    let snapshot = match link {
                        PeerLink::Member(member) => PeerLinkSnapshot::Member(MemberLinkSnapshot {
                            relation: member.relation(),
                            confirmed_position: member.confirmed_position().cloned(),
                            sync: PeerSyncBackoffSnapshot {
                                pending_since_revision: member.sync().pending_since_revision(),
                                retry_attempt: member.sync().retry_attempt(),
                                next_attempt_at_ms: member.sync().next_attempt_at_ms(),
                                last_outcome: member.sync().last_outcome(),
                            },
                            outgoing_decision: member.outgoing_decision().cloned().map(Box::new),
                        }),
                        PeerLink::Departing(departing) => {
                            PeerLinkSnapshot::Departing(DepartingLinkSnapshot {
                                notice: Box::new(departing.notice().clone()),
                                since_ms: departing.since_ms(),
                            })
                        }
                    };
                    (*device, snapshot)
                })
                .collect(),
            effects: self
                .effects
                .values()
                .map(|effect| UnfinishedMemberEffectSnapshot {
                    event_id: effect.event_id(),
                    kind: effect.kind(),
                    phase: effect.phase(),
                    affected_device_ids: effect.affected_device_ids().to_vec(),
                    material: effect.material().clone(),
                })
                .collect(),
            sync_cursor: self.sync_cursor,
        }
    }

    /// 从持久快照恢复，并重新校验全部不变量；不一致的快照被拒绝，不做修复。
    pub fn restore(snapshot: MembershipLedgerSnapshot) -> Result<Self, LedgerTransitionError> {
        let mut effects = BTreeMap::new();
        for effect in snapshot.effects {
            let event_id = effect.event_id;
            let restored = UnfinishedMemberEffect::from_parts(
                effect.event_id,
                effect.kind,
                effect.phase,
                effect.affected_device_ids,
                effect.material,
            );
            if effects.insert(event_id, restored).is_some() {
                return Err(LedgerTransitionError::InvalidSnapshot);
            }
        }
        let membership = Self {
            revision: snapshot.revision,
            history: snapshot.history,
            local_device_id: snapshot.local_device_id,
            local_member: snapshot.local_member,
            peers: snapshot
                .peers
                .into_iter()
                .map(|(device, link)| {
                    let restored = match link {
                        PeerLinkSnapshot::Member(member) => {
                            PeerLink::Member(MemberLink::from_parts(
                                member.relation,
                                member.confirmed_position,
                                PeerSyncBackoff::from_parts(
                                    member.sync.pending_since_revision,
                                    member.sync.retry_attempt,
                                    member.sync.next_attempt_at_ms,
                                    member.sync.last_outcome,
                                ),
                                member.outgoing_decision.map(|decision| *decision),
                            ))
                        }
                        PeerLinkSnapshot::Departing(departing) => PeerLink::Departing(
                            DepartingLink::new(*departing.notice, departing.since_ms),
                        ),
                    };
                    (device, restored)
                })
                .collect(),
            effects,
            sync_cursor: snapshot.sync_cursor,
        };
        membership.validate()?;
        Ok(membership)
    }
}
