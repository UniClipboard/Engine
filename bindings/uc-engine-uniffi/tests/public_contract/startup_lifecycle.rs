use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::lifecycle_targets::ReadGate;
use super::{
    engine_test_guard, lock, BindingConfig, BindingEngineState, BindingError, BindingErrorCategory,
    MemoryHost, MobileEngine, MobileStartupLifecycle, ENGINE_SHUTDOWN_DEADLINE_MS,
};

#[cfg(feature = "test-binding-worker-panic")]
#[test]
fn binding_recovery_creation_explicit_retry_releases_same_engine() {
    use std::process::Command;
    const CHILD: &str = "UC_TEST_BINDING_RETRY_CHILD";
    const FAILURE: &str = "UC_TEST_BINDING_RECOVERY_FAIL";
    if std::env::var_os(CHILD).is_none() {
        for (mode, stage) in [
            ("panic", "thread"),
            ("panic", "runtime"),
            ("panic", "executor-panic"),
            ("lost", "thread"),
            ("lost", "runtime"),
            ("lost", "executor-panic"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap());
            child.args(["--exact", "startup_lifecycle::binding_recovery_creation_explicit_retry_releases_same_engine", "--nocapture"])
                .env(CHILD, root.path()).env(FAILURE, stage).env("UC_TEST_BINDING_RECOVERY_ALLOW", root.path().join("allow-recovery"));
            if mode == "panic" {
                child.env("UC_TEST_BINDING_WORKER_PANIC", root.path());
            } else {
                child.env("UC_TEST_BINDING_DELIVERY_LOST", "1");
            }
            assert!(child.status().unwrap().success(), "{mode} {stage}");
        }
        return;
    }
    let stage = std::env::var(FAILURE).unwrap();
    let mode = if std::env::var_os("UC_TEST_BINDING_DELIVERY_LOST").is_some() {
        "lost"
    } else {
        "panic"
    };
    let root = std::path::PathBuf::from(std::env::var_os(CHILD).unwrap());
    let host = Arc::new(MemoryHost::new(&root));
    let lease_path = root.join("host.lease");
    let lease = std::fs::File::create(&lease_path).unwrap();
    lease.lock().unwrap();
    *lock(&host.startup_probe_lease) = Some(lease);
    let lifecycle = MobileStartupLifecycle::new();
    let result = MobileEngine::start_with_lifecycle(
        BindingConfig {
            app_version: "1.2.3".into(),
            profile_id: "recovery-explicit-retry".into(),
        },
        host.clone(),
        lifecycle.clone(),
    );
    assert!(matches!(result, Err(BindingError::RuntimeUnavailable)));
    let failed = lifecycle.probe_complete_startup(if stage == "executor-panic" {
        2_000
    } else {
        20_000
    });
    assert_eq!(failed.0, "failed");
    assert_eq!(failed.1, Some(BindingError::RuntimeUnavailable));
    assert!(failed.2.is_some());
    if stage == "executor-panic" {
        assert!(!failed.4);
        assert_eq!(failed, lifecycle.probe_complete_startup(0));
        assert!(!lifecycle.probe_retry_delivery());
        assert_eq!(failed, lifecycle.probe_retry_complete_startup(0));
        assert_eq!(
            lifecycle.probe_recovery_failure(),
            Some(("executor".into(), "panicked".into(), false))
        );
        drop(host);
        drop(lifecycle);
        let held = !Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(&lease_path).status().unwrap().success();
        assert!(held);
        if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(std::path::Path::new(&directory).join(format!("binding-{mode}-executor-panic.json")), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual binding entry, injected recovery executor construction panic", "failed":format!("{failed:?}"), "failure_source":"executor panicked", "host_lock_retained":held, "native_mobile":"not established"})).unwrap()).unwrap();
        }
        return;
    }
    assert!(failed.4);
    assert_eq!(failed, lifecycle.probe_complete_startup(0));
    let probe = |path: &std::path::Path| {
        Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(path).status().unwrap().success()
    };
    assert!(!probe(&lease_path));
    let failed_again = lifecycle.probe_retry_complete_startup(20_000);
    assert_eq!(failed_again.0, "failed");
    assert_eq!(failed_again.1, Some(BindingError::RuntimeUnavailable));
    assert!(failed_again.3.is_some());
    assert!(failed_again.4);
    assert!(!probe(&lease_path));
    std::fs::write(root.join("allow-recovery"), b"allow").unwrap();
    let gate = Arc::new(std::sync::Barrier::new(3));
    let retries: Vec<_> = (0..2)
        .map(|_| {
            let lifecycle = lifecycle.clone();
            let gate = gate.clone();
            thread::spawn(move || {
                gate.wait();
                lifecycle.probe_retry_delivery()
            })
        })
        .collect();
    gate.wait();
    let accepted = retries
        .into_iter()
        .map(|retry| retry.join().unwrap())
        .filter(|accepted| *accepted)
        .count();
    assert_eq!(accepted, 1);
    let released = lifecycle.probe_complete_startup(20_000);
    assert_eq!(released.0, "released");
    assert_eq!(released.1, Some(BindingError::RuntimeUnavailable));
    assert_eq!(released.3, failed.2);
    assert_eq!(released, lifecycle.probe_complete_startup(0));
    drop(host);
    assert!(probe(&lease_path));
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        let filename = if mode == "panic" {
            format!("binding-recovery-{stage}-retry.json")
        } else {
            format!("binding-lost-delivery-{stage}-retry.json")
        };
        std::fs::write(std::path::Path::new(&directory).join(filename), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual binding entry, controlled worker creation failure and explicit retry", "mode":mode, "failed":format!("{failed:?}"), "failed_again":format!("{failed_again:?}"), "unified_retry_observed_failure":true, "released":format!("{released:?}"), "concurrent_retries_accepted":accepted, "host_lock_retained_before_retry":true, "host_lock_released_after_retry":true, "native_mobile":"not established"})).unwrap()).unwrap();
    }
}

