//! 拉取准备的私有停止能力；只阻止下一动作，不中断已开始的持久动作。
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct ActivePullPreparationStop {
    token: CancellationToken,
}
impl Default for ActivePullPreparationStop {
    fn default() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }
}
impl ActivePullPreparationStop {
    pub fn check(&self) -> Result<(), ActivePullPreparationCancelled> {
        if self.token.is_cancelled() {
            Err(ActivePullPreparationCancelled)
        } else {
            Ok(())
        }
    }
    pub fn cancel(&self) {
        self.token.cancel();
    }
}

#[derive(Debug, thiserror::Error)]
#[error("active pull preparation cancelled")]
pub struct ActivePullPreparationCancelled;

pub(super) struct StopOnDrop(pub ActivePullPreparationStop);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ActivePullPreparationError {
    #[error("requested content is not available")]
    NotAvailable,
    #[error("session is locked")]
    NotUnlocked,
    #[error("active pull preparation cancelled")]
    Cancelled(#[from] ActivePullPreparationCancelled),
    #[error("active pull preparation failed")]
    Internal(#[source] Box<dyn std::error::Error + Send + Sync>),
}
#[async_trait]
pub trait ActivePullPreparationPort: Send + Sync {
    async fn prepare(
        &self,
        snapshot_hash: &str,
        stop: &ActivePullPreparationStop,
    ) -> Result<Vec<u8>, ActivePullPreparationError>;
}
