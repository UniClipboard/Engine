//! 隔离验证宿主回调崩溃后的变化流状态；不代表完整启动释放回执。
#![allow(dead_code)]

#[path = "../src/contract/host.rs"]
mod host_contract;
#[path = "../src/runtime/reusable_host.rs"]
mod reusable_host;

mod contract {
    pub(crate) use super::host_contract::HostAnalyticsCapabilities;
}

use std::fs::File;
use std::process::Command;
use std::sync::Arc;

pub use host_contract::*;
use reusable_host::ReusableHost;
use uc_engine::HostFileHandle;

struct Storage;
impl HostSecureStorage for Storage {
    fn get(&self, _: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        Ok(None)
    }
    fn set(&self, _: &str, _: &[u8]) -> Result<(), HostCapabilityError> {
        Ok(())
    }
    fn delete(&self, _: &str) -> Result<(), HostCapabilityError> {
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
    HostCapabilityError::new(HostCapabilityErrorCategory::Unavailable, "controlled probe")
}

struct PanickingClipboard(File);
impl HostClipboard for PanickingClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Err(unavailable())
    }
    fn write(&self, _: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        Err(unavailable())
    }
    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
        let _ = self.0.metadata();
        panic!("controlled host callback panic");
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let root = tempfile::tempdir().expect("temporary root");
    let path = root.path().join("host-stream.lease");
    let lease = File::create(&path).expect("create lease");
    lease.lock().expect("hold lease");
    let host = Arc::new(ReusableHost::new(HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temp"),
            root.path().join("logs"),
        ),
        Box::new(Storage),
        Box::new(PanickingClipboard(lease)),
        Box::new(Files),
    )));
    let first_owner = Arc::clone(&host);
    let first = tokio::spawn(async move { first_owner.close_change_stream().await }).await;
    let second = host.close_change_stream().await;
    let probe = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"])
        .arg(&path).status().expect("probe lease");
    assert!(matches!(probe.code(), Some(0 | 1)), "probe failed");
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        std::fs::write(std::path::Path::new(&directory).join("host-stream-panic.json"),
            serde_json::to_vec_pretty(&serde_json::json!({"scope":"isolated production host wrapper, controlled callback panic and independent lock probe", "first_callback_panicked":first.is_err(), "second_close_unconfirmed":second.is_err(), "host_lock_retained":!probe.success(), "complete_startup_release":"not established"})).expect("serialize")).expect("write evidence");
    }
    assert!(first.is_err(), "callback must panic");
    assert!(
        second.is_err(),
        "unknown callback outcome cannot certify release"
    );
    assert!(!probe.success(), "host must retain its controlled lock");
}
