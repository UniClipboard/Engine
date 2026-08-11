use std::fmt;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Once;

use tokio::sync::broadcast;

/// Initialize the tracing subscriber for integration tests.
///
/// Honors the `RUST_LOG` environment filter (default `warn`) and writes
/// through the test writer. Idempotent per process: parallel tests share
/// one subscriber, so the first test that calls this wins and every later
/// test logs through it. Call this at the start of every test that needs
/// engine or adapter logs during diagnosis.
pub fn init_test_tracing() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let result = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "warn".into()),
            )
            .with_test_writer()
            .try_init();
        if result.is_err() {
            eprintln!(
                "init_test_tracing: a subscriber was already installed; \
                 RUST_LOG may not apply to this test process"
            );
        }
    });
}

#[derive(Clone, PartialEq, Eq)]
pub enum DevOperation {
    SeedText { text: String },
    CaptureFilePaths { paths: Vec<PathBuf> },
    ListPairingInvitationAddresses,
    IssueInvitationForAddress { address: IpAddr },
    PublishBlob { bytes: Vec<u8> },
    FetchBlob { ticket: Vec<u8>, entry_id: String },
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
