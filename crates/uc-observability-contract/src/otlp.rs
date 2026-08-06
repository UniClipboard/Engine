//! OTLP-compatible structured timing records.
//!
//! The Engine does not initialize an OTLP exporter. It writes stable tracing
//! events that a host may forward to an OTLP collector, while the same fields
//! remain visible in ordinary diagnostic logs.

use tracing::info;

use crate::analytics::{Direction, PayloadSizeBucket, PayloadType, TransportType};
use crate::FlowId;

/// One completed phase in a successful clipboard synchronization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardSyncStage {
    SourceToDispatch,
    AddressResolution,
    Connection,
    StreamOpen,
    FrameWrite,
    ReceiverApplyWait,
    ReceiverQueue,
    ReceiverPolicy,
    ReceiverDecrypt,
    ReceiverPreflightDecode,
    ReceiverApply,
    DispatchToRemoteCommit,
    CopyToRemoteCommit,
}

impl ClipboardSyncStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::SourceToDispatch => "source_to_dispatch",
            Self::AddressResolution => "address_resolution",
            Self::Connection => "connection",
            Self::StreamOpen => "stream_open",
            Self::FrameWrite => "frame_write",
            Self::ReceiverApplyWait => "receiver_apply_wait",
            Self::ReceiverQueue => "receiver_queue",
            Self::ReceiverPolicy => "receiver_policy",
            Self::ReceiverDecrypt => "receiver_decrypt",
            Self::ReceiverPreflightDecode => "receiver_preflight_decode",
            Self::ReceiverApply => "receiver_apply",
            Self::DispatchToRemoteCommit => "dispatch_to_remote_commit",
            Self::CopyToRemoteCommit => "copy_to_remote_commit",
        }
    }
}

/// A complete, content-free timing record ready for a tracing subscriber.
pub struct ClipboardSyncTiming<'a> {
    pub flow_id: &'a FlowId,
    pub flow_synthetic: bool,
    pub direction: Direction,
    pub payload_type: PayloadType,
    pub payload_size_bucket: PayloadSizeBucket,
    pub transport_type: TransportType,
    pub stage: ClipboardSyncStage,
    pub duration_ms: u32,
}

/// Write one OTLP-compatible clipboard timing record to the structured log.
///
/// The fields deliberately exclude clipboard content, device identity,
/// filenames, and paths. `flow.id` is the existing opaque correlation ID.
pub fn log_clipboard_sync_stage(record: ClipboardSyncTiming<'_>) {
    info!(
        target: "uc_otlp",
        parent: None,
        flow_id = %record.flow_id,
        "event.name" = "clipboard.sync.stage.completed",
        flow_synthetic = record.flow_synthetic,
        sync_direction = direction_name(record.direction),
        sync_payload_type = payload_type_name(record.payload_type),
        sync_payload_size_bucket = payload_size_bucket_name(record.payload_size_bucket),
        sync_transport = transport_type_name(record.transport_type),
        sync_stage = record.stage.as_str(),
        sync_duration_ms = record.duration_ms,
        "clipboard sync stage completed"
    );
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Outbound => "outbound",
        Direction::Inbound => "inbound",
    }
}

fn payload_type_name(payload_type: PayloadType) -> &'static str {
    match payload_type {
        PayloadType::Text => "text",
        PayloadType::Image => "image",
        PayloadType::File => "file",
    }
}

fn payload_size_bucket_name(bucket: PayloadSizeBucket) -> &'static str {
    match bucket {
        PayloadSizeBucket::Lt1Kb => "lt_1kb",
        PayloadSizeBucket::Kb1To100 => "1kb_to_100kb",
        PayloadSizeBucket::Kb100ToMb10 => "100kb_to_10mb",
        PayloadSizeBucket::Gt10Mb => "gt_10mb",
    }
}

fn transport_type_name(transport_type: TransportType) -> &'static str {
    match transport_type {
        TransportType::Local => "local",
        TransportType::P2pDirect => "p2p_direct",
        TransportType::Relay => "relay",
        TransportType::FallbackCloud => "fallback_cloud",
        TransportType::Unknown => "unknown",
    }
}
