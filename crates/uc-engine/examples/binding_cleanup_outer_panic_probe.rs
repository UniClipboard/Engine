//! 真实 Engine 与受控宿主关闭失败；不代表移动设备验收。
use std::collections::HashMap;
use std::fs::File;
use std::process::Command;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_engine::{
    Engine, EngineConfig, HostCapabilities, HostCapabilityError, HostCapabilityErrorCategory,
    HostClipboard, HostClipboardChange, HostClipboardChangeStream, HostClipboardSnapshot,
    HostDirectories, HostFileAccess, HostFileHandle, HostFileMetadata, HostSecureStorage,
    ResourceReleaseOutcome,
};

#[allow(unexpected_cfgs, reason = "直接复用绑定模块；测试特性属于绑定 crate")]
mod startup_delivery {
    include!("../../../bindings/uc-engine-uniffi/src/runtime/startup_delivery.rs");

    pub(super) fn failed_result(owner: &StartupDeliveryOwner) -> bool {
        let state = owner
            .retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        matches!(
            state
                .as_ref()
                .and_then(|retained| retained.cleanup_result.as_ref()),
            Some(BindingResourceRelease::Failed { .. })
        )
    }

    pub(super) fn pending_waiter(owner: &StartupDeliveryOwner) -> bool {
        owner
            .retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|retained| {
                retained.pending_waiter.is_some() && retained.cleanup_result.is_none()
            })
    }

    pub(super) fn retry_failed(owner: &StartupDeliveryOwner) -> bool {
        let failures = owner
            .worker_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let retry_source_retained = matches!(failures.get(1), Some(RecoveryWorkerFailure::Runtime(error)) if error.kind() == std::io::ErrorKind::PermissionDenied);
        let state = owner
            .retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        retry_source_retained
            && state.as_ref().is_some_and(|retained| {
                retained.pending_waiter.is_some() && !retained.settlement_retry_in_progress
            })
    }
}

type BindingError = uc_engine::EngineError;
#[allow(dead_code)]
#[derive(Clone)]
enum BindingResourceRelease {
    Pending,
    Released { error: Option<BindingError> },
    Failed { error: BindingError },
}

impl From<ResourceReleaseOutcome> for BindingResourceRelease {
    fn from(outcome: ResourceReleaseOutcome) -> Self {
        match outcome {
            ResourceReleaseOutcome::Pending => Self::Pending,
            ResourceReleaseOutcome::Released { error } => Self::Released { error },
            ResourceReleaseOutcome::Failed { error } => Self::Failed { error },
        }
    }
}

#[derive(Default)]
struct Storage(Mutex<HashMap<String, Vec<u8>>>);
impl HostSecureStorage for Storage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        self.0
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

struct Files;
impl HostFileAccess for Files {
    fn metadata(&self, _: &HostFileHandle) -> Result<HostFileMetadata, HostCapabilityError> {
        Err(unavailable())
    }
    fn read_chunk(
        &self,
        _: &HostFileHandle,
        _: u64,
        _: u32,
    ) -> Result<Vec<u8>, HostCapabilityError> {
        Err(unavailable())
    }
    fn write_chunk(&self, _: &HostFileHandle, _: u64, _: &[u8]) -> Result<(), HostCapabilityError> {
        Err(unavailable())
    }
    fn finish_write(&self, _: &HostFileHandle) -> Result<(), HostCapabilityError> {
        Err(unavailable())
    }
}

fn unavailable() -> HostCapabilityError {
    HostCapabilityError::new(
        HostCapabilityErrorCategory::Unavailable,
        "controlled failure",
    )
}

struct TestStream {
    lease: File,
    worker_lease: Option<File>,
    entered: Option<std::sync::mpsc::Sender<()>>,
    release: Option<std::sync::mpsc::Receiver<()>>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}