#[cfg(feature = "test-binding-worker-panic")]
#[test]
fn binding_worker_spawn_failure_retains_primary_before_future_drop() {
    use std::process::Command;

    const CHILD: &str = "UC_TEST_BINDING_SPAWN_FAIL";
    if std::env::var_os(CHILD).is_none() {
        let root = tempfile::tempdir().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "startup_lifecycle::binding_worker_spawn_failure_retains_primary_before_future_drop", "--nocapture"])
            .env(CHILD, root.path())
            .status().unwrap();
        assert!(status.success());
        return;
    }
    let root = std::path::PathBuf::from(std::env::var_os(CHILD).unwrap());
    let host = Arc::new(MemoryHost::new(&root));
    let lease_path = root.join("host.lease");
    let lease = std::fs::File::create(&lease_path).unwrap();
    lease.lock().unwrap();
    *lock(&host.startup_probe_lease) = Some(lease);
    let lifecycle = MobileStartupLifecycle::new();
    let start = thread::spawn({
        let lifecycle = lifecycle.clone();
        let host = host.clone();
        move || {
            MobileEngine::start_with_lifecycle(
                BindingConfig {
                    app_version: "1.2.3".into(),
                    profile_id: "spawn-failure".into(),
                },
                host,
                lifecycle,
            )
        }
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    while !root.join("spawn-rejected").exists() {
        assert!(Instant::now() < deadline, "spawn rejection was not reached");
        thread::sleep(Duration::from_millis(10));
    }
    let observed = lifecycle.probe_complete_startup(20_000);
    let repeated = lifecycle.probe_complete_startup(0);
    std::fs::write(root.join("continue"), b"continue").unwrap();
    let result = start.join().unwrap();
    assert!(matches!(result, Err(BindingError::RuntimeUnavailable)));
    assert_eq!(observed.1, Some(BindingError::RuntimeUnavailable));
    assert_eq!(observed, repeated);
    assert_eq!(observed.0, "released");
    drop(host);
    drop(lifecycle);
    let probe = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(&lease_path).status().unwrap();
    assert!(probe.success());
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(std::path::Path::new(&directory).join("binding-spawn-failure.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual binding entry, controlled thread spawn rejection and independent lock", "observation":format!("{observed:?}"), "repeated":format!("{repeated:?}"), "host_lock_released":probe.success(), "native_mobile":"not established"})).unwrap()).unwrap();
    }
}

