use std::sync::Arc;

use anyhow::Context;

use uc_core::MemberRepositoryPort;

use crate::clipboard::write::MobileConsumableBackfill;
use crate::space::lifecycle::UpgradeSpaceUseCase;

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
    pub(crate) async fn prepare_data(&self) -> anyhow::Result<()> {
        self.upgrade_space
            .execute()
            .await
            .context("upgrade space data")?;

        self.mobile_consumable_backfill.backfill_best_effort().await;

        self.member_repo
            .list()
            .await
            .context("list space members")?;

        Ok(())
    }
}
