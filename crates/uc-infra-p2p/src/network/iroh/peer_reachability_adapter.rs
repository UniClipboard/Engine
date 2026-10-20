//! presence 原生边界只导出 Router bridge 与协议常量；业务归 Application 完整 owner。
pub use super::connectivity::IrohPeerReachabilityHandler;
pub use uc_sync_protocol::peer_reachability::{
    LEGACY_PEER_REACHABILITY_ALPN, PEER_REACHABILITY_ALPN,
};

#[cfg(test)]
mod tests;