#[cfg(feature = "test-binding-worker-panic")]
#[test]
fn binding_worker_panic_before_delivery_cleans_up_real_engine() {
    use std::process::Command;

    const CHILD: &str = "UC_TEST_BINDING_WORKER_PANIC";
    if std::env::var_os(CHILD).is_none() {
        let root = tempfile::tempdir().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "startup_lifecycle::binding_worker_panic_before_delivery_cleans_up_real_engine",
                "--nocapture",
            ])
            .env(CHILD, root.path())
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = std::path::PathBuf::from(std::env::var_os(CHILD).unwrap());
    let path = root.join("host.lease");
    let lease = std::fs::File::create(&path).unwrap();
    lease.lock().unwrap();
    let host = Arc::new(MemoryHost::new(&root));
    *lock(&host.startup_probe_lease) = Some(lease);
    let weak = Arc::downgrade(&host);
    let lifecycle = MobileStartupLifecycle::new();
    assert_eq!(
        lifecycle.probe_delivery_release(0),
        ("not_accepted".to_owned(), false)
    );
    let result = MobileEngine::start_with_lifecycle(
        BindingConfig {
            app_version: "1.2.3".into(),
            profile_id: "binding-panic".into(),
        },
        host.clone(),
        lifecycle.clone(),
    );
    let unavailable = matches!(&result, Err(BindingError::RuntimeUnavailable));
    let delivery = lifecycle.probe_delivery_release(20_000);
    let repeated = lifecycle.probe_delivery_release(0);
    let complete = lifecycle.probe_complete_startup(20_000);
    let complete_repeated = lifecycle.probe_complete_startup(0);
    drop(result);
    drop(host);
    drop(lifecycle);
    let deadline = Instant::now() + Duration::from_secs(20);
    while weak.upgrade().is_some() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let host_released = weak.upgrade().is_none();
    let probe = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(&path).status().unwrap();
    let lock_released = probe.success();
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(std::path::Path::new(&directory).join("binding-worker-panic.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual UniFFI startup worker, injected panic, real Engine and independent host object lock", "startup_error":unavailable, "private_delivery_result":delivery, "repeated_result":repeated, "complete_result":format!("{complete:?}"), "complete_repeated":format!("{complete_repeated:?}"), "host_released":host_released, "host_lock_released":lock_released, "native_mobile":"not established"})).unwrap()).unwrap();
    }
    assert!(
        unavailable
            && delivery == ("released".to_owned(), true)
            && repeated == delivery
            && complete.0 == "released"
            && complete.1 == Some(BindingError::RuntimeUnavailable)
            && complete_repeated == complete
            && host_released
            && lock_released
    );
}

#[cfg(feature = "test-binding-worker-panic")]
#[test]
fn binding_delivery_owner_waits_for_worker_runtime_to_drain() {
    use std::process::Command;

    const CHILD: &str = "UC_TEST_BINDING_RUNTIME_BLOCKER";
    if std::env::var_os(CHILD).is_none() {
        let root = tempfile::tempdir().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "startup_lifecycle::binding_delivery_owner_waits_for_worker_runtime_to_drain",
                "--nocapture",
            ])
            .env(CHILD, root.path())
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = std::path::PathBuf::from(std::env::var_os(CHILD).unwrap());
    let host = Arc::new(MemoryHost::new(&root));
    let lifecycle = MobileStartupLifecycle::new();
    let engine = MobileEngine::start_with_lifecycle(
        BindingConfig {
            app_version: "1.2.3".into(),
            profile_id: "binding-drain".into(),
        },
        host,
        lifecycle.clone(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !root.join("ready").is_file() {
        assert!(Instant::now() < deadline, "blocking worker did not start");
        thread::sleep(Duration::from_millis(10));
    }
    let first = engine.shutdown_with_outcome(100);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !root.join("draining").is_file() {
        assert!(
            Instant::now() < deadline,
            "binding runtime did not enter drain"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let pending = matches!(first, uc_engine_uniffi::BindingResourceRelease::Pending);
    let retained = lifecycle.probe_delivery_retained();
    let delivered = lifecycle.probe_delivery_release(0) == ("delivered".to_owned(), false);
    let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(root.join("worker.lease")).status().unwrap();
    let held = status.code() == Some(1);
    std::fs::write(root.join("release"), b"release").unwrap();
    let final_result = engine.shutdown_with_outcome(20_000);
    let released = matches!(
        final_result,
        uc_engine_uniffi::BindingResourceRelease::Released { .. }
    );
    let retained_after = lifecycle.probe_delivery_retained();
    let stable_delivered = lifecycle.probe_delivery_release(0) == ("delivered".to_owned(), false);
    let free = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+'); fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)"]).arg(root.join("worker.lease")).status().unwrap().success();
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(std::path::Path::new(&directory).join("binding-worker-runtime-drain.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual UniFFI worker, real Engine, controlled runtime blocking work and independent lock", "pending":pending, "retained_while_draining":retained, "delivered":delivered, "worker_lock_held":held, "released":released, "retained_after":retained_after, "stable_delivered":stable_delivered, "worker_lock_released":free, "native_mobile":"not established"})).unwrap()).unwrap();
    }
    assert!(
        pending
            && retained
            && delivered
            && held
            && released
            && !retained_after
            && stable_delivered
            && free
    );
}

