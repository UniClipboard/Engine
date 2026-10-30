//! 绑定端到端测试专用宿主变化流。它保留真实启动清理路径。
use std::fs::{File, OpenOptions};
use std::future::pending;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use tokio::sync::Notify;

use crate::{
    HostCapabilities, HostCapabilityError, HostCapabilityErrorCategory, HostClipboard,
    HostClipboardChange, HostClipboardChangeStream, HostClipboardIdentity, HostClipboardSnapshot,
    HostClipboardWriteReceipt,
};

#[derive(Default)]
struct Control {
    calls: AtomicUsize,
    entered: AtomicBool,
    release: Notify,
}

static CONTROL: OnceLock<Arc<Control>> = OnceLock::new();

struct ProbeClipboard {
    inner: Box<dyn HostClipboard>,
    stream: Option<ProbeStream>,
}

impl HostClipboard for ProbeClipboard {
    fn read_with_identity(
        &self,
    ) -> Result<(HostClipboardSnapshot, Option<HostClipboardIdentity>), HostCapabilityError> {
        self.inner.read_with_identity()
    }

    fn write_with_receipt(
        &self,
        snapshot: HostClipboardSnapshot,
    ) -> Result<HostClipboardWriteReceipt, HostCapabilityError> {
        self.inner.write_with_receipt(snapshot)
    }

    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        self.inner.read()
    }

    fn write(&self, snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        self.inner.write(snapshot)
    }

    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
        Ok(self
            .stream
            .take()
            .map(|stream| Box::new(stream) as Box<dyn HostClipboardChangeStream>))
    }
}

struct ProbeStream {
    lease: Option<File>,
    control: Arc<Control>,
}

#[async_trait]
impl HostClipboardChangeStream for ProbeStream {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        pending().await
    }

    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        if self.control.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(HostCapabilityError::new(
                HostCapabilityErrorCategory::Io,
                "controlled startup close failure",
            ));
        }
        self.control.entered.store(true, Ordering::SeqCst);
        self.control.release.notified().await;
        self.lease.take();
        Ok(())
    }

    fn supports_shutdown_retry(&self) -> bool {
        true
    }
}

pub fn probe_wrap_startup_host(
    host: HostCapabilities,
    path: PathBuf,
) -> io::Result<HostCapabilities> {
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    lease.lock()?;
    let control = Arc::clone(CONTROL.get_or_init(|| Arc::new(Control::default())));
    let (directories, secure_storage, clipboard, files, analytics) = host.into_parts();
    let clipboard = ProbeClipboard {
        inner: clipboard,
        stream: Some(ProbeStream {
            lease: Some(lease),
            control,
        }),
    };
    Ok(HostCapabilities::from_shared_parts(
        directories,
        secure_storage,
        Box::new(clipboard),
        files,
        analytics,
    ))
}

pub fn probe_startup_retry_entered() -> bool {
    CONTROL
        .get()
        .is_some_and(|control| control.entered.load(Ordering::SeqCst))
}

pub fn probe_startup_retry_calls() -> usize {
    CONTROL
        .get()
        .map_or(0, |control| control.calls.load(Ordering::SeqCst))
}

pub fn probe_release_startup_retry() {
    if let Some(control) = CONTROL.get() {
        control.release.notify_one();
    }
}
