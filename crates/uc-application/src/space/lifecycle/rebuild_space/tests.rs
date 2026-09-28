use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::SpaceMember;
use uc_core::ports::{
    ClockPort, DeviceIdentityPort, LocalIdentityError, LocalIdentityPort, SettingsPort,
};
use uc_core::security::IdentityFingerprint;
use uc_core::settings::model::Settings;

use super::error::{
    SpaceMembershipRebuildError, SpaceRebuildTransitionError, SpaceSessionRebindError,
};
use super::ports::{
    RebindSpaceSessionPort, SpaceMembershipRebuildPort, SpaceMembershipResetPort,
    SpaceRebuildPreparation, SpaceRebuildTransitionPort,
};
use super::RebuildSpaceUseCase;
use crate::space::membership::{ExcludeMembershipMaintenancePort, MembershipMaintenanceExclusion};

type Calls = Arc<Mutex<Vec<&'static str>>>;
type MaintenanceLock = Arc<tokio::sync::Mutex<()>>;

struct RecordingTransition {
    calls: Calls,
    space_id: SpaceId,
    already_committed: bool,
    maintenance: MaintenanceLock,
}

impl RecordingTransition {
    /// 记录步骤名；成员维护未被排除时记为 `<步骤>-while-maintenance-runs`。
    fn record(&self, step: &'static str, unguarded: &'static str) {
        let name = if self.maintenance.try_lock().is_err() {
            step
        } else {
            unguarded
        };
        self.calls.lock().unwrap().push(name);
    }
}

struct LockMaintenance(MaintenanceLock);

#[async_trait]
impl ExcludeMembershipMaintenancePort for LockMaintenance {
    async fn exclude_membership_maintenance(&self) -> MembershipMaintenanceExclusion {
        MembershipMaintenanceExclusion::new(Arc::clone(&self.0).lock_owned().await)
    }
}

#[async_trait]
impl SpaceRebuildTransitionPort for RecordingTransition {
    async fn prepare(&self) -> Result<SpaceRebuildPreparation, SpaceRebuildTransitionError> {
        self.record("prepare", "prepare-while-maintenance-runs");
        Ok(SpaceRebuildPreparation {
            space_id: self.space_id.clone(),
            already_committed: self.already_committed,
        })
    }

    async fn stage(&self, _space_id: &SpaceId) -> Result<(), SpaceRebuildTransitionError> {
        self.record("stage", "stage-while-maintenance-runs");
        Ok(())
    }

    async fn promote(&self, _space_id: &SpaceId) -> Result<(), SpaceRebuildTransitionError> {
        self.record("promote", "promote-while-maintenance-runs");
        Ok(())
    }

    async fn finalize(&self, _space_id: &SpaceId) -> Result<(), SpaceRebuildTransitionError> {
        self.record("finalize", "finalize-while-maintenance-runs");
        Ok(())
    }
}

struct RecordingRebind(Calls);

#[async_trait]
impl RebindSpaceSessionPort for RecordingRebind {
    async fn rebind_to_space(&self, _space_id: &SpaceId) -> Result<(), SpaceSessionRebindError> {
        self.0.lock().unwrap().push("rebind");
        Ok(())
    }
}

struct RecordingMembershipReset(Calls);

#[async_trait]
impl SpaceMembershipResetPort for RecordingMembershipReset {
    async fn reset(&self) -> Result<(), SpaceMembershipRebuildError> {
        self.0.lock().unwrap().push("reset");
        Ok(())
    }
}

struct RecordingMembershipRebuild(Calls);

#[async_trait]
impl SpaceMembershipRebuildPort for RecordingMembershipRebuild {
    async fn rebuild(
        &self,
        _local_member: &SpaceMember,
    ) -> Result<(), SpaceMembershipRebuildError> {
        self.0.lock().unwrap().push("rebuild");
        Ok(())
    }
}

struct NamedSettings;

