//! `MemberSendGate` — the per-device outbound send gate for active-clipboard
//! state (0xC3).
//!
//! Single source of truth for "should the local device send active-clipboard
//! state to `peer`", shared by every outbound 0xC3 origination path (inbound
//! re-broadcast and restore broadcast). Two independent stages, mirroring
//! [`MemberReceiveGate`](super::receive_gate::MemberReceiveGate):
//!
//! 1. **Device-level kill switch** — `send_enabled`. Cheap; check first.
//! 2. **Content-type filter** — `send_content_types`, AND-of-allowed across
//!    the activation's category set. An empty set passes (raw / unrecognised
//!    payload); a non-empty set passes only when every category in it is
//!    allowed.
//!
//! Both stages **fail open** on a member-repo miss or error: a transient
//! roster/repo glitch must not silently stop propagation (mirrors the bulk
//! dispatch gate's posture).

use std::sync::Arc;

use tracing::{debug, info, warn};

use uc_core::clipboard::ClipboardContentCategorySet;
use uc_core::ids::DeviceId;
use uc_core::MemberRepositoryPort;

use crate::deps::CurrentSpaceMemberScope;

/// Reads a peer's per-device sync preferences to decide whether outbound
/// active-clipboard state to it should be sent.
#[derive(Clone)]
pub(crate) struct MemberSendGate {
    member_repo: Arc<dyn MemberRepositoryPort>,
}

impl MemberSendGate {
    pub(crate) fn new(member_repo: Arc<dyn MemberRepositoryPort>) -> Self {
        Self { member_repo }
    }

