//! 系统剪贴板写入的唯一负责人。
//! 写入前登记未完成动作。写入结束后登记宿主确认的身份或无身份结果。
//! 未完成的写入由独立任务持有，调用方取消等待不会中断清理。

use anyhow::{Error, Result};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::spawn_blocking;
use tracing::Instrument;
use tracing::{info_span, Span};

use uc_core::clipboard::SystemClipboardSnapshot;
use uc_core::ports::clipboard::{SelfWriteAttribution, SelfWriteLedgerPort, SystemClipboardPort};
use uc_observability_contract::diagnostics::ObservationContext;
use uc_observability_contract::{
    error_source::io_error_kind, log_fields::log_vocab_debug, uc_error, uc_info, uc_warn,
};

/// Number of consecutive OS-write failures that trip the circuit.
///
/// Chosen empirically against Sentry issue UNICLIPBOARD-RUST-F (35 minutes /
/// 50 failed writes on a single Windows host whose clipboard was held by an
/// AV/IME process). 5 lets a couple of transient retries through; beyond
/// that the clipboard is clearly unavailable and we stop hammering it.
const CIRCUIT_FAILURE_THRESHOLD: u32 = 5;

/// Default duration the circuit stays open after tripping.
///
/// Long enough that the contending process (AV scan / IME hook / RDP clip
/// proxy) has time to release the clipboard, short enough that recovery is
/// invisible to a typing user. Inbound writes during this window are
/// rejected immediately (no OS call, no guard registration).
///
/// Production `new()` uses this constant; tests use `with_cooldown` to pass
/// a millisecond-scale value so they can exercise the recovery path
/// without literally sleeping 30 seconds.
const DEFAULT_CIRCUIT_OPEN_DURATION: Duration = Duration::from_secs(30);

/// 表示系统剪贴板写入的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardWriteIntent {
    /// A local clipboard history restore (user clicked "restore" in history).
    LocalRestore,
    /// A remote push from a peer device arriving via inbound sync.
    RemotePush,
}

/// 为每次写入持有顺序锁，并在观察归因前发布写入结果。
#[derive(Clone)]
pub struct ClipboardWriteCoordinator {
    system_clipboard: Arc<dyn SystemClipboardPort>,
    clipboard_change_origin: Arc<dyn SelfWriteLedgerPort>,
    /// True while a `write()` call is in progress. Used by workers to detect
    /// genuinely concurrent clipboard writes, replacing the overly broad
    /// `has_pending_origin()` check that conflated attribution guards with
    /// active write operations.
    writing: Arc<AtomicBool>,
    serial: Arc<AsyncMutex<()>>,
    /// Number of consecutive OS-write failures since the last success.
    /// Reset to 0 on any successful write; reset to 0 when the circuit trips
    /// (so the next post-cooldown window starts clean).
    consecutive_failures: Arc<AtomicU32>,
    /// `Some(instant)` while the breaker is open — incoming writes are
    /// rejected without touching the OS clipboard or registering a guard.
    /// `None` means the circuit is closed (normal operation).
    circuit_open_until: Arc<Mutex<Option<Instant>>>,
    /// How long the breaker stays open after tripping. Per-instance so
    /// tests can use a millisecond-scale value while production uses the
    /// `DEFAULT_CIRCUIT_OPEN_DURATION` constant.
    cooldown: Duration,
}

struct WritingGuard<'a>(&'a AtomicBool);

impl Drop for WritingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl ClipboardWriteCoordinator {
    pub fn new(
        system_clipboard: Arc<dyn SystemClipboardPort>,
        clipboard_change_origin: Arc<dyn SelfWriteLedgerPort>,
    ) -> Self {
        Self::with_cooldown(
            system_clipboard,
            clipboard_change_origin,
            DEFAULT_CIRCUIT_OPEN_DURATION,
        )
    }

    /// Construct with a custom circuit-breaker cooldown. Intended for tests
    /// that need to exercise the recovery path without literally waiting
    /// 30 seconds. Not exposed outside the crate in production builds; use
    /// `new` for normal construction.
    pub(crate) fn with_cooldown(
        system_clipboard: Arc<dyn SystemClipboardPort>,
        clipboard_change_origin: Arc<dyn SelfWriteLedgerPort>,
        cooldown: Duration,
    ) -> Self {
        Self {
            system_clipboard,
            clipboard_change_origin,
            writing: Arc::new(AtomicBool::new(false)),
            serial: Arc::new(AsyncMutex::new(())),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            circuit_open_until: Arc::new(Mutex::new(None)),
            cooldown,
        }
    }

