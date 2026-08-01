use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use uc_observability_contract::analytics::{
    AnalyticsIdentityPort, AnalyticsPort, NoopAnalyticsIdentity, NoopAnalyticsSink,
};

use crate::HostFileHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostCapabilityErrorCategory {
    Unavailable,
    PermissionDenied,
    InvalidHandle,
    Io,
}

#[derive(Clone, PartialEq, Eq)]
pub struct HostCapabilityError {
    category: HostCapabilityErrorCategory,
    detail: String,
}

impl HostCapabilityError {
    pub fn new(category: HostCapabilityErrorCategory, detail: impl Into<String>) -> Self {
        Self {
            category,
            detail: detail.into(),
        }
    }

    pub fn category(&self) -> HostCapabilityErrorCategory {
        self.category
    }
}

impl fmt::Debug for HostCapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostCapabilityError")
            .field("category", &self.category)
            .field("detail", &"[REDACTED]")
            .finish()
    }
}

impl fmt::Display for HostCapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "host capability {:?}", self.category)
    }
}

impl std::error::Error for HostCapabilityError {}

#[derive(Clone, PartialEq, Eq)]
pub struct HostDirectories {
    private_data: PathBuf,
    cache: PathBuf,
    temporary: PathBuf,
    logs: PathBuf,
}

impl HostDirectories {
    pub fn new(private_data: PathBuf, cache: PathBuf, temporary: PathBuf, logs: PathBuf) -> Self {
        Self {
            private_data,
            cache,
            temporary,
            logs,
        }
    }

    pub fn private_data(&self) -> &Path {
        &self.private_data
    }

    pub fn cache(&self) -> &Path {
        &self.cache
    }

    pub fn temporary(&self) -> &Path {
        &self.temporary
    }

    pub fn logs(&self) -> &Path {
        &self.logs
    }
}

impl fmt::Debug for HostDirectories {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostDirectories")
            .field("private_data", &"[REDACTED]")
            .field("cache", &"[REDACTED]")
            .field("temporary", &"[REDACTED]")
            .field("logs", &"[REDACTED]")
            .finish()
    }
}

pub trait HostSecureStorage: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError>;
    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError>;
    fn delete(&self, key: &str) -> Result<(), HostCapabilityError>;
}

#[derive(Clone, PartialEq, Eq)]
pub enum HostClipboardRepresentation {
    Inline {
        format: String,
        mime_type: Option<String>,
        bytes: Vec<u8>,
    },
    File {
        format: String,
        handle: HostFileHandle,
        display_name: String,
        mime_type: Option<String>,
        size_bytes: u64,
    },
}

impl fmt::Debug for HostClipboardRepresentation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("HostClipboardRepresentation");
        match self {
            Self::Inline {
                format,
                mime_type,
                bytes,
            } => debug
                .field("kind", &"inline")
                .field("format", format)
                .field("mime_type", mime_type)
                .field("byte_len", &bytes.len()),
            Self::File {
                format,
                mime_type,
                size_bytes,
                ..
            } => debug
                .field("kind", &"file")
                .field("format", format)
                .field("display_name", &"[REDACTED]")
                .field("mime_type", mime_type)
                .field("size_bytes", size_bytes),
        };
        debug.finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct HostClipboardSnapshot {
    pub observed_at_ms: i64,
    pub representations: Vec<HostClipboardRepresentation>,
}

impl fmt::Debug for HostClipboardSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostClipboardSnapshot")
            .field("observed_at_ms", &self.observed_at_ms)
            .field("representation_count", &self.representations.len())
            .finish()
    }
}

pub trait HostClipboard: Send + Sync {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError>;
    fn write(&self, snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError>;

    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
        Ok(None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostClipboardChange {
    Changed,
    Closed,
}

#[async_trait]
pub trait HostClipboardChangeStream: Send {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError>;
    async fn shutdown(&mut self) -> Result<(), HostCapabilityError>;
}

#[derive(Clone, PartialEq, Eq)]
pub struct HostFileMetadata {
    pub display_name: String,
    pub size_bytes: u64,
    pub mime_type: Option<String>,
}

impl fmt::Debug for HostFileMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostFileMetadata")
            .field("display_name", &"[REDACTED]")
            .field("size_bytes", &self.size_bytes)
            .field("mime_type", &self.mime_type)
            .finish()
    }
}

pub trait HostFileAccess: Send + Sync {
    fn metadata(&self, handle: &HostFileHandle) -> Result<HostFileMetadata, HostCapabilityError>;

    fn read_chunk(
        &self,
        handle: &HostFileHandle,
        offset: u64,
        max_bytes: u32,
    ) -> Result<Vec<u8>, HostCapabilityError>;

    fn write_chunk(
        &self,
        handle: &HostFileHandle,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), HostCapabilityError>;

    fn finish_write(&self, handle: &HostFileHandle) -> Result<(), HostCapabilityError>;
}

pub struct HostCapabilities {
    directories: HostDirectories,
    secure_storage: Box<dyn HostSecureStorage>,
    clipboard: Box<dyn HostClipboard>,
    files: Box<dyn HostFileAccess>,
    analytics: HostAnalyticsCapabilities,
}

pub(crate) struct HostAnalyticsCapabilities {
    pub(crate) sink: Arc<dyn AnalyticsPort>,
    pub(crate) identity: Arc<dyn AnalyticsIdentityPort>,
}

impl Default for HostAnalyticsCapabilities {
    fn default() -> Self {
        Self {
            sink: Arc::new(NoopAnalyticsSink),
            identity: Arc::new(NoopAnalyticsIdentity),
        }
    }
}

impl HostCapabilities {
    pub fn new(
        directories: HostDirectories,
        secure_storage: Box<dyn HostSecureStorage>,
        clipboard: Box<dyn HostClipboard>,
        files: Box<dyn HostFileAccess>,
    ) -> Self {
        Self {
            directories,
            secure_storage,
            clipboard,
            files,
            analytics: HostAnalyticsCapabilities::default(),
        }
    }

    /// Supplies host-owned analytics implementations to the production wiring.
    /// The engine owns the resulting facade and passes only contract traits to
    /// its use cases, so hosts never depend on engine-internal assembly types.
    pub fn with_analytics(
        mut self,
        sink: Arc<dyn AnalyticsPort>,
        identity: Arc<dyn AnalyticsIdentityPort>,
    ) -> Self {
        self.analytics = HostAnalyticsCapabilities { sink, identity };
        self
    }

    pub fn directories(&self) -> &HostDirectories {
        &self.directories
    }

    pub fn secure_storage(&self) -> &dyn HostSecureStorage {
        self.secure_storage.as_ref()
    }

    pub fn clipboard(&self) -> &dyn HostClipboard {
        self.clipboard.as_ref()
    }

    pub fn files(&self) -> &dyn HostFileAccess {
        self.files.as_ref()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        HostDirectories,
        Box<dyn HostSecureStorage>,
        Box<dyn HostClipboard>,
        Box<dyn HostFileAccess>,
        HostAnalyticsCapabilities,
    ) {
        (
            self.directories,
            self.secure_storage,
            self.clipboard,
            self.files,
            self.analytics,
        )
    }
}

impl fmt::Debug for HostCapabilities {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostCapabilities")
            .field("directories", &self.directories)
            .field("secure_storage", &"configured")
            .field("clipboard", &"configured")
            .field("files", &"configured")
            .finish()
    }
}
