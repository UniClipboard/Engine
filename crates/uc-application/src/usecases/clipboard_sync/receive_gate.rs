//! `MemberReceiveGate` — the per-device inbound receive gate.
//!
//! Single source of truth for "should the local device accept clipboard
//! state from `peer`", shared by every inbound path (bulk content ingest and
//! the active-clipboard state handler). Two independent stages:
//!
//! 1. **Device-level kill switch** — `receive_enabled`. Cheap; check first.
//! 2. **Content-type filter** — `receive_content_types`, AND-of-allowed
//!    across the snapshot's category set (see `uc-core` `category.rs`). An
//!    empty set passes for wire compatibility; a non-empty set passes only
//!    when every category in it is allowed.
//!
//! Member-repo misses and lookup errors **fail closed**. A local receive
//! preference is a privacy boundary and cannot be bypassed when it cannot be
//! verified.

use std::sync::Arc;

use tracing::{info, warn};

use uc_core::clipboard::ClipboardContentCategorySet;
use uc_core::ids::DeviceId;
use uc_core::MemberRepositoryPort;

/// Reads a peer's per-device sync preferences to decide whether inbound
/// clipboard data from it should be accepted.
pub(crate) struct MemberReceiveGate {
    member_repo: Arc<dyn MemberRepositoryPort>,
}

impl MemberReceiveGate {
    pub(crate) fn new(member_repo: Arc<dyn MemberRepositoryPort>) -> Self {
        Self { member_repo }
    }

    /// Stage 1: device-level kill switch. Returns `true` when the local
    /// device should accept clipboard data from `peer` at all. Reads
    /// `SpaceMember.sync_preferences.receive_enabled`; denies on lookup error
    /// or a missing record because a local receive control must be verifiable
    /// before accepting remote content.
    pub(crate) async fn is_receive_allowed(&self, peer: &DeviceId) -> bool {
        match self.member_repo.get(peer).await {
            Ok(Some(member)) if member.sync_preferences.receive_enabled => true,
            Ok(Some(_)) => {
                info!(
                    peer = %peer.as_str(),
                    reason = "receive_disabled_by_user",
                    "receive gate: dropping inbound per per-device sync preferences"
                );
                false
            }
            Ok(None) => {
                warn!(
                    peer = %peer.as_str(),
                    reason = "member_not_found",
                    "receive gate: dropping inbound because member preferences are unavailable"
                );
                false
            }
            Err(err) => {
                warn!(
                    peer = %peer.as_str(),
                    error = %err,
                    reason = "member_lookup_failed",
                    "receive gate: dropping inbound because member preferences cannot be read"
                );
                false
            }
        }
    }

    /// Stage 2: content-type filter, AND-of-allowed across the snapshot's
    /// category set. Empty category sets remain allowed for compatibility with
    /// legacy or opaque payloads; a non-empty set passes only when every
    /// category is allowed by `receive_content_types`.
    pub(crate) async fn is_receive_category_allowed(
        &self,
        peer: &DeviceId,
        categories: &ClipboardContentCategorySet,
    ) -> bool {
        match self.member_repo.get(peer).await {
            // Recheck the master switch because a user may disable receive
            // after the pre-decryption check and before categories are known.
            Ok(Some(member)) if !member.sync_preferences.receive_enabled => {
                info!(
                    peer = %peer.as_str(),
                    reason = "receive_disabled_by_user",
                    "receive gate: dropping inbound per per-device sync preferences"
                );
                false
            }
            Ok(Some(member))
                if categories.allowed_by(&member.sync_preferences.receive_content_types) =>
            {
                true
            }
            Ok(Some(member)) => {
                info!(
                    peer = %peer.as_str(),
                    categories = %categories.labels(),
                    denied = %categories.denied_labels(&member.sync_preferences.receive_content_types),
                    reason = "content_type_disabled_by_user",
                    "receive gate: dropping inbound per per-device content_types filter"
                );
                false
            }
            Ok(None) => {
                warn!(
                    peer = %peer.as_str(),
                    reason = "member_not_found",
                    "receive gate: dropping inbound because member preferences are unavailable"
                );
                false
            }
            Err(err) => {
                warn!(
                    peer = %peer.as_str(),
                    error = %err,
                    reason = "member_lookup_failed",
                    "receive gate: dropping inbound because member preferences cannot be read"
                );
                false
            }
        }
    }
}
