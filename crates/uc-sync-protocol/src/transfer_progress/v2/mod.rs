//! 新版反向进度的有界线上合同；不决定准入、任务顺序、结果采用或重试。
mod codec;
mod model;

pub use codec::{decode_frame, encode_frame, read_frame, write_frame, FrameRefusal, WireError};
pub use model::{
    ActiveProgress, CancelReason, Envelope, FailureReason, ItemKind, ItemState, LogicalItem,
    Message, Phase, ProducerDescriptor, QueryResult, RejectionReason, SnapshotPage, TaskScope,
    TerminalOutcome, TerminalResult, WireId, WireValueError,
};

pub const TRANSFER_PROGRESS_ALPN: &[u8] = b"uniclipboard/transfer-progress/2";
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
pub const MAX_PAGE_ITEMS: usize = 64;
pub use uc_core::file_transfer::MAX_PROGRESS_ITEMS as MAX_ITEMS;
