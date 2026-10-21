//! 未交付文件的清理失败不能擦除原始获取失败，也不能解除产物结算屏障。
use anyhow::Error;
use std::io;
use std::path::{Path, PathBuf};
use tokio::fs::OpenOptions;
use uc_core::ids::EntryId;

#[derive(Debug, thiserror::Error)]
#[error("receive output cleanup remains incomplete")]
pub(crate) struct OutputCleanupFailure {
    pub(crate) cleanup: io::Error,
    #[source]
    pub(crate) cause: Error,
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

pub(super) async fn remove_uncommitted_output(path: &Path) -> io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