    /// Returns `Some(remaining_duration)` if the breaker is currently open,
    /// `None` if the circuit is closed. Also auto-closes the circuit when
    /// the open window has elapsed (lazy cooldown reset).
    fn circuit_check(&self) -> Option<Duration> {
        let mut guard = self.circuit_open_until.lock().expect("poisoned");
        match *guard {
            Some(until) => {
                let now = Instant::now();
                if now >= until {
                    *guard = None;
                    uc_info!(
                        event = "circuit_recovered",
                        reason = "cooldown_elapsed",
                        "clipboard_write_coordinator: circuit breaker closed after cooldown"
                    );
                    None
                } else {
                    Some(until - now)
                }
            }
            None => None,
        }
    }

    /// Record a failed OS write; trip the breaker if the threshold is reached.
    ///
    /// Returns the new consecutive-failure count (post-increment) so the
    /// caller can include it in its own structured error event without
    /// re-reading the atomic. Note: on a trip the atomic is reset to 0,
    /// but the returned value still reflects the pre-reset count — i.e.
    /// "this was failure number N which tripped the circuit".
    fn record_failure(&self) -> u32 {
        let new_count = self.consecutive_failures.fetch_add(1, Ordering::AcqRel) + 1;
        if new_count >= CIRCUIT_FAILURE_THRESHOLD {
            let until = Instant::now() + self.cooldown;
            *self.circuit_open_until.lock().expect("poisoned") = Some(until);
            self.consecutive_failures.store(0, Ordering::Release);
            uc_warn!(
                event = "circuit_tripped",
                error_kind = "circuit_tripped",
                consecutive_failures = new_count,
                threshold = CIRCUIT_FAILURE_THRESHOLD,
                cooldown_ms = self.cooldown.as_millis() as u64,
                "clipboard_write_coordinator: circuit breaker tripped — pausing OS writes"
            );
        }
        new_count
    }

    /// Record a successful OS write — clears the failure counter and any
    /// stale open circuit (defensive; the open path normally lazy-closes).
    ///
    /// Emits an `info` recovery event if the success follows one or more
    /// failures, so operators can pair the "first failure" log line with
    /// a corresponding "recovered" line.
    fn record_success(&self) {
        let prev = self.consecutive_failures.swap(0, Ordering::AcqRel);
        if prev > 0 {
            *self.circuit_open_until.lock().expect("poisoned") = None;
            uc_info!(
                event = "circuit_recovered",
                reason = "success_after_failure",
                recovered_after_failures = prev,
                "clipboard_write_coordinator: OS clipboard write recovered"
            );
        }
    }

    /// Returns `true` while another `write()` call is actively in progress.
    ///
    /// Unlike the previous `has_pending_origin()`, this only returns `true`
    /// during an actual concurrent write — it does NOT trigger on stale
    /// attribution guards left by previously completed writes.
    pub fn is_write_in_progress(&self) -> bool {
        self.writing.load(Ordering::Acquire)
    }

    /// 写入快照并发布宿主回执。没有可靠身份时不能推定回声。
    pub async fn write(
        &self,
        snapshot: SystemClipboardSnapshot,
        intent: ClipboardWriteIntent,
    ) -> Result<()> {
        let owner = self.clone();
        let observation = ObservationContext::capture();
        // 调用方停止等待后，已接受的写入仍持有顺序锁，直到来源记录清理完成。
        tokio::spawn(
            observation
                .scope(async move {
                    let _serial = owner.serial.lock().await;
                    owner.writing.store(true, Ordering::Release);
                    let _writing = WritingGuard(&owner.writing);
                    owner.write_inner(snapshot, intent).await
                })
                .in_current_span(),
        )
        .await
        .map_err(|source| Error::new(source).context("clipboard write worker failed"))?
    }

