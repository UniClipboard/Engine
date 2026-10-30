//! 发送侧完整进度的稳定宿主合同；不暴露协议登记、绑定或内部序号。
use super::{ReceiveItemFailureReason, ReceiveItemKind};
use serde::{Deserialize, Serialize};
use std::fmt;
use uc_observability_contract::wire_enum;

wire_enum! {
    serde
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum OutboundProgressState {
        Waiting => "waiting",
        Transferring => "transferring",
        Processing => "processing",
        Retrying => "retrying",
        Completed => "completed",
        Failed => "failed",
        Cancelled => "cancelled",
        Unconfirmed => "unconfirmed",
    }
}

wire_enum! {
    serde
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum OutboundProgressPhase {
        Waiting => "waiting",
        Transferring => "transferring",
        Processing => "processing",
        Retrying => "retrying",
    }
}

wire_enum! {
    serde
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum OutboundProgressFreshness {
        Waiting => "waiting",
        Live => "live",
        Unconfirmed => "unconfirmed",
        Settled => "settled",
    }
}

wire_enum! {
    serde
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum OutboundProgressBatchState {
        NoItems => "no_items",
        Waiting => "waiting",
        InProgress => "in_progress",
        Unconfirmed => "unconfirmed",
        PartiallyUnconfirmed => "partially_unconfirmed",
        Completed => "completed",
        Failed => "failed",
        PartiallyFailed => "partially_failed",
        Cancelled => "cancelled",
        PartiallyCancelled => "partially_cancelled",
    }
}

wire_enum! {
    serde
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum OutboundProgressCancellationReason {
        LocalUser => "local_user",
        RemotePeer => "remote_peer",
        Replaced => "replaced",
        Timeout => "timeout",
        ConnectivityRecovery => "connectivity_recovery",
        Unknown => "unknown",
    }
}

wire_enum! {
    serde
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum OutboundProgressReconciliation {
        Running => "running",
        Settled => "settled",
        Superseded => "superseded",
        NotRetained => "not_retained",
        Unsupported => "unsupported",
        Unreachable => "unreachable",
        Busy => "busy",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundProgressItemSummary {
    pub kind: ReceiveItemKind,
    pub index: u16,
    pub state: OutboundProgressState,
    pub last_known_phase: Option<OutboundProgressPhase>,
    pub round: Option<u32>,
    pub completed_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub failure_reason: Option<ReceiveItemFailureReason>,
    pub cancellation_reason: Option<OutboundProgressCancellationReason>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundProgressSummary {
    pub entry_id: String,
    pub target_device_id: String,
    pub attempt_id: Option<String>,
    pub revision: u64,
    pub freshness: OutboundProgressFreshness,
    pub freshness_available: bool,
    pub state: OutboundProgressBatchState,
    pub items_total: u32,
    pub items_waiting: u32,
    pub items_running: u32,
    pub items_completed: u32,
    pub items_failed: u32,
    pub items_cancelled: u32,
    pub items_unconfirmed: u32,
    pub items: Vec<OutboundProgressItemSummary>,
}
impl fmt::Debug for OutboundProgressSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OutboundProgressSummary")
            .field("state", &self.state)
            .field("freshness", &self.freshness)
            .field("items_total", &self.items_total)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct OutboundProgressInput {
    pub entry_id: String,
    pub target_device_id: String,
}
#[derive(Clone, PartialEq, Eq)]
pub struct ReconcileOutboundProgressInput {
    pub entry_id: String,
    pub target_device_id: String,
    pub expected_attempt_id: Option<String>,
}
impl fmt::Debug for OutboundProgressInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OutboundProgressInput(<redacted>)")
    }
}
impl fmt::Debug for ReconcileOutboundProgressInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReconcileOutboundProgressInput")
            .field("has_expected_attempt", &self.expected_attempt_id.is_some())
            .finish_non_exhaustive()
    }
}

impl fmt::Display for OutboundProgressState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.wire())
    }
}

impl fmt::Display for OutboundProgressPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.wire())
    }
}

impl fmt::Display for OutboundProgressFreshness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.wire())
    }
}

impl fmt::Display for OutboundProgressBatchState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.wire())
    }
}

impl fmt::Display for OutboundProgressCancellationReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.wire())
    }
}

impl fmt::Display for OutboundProgressReconciliation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.wire())
    }
}