#[cfg(feature = "test-binding-worker-panic")]
#[test]
fn binding_recovery_worker_creation_failure_retains_host() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Command, Stdio};

    const CHILD: &str = "UC_TEST_BINDING_WORKER_PANIC";
    const FAILURE: &str = "UC_TEST_BINDING_RECOVERY_FAIL";
    if std::env::var_os(CHILD).is_none() {
        for stage in ["thread", "runtime"] {
            let root = tempfile::tempdir().unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "startup_lifecycle::binding_recovery_worker_creation_failure_retains_host",
                    "--nocapture",
                ])
                .env(CHILD, root.path())
                .env(FAILURE, stage)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut output = BufReader::new(child.stdout.take().unwrap());
            let evidence = loop {
                let mut line = String::new();
                assert_ne!(
                    output.read_line(&mut line).unwrap(),
                    0,
                    "child exited before evidence"
                );
                if let Some(value) = line.split("UC_RECOVERY_FAILURE=").nth(1) {
                    break serde_json::from_str::<serde_json::Value>(value.trim()).unwrap();
                }
            };
            let probe = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(root.path().join("host.lease")).status().unwrap();
            assert_eq!(probe.code(), Some(1));
            child.stdin.take().unwrap().write_all(b"x").unwrap();
            assert!(child.wait().unwrap().success());
            assert_eq!(evidence["stage"], stage);
            assert_eq!(evidence["source_kind"], "PermissionDenied");
            assert_eq!(evidence["host_retained"], true);
            assert_eq!(evidence["close_started"], false);
            assert_eq!(evidence["complete_failed"], true);
            assert_eq!(evidence["primary_retained"], true);
            assert_eq!(evidence["cleanup_error_retained"], true);
            assert_eq!(evidence["stable_result"], true);
            if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
                std::fs::create_dir_all(&directory).unwrap();
                std::fs::write(std::path::Path::new(&directory).join(format!("binding-recovery-{stage}-failure.json")), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual binding worker, injected recovery creation failure, retained host and independent lock", "child":evidence, "host_lock_retained":true, "native_mobile":"not established"})).unwrap()).unwrap();
            }
        }
        return;
    }
    let root = std::path::PathBuf::from(std::env::var_os(CHILD).unwrap());
    let host = Arc::new(MemoryHost::new(&root));
    let lease = std::fs::File::create(root.join("host.lease")).unwrap();
    lease.lock().unwrap();
    *lock(&host.startup_probe_lease) = Some(lease);
    let weak = Arc::downgrade(&host);
    let lifecycle = MobileStartupLifecycle::new();
    let result = MobileEngine::start_with_lifecycle(
        BindingConfig {
            app_version: "1.2.3".into(),
            profile_id: "binding-recovery-failure".into(),
        },
        host.clone(),
        lifecycle.clone(),
    );
    assert!(matches!(result, Err(BindingError::RuntimeUnavailable)));
    drop(host);
    let deadline = Instant::now() + Duration::from_secs(20);
    let failure = loop {
        if let Some(failure) = lifecycle.probe_recovery_failure() {
            break failure;
        }
        assert!(
            Instant::now() < deadline,
            "recovery failure was not retained"
        );
        thread::sleep(Duration::from_millis(10));
    };
    let host_retained = weak.upgrade().is_some();
    let complete = lifecycle.probe_complete_startup(20_000);
    let repeated = lifecycle.probe_complete_startup(0);
    let complete_failed = complete.0 == "failed";
    let primary_retained = complete.1 == Some(BindingError::RuntimeUnavailable);
    let cleanup_error_retained = complete.2.is_some();
    let stable_result = complete == repeated;
    drop(lifecycle);
    println!(
        "UC_RECOVERY_FAILURE={}",
        serde_json::json!({"stage":failure.0, "source_kind":failure.1, "close_started":failure.2, "host_retained":host_retained, "complete_failed":complete_failed, "primary_retained":primary_retained, "cleanup_error_retained":cleanup_error_retained, "stable_result":stable_result})
    );
    std::io::stdout().flush().unwrap();
    let mut byte = [0];
    std::io::stdin().read_exact(&mut byte).unwrap();
    assert!(weak.upgrade().is_some());
}

