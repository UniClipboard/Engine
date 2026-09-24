//! V4 形状到成员记录的一次性映射（计划 049 映射表）。
//!
//! 映射只缩小或保持资格，从不由旧关系补造资格：
//! - 仍有待投递移除通知的对端映射为正在离开，窗口从迁移时刻重新计时；
//! - 其余对端映射为成员并沿用关系、确认位置、同步退避与待投递决定；
//! - 已激活效果丢弃，未激活效果按载荷还原为类型化材料；
//! - 最后交给 Core 按每次转换后的同一规则规范化：已不在历史中的成员记录删除（含已移除且无待投递
//!   通知的残留）、有效对端补齐、以本机为目标的决定投递删除、非当前路径效果删除。

use std::collections::BTreeMap;

use uc_application::deps::{
    MembershipHistoryExchangeRecord, MembershipLedgerError, MembershipRecord, SpaceMembershipRecord,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    DepartingLinkSnapshot, HistoricalMembershipSignatureVerifier, MemberEffectKind,
    MemberEffectMaterial, MemberEffectPhase, MemberInstanceId, MemberLinkSnapshot,
    MembershipDecisionV2, MembershipEventV2, MembershipLedger, MembershipLedgerSnapshot,
    MembershipOperationV2, PeerLinkSnapshot, PeerRelation, PeerSyncBackoffSnapshot,
    PeerSyncOutcome, UnfinishedMemberEffectSnapshot, VersionedMembershipHistory,
};

use super::legacy::{
    LegacyDelivery, LegacyEffect, LegacyEffectKind, LegacyEffectPhase, LegacyInitiatedRemoval,
    LegacyLedger, LegacyPeer, LegacyRelationship, LegacySyncOutcome,
};
use super::parse;

pub(super) fn migrate(
    legacy: LegacyLedger,
    verifier: &dyn HistoricalMembershipSignatureVerifier,
    now_ms: i64,
) -> Result<MembershipRecord, MembershipLedgerError> {
    // 迁移本身是一次写入，修订号前进一步。
    let revision = legacy
        .revision
        .checked_add(1)
        .ok_or_else(MembershipLedgerError::corrupt)?;
    let Some(lineage_id) = legacy.lineage_id else {
        if legacy.membership_history.is_some()
            || legacy.local_device_id.is_some()
            || legacy.local_member_instance.is_some()
            || legacy.local_join_active
            || !legacy.branch_recovery.recovery_sessions.is_empty()
        {
            return Err(MembershipLedgerError::corrupt());
        }
        return Ok(MembershipRecord::NoSpace { revision });
    };
    // 旧账本只在没有当前 Space 时关闭加入门禁；有 Space 而门禁关闭的记录无法解释。
    if !legacy.local_join_active {
        return Err(MembershipLedgerError::corrupt());
    }
    let history_bytes = legacy
        .membership_history
        .ok_or(MembershipLedgerError::RecoveryRequired)?;
    let history = VersionedMembershipHistory::decode_persisted_v2(&history_bytes, verifier)
        .map_err(MembershipLedgerError::corrupt_from)?;
    let (Some(local_device_id), Some(local_member)) =
        (legacy.local_device_id, legacy.local_member_instance)
    else {
        return Err(MembershipLedgerError::corrupt());
    };
    if history.lineage_id() != lineage_id {
        return Err(MembershipLedgerError::corrupt());
    }
    let awaiting_local_decision = history.pending_removal_decision(local_member).is_some();
    let peers = legacy
        .peer_reconciliation
        .into_iter()
        .map(|(device, peer)| {
            let link = migrate_peer(&history, device, peer, awaiting_local_decision, now_ms);
            (device, link)
        })
        .collect::<BTreeMap<_, _>>();
    let effects = legacy
        .effect_journal
        .into_values()
        .filter(|effect| effect.phase != LegacyEffectPhase::Activated)
        .map(migrate_effect)
        .collect::<Result<Vec<_>, _>>()?;
    let snapshot = MembershipLedgerSnapshot {
        revision,
        history,
        local_device_id,
        local_member,
        peers,
        effects,
        sync_cursor: legacy.history_sync_cursor,
    };
    let ledger = MembershipLedger::restore_normalized(snapshot)
        .map_err(MembershipLedgerError::corrupt_from)?
        .snapshot();
    Ok(MembershipRecord::Space(Box::new(SpaceMembershipRecord {
        ledger,
        history_exchange: MembershipHistoryExchangeRecord {
            inbound_transfers: legacy
                .inbound_transfers
                .into_iter()
                .map(|(device, transfer)| (device, transfer.into_transfer()))
                .collect(),
            completed_inbound_transfers: legacy.completed_inbound_transfers,
        },
        branch_recovery: legacy.branch_recovery.into_record()?,
    })))
}

