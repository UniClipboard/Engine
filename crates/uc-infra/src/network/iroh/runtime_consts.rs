//! Process-wide iroh runtime configuration.
//!
//! The active node owns these values through `NodeRunLease`. The state is
//! reset before that lease is released, so a replacement node cannot inherit a
//! previous node's bind-time configuration.

#[cfg(not(any(test, feature = "in-process-multi-node")))]
use std::sync::Mutex;

#[cfg(not(any(test, feature = "in-process-multi-node")))]
use tracing::warn;
use uc_core::network::TrustedNetworks;

/// 活动节点在绑定时固定的出站拨号策略。
///
/// 地址发现结果由 endpoint 的 `AddrFilter` 过滤，但直接交给 `connect` 的地址
/// （已保存的对端地址、邀请路由）不经过它；出站拨号读取这份策略，保证两条路径
/// 使用同一可信判定。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DialPolicy {
    pub lan_only: bool,
    pub trusted_networks: TrustedNetworks,
}

#[cfg(not(any(test, feature = "in-process-multi-node")))]
static DIAL_POLICY: Mutex<Option<DialPolicy>> = Mutex::new(None);

/// Install the active node's dial policy. Test multi-node harnesses do not
/// share a process-wide policy and therefore retain the no-op implementation.
pub(crate) fn install_dial_policy(policy: DialPolicy) {
    #[cfg(not(any(test, feature = "in-process-multi-node")))]
    {
        let mut current = DIAL_POLICY.lock().unwrap_or_else(|poisoned| {
            warn!("iroh dial policy lock poisoned while installing policy");
            poisoned.into_inner()
        });
        *current = Some(policy);
    }
    #[cfg(any(test, feature = "in-process-multi-node"))]
    {
        let _ = policy;
    }
}

/// Clear the active node's dial policy before its runtime lease is released.
/// Test multi-node harnesses intentionally have no shared policy.
#[cfg(not(any(test, feature = "in-process-multi-node")))]
pub(crate) fn clear_dial_policy() {
    let mut current = DIAL_POLICY.lock().unwrap_or_else(|poisoned| {
        warn!("iroh dial policy lock poisoned while clearing policy");
        poisoned.into_inner()
    });
    *current = None;
}

/// The active production node's dial policy. Without an active node, and in
/// test multi-node harnesses, this is the default: not LAN-only, no trusted
/// networks.
pub(crate) fn dial_policy() -> DialPolicy {
    #[cfg(not(any(test, feature = "in-process-multi-node")))]
    {
        let current = DIAL_POLICY.lock().unwrap_or_else(|poisoned| {
            warn!("iroh dial policy lock poisoned while reading policy");
            poisoned.into_inner()
        });
        current.clone().unwrap_or_default()
    }
    #[cfg(any(test, feature = "in-process-multi-node"))]
    {
        DialPolicy::default()
    }
}

/// Whether the active production node is in LAN-only mode.
pub(crate) fn lan_only() -> bool {
    dial_policy().lan_only
}
