//! 未交付文件的清理失败不能擦除原始获取失败，也不能解除产物结算屏障。
use super::{ReceiveFailureSettlementError, ReceiveFileProgress, ReceiveOutputFailure};
use anyhow::Error;
use std::io;
use std::path::{Path, PathBuf};
use tokio::fs::OpenOptions;
use uc_core::ids::EntryId;
use uc_core::ports::ReceiveArtifact;

#[derive(Debug, thiserror::Error)]
#[error("receive output cleanup remains incomplete")]
pub(crate) struct OutputCleanupFailure {
    pub(crate) cleanup: io::Error,
    #[source]
    pub(crate) cause: Error,
}

/// 只有完整文件接收负责人存在时移交责任；其他物化保持原清理失败契约。
pub(super) fn output_cleanup_failure(
    progress: Option<&dyn ReceiveFileProgress>,
    artifact: Option<&ReceiveArtifact>,
    failure: ReceiveOutputFailure,
    artifacts: &[ReceiveArtifact],
    journaled: bool,
    cleanup: io::Error,
    cause: Error,
) -> Error {
    if let (Some(progress), Some(artifact)) = (progress, artifact) {
        progress.defer_output_cleanup(artifact, failure, artifacts, journaled, &cause);
        ReceiveFailureSettlementError {
            cause,
            settlement: cleanup.into(),
        }
        .into()
    } else {
        OutputCleanupFailure { cleanup, cause }.into()
    }
}

/// 只有原子创建成功的目标属于本次动作；已有输出不覆盖、不参与本次清理。
pub(super) async fn reserve_managed_output(parent: &Path, filename: &str) -> io::Result<PathBuf> {
    let path = parent.join(filename);
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
    {
        Ok(file) => {
            drop(file);
            Ok(path)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let path = parent.join(format!("{}-{filename}", EntryId::new()));
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .await?;
            Ok(path)
        }
        Err(error) => Err(error),
    }
}

pub(crate) async fn remove_uncommitted_output(path: &Path) -> io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