    async fn write_inner(
        &self,
        snapshot: SystemClipboardSnapshot,
        intent: ClipboardWriteIntent,
    ) -> Result<()> {
        // Circuit breaker check — when the OS clipboard is unavailable
        // (typically held by AV / IME / RDP), keep hammering it produces
        // nothing but Sentry noise (see UNICLIPBOARD-RUST-F: 50 failed
        // writes in 35 minutes on a single host). Skip the OS call entirely
        // and do NOT register a guard — without an actual write the
        // watcher won't fire, so leftover guards would just mis-attribute
        // a future unrelated change.
        if let Some(remaining) = self.circuit_check() {
            uc_warn!(
                event = "circuit_open_skip",
                error_kind = "circuit_open",
                remaining_secs = remaining.as_secs(),
                intent = log_vocab_debug(&intent),
                "clipboard_write_coordinator: circuit breaker open — skipping OS write"
            );
            anyhow::bail!(
                "clipboard write skipped: circuit breaker open ({}s remaining)",
                remaining.as_secs()
            );
        }

        async {
            let attribution = match intent {
                ClipboardWriteIntent::LocalRestore => SelfWriteAttribution::Local,
                ClipboardWriteIntent::RemotePush => SelfWriteAttribution::Remote,
            };
            self.clipboard_change_origin.begin_identified_write().await;

            let clipboard = Arc::clone(&self.system_clipboard);
            let span = Span::current();
            let observation = ObservationContext::capture();
            let written = spawn_blocking(move || {
                observation.sync_scope(|| span.in_scope(|| clipboard.write_with_receipt(snapshot)))
            })
            .await
            .map_err(|source| Error::new(source).context("system clipboard writer failed"))
            .and_then(|result| result);
            let identity = written.as_ref().ok().and_then(|receipt| receipt.clone());
            self.clipboard_change_origin
                .finish_identified_write(identity, attribution)
                .await;
            if let Err(err) = written {
                // Record failure first so the error event below can include
                // the post-increment count and `circuit_tripped` derived state.
                // Whoever reads Sentry can pair `error_kind=os_write_failed`
                // with `consecutive_failures` and `circuit_tripped` to see
                // immediately whether this is the Nth failure in a row.
                let consecutive_failures = self.record_failure();
                let circuit_tripped = consecutive_failures >= CIRCUIT_FAILURE_THRESHOLD;
                uc_error!(
                    event = "os_write_failed",
                    error_kind = "os_write_failed",
                    io_error_kind = io_error_kind(err.as_ref()),
                    intent = log_vocab_debug(&intent),
                    consecutive_failures = consecutive_failures,
                    threshold = CIRCUIT_FAILURE_THRESHOLD,
                    circuit_tripped = circuit_tripped,
                    "clipboard_write_coordinator: OS clipboard write failed"
                );
                return Err(err);
            }

            self.record_success();

            Ok(())
        }
        .instrument(info_span!(
            "clipboard_write_coordinator.write",
            intent = ?intent
        ))
        .await
    }
}

#[cfg(test)]
mod tests {
    //! Circuit-breaker behavioural tests.
    //!
    //! These exercise the three states transitions added in response to
    //! UNICLIPBOARD-RUST-F (50 OS-clipboard failures on a single Windows
    //! host in 35 minutes):
    //!
    //! 1. Five consecutive failures trip the breaker; the sixth call MUST
    //!    short-circuit without invoking the OS port at all.
    //! 2. After the cooldown elapses the breaker auto-closes and subsequent
    //!    writes reach the OS again.
    //! 3. A single success resets the failure counter so a subsequent
    //!    streak doesn't inherit stale credit.
    //!
    //! Tests run on the tokio current-thread runtime; cooldown is shortened
    //! via `with_cooldown` to keep wall-clock waits in the ms range.
    use super::*;
    use async_trait::async_trait;
    use mockall::mock;
    use std::sync::atomic::AtomicU32;
    use tokio::sync::{oneshot, Notify};
    use tokio::task::JoinError;
    use tokio::time::timeout;
    use uc_core::ids::{FormatId, RepresentationId};
    use uc_core::{MimeType, ObservedClipboardRepresentation};

    mock! {
        SystemClipboard {}
        #[async_trait]
        impl SystemClipboardPort for SystemClipboard {
            fn read_snapshot(&self) -> Result<SystemClipboardSnapshot>;
            fn write_snapshot(&self, snapshot: SystemClipboardSnapshot) -> Result<()>;
        }
    }

    mock! {
        ChangeOrigin {}
        #[async_trait]
        impl SelfWriteLedgerPort for ChangeOrigin {}
    }

