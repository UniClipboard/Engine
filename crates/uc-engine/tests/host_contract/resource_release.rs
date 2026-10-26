#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use uc_engine::{
    CreateSpaceInput, Engine, EngineConfig, HostCapabilities, HostCapabilityError,
    HostCapabilityErrorCategory, HostClipboard, HostClipboardChange, HostClipboardChangeStream,
    HostClipboardSnapshot, HostDirectories, Operation, SecretString,
};

use super::{EmptyClipboard, EmptyFiles, MemorySecureStorage};

struct Stream {
    lease: Option<File>,
    fail: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl HostClipboardChangeStream for Stream {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }

    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(HostCapabilityError::new(
                HostCapabilityErrorCategory::Io,
                "controlled required closer failure",
            ));
        }
        self.lease.take();
        Ok(())
    }
}

struct Clipboard(Option<Stream>);
impl HostClipboard for Clipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        EmptyClipboard.read()
    }
    fn write(&self, value: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        EmptyClipboard.write(value)
    }
    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
        Ok(self
            .0
            .take()
            .map(|s| Box::new(s) as Box<dyn HostClipboardChangeStream>))
    }
}

fn available(path: &Path) -> bool {
    let output = Command::new("python3").arg("-c").arg(
        "import sys,fcntl; f=open(sys.argv[1], 'r+');\ntry: fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB)\nexcept BlockingIOError: sys.exit(1)"
    ).arg(path).status().unwrap();
    output.success()
}

fn evidence(name: &str, value: serde_json::Value) {
    let root = std::env::var_os("UC_TEST_ARTIFACTS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "target/test-artifacts/resource-release".into());
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join(format!("{name}.json")),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn required_closer_failure_retains_real_lock_for_retry() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("required-closer.lease");
    let file = File::create(&path).unwrap();
    file.lock().unwrap();
    let fail = Arc::new(AtomicBool::new(true));
    let calls = Arc::new(AtomicUsize::new(0));
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temp"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(Clipboard(Some(Stream {
            lease: Some(file),
            fail: fail.clone(),
            calls: calls.clone(),
        }))),
        Box::new(EmptyFiles),
    );
    let (engine, _events) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("release acceptance".into()),
            passphrase: SecretString::new("release-acceptance-passphrase"),
            passphrase_confirmation: SecretString::new("release-acceptance-passphrase"),
        }))
        .await
        .unwrap();
    assert!(!available(&path));
    let failed = engine.shutdown(Duration::from_secs(15)).await;
    let retained = !available(&path);
    evidence(
        "required-closer",
        serde_json::json!({"shutdown_error":failed.is_err(), "lock_retained_after_failure":retained, "calls":calls.load(Ordering::SeqCst)}),
    );
    assert!(
        failed.is_err(),
        "required closer failure must prevent successful release"
    );
    assert!(retained, "the owner must retain its failed closer handle");
    fail.store(false, Ordering::SeqCst);
    use uc_engine::ResourceReleaseOutcome;
    let recovered = engine.shutdown_with_outcome(Duration::from_secs(15)).await;
    assert!(matches!(
        recovered,
        ResourceReleaseOutcome::Released { error: Some(_) }
    ));
    assert!(available(&path));
    let count = calls.load(Ordering::SeqCst);
    assert!(count >= 2);
    assert_eq!(
        engine.shutdown_with_outcome(Duration::from_secs(15)).await,
        recovered
    );
    assert_eq!(
        engine.suspend_with_outcome(Duration::from_secs(15)).await,
        recovered
    );
    assert_eq!(calls.load(Ordering::SeqCst), count);
    evidence(
        "required-closer-recovered",
        serde_json::json!({"release_confirmed":true,"previous_error_preserved":true,"independent_lock_available":true,"repeat_did_not_close_again":true,"calls":count}),
    );
}
