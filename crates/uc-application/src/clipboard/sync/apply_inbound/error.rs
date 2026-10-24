//! 入站动作错误保留原始来源，对外调试文本仅显示固定分类。
use anyhow::Error as SourceError;
use std::fmt;
use thiserror::Error;

#[derive(Error)]
pub enum ApplyInboundError {
    #[error("clipboard receive stopped")]
    Stopped,
    #[error("clipboard receive worker failed")]
    WorkFailed(#[source] SourceError),
    #[error("dedup query failed")]
    DedupQuery(#[source] SourceError),
    #[error("capture pipeline failed")]
    Capture(#[source] SourceError),
    #[error("clipboard receive failed internally")]
    Internal(#[source] SourceError),
}

impl fmt::Debug for ApplyInboundError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}