#[async_trait]
impl SettingsPort for NamedSettings {
    async fn load(&self) -> anyhow::Result<Settings> {
        let mut settings = Settings::default();
        settings.general.device_name = Some("Rebuild Device".to_owned());
        Ok(settings)
    }

    async fn save(&self, _settings: &Settings) -> anyhow::Result<()> {
        Ok(())
    }
}

struct FixedIdentity;

#[async_trait]
impl LocalIdentityPort for FixedIdentity {
    async fn create(&self) -> Result<IdentityFingerprint, LocalIdentityError> {
        self.ensure().await
    }

    async fn ensure(&self) -> Result<IdentityFingerprint, LocalIdentityError> {
        Ok(IdentityFingerprint::from_raw_string("ABCDEFGHIJKLMNOP").unwrap())
    }

    async fn get_current_fingerprint(
        &self,
    ) -> Result<Option<IdentityFingerprint>, LocalIdentityError> {
        self.ensure().await.map(Some)
    }
}

struct FixedDevice;

impl DeviceIdentityPort for FixedDevice {
    fn current_device_id(&self) -> DeviceId {
        DeviceId::new("rebuild-device")
    }
}

struct FixedClock;

impl ClockPort for FixedClock {
    fn now_ms(&self) -> i64 {
        1_700_000_000_000
    }
}

fn use_case(calls: &Calls, already_committed: bool) -> RebuildSpaceUseCase {
    use_case_with_maintenance(calls, already_committed, MaintenanceLock::default())
}

fn use_case_with_maintenance(
    calls: &Calls,
    already_committed: bool,
    maintenance: MaintenanceLock,
) -> RebuildSpaceUseCase {
    RebuildSpaceUseCase::new(
        Arc::new(NamedSettings),
        Arc::new(FixedIdentity),
        Arc::new(FixedDevice),
        Arc::new(RecordingTransition {
            calls: Arc::clone(calls),
            space_id: SpaceId::from_str("rebuild-target"),
            already_committed,
            maintenance: Arc::clone(&maintenance),
        }),
        Arc::new(RecordingRebind(Arc::clone(calls))),
        Arc::new(RecordingMembershipReset(Arc::clone(calls))),
        Arc::new(RecordingMembershipRebuild(Arc::clone(calls))),
        Arc::new(LockMaintenance(maintenance)),
        Arc::new(FixedClock),
    )
}

#[tokio::test]
async fn a_new_rebuild_runs_every_step_once_in_order() {
    let calls = Calls::default();

    use_case(&calls, false).execute().await.unwrap();

    assert_eq!(
        *calls.lock().unwrap(),
        ["prepare", "stage", "rebind", "reset", "rebuild", "promote", "finalize"]
    );
}

/// manifest 已切到目标、进度日志尚未推进时重启：目标已是当前 Space，但提升记录仍需收尾。
/// 再次 promote 由契约保证幂等，必须先于 finalize，否则 finalize 看到未提升的日志后永久失败。
#[tokio::test]
async fn a_committed_target_is_promoted_again_before_finalization() {
    let calls = Calls::default();

    use_case(&calls, true).execute().await.unwrap();

    assert_eq!(*calls.lock().unwrap(), ["prepare", "promote", "finalize"]);
}

/// 重建改写成员与控制状态并在提交时计算目标摘要；整个过程中成员维护不得运行，结束后恢复。
#[tokio::test]
async fn the_rebuild_excludes_membership_maintenance_until_it_finishes() {
    let calls = Calls::default();
    let maintenance = MaintenanceLock::default();

    use_case_with_maintenance(&calls, false, Arc::clone(&maintenance))
        .execute()
        .await
        .unwrap();

    assert_eq!(
        *calls.lock().unwrap(),
        ["prepare", "stage", "rebind", "reset", "rebuild", "promote", "finalize"]
    );
    assert!(
        maintenance.try_lock().is_ok(),
        "membership maintenance stays excluded after the rebuild"
    );
}
