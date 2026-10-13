//! Shared outbound send of active-clipboard state (0xC3).
//!
//! Single implementation of "send this converged active-clipboard state to a
//! peer under the full outbound gate", reused by every 0xC3 origination path:
//!
//! * inbound re-broadcast (after an inbound observation is honoured — the
//!   core invariant "register advanced ⟺ OS write succeeded ⟺ re-broadcast"),
//! * restore broadcast (after a local history restore advances the register),
//! * peer-online resync (after a peer comes online — send our current
//!   register to it so both ends converge via LWW).
//!
//! The full outbound gate (issue #1017 D2) is applied in the single-target
//! step: `send_enabled` ∧ `send_content_types`, threaded via the activation's
//! content category set.

use futures::{stream, StreamExt};
use std::sync::Arc;

use uc_core::clipboard::{ActiveClipboardState, ClipboardContentCategorySet};
use uc_core::ids::DeviceId;
use uc_core::ports::clipboard::ActiveClipboardDispatchPort;
use uc_core::ports::{PeerAddressRepositoryPort, PeerReachabilityPort, ReachabilityState};
use uc_observability_contract::{error_source::io_error_kind, uc_debug, uc_info, uc_warn};

use crate::deps::{CurrentSpaceMemberScope, CurrentSpaceMemberScopePort};

use super::super::send_gate::MemberSendGate;

/// 触发来自完整业务入口，不以网络方法名推断原因。
#[derive(Clone, Copy)]
pub(crate) enum ActiveStateBroadcastTrigger {
    HistoryRestore,
    PeerOnline,
    InboundConvergence,
    LocalActivation,
}
impl ActiveStateBroadcastTrigger {
    fn name(self) -> &'static str {
        match self {
            Self::HistoryRestore => "history_restore",
            Self::PeerOnline => "peer_online",
            Self::InboundConvergence => "inbound_convergence",
            Self::LocalActivation => "local_activation",
        }
    }
}

/// 每轮只轮询四个目标 future，不为尚未开始的目标分配网络预算或后台任务。
pub(crate) async fn send_active_state_to_targets(
    dispatch: &Arc<dyn ActiveClipboardDispatchPort>,
    send_gate: &MemberSendGate,
    targets: impl IntoIterator<Item = DeviceId>,
    state: &ActiveClipboardState,
    categories: &ClipboardContentCategorySet,
    scope: &CurrentSpaceMemberScope,
    trigger: ActiveStateBroadcastTrigger,
) {
    let outcomes = stream::iter(
        targets
            .into_iter()
            .filter(|target| *target != state.activated_by),
    )
    .map(|target| async move {
        if !send_gate
            .is_send_allowed_with_scope(&target, categories, scope)
            .await
        {
            return (0usize, 0usize, 1usize);
        }
        if let Err(error) = dispatch.dispatch(&target, state).await {
            uc_debug!(
                error_kind = "peer_dispatch",
                io_error_kind = io_error_kind(&error),
                "active state send: per-peer dispatch failed (isolated)"
            );
            (0, 1, 0)
        } else {
            (1, 0, 0)
        }
    })
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
    let (sent, failed, skipped) = outcomes
        .into_iter()
        .fold((0, 0, 0), |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2));
    if sent + failed > 0 {
        uc_info!(
            trigger = trigger.name(),
            outcome = if failed == 0 {
                "sent_locally"
            } else {
                "partial_failure"
            },
            success = sent,
            failed = failed,
            skipped = skipped,
            total = sent + failed + skipped,
            "active clipboard broadcast settled locally; remote application is unconfirmed"
        );
    } else {
        uc_debug!(
            trigger = trigger.name(),
            skipped = skipped,
            "active clipboard broadcast had no network work"
        );
    }
}

/// Fan `state` out to every allowed peer.
///
/// The roster is the set of peers we hold an address for
/// (`peer_addr_repo.list()`), so a peer with no address is silently skipped
/// (offline / never reachable). The device that activated the state
/// (`state.activated_by`) is never echoed back to, and a peer the peer_reachability
/// tracker already reports `Offline` is skipped without dialing. Each surviving
/// target is gated by the full outbound gate (`send_enabled` ∧
/// `send_content_types`, the latter via `categories`). Per-peer dispatch
/// failures are isolated and logged — the register is convergent, so a missed
/// send is recovered by a later advance or a peer-online resync.
pub(crate) async fn fan_out_active_state(
    dispatch: &Arc<dyn ActiveClipboardDispatchPort>,
    peer_addr_repo: &Arc<dyn PeerAddressRepositoryPort>,
    peer_scope: &Arc<dyn CurrentSpaceMemberScopePort>,
    peer_reachability: &Arc<dyn PeerReachabilityPort>,
    send_gate: &MemberSendGate,
    state: &ActiveClipboardState,
    categories: &ClipboardContentCategorySet,
    trigger: ActiveStateBroadcastTrigger,
) {
    let scope = match peer_scope.snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => {
            uc_warn!(
                error_kind = "peer_scope_unavailable",
                trigger = trigger.name(),
                io_error_kind = io_error_kind(&err),
                "active state fan-out skipped: current peer scope unavailable"
            );
            return;
        }
    };
    let records = match peer_addr_repo.list().await {
        Ok(r) => r,
        Err(err) => {
            uc_warn!(
                error_kind = "peer_address_list",
                trigger = trigger.name(),
                io_error_kind = io_error_kind(&err),
                "active state fan-out skipped: peer_addr_repo.list failed"
            );
            return;
        }
    };

    let mut targets = Vec::new();
    for record in records {
        let target = record.device_id;
        if !scope.usable_peer_device_ids.contains(&target) {
            continue;
        }
        // Never echo the state back to the device that activated it.
        if target == state.activated_by {
            continue;
        }
        // Skip peers the peer_reachability tracker already knows are offline (mirrors the
        // 0xC1 dispatch preflight): the roster can carry stale/ghost members,
        // and dialing each costs a multi-second connect timeout. `Unknown` is
        // deliberately NOT pre-filtered — the dispatch adapter marks peers
        // offline on its own dial failures, so an unprobed peer still gets one
        // real attempt rather than being silently dropped.
        if matches!(
            peer_reachability.current_state(&target).await,
            ReachabilityState::Offline
        ) {
            uc_debug!("active state fan-out: skipping peer known offline (deferred)");
            continue;
        }
        targets.push(target);
    }
    send_active_state_to_targets(
        dispatch, send_gate, targets, state, categories, &scope, trigger,
    )
    .await;
}