    /// Full outbound gate (issue #1017 D2): `send_enabled` ∧
    /// `send_content_types`. Returns `true` when the local device should send
    /// active-clipboard state of the given `categories` to `peer`.
    ///
    /// `categories` is the content category set of the activation being
    /// propagated (see `uc-core` `category.rs`). Stage 2 applies the
    /// AND-of-allowed rule: an empty set passes (fail open); a non-empty set
    /// passes only when every category in it is allowed by the peer's
    /// `send_content_types`.
    ///
    /// Fails open on a member-repo miss / error so a transient glitch can't
    /// silently stop propagation.
    pub(crate) async fn is_send_allowed_with_scope(
        &self,
        peer: &DeviceId,
        categories: &ClipboardContentCategorySet,
        scope: &CurrentSpaceMemberScope,
    ) -> bool {
        if !scope.usable_peer_device_ids.contains(peer) {
            info!(
                reason = "membership_scope_blocked",
                "active state send gate: skipping unavailable peer"
            );
            return false;
        }
        match self.member_repo.get(peer).await {
            Ok(Some(member)) => {
                if !member.sync_preferences.send_enabled {
                    debug!(
                        reason = "send_disabled_by_user",
                        "active state send gate: skipping peer per per-device sync preferences"
                    );
                    return false;
                }
                if !categories.allowed_by(&member.sync_preferences.send_content_types) {
                    info!(
                        reason = "content_type_disabled_by_user",
                        "active state send gate: skipping peer per per-device content_types filter"
                    );
                    return false;
                }
                true
            }
            Ok(None) => {
                warn!("active state send gate: peer missing in member repo; failing open");
                true
            }
            Err(err) => {
                warn!(
                    error = %err,
                    "active state send gate: member repo lookup failed; failing open"
                );
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use chrono::Utc;
    use std::io::{self, Write};
    use std::sync::Mutex;

    use crate::deps::CurrentSpaceMemberScope;

    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

    struct CapturedLogWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for CapturedLogWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for CapturedLogs {
        type Writer = CapturedLogWriter;

        fn make_writer(&'writer self) -> Self::Writer {
            CapturedLogWriter(Arc::clone(&self.0))
        }
    }

    impl CapturedLogs {
        fn output(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }
    use uc_core::clipboard::ClipboardContentCategory;
    use uc_core::membership::{MembershipError, SpaceMember};
    use uc_core::settings::model::ContentTypes;
    use uc_core::MemberSyncPreferences;

    /// Member repo returning one configurable member, or `None` for an
    /// "unknown peer" (fail-open) case.
    struct StubRepo {
        member: Option<SpaceMember>,
    }

    fn member_with(send_enabled: bool, send_content_types: ContentTypes) -> SpaceMember {
        let mut prefs = MemberSyncPreferences::default();
        prefs.send_enabled = send_enabled;
        prefs.send_content_types = send_content_types;
        SpaceMember {
            device_id: DeviceId::new("peer"),
            device_name: "peer".to_string(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_raw_string(
                "0123456789abcdef",
            )
            .expect("valid test fingerprint"),
            joined_at: Utc::now(),
            sync_preferences: prefs,
        }
    }

    #[async_trait]
    impl MemberRepositoryPort for StubRepo {
        async fn get(&self, _device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            Ok(self.member.clone())
        }
        async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
            Ok(vec![])
        }
        async fn save(&self, _member: &SpaceMember) -> Result<(), MembershipError> {
            Ok(())
        }
        async fn remove(&self, _device_id: &DeviceId) -> Result<bool, MembershipError> {
            Ok(false)
        }
    }

    fn gate(member: Option<SpaceMember>) -> MemberSendGate {
        MemberSendGate::new(Arc::new(StubRepo { member }))
    }

    fn allowed_scope() -> CurrentSpaceMemberScope {
        CurrentSpaceMemberScope {
            revision: 1,
            local_member_active: true,
            usable_peer_device_ids: vec![DeviceId::new("peer")],
            paused_peer_devices: Vec::new(),
        }
    }

    #[tokio::test]
    async fn peer_outside_final_scope_is_denied_before_preferences() {
        let gate = MemberSendGate::new(Arc::new(StubRepo {
            member: Some(member_with(true, ContentTypes::default())),
        }));
        let scope = CurrentSpaceMemberScope {
            usable_peer_device_ids: Vec::new(),
            ..allowed_scope()
        };

        assert!(
            !gate
                .is_send_allowed_with_scope(
                    &DeviceId::new("peer"),
                    &category_set(ClipboardContentCategory::Text),
                    &scope,
                )
                .await
        );
    }

    #[tokio::test]
    async fn send_gate_logs_do_not_expose_the_peer_device_id() {
        use tracing::instrument::WithSubscriber;

        let gate = gate(Some(member_with(false, ContentTypes::default())));
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(logs.clone())
            .finish();

        let _ = gate
            .is_send_allowed_with_scope(
                &DeviceId::new("sensitive-peer-device"),
                &ClipboardContentCategorySet::empty(),
                &allowed_scope(),
            )
            .with_subscriber(subscriber)
            .await;

        assert!(!logs.output().contains("sensitive-peer-device"));
    }

    fn category_set(c: ClipboardContentCategory) -> ClipboardContentCategorySet {
        let mut s = ClipboardContentCategorySet::empty();
        s.insert(c);
        s
    }

    #[tokio::test]
    async fn send_disabled_blocks() {
        let g = gate(Some(member_with(false, ContentTypes::default())));
        let cats = category_set(ClipboardContentCategory::Text);
        assert!(
            !g.is_send_allowed_with_scope(&DeviceId::new("peer"), &cats, &allowed_scope())
                .await
        );
    }

    #[tokio::test]
    async fn denied_content_type_blocks_even_when_send_enabled() {
        let mut ct = ContentTypes::default();
        ct.image = false;
        let g = gate(Some(member_with(true, ct)));
        let cats = category_set(ClipboardContentCategory::Image);
        assert!(
            !g.is_send_allowed_with_scope(&DeviceId::new("peer"), &cats, &allowed_scope())
                .await
        );
    }

    #[tokio::test]
    async fn allowed_when_enabled_and_category_permitted() {
        let g = gate(Some(member_with(true, ContentTypes::default())));
        let cats = category_set(ClipboardContentCategory::Text);
        assert!(
            g.is_send_allowed_with_scope(&DeviceId::new("peer"), &cats, &allowed_scope())
                .await
        );
    }

    #[tokio::test]
    async fn empty_category_set_fails_open_against_send_enabled() {
        // An unknown payload (empty set) passes the content-type stage; only
        // the device-level switch can stop it.
        let mut ct = ContentTypes::default();
        ct.text = false;
        ct.image = false;
        let g = gate(Some(member_with(true, ct)));
        let cats = ClipboardContentCategorySet::empty();
        assert!(
            g.is_send_allowed_with_scope(&DeviceId::new("peer"), &cats, &allowed_scope())
                .await
        );
    }

    #[tokio::test]
    async fn unknown_peer_fails_open() {
        let g = gate(None);
        let cats = category_set(ClipboardContentCategory::Text);
        assert!(
            g.is_send_allowed_with_scope(&DeviceId::new("peer"), &cats, &allowed_scope())
                .await
        );
    }
}
