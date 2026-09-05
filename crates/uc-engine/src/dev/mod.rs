use std::fmt;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Once, OnceLock};

use crate::observability::{
    DeploymentEnvironment, ObservabilityConfig, ObservabilityResource, OperatingSystem,
    OtlpHttpConfig, ProcessObservabilityHandle, ProcessObservabilityRuntime,
};
use uc_observability_contract::diagnostics::{
    complete_operation, DiagnosticDomain, DiagnosticOperation, DiagnosticRole, OperationCompletion,
};

static TEST_TRACING_INIT: Once = Once::new();
static TEST_OBSERVABILITY: OnceLock<ProcessObservabilityHandle> = OnceLock::new();

/// Initialize the tracing subscriber for integration tests.
///
/// Honors the `RUST_LOG` environment filter (default `warn`) and writes
/// through the test writer. Idempotent per process: parallel tests share
/// one subscriber, so the first test that calls this wins and every later
/// test logs through it. Call this at the start of every test that needs
/// engine or adapter logs during diagnosis.
pub fn init_test_tracing() {
    TEST_TRACING_INIT.call_once(|| {
        if install_test_otlp() {
            return;
        }
        let result = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "warn".into()),
            )
            .with_test_writer()
            .try_init();
        let _ = result;
    });
}

/// 为单进程端到端测试安装显式的本机 OTLP 接收端。
pub fn init_test_tracing_with_otlp(trace_endpoint: &str, log_endpoint: &str) -> bool {
    let trace_endpoint = trace_endpoint.to_owned();
    let log_endpoint = log_endpoint.to_owned();
    TEST_TRACING_INIT.call_once(move || {
        let _ = install_test_otlp_endpoints(&trace_endpoint, &log_endpoint);
    });
    TEST_OBSERVABILITY.get().is_some()
}

/// 为性能对照安装不含远程发送的同一进程运行时。
pub fn init_test_tracing_without_remote() -> bool {
    TEST_TRACING_INIT.call_once(|| {
        let _ = install_test_observability(None);
    });
    TEST_OBSERVABILITY.get().is_some()
}

fn install_test_otlp() -> bool {
    let Ok(trace_endpoint) = std::env::var("UC_TEST_OTLP_TRACE_ENDPOINT") else {
        return false;
    };
    let Ok(log_endpoint) = std::env::var("UC_TEST_OTLP_LOG_ENDPOINT") else {
        return false;
    };
    install_test_otlp_endpoints(&trace_endpoint, &log_endpoint)
}

fn install_test_otlp_endpoints(trace_endpoint: &str, log_endpoint: &str) -> bool {
    let Ok(remote) = OtlpHttpConfig::new_loopback(trace_endpoint, log_endpoint) else {
        return false;
    };
    install_test_observability(Some(remote))
}

fn install_test_observability(remote: Option<OtlpHttpConfig>) -> bool {
    let Ok(resource) = ObservabilityResource::new(
        env!("CARGO_PKG_VERSION"),
        DeploymentEnvironment::Test,
        current_os(),
        "test",
    ) else {
        return false;
    };
    let mut config = ObservabilityConfig::new(resource);
    if let Some(remote) = remote {
        config = config.with_remote(remote);
    }
    let Ok(Ok(installed)) = std::thread::Builder::new()
        .name("uc-test-observability-install".to_owned())
        .spawn(move || ProcessObservabilityRuntime::install(config))
        .and_then(|worker| {
            worker
                .join()
                .map_err(|_| std::io::Error::other("observability install panicked"))
        })
    else {
        return false;
    };
    TEST_OBSERVABILITY.set(installed.handle()).is_ok()
}

/// 在测试进程退出前刷新真实 OTLP batch；未配置远程接收端时为 no-op。
pub fn flush_test_tracing() {
    if let Some(handle) = TEST_OBSERVABILITY.get() {
        let _ = handle.force_flush(std::time::Duration::from_secs(10));
    }
}

#[derive(Debug, Clone, Copy)]
pub enum TestDiagnosticCompletion {
    ProfileStorageUpgrade,
    SessionLifecycle,
}

pub fn emit_test_diagnostic_completion(completion: TestDiagnosticCompletion) {
    let (domain, operation) = match completion {
        TestDiagnosticCompletion::ProfileStorageUpgrade => (
            DiagnosticDomain::Storage,
            DiagnosticOperation::ProfileStorageUpgrade,
        ),
        TestDiagnosticCompletion::SessionLifecycle => (
            DiagnosticDomain::Runtime,
            DiagnosticOperation::SessionLifecycle,
        ),
    };
    complete_operation(OperationCompletion::succeeded(
        domain,
        operation,
        DiagnosticRole::Local,
        std::time::Duration::from_millis(1),
    ));
}

