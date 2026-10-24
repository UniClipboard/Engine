//! 文件目标准备归原下载任务；输出路径只能在准备成功后读取。
use super::transport::{BlobExchangeError, BlobExchangeFailure};
use anyhow::Result;
use futures::future::BoxFuture;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

pub struct BlobOutputTarget {
    preparing: BoxFuture<'static, Result<PathBuf>>,
}
impl fmt::Debug for BlobOutputTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BlobOutputTarget")
            .finish_non_exhaustive()
    }
}
impl From<PathBuf> for BlobOutputTarget {
    fn from(path: PathBuf) -> Self {
        Self {
            preparing: Box::pin(async move { Ok(path) }),
        }
    }
}
impl BlobOutputTarget {
    pub(crate) fn preparing(preparing: BoxFuture<'static, Result<PathBuf>>) -> Self {
        Self { preparing }
    }
    pub(crate) async fn prepare(self) -> Result<PathBuf> {
        self.preparing
            .await
            .map_err(|source| BlobOutputPreparationError(source).into())
    }
}

/// 区分目标准备失败与获取失败；前者由完整接收动作保留原有中止及恢复责任。
#[derive(Debug, thiserror::Error)]
#[error("blob output preparation failed")]
pub(crate) struct BlobOutputPreparationError(#[source] anyhow::Error);

#[derive(Debug, thiserror::Error)]
#[error("blob output path was not prepared exactly once")]
struct OutputPathStateError;

#[derive(Clone, Default)]
pub(super) struct PreparedBlobPath(Arc<OnceLock<PathBuf>>);
impl PreparedBlobPath {
    pub(super) fn publish(&self, path: PathBuf) -> Result<(), BlobExchangeError> {
        // discarded-source[no-information]: OnceLock返回被拒绝的路径值，不是底层错误；不把敏感路径附加到不变量错误。
        self.0.set(path).map_err(|_| {
            BlobExchangeError::new(BlobExchangeFailure::Settlement, OutputPathStateError)
        })
    }
    pub(super) fn path(&self) -> Result<&Path, BlobExchangeError> {
        self.0.get().map(PathBuf::as_path).ok_or_else(|| {
            BlobExchangeError::new(BlobExchangeFailure::Settlement, OutputPathStateError)
        })
    }
}
