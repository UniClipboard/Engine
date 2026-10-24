//! 目录已发布输出的原责任；失败重试不得再次触碰已撤回的目标名称。
use super::ReceiveFailureSettlementError;
use anyhow::Error as SourceError;
use std::fmt::{self, Debug, Formatter};
use std::io::{self, ErrorKind};
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uc_core::ports::atomic_publish::{AtomicPublishPort, PublishError};
use uc_core::ports::{ClockPort, RecordDirectoryPublishPort};
use uc_observability_contract::uc_warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PublishMode {
    NoReplace,
    IntoFreeName,
}

pub(super) async fn publish_via(
    publisher: &dyn AtomicPublishPort,
    mode: PublishMode,
    source: &Path,
    destination: &Path,
) -> Result<(), PublishError> {
    match mode {
        PublishMode::NoReplace => publisher.publish_no_replace(source, destination).await,
        PublishMode::IntoFreeName => publisher.publish_into_free_name(source, destination).await,
    }
}

pub(super) struct DirectoryRollbackJournal {
    pub(super) record: Arc<dyn RecordDirectoryPublishPort>,
    pub(super) clock: Arc<dyn ClockPort>,
    pub(super) entry: String,
    pub(super) attempt: String,
}

#[derive(Debug, thiserror::Error)]
#[error("partial_publication: {visible_roots} root(s) left visible")]
pub struct DirectoryRollbackError {
    pub visible_roots: usize,
    #[source]
    pub cause: SourceError,
    pub additional: Vec<SourceError>,
}

pub struct DirectoryPublication {
    publisher: Arc<dyn AtomicPublishPort>,
    staging: PathBuf,
    mode: PublishMode,
    published: Vec<(PathBuf, PathBuf)>,
    journal: Option<DirectoryRollbackJournal>,
    withdrawn_recorded: bool,
    settled: bool,
}
impl Debug for DirectoryPublication {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("DirectoryPublication")
            .field("root_count", &self.published.len())
            .field("settled", &self.settled)
            .finish_non_exhaustive()
    }
}
impl DirectoryPublication {
    pub(super) fn new(
        publisher: Arc<dyn AtomicPublishPort>,
        staging: PathBuf,
        mode: PublishMode,
    ) -> Self {
        Self {
            publisher,
            staging,
            mode,
            published: Vec::new(),
            journal: None,
            withdrawn_recorded: false,
            settled: false,
        }
    }
    /// 只有Staging记录实际落库后才有可更新的恢复日志。
    pub(super) fn set_journal(&mut self, journal: DirectoryRollbackJournal) {
        self.journal = Some(journal);
    }
    pub(super) fn record(&mut self, final_path: PathBuf, staged_from: PathBuf) {
        self.published.push((final_path, staged_from));
    }

    /// 已保存输出不能回滚；清理成功之前仍由原持有者承担责任。
    pub async fn commit(&mut self) -> Result<(), io::Error> {
        if self.settled {
            return Ok(());
        }
        if let Err(source) = tokio::fs::remove_dir_all(&self.staging).await {
            if source.kind() != ErrorKind::NotFound {
                return Err(source);
            }
        }
        self.settled = true;
        Ok(())
    }

    /// 责任留在原持有者；错误只携带来源，不消费输出或完成标志。
    pub async fn rollback(&mut self) -> Result<(), DirectoryRollbackError> {
        if self.settled {
            return Ok(());
        }
        let mut remaining = Vec::new();
        let mut errors = Vec::new();
        for (final_path, staged_from) in mem::take(&mut self.published).into_iter().rev() {
            match publish_via(
                self.publisher.as_ref(),
                self.mode,
                &final_path,
                &staged_from,
            )
            .await
            {
                Ok(()) => {}
                Err(source) => {
                    errors.push(SourceError::new(source));
                    remaining.push((final_path, staged_from));
                }
            }
        }
        remaining.reverse();
        self.published = remaining;
        if !self.published.is_empty() {
            uc_warn!(
                visible_roots = self.published.len(),
                "some published directory roots could not be withdrawn and remain visible"
            );
        }
        if !self.withdrawn_recorded {
            if let Some(journal) = &self.journal {
                match u32::try_from(self.published.len()) {
                    Ok(count) => match journal
                        .record
                        .record_partial(
                            &journal.entry,
                            &journal.attempt,
                            count,
                            journal.clock.now_ms(),
                        )
                        .await
                    {
                        Ok(()) => self.withdrawn_recorded = count == 0,
                        Err(source) => errors.push(SourceError::new(source)),
                    },
                    Err(source) => errors.push(SourceError::new(source)),
                }
            } else if self.published.is_empty() {
                self.withdrawn_recorded = true;
            }
        }
        let mut errors = errors.into_iter();
        if let Some(cause) = errors.next() {
            return Err(DirectoryRollbackError {
                visible_roots: self.published.len(),
                cause,
                additional: errors.collect(),
            });
        }
        // 已确认撤回的目标可被用户重新使用；先持久记录零可见根，再移除staging。
        if let Err(source) = tokio::fs::remove_dir_all(&self.staging).await {
            if source.kind() != ErrorKind::NotFound {
                return Err(DirectoryRollbackError {
                    visible_roots: 0,
                    cause: source.into(),
                    additional: Vec::new(),
                });
            }
        }
        self.settled = true;
        Ok(())
    }
}
impl Drop for DirectoryPublication {
    fn drop(&mut self) {
        if !self.settled {
            uc_warn!(
                root_count = self.published.len(),
                "directory publication dropped before actual settlement"
            );
        }
    }
}

/// 计数可公开，底层两个来源均保留在结构化错误中。
pub(super) fn directory_rollback_failure(
    cause: SourceError,
    settlement: DirectoryRollbackError,
) -> SourceError {
    let visible_roots = settlement.visible_roots;
    SourceError::new(ReceiveFailureSettlementError {
        cause,
        settlement: settlement.into(),
    })
    .context(format!(
        "partial_publication: {visible_roots} root(s) left visible"
    ))
}
