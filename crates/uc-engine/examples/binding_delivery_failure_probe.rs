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

#[path = "../../../bindings/uc-engine-uniffi/src/runtime/startup_delivery.rs"]
#[allow(unexpected_cfgs, reason = "直接复用绑定模块；测试特性属于绑定 crate")]
#[allow(dead_code, reason = "Probe uses a subset of this module")]
mod startup_delivery;

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
    fail: bool,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}
#[async_trait]
impl HostClipboardChangeStream for TestStream {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }
    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = self.lease.metadata();
        if self.fail {
            Err(unavailable())
        } else {
            Ok(())
        }
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
    let root = tempfile::tempdir().expect("root");
    let lease_path = root.path().join("host.lease");
    let lease = File::create(&lease_path).expect("lease");
    lease.lock().expect("lock");
    let failed_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(Clipboard(Mutex::new(Some(TestStream {
            lease,
            fail: true,
            calls: Arc::clone(&failed_calls),
        })))),
        Box::new(Files),
    );
    let (engine, events) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .expect("real engine");
    let engine = Arc::new(engine);
    let owner = Arc::new(startup_delivery::StartupDeliveryOwner::default());
    assert!(matches!(
        owner.wait_internal(std::time::Duration::ZERO),
        startup_delivery::DeliveryOutcome::NotAccepted
    ));
    let weak = Arc::downgrade(&owner);
    owner.accept(Arc::clone(&engine));
    assert!(matches!(
        owner.wait_internal(std::time::Duration::ZERO),
        startup_delivery::DeliveryOutcome::Pending
    ));
    let outcome = owner
        .shutdown_owned(
            Arc::clone(&engine),
            None,
            uc_engine::EngineError::new(1108, uc_engine::EngineErrorCategory::Internal, false),
        )
        .await;
    let binding_outcome = match &outcome {
        ResourceReleaseOutcome::Failed { error } => BindingResourceRelease::Failed {
            error: error.clone(),
        },
        ResourceReleaseOutcome::Released { error } => BindingResourceRelease::Released {
            error: error.clone(),
        },
        ResourceReleaseOutcome::Pending => BindingResourceRelease::Pending,
    };
    owner.finish(&binding_outcome);
    let failed_observed = matches!(
        owner.wait_internal(std::time::Duration::ZERO),
        startup_delivery::DeliveryOutcome::Failed { .. }
    );
    let guard_owner = Arc::clone(&owner);
    let failed_worker = std::thread::spawn(move || {
        let _guard = startup_delivery::StartupWorkerGuard::new(
            guard_owner,
            uc_engine::EngineError::new(1108, uc_engine::EngineErrorCategory::Internal, false),
        );
        panic!("controlled worker termination after failed cleanup");
    });
    let failed_worker_panicked = failed_worker.join().is_err();
    let held_before_drop = !available(&lease_path);
    drop(events);
    drop(engine);
    drop(owner);
    let held_after_drop = !available(&lease_path);
    let owner_retained = weak.upgrade().is_some();
    let success_root = tempfile::tempdir().expect("success root");
    let success_path = success_root.path().join("host.lease");
    let success_lease = File::create(&success_path).expect("success lease");
    success_lease.lock().expect("success lock");
    let success_host = HostCapabilities::new(
        HostDirectories::new(
            success_root.path().join("private"),
            success_root.path().join("cache"),
            success_root.path().join("temporary"),
            success_root.path().join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(Clipboard(Mutex::new(Some(TestStream {
            lease: success_lease,
            fail: false,
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        })))),
        Box::new(Files),
    );
    let (success_engine, success_events) = Engine::start(EngineConfig::new("2.0.0"), success_host)
        .await
        .expect("success engine");
    let success_engine = Arc::new(success_engine);
    let success_owner = Arc::new(startup_delivery::StartupDeliveryOwner::default());
    let success_weak = Arc::downgrade(&success_owner);
    success_owner.accept(Arc::clone(&success_engine));
    let success_outcome = success_engine.shutdown_until_released().await;
    let success_binding = match &success_outcome {
        ResourceReleaseOutcome::Failed { error } => BindingResourceRelease::Failed {
            error: error.clone(),
        },
        ResourceReleaseOutcome::Released { error } => BindingResourceRelease::Released {
            error: error.clone(),
        },
        ResourceReleaseOutcome::Pending => BindingResourceRelease::Pending,
    };
    success_owner.finish(&success_binding);
    let released_observed = matches!(
        success_owner.wait_internal(std::time::Duration::ZERO),
        startup_delivery::DeliveryOutcome::Released { .. }
    );
    drop(success_events);
    drop(success_engine);
    drop(success_owner);
    let success_released = matches!(success_outcome, ResourceReleaseOutcome::Released { .. })
        && available(&success_path)
        && success_weak.upgrade().is_none();
    // 模拟交付负责人接收 Engine 后，绑定工作线程在调用 finish 前终止。
    let panic_root = tempfile::tempdir().expect("panic root");
    let panic_path = panic_root.path().join("host.lease");
    let panic_lease = File::create(&panic_path).expect("panic lease");
    panic_lease.lock().expect("panic lock");
    let panic_host = HostCapabilities::new(
        HostDirectories::new(
            panic_root.path().join("private"),
            panic_root.path().join("cache"),
            panic_root.path().join("temporary"),
            panic_root.path().join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(Clipboard(Mutex::new(Some(TestStream {
            lease: panic_lease,
            fail: false,
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        })))),
        Box::new(Files),
    );
    let (panic_engine, panic_events) = Engine::start(EngineConfig::new("2.0.0"), panic_host)
        .await
        .expect("panic engine");
    let panic_owner = Arc::new(startup_delivery::StartupDeliveryOwner::default());
    let panic_weak = Arc::downgrade(&panic_owner);
    let worker_owner = Arc::clone(&panic_owner);
    let worker = std::thread::spawn(move || {
        let _guard = startup_delivery::StartupWorkerGuard::new(
            Arc::clone(&worker_owner),
            uc_engine::EngineError::new(1108, uc_engine::EngineErrorCategory::Internal, false),
        );
        worker_owner.accept(Arc::new(panic_engine));
        panic!("controlled binding worker termination");
    });
    let worker_panicked = worker.join().is_err();
    drop(panic_events);
    let panic_outcome = panic_owner.wait_internal(std::time::Duration::from_secs(15));
    let panic_error_retained = matches!(
        panic_outcome,
        startup_delivery::DeliveryOutcome::Released {
            worker_error: Some(_),
            ..
        }
    );
    drop(panic_owner);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while panic_weak.upgrade().is_some() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let panic_owner_released = panic_weak.upgrade().is_none();
    let panic_lock_released = available(&panic_path);
    let recovery_root = tempfile::tempdir().expect("recovery root");
    let recovery_path = recovery_root.path().join("host.lease");
    let recovery_lease = File::create(&recovery_path).expect("recovery lease");
    recovery_lease.lock().expect("recovery lock");
    let recovery_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let recovery_host = HostCapabilities::new(
        HostDirectories::new(
            recovery_root.path().join("private"),
            recovery_root.path().join("cache"),
            recovery_root.path().join("temporary"),
            recovery_root.path().join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(Clipboard(Mutex::new(Some(TestStream {
            lease: recovery_lease,
            fail: true,
            calls: Arc::clone(&recovery_calls),
        })))),
        Box::new(Files),
    );
    let (recovery_engine, recovery_events) =
        Engine::start(EngineConfig::new("2.0.0"), recovery_host)
            .await
            .expect("recovery engine");
    let recovery_owner = Arc::new(startup_delivery::StartupDeliveryOwner::default());
    let recovery_weak = Arc::downgrade(&recovery_owner);
    let worker_owner = Arc::clone(&recovery_owner);
    let recovery_worker = std::thread::spawn(move || {
        let _guard = startup_delivery::StartupWorkerGuard::new(
            Arc::clone(&worker_owner),
            uc_engine::EngineError::new(1108, uc_engine::EngineErrorCategory::Internal, false),
        );
        worker_owner.accept(Arc::new(recovery_engine));
        panic!("controlled worker termination before failed recovery");
    });
    let recovery_panicked = recovery_worker.join().is_err();
    drop(recovery_events);
    drop(recovery_owner);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while recovery_calls.load(std::sync::atomic::Ordering::SeqCst) == 0
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let recovery_retained = recovery_weak.upgrade().is_some();
    let recovery_lock_held = !available(&recovery_path);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        std::fs::write(std::path::Path::new(&directory).join("binding-delivery-failure.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"production binding delivery owner, real Engines, controlled host closers and simulated worker termination; independent lock probes", "cleanup_failed":matches!(outcome, ResourceReleaseOutcome::Failed { .. }), "failed_result_observed":failed_observed, "released_result_observed":released_observed, "worker_error_preserved":panic_error_retained, "lock_held_before_drop":held_before_drop, "lock_held_after_drop":held_after_drop, "owner_retained_after_observer_drop":owner_retained, "successful_cleanup_releases_owner":success_released, "failed_worker_panicked":failed_worker_panicked, "failed_close_calls":failed_calls.load(std::sync::atomic::Ordering::SeqCst), "simulated_worker_panicked":worker_panicked, "panic_owner_released":panic_owner_released, "panic_host_lock_released":panic_lock_released, "recovery_panicked":recovery_panicked, "recovery_retained":recovery_retained, "recovery_lock_held":recovery_lock_held, "recovery_close_calls":recovery_calls.load(std::sync::atomic::Ordering::SeqCst), "actual_binding_worker":"not exercised", "native_mobile":"not established"})).expect("serialize")).expect("write evidence");
    }
    assert!(
        matches!(outcome, ResourceReleaseOutcome::Failed { .. })
            && held_before_drop
            && held_after_drop
            && owner_retained
            && failed_worker_panicked
            && failed_calls.load(std::sync::atomic::Ordering::SeqCst) == 1
    );
    assert!(success_released && failed_observed && released_observed && panic_error_retained);
    assert!(worker_panicked && panic_owner_released && panic_lock_released);
    assert!(
        recovery_panicked
            && recovery_retained
            && recovery_lock_held
            && recovery_calls.load(std::sync::atomic::Ordering::SeqCst) == 1
    );
}