    fn make_snapshot() -> SystemClipboardSnapshot {
        SystemClipboardSnapshot {
            ts_ms: 0,
            representations: vec![ObservedClipboardRepresentation::new(
                RepresentationId::new(),
                FormatId::from("text"),
                Some(MimeType("text/plain".to_string())),
                b"x".to_vec(),
            )],
            file_content_digests: Vec::new(),
            file_set_v1_component: None,
        }
    }

    fn permissive_origin_mock() -> MockChangeOrigin {
        MockChangeOrigin::new()
    }

    #[tokio::test]
    async fn host_panic_preserves_its_source_and_releases_the_writing_state() {
        let mut clipboard = MockSystemClipboard::new();
        clipboard
            .expect_write_snapshot()
            .times(1)
            .returning(|_| panic!("private callback payload"));
        let coordinator =
            ClipboardWriteCoordinator::new(Arc::new(clipboard), Arc::new(permissive_origin_mock()));
        let error = coordinator
            .write(make_snapshot(), ClipboardWriteIntent::RemotePush)
            .await
            .unwrap_err();
        assert!(error
            .chain()
            .filter_map(|source| source.downcast_ref::<JoinError>())
            .any(JoinError::is_panic));
        assert!(!error.to_string().contains("private"));
        assert!(!coordinator.writing.load(Ordering::SeqCst));
    }

    struct HeldFirstWrite {
        calls: AtomicU32,
        entered: Notify,
        release: Mutex<Option<oneshot::Receiver<()>>>,
    }

    impl SystemClipboardPort for HeldFirstWrite {
        fn read_snapshot(&self) -> Result<SystemClipboardSnapshot> {
            panic!("unexpected read");
        }

        fn write_snapshot(&self, _snapshot: SystemClipboardSnapshot) -> Result<()> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                let release = self.release.lock().unwrap().take().unwrap();
                self.entered.notify_one();
                release.blocking_recv().unwrap();
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn cancelled_waiter_keeps_the_started_write_and_its_order() {
        let (release, blocked) = oneshot::channel();
        let clipboard = Arc::new(HeldFirstWrite {
            calls: AtomicU32::new(0),
            entered: Notify::new(),
            release: Mutex::new(Some(blocked)),
        });
        let coordinator = Arc::new(ClipboardWriteCoordinator::new(
            clipboard.clone(),
            Arc::new(permissive_origin_mock()),
        ));
        let first = tokio::spawn({
            let coordinator = coordinator.clone();
            async move {
                coordinator
                    .write(make_snapshot(), ClipboardWriteIntent::RemotePush)
                    .await
            }
        });
        clipboard.entered.notified().await;
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        let writing = coordinator.is_write_in_progress();
        let mut second =
            Box::pin(coordinator.write(make_snapshot(), ClipboardWriteIntent::LocalRestore));
        let premature = timeout(Duration::from_millis(20), second.as_mut()).await;
        let calls = clipboard.calls.load(Ordering::SeqCst);
        release.send(()).unwrap();
        assert!(writing);
        assert!(premature.is_err());
        assert_eq!(calls, 1);
        second.await.unwrap();
        assert_eq!(clipboard.calls.load(Ordering::SeqCst), 2);
        assert!(!coordinator.is_write_in_progress());
    }

    #[tokio::test]
    async fn concurrent_writes_wait_for_the_previous_complete_write() {
        let (release, blocked) = oneshot::channel();
        let clipboard = Arc::new(HeldFirstWrite {
            calls: AtomicU32::new(0),
            entered: Notify::new(),
            release: Mutex::new(Some(blocked)),
        });
        let coordinator = Arc::new(ClipboardWriteCoordinator::new(
            clipboard.clone(),
            Arc::new(permissive_origin_mock()),
        ));
        let first = tokio::spawn({
            let coordinator = coordinator.clone();
            async move {
                coordinator
                    .write(make_snapshot(), ClipboardWriteIntent::RemotePush)
                    .await
            }
        });
        clipboard.entered.notified().await;
        let mut second =
            Box::pin(coordinator.write(make_snapshot(), ClipboardWriteIntent::LocalRestore));
        let premature = timeout(Duration::from_millis(20), second.as_mut()).await;
        let before_release = clipboard.calls.load(Ordering::SeqCst);
        release.send(()).unwrap();
        first.await.unwrap().unwrap();
        assert!(premature.is_err());
        assert_eq!(before_release, 1);
        second.await.unwrap();
        assert_eq!(clipboard.calls.load(Ordering::SeqCst), 2);
        assert!(!coordinator.writing.load(Ordering::SeqCst));
    }

