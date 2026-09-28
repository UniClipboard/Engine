use std::sync::Arc;

use anyhow::Context;

use uc_core::MemberRepositoryPort;

use crate::clipboard::write::MobileConsumableBackfill;
use crate::space::lifecycle::upgrade_space::UpgradeSpaceError;
use crate::space::lifecycle::UpgradeSpaceUseCase;

/// 准备本地会话数据的失败：升级暂时不可用时持久进度可续做，调用方可据此报告为可重试。
#[derive(Debug, thiserror::Error)]
pub(crate) enum SessionReadinessError {
    #[error("space data upgrade is temporarily unavailable")]
    UpgradeUnavailable(#[source] UpgradeSpaceError),
    #[error("failed to prepare local session data")]
    Failed(#[source] anyhow::Error),
}

pub(crate) struct LocalSessionReadiness {
    upgrade_space: Arc<UpgradeSpaceUseCase>,
    mobile_consumable_backfill: Arc<dyn MobileConsumableBackfill>,
    member_repo: Arc<dyn MemberRepositoryPort>,
}

impl LocalSessionReadiness {
    pub(crate) fn new(
        upgrade_space: Arc<UpgradeSpaceUseCase>,
        mobile_consumable_backfill: Arc<dyn MobileConsumableBackfill>,
        member_repo: Arc<dyn MemberRepositoryPort>,
    ) -> Self {
        Self {
            upgrade_space,
            mobile_consumable_backfill,
            member_repo,
        }
    }

    /// 解锁或恢复会话后准备本地数据；两条路径的准备步骤相同。
    pub(crate) async fn prepare_data(&self) -> Result<(), SessionReadinessError> {
        self.upgrade_space.execute().await.map_err(|error| {
            if error.is_temporarily_unavailable() {
                SessionReadinessError::UpgradeUnavailable(error)
            } else {
                SessionReadinessError::Failed(
                    anyhow::Error::new(error).context("upgrade space data"),
                )
            }
        })?;

        self.mobile_consumable_backfill.backfill_best_effort().await;

        self.member_repo
            .list()
            .await
            .context("list space members")
            .map_err(SessionReadinessError::Failed)?;

        Ok(())
    }
}
