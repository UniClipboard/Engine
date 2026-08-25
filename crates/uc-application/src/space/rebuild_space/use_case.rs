//! 给定一个可选的已持久化目标 Space，创建或继续重建单设备
//! Space，并在成功后返回该 SpaceId。
use std::sync::Arc;

use crate::space::rebuild_space::ports::{
    RebindSpaceSessionPort, SpaceMembershipRebuildPort, SpaceMembershipResetPort,
};
use chrono::{DateTime, Utc};

use super::error::RebuildSpaceError;
use super::ports::SpaceRebuildTransitionPort;
use uc_core::ports::{ClockPort, DeviceIdentityPort};
use uc_core::{
    ids::SpaceId,
    ports::{LocalIdentityPort, SettingsPort},
};
use uc_core::{DeviceId, IdentityFingerprint, MemberSyncPreferences, SpaceMember};

pub(crate) struct RebuildSpaceUseCase {
    settings: Arc<dyn SettingsPort>,
    local_identity: Arc<dyn LocalIdentityPort>,
    device_identity: Arc<dyn DeviceIdentityPort>,
    rebuild_transition: Arc<dyn SpaceRebuildTransitionPort>,
    rebind_space_session: Arc<dyn RebindSpaceSessionPort>,
    membership_reset: Arc<dyn SpaceMembershipResetPort>,
    membership_rebuilder: Arc<dyn SpaceMembershipRebuildPort>,
    clock: Arc<dyn ClockPort>,
    execution_lock: tokio::sync::Mutex<()>,
}

struct RebuildContext {
    space_id: SpaceId,
    already_committed: bool,
    device_id: DeviceId,
    device_name: String,
    identity_fingerprint: IdentityFingerprint,
}

impl RebuildSpaceUseCase {
    pub(crate) fn new(
        settings: Arc<dyn SettingsPort>,
        local_identity: Arc<dyn LocalIdentityPort>,
        device_identity: Arc<dyn DeviceIdentityPort>,
        rebuild_transition: Arc<dyn SpaceRebuildTransitionPort>,
        rebind_space_session: Arc<dyn RebindSpaceSessionPort>,
        membership_reset: Arc<dyn SpaceMembershipResetPort>,
        membership_rebuilder: Arc<dyn SpaceMembershipRebuildPort>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            settings,
            local_identity,
            device_identity,
            rebuild_transition,
            rebind_space_session,
            membership_reset,
            membership_rebuilder,
            clock,
            execution_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) async fn execute(&self) -> Result<SpaceId, RebuildSpaceError> {
        let _guard = self.execution_lock.lock().await;

        let ctx = self.prepare().await?;

        if ctx.already_committed {
            self.finalize(&ctx).await?;
            return Ok(ctx.space_id);
        }

        self.stage(&ctx).await?;
        self.rebuild(&ctx).await?;
        self.commit(&ctx).await?;
        self.finalize(&ctx).await?;

        Ok(ctx.space_id)
    }

    async fn prepare(&self) -> Result<RebuildContext, RebuildSpaceError> {
        let settings = self
            .settings
            .load()
            .await
            .map_err(RebuildSpaceError::preparation)?;

        let device_name = settings
            .general
            .device_name
            .filter(|name| !name.trim().is_empty())
            .ok_or(RebuildSpaceError::DeviceNameUnavailable)?;

        let identity_fingerprint = self
            .local_identity
            .ensure()
            .await
            .map_err(RebuildSpaceError::preparation)?;

        let device_id = self.device_identity.current_device_id();

        let preparation = self
            .rebuild_transition
            .prepare()
            .await
            .map_err(RebuildSpaceError::preparation)?;

        Ok(RebuildContext {
            space_id: preparation.space_id,
            already_committed: preparation.already_committed,
            device_id,
            device_name,
            identity_fingerprint,
        })
    }

    async fn stage(&self, ctx: &RebuildContext) -> Result<(), RebuildSpaceError> {
        self.rebuild_transition
            .stage(&ctx.space_id)
            .await
            .map_err(RebuildSpaceError::staging)?;

        Ok(())
    }

    async fn rebuild(&self, ctx: &RebuildContext) -> Result<(), RebuildSpaceError> {
        self.rebind_space_session
            .rebind_to_space(&ctx.space_id)
            .await
            .map_err(RebuildSpaceError::rebuild)?;

        self.membership_reset
            .reset()
            .await
            .map_err(RebuildSpaceError::rebuild)?;

        let member = SpaceMember {
            device_id: ctx.device_id.clone(),
            device_name: ctx.device_name.clone(),
            identity_fingerprint: ctx.identity_fingerprint.clone(),
            joined_at: self.now_utc()?,
            sync_preferences: MemberSyncPreferences::default(),
        };

        self.membership_rebuilder
            .rebuild(&member)
            .await
            .map_err(RebuildSpaceError::rebuild)?;
        Ok(())
    }

    async fn commit(&self, ctx: &RebuildContext) -> Result<(), RebuildSpaceError> {
        self.rebuild_transition
            .promote(&ctx.space_id)
            .await
            .map_err(RebuildSpaceError::commit)?;
        Ok(())
    }

    async fn finalize(&self, ctx: &RebuildContext) -> Result<(), RebuildSpaceError> {
        self.rebuild_transition
            .finalize(&ctx.space_id)
            .await
            .map_err(RebuildSpaceError::finalize)?;
        Ok(())
    }

    fn now_utc(&self) -> Result<DateTime<Utc>, RebuildSpaceError> {
        DateTime::<Utc>::from_timestamp_millis(self.clock.now_ms())
            .ok_or(RebuildSpaceError::InvalidClock)
    }
}
