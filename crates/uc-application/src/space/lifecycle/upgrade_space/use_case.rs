use std::sync::Arc;

use super::error::UpgradeSpaceError;
use crate::space::lifecycle::{CurrentSpaceIdentityPort, RebuildSpaceUseCase};
use uc_core::ports::EngineVersionStatePort;

pub(crate) struct EngineVersionTransition {
    pub(crate) previous: Option<semver::Version>,
    pub(crate) current: semver::Version,
}

pub(crate) struct UpgradeSpaceUseCase {
    current_engine_version: String,
    rebuild_space: Arc<RebuildSpaceUseCase>,
    version_state: Arc<dyn EngineVersionStatePort>,
    current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
}

impl EngineVersionTransition {
    /// 当前版本必须已经达到目标版本。
    /// 之前的版本必须低于目标版本。
    /// 如果旧安装没有版本记录，也视为从目标版本之前升级而来。
    pub(crate) fn crosses(&self, milestone: &semver::Version) -> bool {
        self.current >= *milestone
            && self
                .previous
                .as_ref()
                .is_none_or(|previous| previous < milestone)
    }
}

impl UpgradeSpaceUseCase {
    pub(crate) fn new(
        current_engine_version: String,
        rebuild_space: Arc<RebuildSpaceUseCase>,
        version_state: Arc<dyn EngineVersionStatePort>,
        current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
    ) -> Self {
        Self {
            current_engine_version,
            rebuild_space,
            version_state,
            current_space_identity,
        }
    }

    async fn pending_transition(
        &self,
    ) -> Result<Option<EngineVersionTransition>, UpgradeSpaceError> {
        let current = semver::Version::parse(&self.current_engine_version)
            .map_err(UpgradeSpaceError::InvalidVersion)?;
        let previous = self
            .version_state
            .read()
            .await
            .map_err(UpgradeSpaceError::ReadVersion)?
            .map(|version| semver::Version::parse(&version))
            .transpose()
            .map_err(UpgradeSpaceError::InvalidVersion)?;

        if previous
            .as_ref()
            .is_some_and(|previous| previous >= &current)
        {
            return Ok(None);
        }

        Ok(Some(EngineVersionTransition { previous, current }))
    }

    pub(crate) async fn execute(&self) -> Result<(), UpgradeSpaceError> {
        let Some(transition) = self.pending_transition().await? else {
            return Ok(());
        };

        let legacy_profile_isolation_version =
            semver::Version::parse("1.1.0-rc.5").map_err(UpgradeSpaceError::InvalidVersion)?;
        let current_space_id = self
            .current_space_identity
            .current_space_id()
            .await
            .map_err(|error| UpgradeSpaceError::ReadSetupState(error.to_string()))?;

        if current_space_id.is_some() && transition.crosses(&legacy_profile_isolation_version) {
            self.rebuild_space
                .execute()
                .await
                .map_err(UpgradeSpaceError::Rebuild)?;
        }

        self.version_state
            .write(&transition.current.to_string())
            .await
            .map_err(UpgradeSpaceError::RecordVersion)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_crossing_milestone_requires_migration() {
        let transition = EngineVersionTransition {
            previous: Some(semver::Version::new(0, 19, 0)),
            current: semver::Version::new(0, 20, 0),
        };
        let milestone = semver::Version::new(0, 20, 0);

        assert!(transition.crosses(&milestone));
    }
}
