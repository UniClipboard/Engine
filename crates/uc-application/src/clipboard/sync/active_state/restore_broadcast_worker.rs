//! `RestoreBroadcastWorker` — debounced, gated outbound broadcast of local
//! history restores (issue #1017 PR4).
//!
//! A local history restore that advances the active-clipboard register offers
//! the activation here via the application-internal restore-broadcast channel.
//! This worker:
//!
//! 1. **Coalesces** rapid restores: it keeps only the latest offered request
//!    and emits a single broadcast after a quiet window (D7, ~300ms). A user
//!    clicking through several history entries quickly produces one announce
//!    of the final selection, not one per click.
//! 2. **Feature-gates** on `sync_on_restore` (default off): when disabled the
//!    register still advanced locally, but nothing is announced to peers.
//! 3. **Per-peer gates** through the shared fan-out: `send_enabled` ∧
//!    `send_content_types` (D2), identical to the inbound re-broadcast path.
//!
//! The gate is re-read from settings at emit time (not at offer time) so a
//! toggle taking effect between the restore and the debounced emit is honoured.

use std::sync::Arc;
use std::time::Duration;

#[cfg(test)]
use tokio::task::JoinHandle;
use tokio::time::sleep_until;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use uc_core::ports::clipboard::ActiveClipboardDispatchPort;
use uc_core::ports::{PeerAddressRepositoryPort, PeerReachabilityPort, SettingsPort};
use uc_core::MemberRepositoryPort;
use uc_observability_contract::{error_source::io_error_kind, uc_debug, uc_warn};

use crate::clipboard::write::{RestoreBroadcastReceiver, RestoreBroadcastRequest};
use crate::deps::CurrentSpaceMemberScopePort;

use super::super::send_gate::MemberSendGate;
use super::fanout::{fan_out_active_state, ActiveStateBroadcastTrigger};

/// Debounce window for coalescing rapid restores into one broadcast (D7).
const RESTORE_BROADCAST_DEBOUNCE: Duration = Duration::from_millis(300);

/// Dependencies for the restore-broadcast worker.
pub(crate) struct RestoreBroadcastWorker {
    rx: RestoreBroadcastReceiver,
    settings: Arc<dyn SettingsPort>,
    dispatch: Arc<dyn ActiveClipboardDispatchPort>,
    peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    peer_scope: Arc<dyn CurrentSpaceMemberScopePort>,
    peer_reachability: Arc<dyn PeerReachabilityPort>,
    send_gate: MemberSendGate,
}

impl RestoreBroadcastWorker {
    pub(crate) fn new(
        rx: RestoreBroadcastReceiver,
        settings: Arc<dyn SettingsPort>,
        dispatch: Arc<dyn ActiveClipboardDispatchPort>,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
        peer_scope: Arc<dyn CurrentSpaceMemberScopePort>,
        peer_reachability: Arc<dyn PeerReachabilityPort>,
        member_repo: Arc<dyn MemberRepositoryPort>,
    ) -> Self {
        Self {
            rx,
            settings,
            dispatch,
            peer_addr_repo,
            peer_scope: Arc::clone(&peer_scope),
            peer_reachability,
            send_gate: MemberSendGate::new(member_repo),
        }
    }

    #[cfg(test)]
    fn spawn(self) -> JoinHandle<()> {
        tokio::spawn(self.run(CancellationToken::new()))
    }

    #[instrument(name = "active_state.restore_broadcast_loop", skip_all)]
    pub(crate) async fn run(mut self, cancel: CancellationToken) {
        loop {
            // Block until the next offer (or all senders drop → exit).
            let latest = tokio::select! {
                biased;
                _ = cancel.cancelled() => return,
                latest = self.rx.recv() => latest,
            };
            let mut latest = match latest {
                Some(req) => req,
                None => {
                    uc_debug!("restore broadcast worker: all senders dropped; exiting");
                    return;
                }
            };

            // Debounce: keep draining for the quiet window, replacing `latest`
            // with each newer offer. A fresh offer that arrives during the
            // window restarts it, so the timer measures quiet time, not a fixed
            // batch interval.
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return,
                    maybe = self.rx.recv() => match maybe {
                        Some(req) => {
                            if req.version != latest.version { latest = req; }
                        }
                        None => {
                            // Senders gone mid-window: emit what we have, then
                            // the outer loop's recv will see the close and exit.
                            break;
                        }
                    },
                    _ = sleep_until(latest.arrived_at + RESTORE_BROADCAST_DEBOUNCE) => break,
                }
            }