#[test]
fn mobile_startup_accepts_a_pause_before_the_engine_is_returned() {
    let _guard = engine_test_guard();
    let root = tempfile::tempdir().unwrap();
    let host = Arc::new(MemoryHost::new(root.path()));
    let config = BindingConfig {
        app_version: "1.2.3".into(),
        profile_id: "mobile-startup-lifecycle".into(),
    };
    let initial = MobileEngine::start(config.clone(), host.clone()).unwrap();
    initial
        .create_space(
            Some("mobile startup lifecycle".into()),
            "correct horse battery staple".into(),
        )
        .unwrap();
    initial.shutdown(ENGINE_SHUTDOWN_DEADLINE_MS).unwrap();

    let (entered, waiting) = mpsc::channel();
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    *lock(&host.secure_read_gate) = Some(ReadGate {
        key_prefix: "kek:v1:",
        matches_before_wait: 0,
        entered,
        release: release.clone(),
    });
    let lifecycle = MobileStartupLifecycle::new();
    let starting = thread::spawn({
        let lifecycle = lifecycle.clone();
        let host = host.clone();
        let config = config.clone();
        move || MobileEngine::start_with_lifecycle(config, host, lifecycle)
    });
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();

    #[cfg(feature = "test-binding-worker-panic")]
    let complete_waiter = {
        assert_eq!(lifecycle.probe_complete_startup(0).0, "pending");
        let lifecycle = lifecycle.clone();
        thread::spawn(move || lifecycle.probe_complete_startup(20_000))
    };

    #[cfg(feature = "test-binding-worker-panic")]
    let (duplicate_finished, duplicate) = {
        // 给等待线程机会进入已接受启动的等待，再并发复用同一生命周期。
        thread::sleep(Duration::from_millis(50));
        let (sent, received) = mpsc::channel();
        let lifecycle = lifecycle.clone();
        let host = host.clone();
        let config = config.clone();
        let duplicate = thread::spawn(move || {
            let rejected = matches!(
                MobileEngine::start_with_lifecycle(config, host, lifecycle),
                Err(BindingError::Engine {
                    category: BindingErrorCategory::InvalidState,
                    ..
                })
            );
            sent.send(rejected).unwrap();
        });
        (
            received.recv_timeout(Duration::from_secs(1)).ok(),
            duplicate,
        )
    };

    let started_at = Instant::now();
    assert!(matches!(
        lifecycle.suspend_with_deadline(0),
        Err(BindingError::Engine {
            category: BindingErrorCategory::DeadlineExceeded,
            ..
        })
    ));
    assert!(started_at.elapsed() < Duration::from_secs(1));
    *lock(&release.0) = true;
    release.1.notify_all();

    #[cfg(feature = "test-binding-worker-panic")]
    {
        duplicate.join().unwrap();
        assert_eq!(duplicate_finished, Some(true));
    }
    let engine = starting.join().unwrap().unwrap();
    #[cfg(feature = "test-binding-worker-panic")]
    assert_eq!(complete_waiter.join().unwrap().0, "delivered");
    #[cfg(feature = "test-binding-worker-panic")]
    assert_eq!(
        lifecycle.probe_delivery_release(0),
        ("delivered".to_owned(), false)
    );
    assert!(matches!(
        engine.lifecycle_state().unwrap(),
        BindingEngineState::Quiesced | BindingEngineState::Suspended
    ));
    assert!(engine.list_devices().is_err());
    lifecycle
        .suspend_with_deadline(ENGINE_SHUTDOWN_DEADLINE_MS)
        .unwrap();
    assert_eq!(
        engine.lifecycle_state().unwrap(),
        BindingEngineState::Suspended
    );
    lifecycle.resume().unwrap();
    assert!(engine.list_devices().is_ok());

    let another_root = tempfile::tempdir().unwrap();
    let reused = MobileEngine::start_with_lifecycle(
        BindingConfig {
            app_version: "1.2.3".into(),
            profile_id: "reused-startup-lifecycle".into(),
        },
        Arc::new(MemoryHost::new(another_root.path())),
        lifecycle.clone(),
    );
    assert!(matches!(
        reused,
        Err(BindingError::Engine {
            category: BindingErrorCategory::InvalidState,
            ..
        })
    ));
    engine.shutdown(ENGINE_SHUTDOWN_DEADLINE_MS).unwrap();
    assert!(matches!(
        lifecycle.resume(),
        Err(BindingError::Engine {
            category: BindingErrorCategory::InvalidState,
            ..
        })
    ));
}

