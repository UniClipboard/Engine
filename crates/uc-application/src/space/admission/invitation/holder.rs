//! In-memory [`PairingInvitation`] holder for sponsor-side pending
//! invitations.
//!
//! Slice 1 decision Q-2: invitations live **only** in this holder and are
//! dropped when the process exits. No replacement axis (no "redis holder"
//! etc.) — invitations are intrinsically short-lived (typical TTL ≤ 10
//! minutes) and re-issuing on next launch is acceptable, so we don't need
//! a port here.
//!
//! Operations:
//! * `insert` — parking path, called by `IssuePairingInvitationUseCase`
//!   after a successful rendezvous issue.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use tokio::sync::Mutex;

use crate::space::lifecycle::PendingSpaceInvitationResetPort;

use uc_core::membership::InvitationId;
use uc_core::pairing::invitation::{InvitationCode, InvitationState, PairingInvitation};

/// Process-local map of outstanding [`PairingInvitation`]s keyed by code.
///
/// The code is chosen as the key because the sponsor-side `Incoming`
/// event (P7e) carries only the joiner-echoed code, not the aggregate's
/// internal pointer.
pub(crate) struct InMemoryPairingInvitationHolder {
    state: Mutex<InvitationHolderState>,
}

#[derive(Default)]
struct InvitationHolderState {
    by_code: HashMap<InvitationCode, PairingInvitation>,
    code_by_id: BTreeMap<InvitationId, InvitationCode>,
}

impl InMemoryPairingInvitationHolder {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(InvitationHolderState::default()),
        }
    }

    /// Insert (or overwrite) the aggregate keyed by its code.
    ///
    /// Overwrite semantics: a fresh `issue_invitation()` that reuses the
    /// same code (rendezvous adapter decides code uniqueness) replaces the
    /// previous slot. The caller's invariant is "the latest issue wins";
    /// we don't enforce a "single pending per device" rule here because
    /// that's a UI-level policy decision, not a core invariant.
    pub(crate) async fn insert(&self, invitation: PairingInvitation) {
        let code = invitation.code().clone();
        let invitation_id = invitation.invitation_id();
        let mut state = self.state.lock().await;
        if let Some(previous) = state.by_code.remove(&code) {
            state.code_by_id.remove(&previous.invitation_id());
        }
        if let Some(previous_code) = state.code_by_id.insert(invitation_id, code.clone()) {
            state.by_code.remove(&previous_code);
        }
        state.by_code.insert(code, invitation);
    }

    /// Snapshot the **earliest-expiring** outstanding invitation, if any.
    ///
    /// Returned without consuming, intended for read-only query paths
    /// (Slice4 P3 T3.2 `SpaceFacade::query_setup_state`). Callers
    /// must not assume a single pending invitation exists; this just
    /// gives a deterministic representative when multiple are parked.
    pub(crate) async fn snapshot_earliest(
        &self,
    ) -> Option<(
        InvitationCode,
        uc_core::pairing::invitation::FullInvitation,
        DateTime<Utc>,
    )> {
        let state = self.state.lock().await;
        state
            .by_code
            .values()
            .filter_map(|inv| match inv.state() {
                InvitationState::Pending { expires_at } => Some((
                    inv.code().clone(),
                    inv.full_invitation().clone(),
                    *expires_at,
                )),
                _ => None,
            })
            .min_by_key(|(_, _, exp)| *exp)
    }

    pub(crate) async fn pending_codes(&self) -> Vec<InvitationCode> {
        self.state.lock().await.by_code.keys().cloned().collect()
    }

    /// Drop every outstanding invitation, returning the count removed.
    ///
    /// Used by Slice4 P3 T3.2 `SpaceFacade::cancel_invitation` and
    /// `reset` to wipe in-flight pairing state.
    pub(crate) async fn cancel_all(&self) -> usize {
        let mut state = self.state.lock().await;
        let count = state.by_code.len();
        state.by_code.clear();
        state.code_by_id.clear();
        count
    }

    /// Count of outstanding entries (test-only — not part of the
    /// application-facing surface).
    #[cfg(test)]
    pub(crate) async fn len(&self) -> usize {
        self.state.lock().await.by_code.len()
    }

    /// Test-only: look up by code without consuming the aggregate.
    #[cfg(test)]
    pub(crate) async fn get_for_test(&self, code: &InvitationCode) -> Option<PairingInvitation> {
        self.state.lock().await.by_code.get(code).cloned()
    }
}

#[async_trait::async_trait]
impl PendingSpaceInvitationResetPort for InMemoryPairingInvitationHolder {
    async fn cancel_all(&self) -> usize {
        InMemoryPairingInvitationHolder::cancel_all(self).await
    }
}

impl Default for InMemoryPairingInvitationHolder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::{DateTime, Duration, Utc};

    use uc_core::ids::DeviceId;
    use uc_core::pairing::invitation::InvitationState;

    fn fixed_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-04-20T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn pending(code: &str) -> PairingInvitation {
        let issued = fixed_now();
        let expires = issued + Duration::minutes(5);
        let invitation_byte = code.as_bytes().first().copied().unwrap_or(1);
        let (invitation, _) = PairingInvitation::issue(
            uc_core::membership::InvitationId::from_bytes([invitation_byte; 32])
                .expect("valid invitation id"),
            InvitationCode::new(code),
            uc_core::pairing::invitation::FullInvitation::new(format!("ucspace1_{code}"))
                .expect("valid full invitation"),
            issued,
            expires,
            DeviceId::new("device-1"),
            0,
        );
        invitation
    }

    #[tokio::test]
    async fn insert_stores_aggregate_by_code() {
        let holder = InMemoryPairingInvitationHolder::new();
        holder.insert(pending("ABCD-1234")).await;
        assert_eq!(holder.len().await, 1);
        let stored = holder
            .get_for_test(&InvitationCode::new("ABCD-1234"))
            .await
            .expect("aggregate stored");
        assert!(matches!(stored.state(), InvitationState::Pending { .. }));
    }

    #[tokio::test]
    async fn insert_with_same_code_overwrites() {
        let holder = InMemoryPairingInvitationHolder::new();
        holder.insert(pending("SAME")).await;
        holder.insert(pending("SAME")).await;
        assert_eq!(holder.len().await, 1, "overwrite, not duplicate");
    }

    #[tokio::test]
    async fn distinct_codes_coexist() {
        let holder = InMemoryPairingInvitationHolder::new();
        holder.insert(pending("ONE")).await;
        holder.insert(pending("TWO")).await;
        assert_eq!(holder.len().await, 2);
    }
}