    /// 5 consecutive OS failures must trip the breaker, after which the
    /// 6th call short-circuits without invoking the underlying port.
    /// Mockall's `.times(5)` asserts exactly 5 OS calls — if the 6th
    /// leaked through, mock expectations would fail on drop.
    #[tokio::test]
    async fn five_failures_trip_breaker_and_block_sixth_call() {
        let mut clipboard = MockSystemClipboard::new();
        clipboard
            .expect_write_snapshot()
            .times(5)
            .returning(|_| Err(anyhow::anyhow!("simulated OS clipboard failure")));

        let coord =
            ClipboardWriteCoordinator::new(Arc::new(clipboard), Arc::new(permissive_origin_mock()));

        // First five calls go to the OS and each fail.
        for i in 0..5 {
            let r = coord
                .write(make_snapshot(), ClipboardWriteIntent::RemotePush)
                .await;
            assert!(r.is_err(), "call {i} should propagate OS Err");
        }

        // Sixth call must NOT reach the mock (times(5) would fail otherwise).
        let err = coord
            .write(make_snapshot(), ClipboardWriteIntent::RemotePush)
            .await
            .expect_err("breaker should reject 6th call");
        assert!(
            err.to_string().contains("circuit breaker open"),
            "expected circuit-open error, got: {err}"
        );
    }

    /// After cooldown elapses the breaker auto-closes; the next call
    /// reaches the OS and (in this test) succeeds.
    #[tokio::test]
    async fn breaker_recovers_after_cooldown() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();

        let mut clipboard = MockSystemClipboard::new();
        // 5 fails to trip + 1 success after cooldown = 6 OS calls total.
        clipboard
            .expect_write_snapshot()
            .times(6)
            .returning(move |_| {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n < 5 {
                    Err(anyhow::anyhow!("simulated OS failure"))
                } else {
                    Ok(())
                }
            });

        let coord = ClipboardWriteCoordinator::with_cooldown(
            Arc::new(clipboard),
            Arc::new(permissive_origin_mock()),
            Duration::from_millis(80),
        );

        // Trip the breaker.
        for _ in 0..5 {
            let _ = coord
                .write(make_snapshot(), ClipboardWriteIntent::RemotePush)
                .await;
        }

        // Immediate retry blocked by open breaker.
        let err = coord
            .write(make_snapshot(), ClipboardWriteIntent::RemotePush)
            .await
            .expect_err("breaker should still be open");
        assert!(
            err.to_string().contains("circuit breaker open"),
            "got: {err}"
        );

        // Wait past cooldown — slightly more than 80ms to avoid flakiness.
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Now the breaker should be closed; write reaches OS and succeeds.
        coord
            .write(make_snapshot(), ClipboardWriteIntent::RemotePush)
            .await
            .expect("write should succeed after cooldown");
    }

    /// A single intervening success resets the consecutive-failure counter,
    /// so a 4-fail / success / 4-fail pattern (8 fails total but no
    /// 5-in-a-row streak) must NOT trip the breaker.
    #[tokio::test]
    async fn success_resets_consecutive_failure_counter() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();

        let mut clipboard = MockSystemClipboard::new();
        // Pattern: fail x4, ok, fail x4, ok — 10 total OS calls, no trip.
        // If counter wasn't reset by the first success, the 5th overall fail
        // (which is the 4th in the second streak) would trip and the 10th
        // call would be short-circuited, failing `.times(10)`.
        clipboard
            .expect_write_snapshot()
            .times(10)
            .returning(move |_| {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n == 4 || n == 9 {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("simulated OS failure"))
                }
            });

        let coord =
            ClipboardWriteCoordinator::new(Arc::new(clipboard), Arc::new(permissive_origin_mock()));

        for _ in 0..10 {
            let _ = coord
                .write(make_snapshot(), ClipboardWriteIntent::RemotePush)
                .await;
        }
        // Mock's times(10) asserts every call reached OS — none short-circuited.
    }
}
