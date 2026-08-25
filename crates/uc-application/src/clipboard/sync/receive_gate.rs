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
use uc_core::MemberSyncPreferences;

use crate::deps::CurrentSpaceMemberScopePort;

/// Reads a peer's per-device sync preferences to decide whether inbound
/// clipboard data from it should be accepted.
pub(crate) struct MemberReceiveGate {
    member_repo: Arc<dyn MemberRepositoryPort>,
    member_scope: Arc<dyn CurrentSpaceMemberScopePort>,
}

pub(crate) struct MemberReceivePermit {
    peer: DeviceId,
    revision: u64,
    preferences: MemberSyncPreferences,
}

impl MemberReceiveGate {
    pub(crate) fn new(
        member_repo: Arc<dyn MemberRepositoryPort>,
        member_scope: Arc<dyn CurrentSpaceMemberScopePort>,
    ) -> Self {
        Self {
            member_repo,
            member_scope,
        }
    }

    /// Stage 1: device-level kill switch. Returns `true` when the local
    /// device should accept clipboard data from `peer` at all. Reads
    /// `SpaceMember.sync_preferences.receive_enabled`; denies on lookup error
    /// or a missing record because a local receive control must be verifiable
    /// before accepting remote content.
    pub(crate) async fn authorize(&self, peer: &DeviceId) -> Option<MemberReceivePermit> {
        let scope = match self.member_scope.snapshot().await {
            Ok(scope) if scope.usable_peer_device_ids.contains(peer) => scope,
            _ => {
                info!(
                    reason = "membership_scope_blocked",
                    "receive gate: dropping inbound from unavailable peer"
                );
                return None;
            }
        };
        match self.member_repo.get(peer).await {
            Ok(Some(member)) if member.sync_preferences.receive_enabled => {
                Some(MemberReceivePermit {
                    peer: peer.clone(),
                    revision: scope.revision,
                    preferences: member.sync_preferences,
                })
            }
            Ok(Some(_)) => {
                info!(
                    reason = "receive_disabled_by_user",
                    "receive gate: dropping inbound per per-device sync preferences"
                );
                None
            }
            Ok(None) => {
                warn!(
                    reason = "member_not_found",
                    "receive gate: dropping inbound because member preferences are unavailable"
                );
                None
            }
            Err(_err) => {
                warn!(
                    reason = "member_lookup_failed",
                    "receive gate: dropping inbound because member preferences cannot be read"
                );
                None
            }
        }
    }

    pub(crate) fn is_receive_category_allowed(
        &self,
        permit: &MemberReceivePermit,
        categories: &ClipboardContentCategorySet,
    ) -> bool {
        let _ = (&permit.peer, permit.revision);
        if categories.allowed_by(&permit.preferences.receive_content_types) {
            true
        } else {
            info!(
                reason = "content_type_disabled_by_user",
                "receive gate: dropping inbound per per-device content_types filter"
            );
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use chrono::Utc;
    use uc_core::membership::{MembershipError, SpaceMember};
    use uc_core::{MemberRepositoryPort, MemberSyncPreferences};

    use crate::deps::{
        CurrentSpaceMemberScope, CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct AllowingMemberRepo;

    #[async_trait]
    impl MemberRepositoryPort for AllowingMemberRepo {
        async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            Ok(Some(SpaceMember {
                device_id: device_id.clone(),
                device_name: "peer".to_owned(),
                identity_fingerprint: uc_core::security::IdentityFingerprint::from_raw_string(
                    "0123456789abcdef",
                )
                .unwrap(),
                joined_at: Utc::now(),
                sync_preferences: MemberSyncPreferences::default(),
            }))
        }

        async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
            Ok(Vec::new())
        }

        async fn save(&self, _member: &SpaceMember) -> Result<(), MembershipError> {
            Ok(())
        }

        async fn remove(&self, _device_id: &DeviceId) -> Result<bool, MembershipError> {
            Ok(false)
        }
    }

    struct EmptyScope;

    struct CountingScope(AtomicUsize);

    #[async_trait]
    impl CurrentSpaceMemberScopePort for EmptyScope {
        async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
            Ok(CurrentSpaceMemberScope {
                revision: 1,
                local_member_active: true,
                usable_peer_device_ids: Vec::new(),
                paused_peer_devices: Vec::new(),
            })
        }
    }

    #[async_trait]
    impl CurrentSpaceMemberScopePort for CountingScope {
        async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(CurrentSpaceMemberScope {
                revision: 7,
                local_member_active: true,
                usable_peer_device_ids: vec![DeviceId::new("peer")],
                paused_peer_devices: Vec::new(),
            })
        }
    }

    #[tokio::test]
    async fn peer_outside_final_scope_is_rejected_before_decryption() {
        let gate = MemberReceiveGate::new(Arc::new(AllowingMemberRepo), Arc::new(EmptyScope));

        assert!(gate.authorize(&DeviceId::new("peer")).await.is_none());
    }

    #[tokio::test]
    async fn one_receive_operation_reuses_one_membership_revision() {
        let scope = Arc::new(CountingScope(AtomicUsize::new(0)));
        let gate = MemberReceiveGate::new(Arc::new(AllowingMemberRepo), scope.clone());

        let permit = gate.authorize(&DeviceId::new("peer")).await.unwrap();
        assert!(gate.is_receive_category_allowed(&permit, &ClipboardContentCategorySet::empty(),));

        assert_eq!(scope.0.load(Ordering::SeqCst), 1);
    }
}
