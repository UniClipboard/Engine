//! 目标准备及产物事实归原下载任务，父流程继续负责本项实际清理与交接。
use super::cleanup::reserve_managed_output;
use super::workflow::managed_filename;
use crate::transfer::blob::facade::BlobOutputTarget;
use anyhow::Context;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use uc_core::ports::inbound_file_target::ReserveInboundFileTargetPort;
use uc_core::ports::{
    ClockPort, ReceiveArtifact, ReceiveArtifactOwnership, ReceiveArtifactPhase,
    ReceiveArtifactRecord, ReceiveArtifactResolution, RecordReceiveArtifactsPort,
};

#[derive(Default)]
pub(super) struct PreparedReceiveOutput {
    pub(super) artifact: Option<ReceiveArtifact>,
    pub(super) journaled: bool,
}

pub(super) struct FileOutputJournal {
    pub(super) log: Arc<dyn RecordReceiveArtifactsPort>,
    pub(super) clock: Arc<dyn ClockPort>,
    pub(super) entry_id: String,
    pub(super) attempt_id: String,
    pub(super) previous: Vec<ReceiveArtifact>,
}

pub(super) struct FileOutputPreparation {
    pub(super) target_reserver: Option<Arc<dyn ReserveInboundFileTargetPort>>,
    pub(super) managed_parent: PathBuf,
    pub(super) filename: Option<String>,
    pub(super) index: usize,
    pub(super) used_names: Arc<Mutex<HashSet<String>>>,
    pub(super) item_id: String,
    pub(super) journal: Option<FileOutputJournal>,
}
impl FileOutputPreparation {
    pub(super) fn into_target(
        self,
        receipt: Arc<Mutex<PreparedReceiveOutput>>,
    ) -> BlobOutputTarget {
        BlobOutputTarget::preparing(Box::pin(async move {
            let reserved = match self.target_reserver {
                Some(reserver) => {
                    reserver
                        .reserve_target(self.filename.as_deref().unwrap_or_default())
                        .await
                }
                None => None,
            };
            let from_reserver = reserved.is_some();
            let path = match reserved {
                Some(path) => path,
                None => {
                    let filename = managed_filename(
                        self.filename.as_deref(),
                        self.index,
                        &mut self
                            .used_names
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner),
                    );
                    tokio::fs::create_dir_all(&self.managed_parent).await?;
                    reserve_managed_output(&self.managed_parent, &filename)
                        .await
                        .context("reserve managed receive output")?
                }
            };
            let artifact = ReceiveArtifact {
                item_id: self.item_id,
                staged_path: path.clone(),
                final_path: path.clone(),
                ownership: if from_reserver {
                    ReceiveArtifactOwnership::UserDestination
                } else {
                    ReceiveArtifactOwnership::ManagedStaging
                },
            };
            // 先保留本次创建事实；日志写入失败仍须由完整接收动作清理实际目标。
            receipt
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .artifact = Some(artifact.clone());
            if let Some(mut journal) = self.journal {
                journal.previous.push(artifact);
                journal
                    .log
                    .record_receive_artifacts(&ReceiveArtifactRecord {
                        entry_id: journal.entry_id,
                        attempt_id: journal.attempt_id,
                        phase: ReceiveArtifactPhase::Preparing,
                        resolution: ReceiveArtifactResolution::Pending,
                        artifacts: journal.previous,
                        updated_at_ms: journal.clock.now_ms(),
                    })
                    .await?;
                receipt
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .journaled = true;
            }
            Ok(path)
        }))
    }
}