#[test]
fn failed_host_preparation_finishes_the_startup_lifecycle() {
    let _guard = engine_test_guard();
    let root = tempfile::tempdir().unwrap();
    let host = Arc::new(MemoryHost::new(root.path()));
    std::fs::write(&host.private_data, b"blocks directory creation").unwrap();
    let lifecycle = MobileStartupLifecycle::new();
    let initial = lifecycle.wait_release(0);
    assert_eq!(
        initial.state,
        uc_engine_uniffi::BindingStartupReleaseState::NotStarted
    );
    assert_ne!(
        initial.attempt_id,
        MobileStartupLifecycle::new().wait_release(0).attempt_id
    );
    let result = MobileEngine::start_with_lifecycle(
        BindingConfig {
            app_version: "1.2.3".into(),
            profile_id: "startup-host-failure".into(),
        },
        host,
        lifecycle.clone(),
    );
    let primary = result.err().expect("host preparation must fail");
    let public_receipt = lifecycle.wait_release(0);
    assert_eq!(
        public_receipt.state,
        uc_engine_uniffi::BindingStartupReleaseState::Released
    );
    assert_eq!(public_receipt.attempt_id, initial.attempt_id);
    assert_eq!(public_receipt.startup_error, Some(primary.clone()));
    assert!(public_receipt.cleanup_error.is_none());
    assert!(!public_receipt.explicit_retry_supported);
    let public_retry = lifecycle.retry_release(0);
    assert_eq!(
        (public_retry.state, public_retry.attempt_id),
        (public_receipt.state, public_receipt.attempt_id.clone())
    );
    #[cfg(feature = "test-binding-worker-panic")]
    assert_eq!(
        lifecycle.probe_complete_startup(0),
        ("released".into(), Some(primary.clone()), None, None, false)
    );
    #[cfg(feature = "test-binding-worker-panic")]
    assert_eq!(
        lifecycle.probe_retry_complete_startup(0),
        lifecycle.probe_complete_startup(0)
    );
    #[cfg(feature = "test-binding-worker-panic")]
    assert_eq!(
        lifecycle.probe_delivery_release(0),
        ("not_accepted".to_owned(), false)
    );
    let error = lifecycle.suspend_with_deadline(100).unwrap_err();
    assert!(
        !matches!(
            error,
            BindingError::Engine {
                category: BindingErrorCategory::DeadlineExceeded,
                ..
            }
        ),
        "a completed failed start must not leave its lifecycle waiting"
    );
    assert_eq!(
        error, primary,
        "lifecycle must preserve the host preparation error"
    );
    let reused = MobileEngine::start_with_lifecycle(
        BindingConfig {
            app_version: "1.2.3".into(),
            profile_id: "startup-reused".into(),
        },
        Arc::new(MemoryHost::new(root.path())),
        lifecycle.clone(),
    );
    assert!(matches!(
        reused,
        Err(BindingError::Engine {
            category: BindingErrorCategory::InvalidState,
            ..
        })
    ));
    let repeated = lifecycle.suspend_with_deadline(100).unwrap_err();
    assert_eq!(repeated, primary);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            std::path::Path::new(&directory).join("binding-preparation-failure.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "scope": "real binding host directory failure",
                "primary": format!("{primary:?}"),
                "lifecycle_error": format!("{error:?}"),
                "reused_attempt_rejected": true,
                "original_error_retained": repeated == primary,
                "public_receipt": format!("{public_receipt:?}"),
                "attempt_stable": initial.attempt_id == public_receipt.attempt_id
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

/// 绑定执行器创建失败时，启动负责人不能直接丢弃宿主能力。
#[cfg(target_os = "macos")]
#[test]
fn binding_executor_creation_failure_retains_startup_host() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};

    const CHILD: &str = "UC_TEST_BINDING_EXECUTOR_FAILURE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let root = tempfile::tempdir().unwrap();
        let lease_path = root.path().join("host.lease");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "startup_lifecycle::binding_executor_creation_failure_retains_startup_host",
                "--nocapture",
            ])
            .env(CHILD, root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let evidence = loop {
            let mut line = String::new();
            assert!(
                reader.read_line(&mut line).unwrap() != 0,
                "child exited before evidence"
            );
            if let Some(json) = line.split("UC_BINDING_EXECUTOR_EVIDENCE=").nth(1) {
                break json.trim().to_owned();
            }
        };
        let observation: serde_json::Value = serde_json::from_str(&evidence).unwrap();
        let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(&lease_path).status().unwrap();
        assert!(matches!(status.code(), Some(0 | 1)));
        let held = !status.success();
        child.stdin.take().unwrap().write_all(b"x").unwrap();
        let exit = child.wait().unwrap();
        assert!(exit.success());
        if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(std::path::Path::new(&directory).join("binding-executor-failure.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"real UniFFI entry point, isolated descriptor exhaustion, host object retention and independent host lock probe", "child":observation, "host_lock_retained":held, "startup_release_receipt":"not established", "device_acceptance":"not established"})).unwrap()).unwrap();
        }
        assert_eq!(observation["executor_builder_failed"], true);
        assert_eq!(observation["worker_thread_available"], true);
        assert_eq!(observation["runtime_unavailable"], true);
        assert_eq!(observation["host_retained"], true);
        #[cfg(feature = "test-binding-worker-panic")]
        {
            assert_eq!(observation["complete_unconfirmed"], true);
            assert_eq!(observation["primary_retained"], true);
            assert_eq!(observation["stable_result"], true);
        }
        assert!(held, "unconfirmed host owner must retain the host object");
        return;
    }
    let root = std::path::PathBuf::from(std::env::var_os(CHILD).unwrap());
    let host = Arc::new(MemoryHost::new(&root));
    for path in [&host.private_data, &host.cache, &host.temporary] {
        std::fs::create_dir_all(path).unwrap();
    }
    let lease = std::fs::File::create(root.join("host.lease")).unwrap();
    lease.lock().unwrap();
    *lock(&host.startup_probe_lease) = Some(lease);
    let weak = Arc::downgrade(&host);
    let lifecycle = MobileStartupLifecycle::new();
    let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    // SAFETY: 指针指向可写的 rlimit，子进程不修改父进程的限制。
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) },
        0
    );
    // SAFETY: getrlimit 已成功写入该结构。
    let mut limit = unsafe { limit.assume_init() };
    limit.rlim_cur = limit.rlim_cur.min(128);
    // SAFETY: 结构有效，且仅降低本子进程的软限制。
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) }, 0);
    let mut fillers = Vec::new();
    while let Ok(file) = std::fs::File::open("/dev/null") {
        fillers.push(file);
    }
    let executor_builder_failed = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .is_err();
    let worker_thread_available = std::thread::Builder::new()
        .spawn(|| ())
        .map(|worker| worker.join().is_ok())
        .unwrap_or(false);
    let result = MobileEngine::start_with_lifecycle(
        BindingConfig {
            app_version: "1.2.3".into(),
            profile_id: "binding-executor-failure".into(),
        },
        host.clone(),
        lifecycle.clone(),
    );
    let runtime_unavailable = matches!(result, Err(BindingError::RuntimeUnavailable));
    drop(result);
    drop(host);
    std::thread::sleep(Duration::from_millis(300));
    let host_retained = weak.upgrade().is_some();
    #[cfg(feature = "test-binding-worker-panic")]
    let (complete_unconfirmed, primary_retained, stable_result) = {
        let complete = lifecycle.probe_complete_startup(1000);
        let repeated = lifecycle.probe_complete_startup(0);
        (
            complete.0 == "failed" || complete.0 == "pending",
            complete.1 == Some(BindingError::RuntimeUnavailable),
            complete == repeated,
        )
    };
    #[cfg(not(feature = "test-binding-worker-panic"))]
    let (complete_unconfirmed, primary_retained, stable_result) = (false, false, false);
    println!(
        "UC_BINDING_EXECUTOR_EVIDENCE={}",
        serde_json::json!({"executor_builder_failed":executor_builder_failed, "worker_thread_available":worker_thread_available, "runtime_unavailable":runtime_unavailable, "host_retained":host_retained, "complete_unconfirmed":complete_unconfirmed, "primary_retained":primary_retained, "stable_result":stable_result})
    );
    std::io::stdout().flush().unwrap();
    let mut byte = [0];
    use std::io::Read;
    std::io::stdin().read_exact(&mut byte).unwrap();
    drop(fillers);
    assert!(matches!(
        lifecycle.suspend_with_deadline(100),
        Err(BindingError::RuntimeUnavailable)
    ));
}

