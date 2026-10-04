//! SearchKeyDerivationPort — derives a SearchKey from the unlocked MasterKey.
//!
//! 实现位于 `uc-infra-storage`（Phase 90）。按架构规范以 HKDF-SHA256 按 profile 派生；
//! uc-core 只看到不透明的 SearchKey 输出，原始 MasterKey 字节不跨越 port 边界。

use crate::search::{RenderKey, SearchError, SearchKeyContext};
use async_trait::async_trait;

/// Port for deriving search subkeys from the currently-unlocked encryption session.
///
/// 由 `uc-infra-storage` 实现（Phase 90），以 `Arc<dyn SearchKeyDerivationPort + Send + Sync>` 注入。
#[async_trait]
pub trait SearchKeyDerivationPort: Send + Sync {
    /// Derive a SearchKey for the currently-unlocked encryption session.
    ///
    /// Returns `SearchError::SessionLocked` if no master key is available.
    /// The derivation uses HKDF-SHA256 scoped to the active profile.
    async fn derive_search_key(&self) -> Result<SearchKeyContext, SearchError>;

    /// Derive a RenderKey for the currently-unlocked encryption session.
    ///
    /// Distinct from [`derive_search_key`](Self::derive_search_key): the render
    /// key is an AEAD key for the per-entry render payload, derived with a
    /// separate HKDF `info` label so it never doubles as the HMAC term-tag key.
    ///
    /// Returns `SearchError::SessionLocked` if no master key is available.
    /// The derivation uses HKDF-SHA256 scoped to the active profile.
    async fn derive_render_key(&self) -> Result<RenderKey, SearchError>;
}
