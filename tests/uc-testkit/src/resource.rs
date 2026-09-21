use std::{
    io,
    net::{SocketAddr, TcpListener},
    path::Path,
    sync::{Arc, Mutex},
};

use tempfile::TempDir;

use crate::{CleanupStatus, ResourceReport, budget::lock};

pub(crate) type ResourceTracker = Arc<Mutex<Vec<ResourceReport>>>;

fn register(tracker: &ResourceTracker, kind: &'static str, label: &'static str) -> usize {
    let mut resources = lock(tracker);
    let index = resources.len();
    resources.push(ResourceReport {
        kind: kind.to_owned(),
        label: label.to_owned(),
        cleanup: CleanupStatus::Pending,
    });
    index
}

fn complete(tracker: &ResourceTracker, index: usize, status: CleanupStatus) {
    if let Some(resource) = lock(tracker).get_mut(index) {
        resource.cleanup = status;
    }
}

pub struct TempDirLease {
    directory: Option<TempDir>,
    tracker: ResourceTracker,
    index: usize,
}

impl TempDirLease {
    pub(crate) fn new(tracker: ResourceTracker, label: &'static str) -> io::Result<Self> {
        let directory = tempfile::Builder::new().prefix("uc-testkit-").tempdir()?;
        let index = register(&tracker, "temp-dir", label);
        Ok(Self {
            directory: Some(directory),
            tracker,
            index,
        })
    }

    pub fn path(&self) -> &Path {
        match &self.directory {
            Some(directory) => directory.path(),
            None => Path::new(""),
        }
    }

    fn release(&mut self) {
        let status = match self.directory.take() {
            Some(directory) => match directory.close() {
                Ok(()) => CleanupStatus::Completed,
                Err(_) => CleanupStatus::Failed,
            },
            None => CleanupStatus::Completed,
        };
        complete(&self.tracker, self.index, status);
    }
}

impl Drop for TempDirLease {
    fn drop(&mut self) {
        self.release();
    }
}

pub struct TcpPortLease {
    listener: Option<TcpListener>,
    tracker: ResourceTracker,
    index: usize,
}

impl TcpPortLease {
    pub(crate) fn new(tracker: ResourceTracker, label: &'static str) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let index = register(&tracker, "tcp-port", label);
        Ok(Self {
            listener: Some(listener),
            tracker,
            index,
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        match &self.listener {
            Some(listener) => listener.local_addr(),
            None => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "port lease already released",
            )),
        }
    }
}

impl Drop for TcpPortLease {
    fn drop(&mut self) {
        self.listener.take();
        complete(&self.tracker, self.index, CleanupStatus::Completed);
    }
}
