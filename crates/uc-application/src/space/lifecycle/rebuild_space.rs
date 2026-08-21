//! 给定一个可选的已持久化目标 Space，创建或继续重建单设备
//! Space，并在成功后返回该 SpaceId。
use std::sync::Arc;

use chrono::{DateTime, Utc};
use uc_core::setup::SetupStatus;

use super::errors::RebuildSpaceError;
use uc_core::membership::SpaceMembershipRebuildPort;
use uc_core::ports::space::{RebindSpaceSessionPort, SpaceRebuildTransitionPort};
use uc_core::ports::{ClockPort, DeviceIdentityPort, SetupStatusPort};
use uc_core::{
    ids::SpaceId,
    ports::{LocalIdentityPort, SettingsPort},
};
use uc_core::{DeviceId, IdentityFingerprint, MemberSyncPreferences, SpaceMember};

pub(crate) struct RebuildSpaceUseCase {
    pub(crate) settings: Arc<dyn SettingsPort>,
    pub(crate) local_identity: Arc<dyn LocalIdentityPort>,
    pub(crate) device_identity: Arc<dyn DeviceIdentityPort>,
    pub(crate) rebuild_transition: Arc<dyn SpaceRebuildTransitionPort>,
    pub(crate) rebind_space_session: Arc<dyn RebindSpaceSessionPort>,
    pub(crate) membership_rebuilder: Arc<dyn SpaceMembershipRebuildPort>,
    pub(crate) clock: Arc<dyn ClockPort>,
    pub(crate) setup_status: Arc<dyn SetupStatusPort>,
}

pub(crate) struct RebuildContext {
    pub(crate) space_id: SpaceId,
    pub(crate) device_id: DeviceId,
    pub(crate) device_name: String,
    pub(crate) identity_fingerprint: IdentityFingerprint,
}

impl RebuildSpaceUseCase {
    pub(crate) async fn execute(&self) -> Result<SpaceId, RebuildSpaceError> {
        let ctx = self
            .prepare()
            .await
            .map_err(RebuildSpaceError::preparation)?;

        self.stage(&ctx).await.map_err(RebuildSpaceError::staging)?;
        self.rebuild(&ctx)
            .await
            .map_err(RebuildSpaceError::rebuild)?;
        self.commit(&ctx).await.map_err(RebuildSpaceError::commit)?;
        self.finalize(&ctx)
            .await
            .map_err(RebuildSpaceError::finalize)?;

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

        let space_id = self
            .rebuild_transition
            .prepare()
            .await
            .map_err(RebuildSpaceError::preparation)?;

        Ok(RebuildContext {
            space_id,
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
            .map_err(RebuildSpaceError::commit);
        Ok(())
    }

    async fn finalize(&self, ctx: &RebuildContext) -> Result<(), RebuildSpaceError> {
        let status = SetupStatus {
            has_completed: true,
            space_id: Some(ctx.space_id.clone()),
            re_pairing_required: false,
        };
        self.setup_status.set_status(&status);
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
