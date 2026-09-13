use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use tokio::sync::Notify;
use uc_engine::{
    HostCapabilityError, HostCapabilityErrorCategory, HostFileAccess, HostFileHandle,
    HostFileMetadata,
};

use super::{host_error, lock_unpoisoned};

#[derive(Clone)]
enum RegisteredFileSource {
    Path(PathBuf),
    Bytes(Arc<[u8]>),
}

#[derive(Clone)]
struct RegisteredFile {
    source: RegisteredFileSource,
    display_name: String,
    mime_type: Option<String>,
}

#[derive(Default)]
struct FileControlState {
    block_next_read: bool,
    read_blocked: bool,
}

struct FileControl {
    state: Mutex<FileControlState>,
    released: Condvar,
    read_started: Notify,
}

#[derive(Clone)]
pub(super) struct ProbeFiles {
    next_handle: Arc<AtomicU64>,
    files: Arc<Mutex<HashMap<String, RegisteredFile>>>,
    control: Arc<FileControl>,
}

impl Default for ProbeFiles {
    fn default() -> Self {
        Self {
            next_handle: Arc::new(AtomicU64::new(0)),
            files: Arc::new(Mutex::new(HashMap::new())),
            control: Arc::new(FileControl {
                state: Mutex::new(FileControlState::default()),
                released: Condvar::new(),
                read_started: Notify::new(),
            }),
        }
    }
}

impl ProbeFiles {
    pub(super) fn register(
        &self,
        path: PathBuf,
        display_name: String,
        mime_type: Option<String>,
    ) -> HostFileHandle {
        self.register_source(RegisteredFile {
            source: RegisteredFileSource::Path(path),
            display_name,
            mime_type,
        })
    }

    pub(super) fn register_fixture(&self, bytes: Vec<u8>) -> HostFileHandle {
        self.register_source(RegisteredFile {
            source: RegisteredFileSource::Bytes(bytes.into()),
            display_name: "lifecycle-probe.bin".to_owned(),
            mime_type: Some("application/octet-stream".to_owned()),
        })
    }

    pub(super) fn prepare_blocked_read(&self) {
        let mut state = lock_unpoisoned(&self.control.state);
        state.block_next_read = true;
        state.read_blocked = false;
    }

    pub(super) async fn wait_until_read_starts(&self) {
        loop {
            let started = self.control.read_started.notified();
            if lock_unpoisoned(&self.control.state).read_blocked {
                return;
            }
            started.await;
        }
    }

    pub(super) fn release_read(&self) {
        let mut state = lock_unpoisoned(&self.control.state);
        state.read_blocked = false;
        self.control.released.notify_all();
    }

    fn register_source(&self, file: RegisteredFile) -> HostFileHandle {
        let handle = format!(
            "probe-file-{}",
            self.next_handle.fetch_add(1, Ordering::Relaxed)
        );
        lock_unpoisoned(&self.files).insert(handle.clone(), file);
        HostFileHandle::new(handle)
    }

    fn lookup(&self, handle: &HostFileHandle) -> Result<RegisteredFile, HostCapabilityError> {
        lock_unpoisoned(&self.files)
            .get(handle.as_str())
            .cloned()
            .ok_or_else(|| host_error(HostCapabilityErrorCategory::InvalidHandle))
    }

    fn wait_if_read_blocked(&self) {
        let mut state = lock_unpoisoned(&self.control.state);
        if state.block_next_read {
            state.block_next_read = false;
            state.read_blocked = true;
            self.control.read_started.notify_waiters();
            while state.read_blocked {
                state = self
                    .control
                    .released
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
    }
}

impl HostFileAccess for ProbeFiles {
    fn metadata(&self, handle: &HostFileHandle) -> Result<HostFileMetadata, HostCapabilityError> {
        let file = self.lookup(handle)?;
        let size_bytes = match &file.source {
            RegisteredFileSource::Path(path) => std::fs::metadata(path)
                .map_err(|_| host_error(HostCapabilityErrorCategory::Io))?
                .len(),
            RegisteredFileSource::Bytes(bytes) => bytes.len() as u64,
        };
        Ok(HostFileMetadata {
            display_name: file.display_name,
            size_bytes,
            mime_type: file.mime_type,
        })
    }

    fn read_chunk(
        &self,
        handle: &HostFileHandle,
        offset: u64,
        max_bytes: u32,
    ) -> Result<Vec<u8>, HostCapabilityError> {
        let file = self.lookup(handle)?;
        self.wait_if_read_blocked();
        match file.source {
            RegisteredFileSource::Path(path) => {
                let mut input =
                    File::open(path).map_err(|_| host_error(HostCapabilityErrorCategory::Io))?;
                input
                    .seek(SeekFrom::Start(offset))
                    .map_err(|_| host_error(HostCapabilityErrorCategory::Io))?;
                let mut bytes = vec![0; max_bytes as usize];
                let read = input
                    .read(&mut bytes)
                    .map_err(|_| host_error(HostCapabilityErrorCategory::Io))?;
                bytes.truncate(read);
                Ok(bytes)
            }
            RegisteredFileSource::Bytes(bytes) => {
                let start = usize::try_from(offset)
                    .map_err(|_| host_error(HostCapabilityErrorCategory::Io))?;
                if start >= bytes.len() {
                    return Ok(Vec::new());
                }
                let end = start.saturating_add(max_bytes as usize).min(bytes.len());
                Ok(bytes[start..end].to_vec())
            }
        }
    }

    fn write_chunk(
        &self,
        handle: &HostFileHandle,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), HostCapabilityError> {
        let file = self.lookup(handle)?;
        let RegisteredFileSource::Path(path) = file.source else {
            return Err(host_error(HostCapabilityErrorCategory::InvalidHandle));
        };
        let mut output = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(path)
            .map_err(|_| host_error(HostCapabilityErrorCategory::Io))?;
        output
            .seek(SeekFrom::Start(offset))
            .and_then(|_| output.write_all(bytes))
            .map_err(|_| host_error(HostCapabilityErrorCategory::Io))
    }

    fn finish_write(&self, handle: &HostFileHandle) -> Result<(), HostCapabilityError> {
        let file = self.lookup(handle)?;
        let RegisteredFileSource::Path(path) = file.source else {
            return Err(host_error(HostCapabilityErrorCategory::InvalidHandle));
        };
        OpenOptions::new()
            .write(true)
            .open(path)
            .and_then(|output| output.sync_all())
            .map_err(|_| host_error(HostCapabilityErrorCategory::Io))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{HostFileAccess, ProbeFiles};

    #[tokio::test]
    async fn controlled_file_read_stays_blocked_until_released() {
        let files = ProbeFiles::default();
        let handle = files.register_fixture(vec![7; 1024]);
        files.prepare_blocked_read();
        let reading = tokio::task::spawn_blocking({
            let files = files.clone();
            move || files.read_chunk(&handle, 0, 512)
        });
        tokio::time::timeout(Duration::from_secs(1), files.wait_until_read_starts())
            .await
            .unwrap();
        assert!(!reading.is_finished());
        files.release_read();
        assert_eq!(reading.await.unwrap().unwrap(), vec![7; 512]);
    }
}
