use std::sync::Arc;

use uc_core::MemberRepositoryPort;

use crate::clipboard::write::MobileConsumableBackfill;
use crate::space::upgrade_space::UpgradeSpaceUseCase;

pub(crate) struct PostSessionReadiness {
    upgrade_space: Arc<UpgradeSpaceUseCase>,
    mobile_consumable_backfill: Arc<dyn MobileConsumableBackfill>,
    member_repo: Arc<dyn MemberRepositoryPort>,
}

impl PostSessionReadiness {
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

    pub(crate) async fn complete_after_unlock(&self) -> Result<(), String> {
        self.prepare_data().await
    }

    pub(crate) async fn complete_after_resume(&self) -> Result<(), String> {
        self.prepare_data().await
    }

    async fn prepare_data(&self) -> Result<(), String> {
        self.upgrade_space
            .execute()
            .await
            .map_err(|error| error.to_string())?;

        self.mobile_consumable_backfill.backfill_best_effort().await;

        self.member_repo
            .list()
            .await
            .map_err(|error| error.to_string())?;

        Ok(())
    }
}