#[async_trait]
impl HostClipboardChangeStream for TestStream {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }
    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let lease = self.worker_lease.take().expect("worker lease");
        let entered = self.entered.take().expect("worker entry");
        let release = self.release.take().expect("worker gate");
        let blocking = tokio::task::spawn_blocking(move || {
            let _lease = lease;
            std::fs::write(
                std::env::var_os("UC_TEST_BINDING_CLEANUP_OUTER_PANIC").expect("marker"),
                b"ready",
            )
            .expect("marker write");
            entered.send(()).expect("worker entry");
            release.recv().expect("worker gate");
        });
        let _ = self.lease.metadata();
        let _ = blocking.await;
        Ok(())
    }
}

struct Clipboard(Mutex<Option<TestStream>>);
impl HostClipboard for Clipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(HostClipboardSnapshot {
            observed_at_ms: 0,
            representations: Vec::new(),
        })
    }
    fn write(&self, _: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        Ok(())
    }
    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .take()
            .map(|stream| Box::new(stream) as Box<dyn HostClipboardChangeStream>))
    }
}

fn available(path: &std::path::Path) -> bool {
    let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(path).status().expect("independent lock probe");
    assert!(matches!(status.code(), Some(0 | 1)));
    status.success()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    const CHILD: &str = "UC_TEST_BINDING_CLEANUP_OUTER_PANIC";
    let retry_fails = std::env::args().any(|arg| arg == "--observer-retry-fail");
    let observer_lost = std::env::args().any(|arg| arg == "--observer-retry-lost");
    let observer_panic = std::env::args().any(|arg| arg == "--observer-panic");
    let observer_run_panic = std::env::args().any(|arg| arg == "--observer-run-panic");
    let observer_polled_panic = std::env::args().any(|arg| arg == "--observer-polled-panic");
    let observer_drop_panic = std::env::args().any(|arg| arg == "--observer-drop-panic");
    let observer_retry = retry_fails
        || observer_lost
        || observer_panic
        || observer_run_panic
        || observer_polled_panic
        || observer_drop_panic
        || std::env::args().any(|arg| arg == "--observer-retry");
    let observer_fail = observer_retry || std::env::args().any(|arg| arg == "--observer-fail");
    let engine_worker =
        observer_polled_panic || std::env::args().any(|arg| arg == "--engine-worker");
    if std::env::var_os(CHILD).is_none() {
        let marker_root = tempfile::tempdir().expect("marker root");
        let mut command = Command::new(std::env::current_exe().expect("executable"));
        command.env(CHILD, marker_root.path().join("ready"));
        if observer_fail {
            command
                .arg(if observer_drop_panic {
                    "--observer-drop-panic"
                } else if observer_polled_panic {
                    "--observer-polled-panic"
                } else if observer_run_panic {
                    "--observer-run-panic"
                } else if observer_panic {
                    "--observer-panic"
                } else if retry_fails {
                    "--observer-retry-fail"
                } else if observer_lost {
                    "--observer-retry-lost"
                } else if observer_retry {
                    "--observer-retry"
                } else {
                    "--observer-fail"
                })
                .env(
                    "UC_TEST_BINDING_OBSERVER_FAIL",
                    if observer_drop_panic {
                        "drop-panic"
                    } else if observer_polled_panic {
                        "polled-panic"
                    } else if observer_run_panic {
                        "run-panic"
                    } else if observer_panic {
                        "panic"
                    } else {
                        "1"
                    },
                );
            if observer_polled_panic {
                command.env("UC_TEST_BINDING_OBSERVER_POLL_DIR", marker_root.path());
            }
        }
        if retry_fails {
            command.env("UC_TEST_BINDING_SETTLEMENT_RETRY_FAIL", "1");
        }
        if engine_worker {
            command.arg("--engine-worker");
        }
        let status = command.status().expect("child");
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().expect("root");
    let host_path = root.path().join("host.lease");
    let host_lease = File::create(&host_path).expect("host lease");
    host_lease.lock().expect("host lock");
    let worker_path = root.path().join("worker.lease");
    let worker_lease = File::create(&worker_path).expect("worker lease");
    worker_lease.lock().expect("worker lock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (entered, waiting) = std::sync::mpsc::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(Clipboard(Mutex::new(Some(TestStream {
            lease: host_lease,
            worker_lease: Some(worker_lease),
            entered: Some(entered),
            release: Some(gate),
            calls: Arc::clone(&calls),
        })))),
        Box::new(Files),
    );
    let (engine, events) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .expect("engine");
    let engine = Arc::new(engine);
    let engine_worker_path = root.path().join("engine-worker.lease");
    let engine_worker_lease = File::create(&engine_worker_path).expect("engine worker lease");
    let (engine_release, engine_gate) = std::sync::mpsc::channel::<()>();
    if engine_worker {
        engine_worker_lease.lock().expect("engine worker lock");
        let (entered, ready) = std::sync::mpsc::channel::<()>();
        #[cfg(feature = "dev-tools")]
        assert!(engine.probe_spawn_owned_blocking(move || {
            let _lease = engine_worker_lease;
            entered.send(()).expect("engine worker entered");
            engine_gate.recv().expect("engine worker release");
        }));
        #[cfg(not(feature = "dev-tools"))]
        panic!("engine worker probe requires dev-tools");
        tokio::task::spawn_blocking(move || ready.recv_timeout(Duration::from_secs(15)))
            .await
            .expect("join")
            .expect("engine worker entered");
    }
    let owner = Arc::new(startup_delivery::StartupDeliveryOwner::default());
    owner.accept(Arc::clone(&engine));
    let weak = Arc::downgrade(&owner);
    let worker_owner = Arc::clone(&owner);
    let closing = tokio::spawn(async move {
        worker_owner
            .shutdown_owned(
                engine,
                None,
                uc_engine::EngineError::new(1108, uc_engine::EngineErrorCategory::Internal, false),
            )
            .await
    });
    tokio::task::spawn_blocking(move || waiting.recv_timeout(Duration::from_secs(15)))
        .await
        .expect("join")
        .expect("blocking worker entered");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let pending = !closing.is_finished();
    let host_held = !available(&host_path);
    let worker_held = !available(&worker_path);
    release.send(()).expect("release worker");
    let engine_worker_pending = if engine_worker {
        let deadline = Instant::now()
            + Duration::from_secs(if observer_panic || observer_run_panic {
                2
            } else {
                15
            });
        while !available(&worker_path) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let pending = !closing.is_finished() && !available(&engine_worker_path);
        if !observer_retry {
            engine_release.send(()).expect("release engine worker");
        }
        pending
    } else {
        true
    };
    if observer_polled_panic {
        let marker = std::path::PathBuf::from(
            std::env::var_os("UC_TEST_BINDING_OBSERVER_POLL_DIR").expect("marker directory"),
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        while !marker.join("observer-polled").is_file() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(marker.join("observer-polled").is_file());
        assert!(!closing.is_finished() && !available(&engine_worker_path));
        std::fs::write(marker.join("panic-observer"), b"panic").expect("panic marker");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !closing.is_finished(),
            "original waiter must remain pending while observer runtime drains"
        );
        assert!(
            !available(&engine_worker_path),
            "Engine work must remain held while observer runtime drains"
        );
        assert!(
            !owner.retry_settlement(),
            "a second observer must not start while the first is draining"
        );
        engine_release.send(()).expect("release engine worker");
    }
    if observer_fail {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !startup_delivery::pending_waiter(&owner) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let observer_pending = !closing.is_finished();
        let retained_waiter = startup_delivery::pending_waiter(&owner);
        let source = owner.probe_worker_failure();
        if observer_panic || observer_run_panic || observer_polled_panic || observer_drop_panic {
            assert!(
                retained_waiter,
                "observer panic must retain the original waiter"
            );
            assert!(
                matches!(source, Some((ref stage, ref kind, _)) if stage == "observer" && kind == "terminated")
            );
        }
        if observer_lost {
            closing.abort();
            let _ = closing.await;
            drop(owner);
            let owner = weak
                .upgrade()
                .expect("same owner retained after observer loss");
            let retry_accepted = owner.retry_settlement();
            let deadline = Instant::now() + Duration::from_secs(15);
            while !startup_delivery::failed_result(&owner) && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let result_recorded = startup_delivery::failed_result(&owner);
            let source_preserved = owner.probe_worker_failure() == source;
            drop(events);
            drop(owner);
            let retained = weak.upgrade().is_some();
            let worker_free = available(&worker_path);
            let host_held_after = !available(&host_path);
            let count = calls.load(Ordering::SeqCst);
            if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
                std::fs::create_dir_all(&dir).expect("directory");
                std::fs::write(std::path::Path::new(&dir).join("binding-cleanup-observer-retry-lost.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"real Engine, settlement retry after loss of original observer", "original_waiter_retained":retained_waiter, "retry_accepted":retry_accepted, "failed_result_recorded":result_recorded, "original_source_preserved":source_preserved, "owner_retained":retained, "worker_lock_released":worker_free, "host_lock_held":host_held_after, "close_calls":count, "native_mobile":"not established"})).expect("json")).expect("artifact");
            }
            assert!(
                pending
                    && host_held
                    && worker_held
                    && observer_pending
                    && retained_waiter
                    && retry_accepted
                    && result_recorded
                    && source_preserved
                    && retained
                    && worker_free
                    && host_held_after
                    && count == 1
            );
            return;
        }
        if observer_retry {
            let retry_accepted = owner.retry_settlement();
            let repeated_failure_retained = if retry_fails {
                let deadline = Instant::now() + Duration::from_secs(15);
                while !startup_delivery::retry_failed(&owner) && Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                let retained = startup_delivery::retry_failed(&owner) && !closing.is_finished();
                let accepted = owner.retry_settlement();
                retained && accepted
            } else {
                true
            };
            let concurrent_rejected = if engine_worker && !observer_polled_panic {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let rejected = !owner.retry_settlement() && !closing.is_finished();
                engine_release.send(()).expect("release engine worker");
                rejected
            } else {
                true
            };
            let outcome = tokio::time::timeout(Duration::from_secs(15), closing)
                .await
                .expect("same waiter settled")
                .expect("waiter join");
            let retry_failed = matches!(outcome, ResourceReleaseOutcome::Failed { .. });
            let result_recorded = startup_delivery::failed_result(&owner);
            let repeated_retry = owner.retry_settlement();
            let source_preserved = owner.probe_worker_failure() == source;
            let host_held_after = !available(&host_path);
            let worker_free = available(&worker_path);
            let engine_worker_free = !engine_worker || available(&engine_worker_path);
            let count = calls.load(Ordering::SeqCst);
            owner.finish(&BindingResourceRelease::from(outcome));
            drop(events);
            drop(owner);
            let retained = weak.upgrade().is_some();
            if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
                std::fs::create_dir_all(&dir).expect("directory");
                let name = if observer_drop_panic {
                    "binding-cleanup-observer-drop-panic.json"
                } else if observer_polled_panic {
                    "binding-cleanup-observer-polled-panic.json"
                } else if observer_run_panic {
                    "binding-cleanup-observer-run-panic.json"
                } else if observer_panic {
                    "binding-cleanup-observer-panic.json"
                } else if retry_fails {
                    "binding-cleanup-observer-retry-failed.json"
                } else if engine_worker {
                    "binding-cleanup-observer-concurrent.json"
                } else {
                    "binding-cleanup-observer-retry.json"
                };
                std::fs::write(std::path::Path::new(&dir).join(name), serde_json::to_vec_pretty(&serde_json::json!({"scope":"real Engine, failed settlement observer and explicit retry", "pending_before_retry":observer_pending, "original_waiter_retained":retained_waiter, "retry_accepted":retry_accepted, "repeated_failure_retained":repeated_failure_retained, "concurrent_retry_rejected":concurrent_rejected, "observer_polled_before_panic":observer_polled_panic, "retry_rejected_while_observer_draining":observer_polled_panic, "engine_worker_held":engine_worker, "engine_worker_lock_pending":engine_worker_pending, "engine_worker_lock_released":engine_worker_free, "same_waiter_failed":retry_failed, "private_result_recorded":result_recorded, "later_retry_accepted":repeated_retry, "original_source_preserved":source_preserved, "owner_retained":retained, "worker_lock_released":worker_free, "host_lock_held":host_held_after, "close_calls":count, "native_mobile":"not established"})).expect("json")).expect("artifact");
            }
            assert!(
                pending
                    && host_held
                    && worker_held
                    && engine_worker_pending
                    && observer_pending
                    && retained_waiter
                    && retry_accepted
                    && repeated_failure_retained
                    && concurrent_rejected
                    && retry_failed
                    && result_recorded
                    && !repeated_retry
                    && source_preserved
                    && retained
                    && worker_free
                    && engine_worker_free
                    && host_held_after
                    && count == 1
            );
            return;
        }
        closing.abort();
        drop(events);
        drop(owner);
        let retained = weak.upgrade().is_some();
        let worker_free = available(&worker_path);
        let host_held_after = !available(&host_path);
        let count = calls.load(Ordering::SeqCst);
        if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
            std::fs::create_dir_all(&dir).expect("directory");
            std::fs::write(std::path::Path::new(&dir).join("binding-cleanup-observer-failure.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"real Engine, injected cleanup worker panic and observation runtime failure", "pending_while_worker_held":pending, "observer_pending_after_worker":observer_pending, "retained_waiter":retained_waiter, "failure_source":source, "owner_retained_after_observer_loss":retained, "worker_lock_released":worker_free, "host_lock_held":host_held_after, "close_calls":count, "native_mobile":"not established"})).expect("json")).expect("artifact");
        }
        assert!(
            pending
                && host_held
                && worker_held
                && observer_pending
                && retained_waiter
                && retained
                && worker_free
                && host_held_after
                && count == 1
        );
        assert!(
            matches!(source, Some((ref stage, ref kind, true)) if stage == "runtime" && kind == "PermissionDenied")
        );
        return;
    }
    let outcome = tokio::time::timeout(Duration::from_secs(15), closing)
        .await
        .expect("shutdown settled")
        .expect("shutdown join");
    let failed = matches!(outcome, ResourceReleaseOutcome::Failed { .. });
    let binding = BindingResourceRelease::from(outcome);
    let settled_failed = startup_delivery::failed_result(&owner);
    owner.finish(&binding);
    drop(events);
    drop(owner);
    let retained = weak.upgrade().is_some();
    let worker_free = available(&worker_path);
    let host_held_after = !available(&host_path);
    let count = calls.load(Ordering::SeqCst);
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        let name = if engine_worker {
            "binding-cleanup-outer-engine-worker.json"
        } else {
            "binding-cleanup-outer-panic.json"
        };
        std::fs::write(std::path::Path::new(&dir).join(name), serde_json::to_vec_pretty(&serde_json::json!({"scope":"real Engine, injected outer binding cleanup worker panic, accepted blocking worker and independent locks", "pending_while_worker_held":pending, "pending_while_engine_worker_held":engine_worker_pending, "host_lock_held":host_held, "worker_lock_held":worker_held, "failed_after_worker":failed, "private_failed_result":settled_failed, "owner_retained":retained, "worker_lock_released":worker_free, "host_lock_held_after":host_held_after, "close_calls":count, "cleanup_thread_panic":"injected", "native_mobile":"not established"})).expect("json")).expect("artifact");
    }
    assert!(
        pending
            && engine_worker_pending
            && host_held
            && worker_held
            && failed
            && settled_failed
            && retained
            && worker_free
            && host_held_after
            && count == 1
    );
}