fn migrate_peer(
    history: &VersionedMembershipHistory,
    device: DeviceId,
    peer: LegacyPeer,
    awaiting_local_decision: bool,
    now_ms: i64,
) -> PeerLinkSnapshot {
    let mut notice = None;
    let mut decision = None;
    for delivery in peer.restricted_delivery {
        match delivery {
            LegacyDelivery::Event(event) if removes(history, &event, device) => {
                notice = Some(event);
            }
            LegacyDelivery::Event(_) => {}
            LegacyDelivery::Decision(signed) => decision = Some(signed),
        }
    }
    if let Some(notice) = notice {
        return PeerLinkSnapshot::Departing(DepartingLinkSnapshot {
            notice: Box::new(notice),
            since_ms: now_ms,
        });
    }
    PeerLinkSnapshot::Member(MemberLinkSnapshot {
        relation: migrate_relation(peer.relationship, awaiting_local_decision),
        confirmed_position: peer
            .confirmed_position
            .map(|position| position.into_position()),
        sync: PeerSyncBackoffSnapshot {
            pending_since_revision: peer.sync_state.pending_since_revision,
            retry_attempt: peer.sync_state.retry_attempt,
            next_attempt_at_ms: peer.sync_state.next_attempt_at_ms,
            last_outcome: match peer.sync_state.last_attempt_outcome {
                LegacySyncOutcome::Never => PeerSyncOutcome::Never,
                LegacySyncOutcome::Deferred => PeerSyncOutcome::Deferred,
                LegacySyncOutcome::Acked => PeerSyncOutcome::Acked,
                LegacySyncOutcome::StableRejected => PeerSyncOutcome::StableRejected,
            },
        },
        outgoing_decision: decision.map(Box::new),
    })
}

/// 旧关系“等待移除决定”同时用于移除发起方与被移除方；只有本机确有待决定的移除时才表示等待本机
/// 决定，其余情况重新核对。
fn migrate_relation(
    relationship: LegacyRelationship,
    awaiting_local_decision: bool,
) -> PeerRelation {
    match relationship {
        LegacyRelationship::Unknown => PeerRelation::Unconfirmed,
        LegacyRelationship::Consistent => PeerRelation::Consistent,
        LegacyRelationship::UpgradeRequired => PeerRelation::UpgradeRequired,
        LegacyRelationship::PendingRemovalDecision if awaiting_local_decision => {
            PeerRelation::AwaitingLocalDecision
        }
        LegacyRelationship::PendingRemovalDecision => PeerRelation::Unconfirmed,
        LegacyRelationship::Diverged => PeerRelation::Diverged,
        LegacyRelationship::Invalid => PeerRelation::Invalid,
    }
}

/// 移除通知必须是历史中移除该设备的事件。
fn removes(
    history: &VersionedMembershipHistory,
    event: &MembershipEventV2,
    device: DeviceId,
) -> bool {
    let MembershipOperationV2::RemoveDevice { member } = event.operation else {
        return false;
    };
    history.event(event.event_id()).is_some() && device_of(history, member) == Some(device)
}

fn device_of(history: &VersionedMembershipHistory, member: MemberInstanceId) -> Option<DeviceId> {
    history
        .admission_facts_for(member)
        .map(|facts| facts.device_id)
}

fn migrate_effect(
    effect: LegacyEffect,
) -> Result<UnfinishedMemberEffectSnapshot, MembershipLedgerError> {
    let phase = match effect.phase {
        LegacyEffectPhase::Prepared => MemberEffectPhase::Prepared,
        LegacyEffectPhase::MemberFactsApplied => MemberEffectPhase::MemberFactsApplied,
        LegacyEffectPhase::SecurityApplied => MemberEffectPhase::SecurityApplied,
        LegacyEffectPhase::Activated => return Err(MembershipLedgerError::corrupt()),
    };
    let kind = match effect.kind {
        LegacyEffectKind::AddDevice => MemberEffectKind::AddDevice,
        LegacyEffectKind::RemoveDevice => MemberEffectKind::RemoveDevice,
    };
    Ok(UnfinishedMemberEffectSnapshot {
        event_id: effect.event_id,
        kind,
        phase,
        affected_device_ids: effect.affected_device_ids,
        material: effect_material(kind, &effect.payload)?,
    })
}

/// 旧载荷不带类型标记：加入只可能是事件；移除可能是事件、本机发起的移除或本机接受的决定。
/// 三种布局按完整长度严格解析，恰好一种成立才采用。
pub(super) fn effect_material(
    kind: MemberEffectKind,
    payload: &[u8],
) -> Result<MemberEffectMaterial, MembershipLedgerError> {
    let event = parse::<MembershipEventV2>(payload).ok();
    if kind == MemberEffectKind::AddDevice {
        return event
            .map(MemberEffectMaterial::Event)
            .ok_or_else(MembershipLedgerError::corrupt);
    }
    let initiated = parse::<LegacyInitiatedRemoval>(payload).ok();
    let decision = parse::<MembershipDecisionV2>(payload).ok();
    match (event, initiated, decision) {
        (Some(event), None, None) => Ok(MemberEffectMaterial::Event(event)),
        (None, Some(initiated), None) => Ok(MemberEffectMaterial::InitiatedRemoval {
            event: initiated.event,
            retained_device_ids: initiated.retained_device_ids,
        }),
        (None, None, Some(decision)) => Ok(MemberEffectMaterial::Decision(decision)),
        _ => Err(MembershipLedgerError::corrupt()),
    }
}
