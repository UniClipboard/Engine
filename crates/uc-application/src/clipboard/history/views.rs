//! History view types shared by the history use cases and the maintenance
//! runtime. Defined in the history domain (ADR-018 stage 3); the facade
//! re-exports them as its external contract.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardListInput {
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryProjectionView {
    pub id: String,
    pub preview: String,
    pub has_detail: bool,
    pub size_bytes: i64,
    pub captured_at: i64,
    pub content_type: String,
    pub thumbnail_url: Option<String>,
    pub is_encrypted: bool,
    pub is_favorited: bool,
    pub updated_at: i64,
    pub active_time: i64,
    pub file_transfer_status: Option<String>,
    pub file_transfer_reason: Option<String>,
    pub content_tags: Vec<String>,
    pub link_urls: Option<Vec<String>>,
    pub link_domains: Option<Vec<String>>,
    pub file_sizes: Option<Vec<i64>>,
    pub image_width: Option<i32>,
    pub image_height: Option<i32>,
    /// Whether this file entry was captured as a directory. Sourced from the
    /// single `EntryFileSet::has_directory_structure()` authority; `false` for
    /// non-file entries or when no manifest is available. The sender UI keys off
    /// this to render status only (no byte percentage) for directory sends.
    pub is_directory: bool,
    /// `paste_rep` 的 payload_state, 仅在 `Lost` 时输出。其他状态为 `None`。
    /// 前端按此判断"该 entry 点了能不能粘贴" —— 粘贴行为基于 paste_rep,
    /// 而 list 里的 preview 基于 preview_rep, 两者可能不同。
    pub payload_state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryDetailView {
    pub id: String,
    pub content: String,
    pub size_bytes: i64,
    pub created_at_ms: i64,
    pub active_time_ms: i64,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryResourceView {
    pub blob_id: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: i64,
    pub url: Option<String>,
    pub inline_data: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardStatsView {
    pub total_items: i64,
    pub total_size: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearHistoryResultView {
    pub deleted_count: u64,
    pub failed_entries: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupResultView {
    pub files_removed: u32,
    pub bytes_reclaimed: u64,
    pub entries_deleted: u32,
    pub orphans_removed: u32,
    pub errors: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileResultView {
    pub entries_scanned: u32,
    pub entries_deleted: u32,
    pub errors: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetentionEnforcementResultView {
    pub entries_deleted: u32,
    pub errors: u32,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ClipboardHistoryError {
    #[error("entry not found")]
    NotFound,
    #[error("unsupported clipboard content")]
    UnsupportedContent,
    #[error("clipboard history operation failed: {0}")]
    Internal(String),
}