#[cfg(feature = "test-binding-worker-panic")]
#[test]
fn binding_real_engine_rejected_config_retains_startup_receipt() {
    use std::process::Command;
    const CHILD: &str = "UC_TEST_BINDING_INVALID_ENGINE_CONFIG";
    if std::env::var_os(CHILD).is_none() {
        let root = tempfile::tempdir().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "startup_lifecycle::binding_real_engine_rejected_config_retains_startup_receipt",
                "--nocapture",
            ])
            .env(CHILD, root.path())
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = std::path::PathBuf::from(std::env::var_os(CHILD).unwrap());
    let path = root.join("host.lease");
    let lease = std::fs::File::create(&path).unwrap();
    lease.lock().unwrap();
    let host = Arc::new(MemoryHost::new(&root));
    *lock(&host.startup_probe_lease) = Some(lease);
    let weak = Arc::downgrade(&host);
    let lifecycle = MobileStartupLifecycle::new();
    let result = MobileEngine::start_with_lifecycle(
        BindingConfig {
            app_version: "1.2.3".into(),
            profile_id: "invalid-engine-config".into(),
        },
        host.clone(),
        lifecycle.clone(),
    );
    let primary = result.err();
    let receipt = lifecycle.probe_complete_startup(20_000);
    let repeated = lifecycle.probe_complete_startup(0);
    let explicit_retry = lifecycle.probe_retry_complete_startup(0);
    drop(host);
    let deadline = Instant::now() + Duration::from_secs(5);
    while weak.upgrade().is_some() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let released_host = weak.upgrade().is_none();
    let available = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry: fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError: sys.exit(1)"]).arg(&path).status().unwrap().success();
    let valid = primary.is_some()
        && receipt.0 == "released"
        && receipt.1 == primary
        && receipt.2.is_none()
        && receipt.3.is_none()
        && !receipt.4
        && receipt == repeated
        && receipt == explicit_retry
        && released_host
        && available;
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(std::path::Path::new(&directory).join("binding-invalid-engine-startup.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual binding constructor, real Engine rejected configuration, independent host lock", "primary_present":primary.is_some(), "receipt":format!("{receipt:?}"), "stable":receipt==repeated, "retry_does_not_restart":receipt==explicit_retry, "host_released":released_host, "host_lock_available":available, "native_device":"not established"})).unwrap()).unwrap();
    }
    assert!(
        valid,
        "binding startup must retain real Engine failure and cleanup receipt"
    );
}

