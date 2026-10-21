use super::DirectoryPublication;
use crate::clipboard::sync::payload_codec::V3BlobRef;
use crate::transfer::receive::batch::ReceiveBatch;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uc_core::clipboard::{EntryFileSet, FileSetMemberKind};
use uc_core::ids::{DeviceId, EntryId, RepresentationId};
use uc_core::ports::ReceiveArtifact;
use uc_core::SystemClipboardSnapshot;

/// 描述一份未完成 materialize 的 file blob。`reason` 由 file_transfer
/// projection 单独承载(P1-10 落地),此处只表达"这个文件在 partial entry
/// 里是缺失的"+ 元数据,前端按 filename + size 渲染。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingFileRef {
    pub filename: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundFileSetManifest {
    pub members: Vec<InboundFileSetMember>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundFileSetMember {
    pub root_index: u32,
    pub root_name: String,
    pub root_is_file: bool,
    pub relative_path: String,
    pub kind: FileSetMemberKind,
    pub blob_ref_index: Option<u32>,
}

/// 保存成功后可用于下一次接收的验证事实；不改变持久产物格式。
#[derive(Debug, Clone)]
pub(crate) struct ReceivedItem {
    pub(crate) blob_index: usize,
    pub(crate) plaintext_hash: [u8; 32],
    pub(crate) bytes: u64,
    pub(crate) output: ReceivedOutput,
}
#[derive(Debug, Clone)]
pub(crate) enum ReceivedOutput {
    File(ReceiveArtifact),
    Representation(RepresentationId),
}
/// 始终引用完整原始blob数组；复用项不属于新尝试的清理集合。
pub(crate) struct ReceiveSelection {
    pub(crate) selected: HashSet<usize>,
    pub(crate) landed: HashMap<usize, ReceivedItem>,
}

pub struct ReceiveWorkPlan {
    from_device: DeviceId,
    receiver_entry_id: EntryId,
    pub(crate) snapshot: SystemClipboardSnapshot,
    pub(crate) blob_refs: Vec<V3BlobRef>,
    pub(crate) file_set_manifest: Option<InboundFileSetManifest>,
    attempt_id: Option<String>,
    expected_snapshot_hash: Option<String>,
    pub(super) original_blob_indices: Vec<usize>,
    pub(super) selection: Option<Arc<ReceiveSelection>>,
    cancellation: CancellationToken,
    batch: Arc<ReceiveBatch>,
}

impl ReceiveWorkPlan {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        from_device: DeviceId,
        receiver_entry_id: EntryId,
        snapshot: SystemClipboardSnapshot,
        blob_refs: Vec<V3BlobRef>,
        file_set_manifest: Option<InboundFileSetManifest>,
        attempt_id: Option<String>,
        expected_snapshot_hash: Option<String>,
    ) -> Self {
        let original_blob_indices = (0..blob_refs.len()).collect();
        Self {
            from_device,
            receiver_entry_id,
            snapshot,
            blob_refs,
            file_set_manifest,
            attempt_id,
            expected_snapshot_hash,
            original_blob_indices,
            selection: None,
            cancellation: CancellationToken::new(),
            batch: Arc::new(ReceiveBatch::default()),
        }
    }

    pub(crate) fn with_selection(mut self, selection: ReceiveSelection) -> Self {
        self.selection = Some(Arc::new(selection));
        self
    }

    pub(crate) fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }

    pub(crate) fn with_batch(mut self, batch: Arc<ReceiveBatch>) -> Self {
        self.batch = batch;
        self
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        DeviceId,
        EntryId,
        SystemClipboardSnapshot,
        Vec<V3BlobRef>,
        Option<InboundFileSetManifest>,
        Option<String>,
        Option<String>,
        CancellationToken,
        Arc<ReceiveBatch>,
    ) {
        debug_assert_eq!(self.original_blob_indices.len(), self.blob_refs.len());
        (
            self.from_device,
            self.receiver_entry_id,
            self.snapshot,
            self.blob_refs,
            self.file_set_manifest,
            self.attempt_id,
            self.expected_snapshot_hash,
            self.cancellation,
            self.batch,
        )
    }
}

/// `InboundBlobMaterializer::materialize_plan` 的返回值。
///
/// `is_partial()` 是"该 snapshot 不完整"的权威信号 —— 调用方据此跳过
/// OS clipboard write 与 dedup 登记。`missing` 单独列出 file-rep 阶段
/// 未完成文件的元数据,供前端渲染占位卡片用。
///
/// **关键不变量**:`is_partial()` 不等于 `!missing.is_empty()`。rep-bound
/// blob(图片 / 大二进制)被 cancel 时,materializer 会从 `snapshot.
/// representations` 中删除未完成的 rep —— 这本身已经让 snapshot 半残,
/// 但 `missing` 只用于 *file* 列表,如果 envelope 没有 file_refs,
/// `missing` 仍是空。仅靠 `!missing.is_empty()` 判定会把这种情况误判
/// 为 complete,把半残 snapshot 当真相落入 dedup 表 + OS 剪贴板。
#[derive(Debug)]
pub struct MaterializeResult {
    pub(crate) completed: Vec<ReceivedItem>,
    pub snapshot: SystemClipboardSnapshot,
    pub missing: Vec<MissingFileRef>,
    /// 真正的"是否 partial"标志。`complete()` 置 false,`finalize_partial`
    /// 置 true。`pub(crate)` 让 crate 内的 mock 测试能构造,外部调用方
    /// 统一通过 `is_partial()` 读,避免新调用方又掉进"missing 空 ⇒
    /// complete"陷阱。
    pub(crate) partial: bool,
    pub(crate) outcome: MaterializeOutcome,
    /// Present when this result put directory roots at their final location.
    ///
    /// Those roots are visible but provisional: the caller owes them either a
    /// [`DirectoryPublication::commit`] once the entry is durably recorded, or
    /// a [`DirectoryPublication::rollback`] on any path that abandons the
    /// entry. Take it with [`Self::take_publication`] and settle it on every
    /// branch — dropping it leaves user-visible roots with nothing behind them.
    pub(crate) publication: Option<DirectoryPublication>,
    /// Receiver-local manifest for a fully materialized directory. Paths refer
    /// only to this device's published roots and are sealed by the commit port.
    pub(crate) directory_file_set: Option<EntryFileSet>,
    pub(crate) has_receive_artifacts: bool,
    pub(crate) receive_artifacts: Vec<ReceiveArtifact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializeOutcome {
    Complete,
    PartialCancelled,
    PartialFailed,
}

impl MaterializeResult {
    pub fn complete(snapshot: SystemClipboardSnapshot) -> Self {
        Self {
            completed: Vec::new(),
            snapshot,
            missing: Vec::new(),
            partial: false,
            outcome: MaterializeOutcome::Complete,
            publication: None,
            directory_file_set: None,
            has_receive_artifacts: false,
            receive_artifacts: Vec::new(),
        }
    }

    pub fn is_partial(&self) -> bool {
        self.partial
    }

    pub fn outcome(&self) -> MaterializeOutcome {
        self.outcome
    }

    /// Take ownership of the publication this result is holding, if any.
    pub fn take_publication(&mut self) -> Option<DirectoryPublication> {
        self.publication.take()
    }

    pub(crate) fn take_receive_artifacts(&mut self) -> Vec<ReceiveArtifact> {
        std::mem::take(&mut self.receive_artifacts)
    }
}
