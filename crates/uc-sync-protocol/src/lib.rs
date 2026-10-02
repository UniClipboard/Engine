//! 与传输无关的同步协议线上格式。
//!
//! 本 crate 只包含帧结构、版本与 magic 常量、大小上限以及基于
//! `AsyncRead` / `AsyncWrite` 的编解码，不包含拨号、重试、准入判断、密钥、
//! 存储或任何传输库类型。流程负责人仍在 `uc-application`，传输适配器在
//! `uc-infra`。

pub mod active_clipboard_pull;
pub mod active_clipboard_state;
pub mod clipboard;
pub mod membership_branch_recovery;
pub mod space_admission;
pub mod trace_context;
pub mod transfer_progress;

pub use trace_context::{WireTraceContext, TRACEPARENT_MAX_BYTES};
