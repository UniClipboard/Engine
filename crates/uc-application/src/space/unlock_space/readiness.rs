use std::sync::Arc;

use tracing::{info, warn};
use uc_core::ports::PresencePort;
use uc_core::MemberRepositoryPort;

use crate::clipboard::write::MobileConsumableBackfill;
use crate::space::connectivity::reachability::EnsureReachableAllUseCase;
use crate::space::upgrade_space::UpgradeSpaceUseCase;

pub struct PostSessionReadiness {
    upgrade_space: Arc<UpgradeSpaceUseCase>,
    mobile_consumable_backfill: Arc<dyn MobileConsumableBackfill>,
    member_repo: Arc<dyn MemberRepositoryPort>,
    presence: Arc<dyn PresencePort>,
    ensure_reachable_all: Arc<EnsureReachableAllUseCase>,
}

impl PostSessionReadiness {
    pub fn new(
        upgrade_space: Arc<UpgradeSpaceUseCase>,
        mobile_consumable_backfill: Arc<dyn MobileConsumableBackfill>,
        member_repo: Arc<dyn MemberRepositoryPort>,
        presence: Arc<dyn PresencePort>,
        ensure_reachable_all: Arc<EnsureReachableAllUseCase>,
    ) -> Self {
        Self {
            upgrade_space,
            mobile_consumable_backfill,
            member_repo,
            presence,
            ensure_reachable_all,
        }
    }

    pub(crate) async fn complete_after_unlock(&self) -> Result<(), String> {
        self.prepare_data().await?;
        self.prime_presence().await;
        Ok(())
    }

    pub(crate) async fn complete_after_resume(&self) -> Result<(), String> {
        self.prepare_data().await?;
        let presence = Arc::clone(&self.presence);
        let ensure_reachable_all = Arc::clone(&self.ensure_reachable_all);
        tokio::spawn(async move {
            presence.activate().await;
            prime_presence(ensure_reachable_all.as_ref(), "resume").await;
        });
        Ok(())
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

    async fn prime_presence(&self) {
        self.presence.activate().await;
        prime_presence(self.ensure_reachable_all.as_ref(), "unlock").await;
    }
}

async fn prime_presence(ensure_reachable_all: &EnsureReachableAllUseCase, context: &'static str) {
    match ensure_reachable_all.execute().await {
        Ok(report) => {
            info!(
                context,
                total = report.total,
                online = report.online,
                offline = report.offline,
                errors = report.errors.len(),
                "ensure_reachable_all completed after session readiness"
            );
        }
        Err(error) => {
            warn!(
                context,
                error = %error,
                "ensure_reachable_all failed after session readiness; presence will recover lazily"
            );
        }
    }
}
