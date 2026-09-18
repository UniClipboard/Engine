use std::fs::File;
use std::sync::Arc;

use super::{lock, ProfileContentKeyVault, ProfileContentKeyVaultError, ProfileKeyReadLease};

impl ProfileContentKeyVault {
    pub(crate) fn begin_read_reuse(
        &self,
    ) -> Result<ProfileKeyReadLease, ProfileContentKeyVaultError> {
        ProfileKeyReadLease::begin(&self.reads)
    }

    pub(crate) fn close(&self) {
        lock(&self.reads).close();
    }

    pub(crate) async fn suspend(&self) {
        let _io = self.io_lock.lock().await;
        lock(&self.reads).suspend();
    }

    pub(crate) async fn resume(&self) -> Result<(), ProfileContentKeyVaultError> {
        self.run_owned(|owner| async move {
            let _io = owner.io_lock.lock().await;
            {
                let state = lock(&owner.reads);
                if state.closed {
                    return Err(ProfileContentKeyVaultError::Closed);
                }
                if !state.suspended {
                    return Ok(());
                }
            }
            let lease = Arc::new(owner.persistence.acquire_lease().await?);
            let mut state = lock(&owner.reads);
            // 获取文件租约期间允许最终关闭封口，但不能随后重新开放。
            if state.closed {
                return Err(ProfileContentKeyVaultError::Closed);
            }
            state.lease = Some(lease);
            state.suspended = false;
            Ok(())
        })
        .await
    }

    // 调用方持有 I/O 顺序；租约与代次在同一状态临界区捕获。
    pub(super) async fn operation_lease(
        &self,
    ) -> Result<(Arc<File>, Arc<()>), ProfileContentKeyVaultError> {
        {
            let state = lock(&self.reads);
            state.check_open()?;
            if let Some(file) = &state.lease {
                return Ok((file.clone(), state.generation.clone()));
            }
        }
        let file = Arc::new(self.persistence.acquire_lease().await?);
        let mut state = lock(&self.reads);
        state.check_open()?;
        if state.reusable {
            state.lease = Some(file.clone());
        }
        Ok((file, state.generation.clone()))
    }
}