            let context = latest.request.context.clone();
            tokio::select! { _ = cancel.cancelled() => return, _ = context.scope(self.emit(latest.request)) => {} }
        }
    }

    /// Feature-gate then fan out one coalesced activation.
    async fn emit(&self, request: RestoreBroadcastRequest) {
        // Re-read the toggle at emit time so a setting change between the
        // restore and this debounced emit is respected.
        let sync_on_restore = match self.settings.load().await {
            Ok(settings) => settings.sync.sync_enabled && settings.sync.sync_on_restore,
            Err(err) => {
                // Fail closed: if we can't confirm the user opted in, don't
                // announce. A restore that should have broadcast is recovered
                // by the next restore or a peer-online resync.
                uc_warn!(
                    error_kind = "settings_load",
                    io_error_kind = io_error_kind(err.as_ref()),
                    "restore broadcast skipped: settings load failed"
                );
                return;
            }
        };
        if !sync_on_restore {
            uc_debug!("restore broadcast skipped: sync_on_restore disabled");
            return;
        }

        fan_out_active_state(
            &self.dispatch,
            &self.peer_addr_repo,
            &self.peer_scope,
            &self.peer_reachability,
            &self.send_gate,
            &request.state,
            &request.categories,
            ActiveStateBroadcastTrigger::HistoryRestore,
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use crate::clipboard::write::{restore_broadcast_channel, RestoreBroadcastTrigger};
    use async_trait::async_trait;
    use chrono::Utc;

    use crate::deps::{
        CurrentSpaceMemberScope, CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort,
    };
    use uc_core::clipboard::{
        ActiveClipboardState, ClipboardContentCategory, ClipboardContentCategorySet,
    };
    use uc_core::ids::{DeviceId, EntryId};
    use uc_core::membership::{MembershipError, SpaceMember};
    use uc_core::ports::clipboard::ActiveClipboardDispatchError;
    use uc_core::ports::{
        PeerAddressError, PeerAddressRecord, PeerReachabilityChanged, PeerReachabilityError,
        PeerReachabilityPort, ReachabilityState,
    };
    use uc_core::settings::model::Settings;
    use uc_core::MemberSyncPreferences;

    /// Presence fake reporting every device with a fixed reachability. The
    /// broadcast tests want their roster peer reachable, so `Online` is the
    /// default; `Offline` exercises the fan-out skip.
    struct StaticPeerReachability(ReachabilityState);
    #[async_trait]
    impl PeerReachabilityPort for StaticPeerReachability {
        async fn ensure_reachable(
            &self,
            _device: &DeviceId,
        ) -> Result<ReachabilityState, PeerReachabilityError> {
            Ok(self.0)
        }
        async fn current_state(&self, _device: &DeviceId) -> ReachabilityState {
            self.0
        }
        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<PeerReachabilityChanged> {
            tokio::sync::broadcast::channel(1).1
        }
    }

    // ---- spies / fakes ------------------------------------------------------

    struct FixedSettings {
        sync_on_restore: bool,
    }
    #[async_trait]
    impl SettingsPort for FixedSettings {
        async fn load(&self) -> anyhow::Result<Settings> {
            let mut s = Settings::default();
            s.sync.sync_on_restore = self.sync_on_restore;
            Ok(s)
        }
        async fn save(&self, _settings: &Settings) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// Records the content hashes dispatched, in order.
    #[derive(Default)]
    struct DispatchSpy {
        sent: Mutex<Vec<(String, String)>>, // (target, snapshot_hash)
    }
    #[async_trait]
    impl ActiveClipboardDispatchPort for DispatchSpy {
        async fn dispatch(
            &self,
            target: &DeviceId,
            state: &ActiveClipboardState,
        ) -> Result<(), ActiveClipboardDispatchError> {
            self.sent
                .lock()
                .unwrap()
                .push((target.as_str().to_string(), state.snapshot_hash.clone()));
            Ok(())
        }
    }

    /// One peer in the roster.
    struct OnePeerAddrRepo {
        device: DeviceId,
    }
    #[async_trait]
    impl PeerAddressRepositoryPort for OnePeerAddrRepo {
        async fn get(
            &self,
            _device: &DeviceId,
        ) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
            Ok(None)
        }
        async fn upsert(&self, _record: &PeerAddressRecord) -> Result<(), PeerAddressError> {
            Ok(())
        }
        async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
            Ok(vec![PeerAddressRecord {
                device_id: self.device.clone(),
                addr_blob: vec![],
                observed_at: Utc::now(),
            }])
        }
        async fn remove(&self, _device: &DeviceId) -> Result<(), PeerAddressError> {
            Ok(())
        }
    }

    struct AllowAllMembers;
    #[async_trait]
    impl MemberRepositoryPort for AllowAllMembers {
        async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            Ok(Some(SpaceMember {
                device_id: device_id.clone(),
                device_name: "peer".to_string(),
                identity_fingerprint: uc_core::security::IdentityFingerprint::from_raw_string(
                    "0123456789abcdef",
                )
                .expect("valid test fingerprint"),
                joined_at: Utc::now(),
                sync_preferences: MemberSyncPreferences::default(),
            }))
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

    struct FixedPeerScope(Vec<DeviceId>);

    #[async_trait]
    impl CurrentSpaceMemberScopePort for FixedPeerScope {
        async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
            Ok(CurrentSpaceMemberScope {
                revision: 1,
                local_member_active: true,
                usable_peer_device_ids: self.0.clone(),
                paused_peer_devices: Vec::new(),
            })
        }
    }

    fn offer(trigger: &RestoreBroadcastTrigger, request: RestoreBroadcastRequest) {
        trigger.offer(request.state, request.categories);
    }

    static NEXT_ACTIVATION: std::sync::atomic::AtomicI64 =
        std::sync::atomic::AtomicI64::new(10_000);

    fn request(snapshot_hash: &str) -> RestoreBroadcastRequest {
        let mut categories = ClipboardContentCategorySet::empty();
        categories.insert(ClipboardContentCategory::Text);
        RestoreBroadcastRequest {
            state: ActiveClipboardState::new(
                snapshot_hash,
                EntryId::new(),
                NEXT_ACTIVATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                DeviceId::new("self"),
            ),
            categories,
            context: uc_observability_contract::diagnostics::ObservationContext::capture(),
        }
    }

    fn build_worker(
        sync_on_restore: bool,
        peer_reachability: ReachabilityState,
    ) -> (
        RestoreBroadcastWorker,
        RestoreBroadcastTrigger,
        Arc<DispatchSpy>,
    ) {
        let (tx, rx) = restore_broadcast_channel();
        let dispatch = Arc::new(DispatchSpy::default());
        let worker = RestoreBroadcastWorker::new(
            rx,
            Arc::new(FixedSettings { sync_on_restore }),
            Arc::clone(&dispatch) as Arc<dyn ActiveClipboardDispatchPort>,
            Arc::new(OnePeerAddrRepo {
                device: DeviceId::new("peer-1"),
            }),
            Arc::new(FixedPeerScope(vec![DeviceId::new("peer-1")])),
            Arc::new(StaticPeerReachability(peer_reachability)),
            Arc::new(AllowAllMembers),
        );
        (worker, tx, dispatch)
    }

    #[tokio::test]
    async fn removed_peer_with_stale_address_does_not_receive_restore_broadcast() {
        let (tx, rx) = restore_broadcast_channel();
        let dispatch = Arc::new(DispatchSpy::default());
        let worker = RestoreBroadcastWorker::new(
            rx,
            Arc::new(FixedSettings {
                sync_on_restore: true,
            }),
            Arc::clone(&dispatch) as Arc<dyn ActiveClipboardDispatchPort>,
            Arc::new(OnePeerAddrRepo {
                device: DeviceId::new("peer-1"),
            }),
            Arc::new(FixedPeerScope(vec![])),
            Arc::new(StaticPeerReachability(ReachabilityState::Online)),
            Arc::new(AllowAllMembers),
        );
        let handle = worker.spawn();

        offer(&tx, request("blake3v1:aa"));
        tokio::time::sleep(RESTORE_BROADCAST_DEBOUNCE + Duration::from_millis(80)).await;

        assert!(dispatch.sent.lock().unwrap().is_empty());
        handle.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_during_debounce_does_not_broadcast_pending_restore() {
        let (worker, tx, dispatch) = build_worker(true, ReachabilityState::Online);
        let cancel = CancellationToken::new();
        offer(&tx, request("blake3v1:aa"));
        let task = tokio::spawn(worker.run(cancel.clone()));
        tokio::task::yield_now().await;
        cancel.cancel();
        tokio::time::timeout(Duration::from_millis(1), task)
            .await
            .unwrap()
            .unwrap();
        assert!(dispatch.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn sync_on_restore_disabled_does_not_broadcast() {
        let (worker, tx, dispatch) = build_worker(false, ReachabilityState::Online);
        let handle = worker.spawn();

        offer(&tx, request("blake3v1:aa"));
        // Give the worker past the debounce window + a slice of slack.
        tokio::time::sleep(RESTORE_BROADCAST_DEBOUNCE + Duration::from_millis(80)).await;

        assert!(
            dispatch.sent.lock().unwrap().is_empty(),
            "no broadcast must happen while sync_on_restore is off"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn enabled_broadcasts_to_allowed_peer() {
        let (worker, tx, dispatch) = build_worker(true, ReachabilityState::Online);
        let handle = worker.spawn();

        offer(&tx, request("blake3v1:aa"));
        tokio::time::sleep(RESTORE_BROADCAST_DEBOUNCE + Duration::from_millis(80)).await;

        let sent = dispatch.sent.lock().unwrap();
        assert_eq!(sent.len(), 1, "exactly one peer is in the roster");
        assert_eq!(sent[0], ("peer-1".to_string(), "blake3v1:aa".to_string()));
        drop(sent);
        handle.abort();
    }

    #[tokio::test]
    async fn upgrade_required_peer_does_not_receive_restore_broadcast() {
        let (tx, rx) = restore_broadcast_channel();
        let dispatch = Arc::new(DispatchSpy::default());
        let worker = RestoreBroadcastWorker::new(
            rx,
            Arc::new(FixedSettings {
                sync_on_restore: true,
            }),
            Arc::clone(&dispatch) as Arc<dyn ActiveClipboardDispatchPort>,
            Arc::new(OnePeerAddrRepo {
                device: DeviceId::new("peer-1"),
            }),
            Arc::new(FixedPeerScope(vec![])),
            Arc::new(StaticPeerReachability(ReachabilityState::Online)),
            Arc::new(AllowAllMembers),
        );
        let handle = worker.spawn();

        offer(&tx, request("blake3v1:aa"));
        tokio::time::sleep(RESTORE_BROADCAST_DEBOUNCE + Duration::from_millis(80)).await;

        assert!(
            dispatch.sent.lock().unwrap().is_empty(),
            "a peer that must upgrade must not receive restored clipboard content"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn rapid_restores_coalesce_to_latest() {
        let (worker, tx, dispatch) = build_worker(true, ReachabilityState::Online);
        let handle = worker.spawn();

        // Three offers inside the debounce window — only the last should win.
        offer(&tx, request("blake3v1:aa"));
        tokio::time::sleep(Duration::from_millis(40)).await;
        offer(&tx, request("blake3v1:bb"));
        tokio::time::sleep(Duration::from_millis(40)).await;
        offer(&tx, request("blake3v1:cc"));

        tokio::time::sleep(RESTORE_BROADCAST_DEBOUNCE + Duration::from_millis(120)).await;

        let sent = dispatch.sent.lock().unwrap();
        assert_eq!(sent.len(), 1, "rapid restores coalesce into one broadcast");
        assert_eq!(
            sent[0].1, "blake3v1:cc",
            "the coalesced broadcast carries the latest restore"
        );
        drop(sent);
        handle.abort();
    }

    #[tokio::test]
    async fn known_offline_peer_is_skipped() {
        // The only roster peer is already known offline → the fan-out skips it
        // without dialing, so nothing is dispatched even with sync_on_restore on.
        let (worker, tx, dispatch) = build_worker(true, ReachabilityState::Offline);
        let handle = worker.spawn();

        offer(&tx, request("blake3v1:aa"));
        tokio::time::sleep(RESTORE_BROADCAST_DEBOUNCE + Duration::from_millis(80)).await;

        assert!(
            dispatch.sent.lock().unwrap().is_empty(),
            "a peer known offline must be skipped, not dialed"
        );
        handle.abort();
    }
    struct SixPeerAddresses;
    #[async_trait]
    impl PeerAddressRepositoryPort for SixPeerAddresses {
        async fn get(&self, _: &DeviceId) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
            Ok(None)
        }
        async fn upsert(&self, _: &PeerAddressRecord) -> Result<(), PeerAddressError> {
            Ok(())
        }
        async fn remove(&self, _: &DeviceId) -> Result<(), PeerAddressError> {
            Ok(())
        }
        async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
            Ok((0..6)
                .map(|n| PeerAddressRecord {
                    device_id: DeviceId::new(format!("peer-{n}")),
                    addr_blob: vec![],
                    observed_at: Utc::now(),
                })
                .collect())
        }
    }
    struct SlowDispatch {
        started: tokio::sync::mpsc::UnboundedSender<DeviceId>,
        release: Arc<tokio::sync::Semaphore>,
    }
    #[async_trait]
    impl ActiveClipboardDispatchPort for SlowDispatch {
        async fn dispatch(
            &self,
            target: &DeviceId,
            _: &ActiveClipboardState,
        ) -> Result<(), ActiveClipboardDispatchError> {
            self.started.send(target.clone()).unwrap();
            self.release.acquire().await.unwrap().forget();
            Ok(())
        }
    }
    /// 首个慢目标不能阻止健康目标开始，且每轮最多四个在途调用。
    #[tokio::test]
    async fn broadcast_starts_four_targets_before_waiting_for_slow_peer() {
        let (started_tx, mut started) = tokio::sync::mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let dispatch: Arc<dyn ActiveClipboardDispatchPort> = Arc::new(SlowDispatch {
            started: started_tx,
            release: release.clone(),
        });
        let addresses: Arc<dyn PeerAddressRepositoryPort> = Arc::new(SixPeerAddresses);
        let scope: Arc<dyn CurrentSpaceMemberScopePort> = Arc::new(FixedPeerScope(
            (0..6).map(|n| DeviceId::new(format!("peer-{n}"))).collect(),
        ));
        let reachability: Arc<dyn PeerReachabilityPort> =
            Arc::new(StaticPeerReachability(ReachabilityState::Online));
        let task = tokio::spawn(async move {
            let request = request("blake3v1:concurrency");
            fan_out_active_state(
                &dispatch,
                &addresses,
                &scope,
                &reachability,
                &MemberSendGate::new(Arc::new(AllowAllMembers)),
                &request.state,
                &request.categories,
                ActiveStateBroadcastTrigger::HistoryRestore,
            )
            .await;
        });
        let four = tokio::time::timeout(Duration::from_secs(1), async {
            for _ in 0..4 {
                started.recv().await.unwrap();
            }
        })
        .await;
        assert!(
            started.try_recv().is_err(),
            "only four targets may start before a slot is free"
        );
        release.add_permits(6);
        task.await.unwrap();
        assert!(
            four.is_ok(),
            "slow first target must not serialize the entire round"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn late_old_restore_does_not_replace_newer_activation() {
        let (worker, tx, dispatch) = build_worker(true, ReachabilityState::Online);
        let handle = worker.spawn();
        let mut newer = request("blake3v1:newer");
        newer.state.activated_at_ms = 2_000;
        let mut older = request("blake3v1:older");
        older.state.activated_at_ms = 1_000;
        offer(&tx, newer);
        offer(&tx, older);
        tokio::time::sleep(RESTORE_BROADCAST_DEBOUNCE + Duration::from_millis(1)).await;
        assert_eq!(
            dispatch.sent.lock().unwrap()[0].1,
            "blake3v1:newer",
            "arrival order must not override activation order"
        );
        handle.abort();
    }

    struct HeldRoundDispatch {
        started: tokio::sync::mpsc::UnboundedSender<String>,
        release: Arc<tokio::sync::Semaphore>,
    }
    #[async_trait]
    impl ActiveClipboardDispatchPort for HeldRoundDispatch {
        async fn dispatch(
            &self,
            _: &DeviceId,
            state: &ActiveClipboardState,
        ) -> Result<(), ActiveClipboardDispatchError> {
            self.started.send(state.snapshot_hash.clone()).unwrap();
            self.release.acquire().await.unwrap().forget();
            Ok(())
        }
    }
    #[tokio::test(start_paused = true)]
    async fn current_round_finishes_and_only_latest_waiting_restore_starts_without_extra_debounce()
    {
        let (mut worker, trigger, _) = build_worker(true, ReachabilityState::Online);
        let (started, mut calls) = tokio::sync::mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        worker.dispatch = Arc::new(HeldRoundDispatch {
            started,
            release: release.clone(),
        });
        let cancel = CancellationToken::new();
        let task = tokio::spawn(worker.run(cancel.clone()));
        offer(&trigger, request("A"));
        assert_eq!(calls.recv().await.unwrap(), "A");
        let older = request("B");
        offer(&trigger, older.clone());
        offer(&trigger, request("C"));
        let latest = request("D");
        offer(&trigger, latest.clone());
        tokio::time::advance(Duration::from_millis(350)).await;
        offer(&trigger, older);
        offer(&trigger, latest);
        assert!(
            calls.try_recv().is_err(),
            "new restore must not preempt current round"
        );
        let finished_at = tokio::time::Instant::now();
        release.add_permits(1);
        assert_eq!(calls.recv().await.unwrap(), "D");
        assert_eq!(
            finished_at,
            tokio::time::Instant::now(),
            "elapsed quiet window must not restart after round completion"
        );
        release.add_permits(1);
        drop(trigger);
        task.await.unwrap();
        assert!(
            calls.try_recv().is_err(),
            "B and C are coalesced, never separate rounds"
        );
    }
}