#[cfg(feature = "test-binding-worker-panic")]
#[test]
fn binding_real_engine_startup_retry_keeps_host_owner() {
    use std::process::Command;
    const CHILD: &str = "UC_TEST_BINDING_STARTUP_RETRY_ROOT";
    if std::env::var_os(CHILD).is_none() {
        let root = tempfile::tempdir().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "startup_lifecycle::binding_real_engine_startup_retry_keeps_host_owner",
                "--nocapture",
            ])
            .env(CHILD, root.path())
            .env("UC_TEST_BINDING_INVALID_ENGINE_CONFIG", "1")
            .env(
                "UC_TEST_BINDING_STARTUP_RETRY_LOCK",
                root.path().join("stream.lease"),
            )
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = std::path::PathBuf::from(std::env::var_os(CHILD).unwrap());
    let path = root.join("stream.lease");
    let host = Arc::new(MemoryHost::new(&root));
    let lifecycle = MobileStartupLifecycle::new();
    let result = MobileEngine::start_with_lifecycle(
        BindingConfig {
            app_version: "1.2.3".into(),
            profile_id: "binding-engine-retry".into(),
        },
        host,
        lifecycle.clone(),
    );
    let primary = result.err().expect("Engine rejects invalid config");
    let first = lifecycle.probe_complete_startup(20_000);
    let repeated = lifecycle.probe_complete_startup(0);
    let public_first = lifecycle.wait_release(0);
    assert_eq!(
        public_first.state,
        uc_engine_uniffi::BindingStartupReleaseState::Failed
    );
    assert_eq!(public_first.startup_error, Some(primary.clone()));
    assert!(public_first.explicit_retry_supported);
    let available = || {
        Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry: fcntl.flock(f,fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError: sys.exit(1)"]).arg(&path).status().unwrap().success()
    };
    let first_held = !available();
    let observed_calls = uc_engine::probe_startup_retry_calls();
    let first_valid = first.0 == "failed"
        && first.1 == Some(primary.clone())
        && first.2.is_some()
        && first.4
        && first == repeated
        && first_held;
    let public_retry = lifecycle.retry_release(0);
    assert_eq!(
        public_retry.state,
        uc_engine_uniffi::BindingStartupReleaseState::Pending
    );
    assert_eq!(public_retry.attempt_id, public_first.attempt_id);
    let retry = lifecycle.probe_retry_complete_startup(0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !uc_engine::probe_startup_retry_entered() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let entered = uc_engine::probe_startup_retry_entered();
    let pending = lifecycle.probe_complete_startup(0);
    let retry_again = lifecycle.probe_retry_complete_startup(0);
    let pending_calls = uc_engine::probe_startup_retry_calls();
    let held = !available();
    uc_engine::probe_release_startup_retry();
    let settled = lifecycle.probe_complete_startup(20_000);
    let public_settled = lifecycle.wait_release(0);
    assert_eq!(
        public_settled.state,
        uc_engine_uniffi::BindingStartupReleaseState::Released
    );
    assert_eq!(public_settled.attempt_id, public_first.attempt_id);
    assert_eq!(public_settled.startup_error, Some(primary.clone()));
    assert_eq!(public_settled.operation_error, public_first.cleanup_error);
    let stable = lifecycle.probe_complete_startup(0) == settled;
    let closed = available();
    let final_calls = uc_engine::probe_startup_retry_calls();
    let valid = first_valid
        && observed_calls == 1
        && pending_calls == 2
        && final_calls == 2
        && retry.0 == "pending"
        && entered
        && pending.0 == "pending"
        && retry_again.0 == "pending"
        && held
        && settled.0 == "released"
        && settled.1 == Some(primary)
        && settled.3 == first.2
        && !settled.4
        && stable
        && closed;
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(std::path::Path::new(&directory).join("binding-engine-startup-retry.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual binding constructor and real Engine, controlled retryable host stream and independent lock", "first":format!("{first:?}"), "public_first":format!("{public_first:?}"), "public_retry":format!("{public_retry:?}"), "public_settled":format!("{public_settled:?}"), "retry":format!("{retry:?}"), "pending":format!("{pending:?}"), "retry_again":format!("{retry_again:?}"), "settled":format!("{settled:?}"), "observed_calls":observed_calls, "pending_calls":pending_calls, "final_calls":final_calls, "first_lock_held":first_held, "retry_lock_held":held, "final_lock_released":closed, "stable":stable, "native_device":"not established"})).unwrap()).unwrap();
    }
    assert!(
        valid,
        "binding must retain Engine startup retry and host owner"
    );
}