fn current_os() -> OperatingSystem {
    if cfg!(target_os = "ios") {
        OperatingSystem::Ios
    } else if cfg!(target_os = "android") {
        OperatingSystem::Android
    } else if cfg!(target_os = "macos") {
        OperatingSystem::Macos
    } else if cfg!(target_os = "windows") {
        OperatingSystem::Windows
    } else if cfg!(target_os = "linux") {
        OperatingSystem::Linux
    } else {
        OperatingSystem::Other
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum DevOperation {
    SeedText { text: String },
    CaptureFilePaths { paths: Vec<PathBuf> },
    ListPairingInvitationAddresses,
    IssueInvitationForAddress { address: IpAddr },
    PublishBlob { bytes: Vec<u8> },
    FetchBlob { ticket: Vec<u8>, entry_id: String },
    QueryNetworkEndpointId,
    SetNetworkPartition { blocked_endpoint_ids: Vec<[u8; 32]> },
}

impl fmt::Debug for DevOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::SeedText { .. } => "seed_text",
            Self::CaptureFilePaths { .. } => "capture_file_paths",
            Self::ListPairingInvitationAddresses => "list_pairing_invitation_addresses",
            Self::IssueInvitationForAddress { .. } => "issue_invitation_for_address",
            Self::PublishBlob { .. } => "publish_blob",
            Self::FetchBlob { .. } => "fetch_blob",
            Self::QueryNetworkEndpointId => "query_network_endpoint_id",
            Self::SetNetworkPartition { .. } => "set_network_partition",
        };
        formatter
            .debug_struct("DevOperation")
            .field("kind", &kind)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DevCapturedFileSetLine {
    pub line_index: i64,
    pub root_index: Option<i64>,
    pub root_name: Option<String>,
    pub relative_path: Option<String>,
    pub member_kind: Option<String>,
    pub line_kind: String,
    pub exclude_reason: Option<String>,
}

impl fmt::Debug for DevCapturedFileSetLine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DevCapturedFileSetLine")
            .field("line_index", &self.line_index)
            .field("root_index", &self.root_index)
            .field("has_root_name", &self.root_name.is_some())
            .field("has_relative_path", &self.relative_path.is_some())
            .field("has_member_kind", &self.member_kind.is_some())
            .field("line_kind", &self.line_kind)
            .field("has_exclude_reason", &self.exclude_reason.is_some())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DevCapturedFileSet {
    pub entry_id: String,
    pub deduplicated: bool,
    pub snapshot_hash: String,
    pub directory_structure: bool,
    pub content_digest_count: usize,
    pub lines: Vec<DevCapturedFileSetLine>,
}

impl fmt::Debug for DevCapturedFileSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DevCapturedFileSet")
            .field("entry_id", &self.entry_id)
            .field("deduplicated", &self.deduplicated)
            .field("directory_structure", &self.directory_structure)
            .field("content_digest_count", &self.content_digest_count)
            .field("line_count", &self.lines.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DevPairingInvitationAddress {
    pub ip: IpAddr,
    pub port: u16,
}

impl fmt::Debug for DevPairingInvitationAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DevPairingInvitationAddress")
            .field("address", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DevInvitation {
    pub code: String,
    pub expires_at_ms: i64,
}

impl fmt::Debug for DevInvitation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DevInvitation")
            .field("code", &"[REDACTED]")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DevBlobPublished {
    pub ticket: Vec<u8>,
    pub entry_id: String,
    pub plaintext_hash: Vec<u8>,
    pub digest: Vec<u8>,
    pub reused_existing: bool,
}

impl fmt::Debug for DevBlobPublished {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DevBlobPublished")
            .field("entry_id", &self.entry_id)
            .field("reused_existing", &self.reused_existing)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum DevOperationResult {
    TextSeeded {
        entry_id: String,
    },
    FilePathsCaptured(DevCapturedFileSet),
    PairingInvitationAddresses(Vec<DevPairingInvitationAddress>),
    InvitationIssued(DevInvitation),
    BlobPublished(DevBlobPublished),
    BlobFetched {
        bytes: Vec<u8>,
        entry_id: String,
        plaintext_hash: Vec<u8>,
        digest: Vec<u8>,
    },
    NetworkEndpointId([u8; 32]),
    NetworkPartitionUpdated {
        blocked_peer_count: usize,
    },
}

impl fmt::Debug for DevOperationResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::TextSeeded { .. } => "text_seeded",
            Self::FilePathsCaptured(_) => "file_paths_captured",
            Self::PairingInvitationAddresses(_) => "pairing_invitation_addresses",
            Self::InvitationIssued(_) => "invitation_issued",
            Self::BlobPublished(_) => "blob_published",
            Self::BlobFetched { .. } => "blob_fetched",
            Self::NetworkEndpointId(_) => "network_endpoint_id",
            Self::NetworkPartitionUpdated { .. } => "network_partition_updated",
        };
        formatter
            .debug_struct("DevOperationResult")
            .field("kind", &kind)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn development_debug_output_redacts_content_paths_and_tokens() {
        let operation = DevOperation::SeedText {
            text: "private clipboard text".into(),
        };
        let captured = DevOperationResult::FilePathsCaptured(DevCapturedFileSet {
            entry_id: "entry-1".into(),
            deduplicated: false,
            snapshot_hash: "private-snapshot-hash".into(),
            directory_structure: true,
            content_digest_count: 0,
            lines: vec![DevCapturedFileSetLine {
                line_index: 1,
                root_index: Some(0),
                root_name: Some("private-root".into()),
                relative_path: Some("private/file.txt".into()),
                member_kind: Some("f".into()),
                line_kind: "file".into(),
                exclude_reason: None,
            }],
        });
        let blob = DevOperationResult::BlobPublished(DevBlobPublished {
            ticket: b"private-ticket".to_vec(),
            entry_id: "entry-2".into(),
            plaintext_hash: b"private-plaintext-hash".to_vec(),
            digest: b"private-digest".to_vec(),
            reused_existing: false,
        });
        let address = DevPairingInvitationAddress {
            ip: "203.0.113.42".parse().expect("test address should parse"),
            port: 4242,
        };

        let debug = format!("{operation:?} {captured:?} {blob:?} {address:?}");
        for secret in [
            "private clipboard text",
            "private-root",
            "private/file.txt",
            "private-snapshot-hash",
            "private-ticket",
            "private-plaintext-hash",
            "private-digest",
            "203.0.113.42",
        ] {
            assert!(!debug.contains(secret));
        }
    }
}
