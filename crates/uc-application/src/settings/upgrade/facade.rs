//! [`UpgradeFacade`] —— P1 thin 升级检测对外入口。
//!
//! 调用方组合：
//! 1. 启动期调一次 [`UpgradeFacade::detect_on_startup`]，结果作为 UI / CLI
//!    决策输入（弹窗、提示、跳过等）。
//! 2. 用户确认后调 [`UpgradeFacade::acknowledge`] 把游标推进到当前版本，
//!    下次启动得 `NoChange`。
//!
//! 当前版本字符串由调用方传入（典型为 `env!("CARGO_PKG_VERSION")`），
//! facade / use case 不依赖任何构建期常量，便于测试与多入口复用。

use std::sync::Arc;

use thiserror::Error;

use uc_core::ports::AppVersionStatePort;

use crate::space::current_space::CurrentSpaceIdentityPort;

use crate::settings::upgrade::acknowledge::{
    AcknowledgeError as InnerAcknowledgeError, AcknowledgeUseCase,
};
use crate::settings::upgrade::detect::{
    DetectUpgradeError as InnerDetectUpgradeError, DetectUpgradeUseCase,
};
use crate::settings::upgrade::status::UpgradeStatus as InnerUpgradeStatus;

/// 对外 re-export：升级状态判定结果。
pub type UpgradeStatus = InnerUpgradeStatus;

/// `UpgradeFacade::detect_on_startup` 的错误。
///
/// 内部 `DetectUpgradeError` 的 1:1 镜像；保持 facade 与 use case 错误类型
/// 解耦，未来 use case 内部错误演化不会破坏对外 API。
#[derive(Debug, Error)]
pub enum DetectUpgradeError {
    #[error("current build version is malformed: {0}")]
    CurrentVersionMalformed(String),

    #[error("read app version cursor failed: {0}")]
    ReadCursor(String),

    #[error("read setup status failed: {0}")]
    ReadCurrentSpace(String),
}

impl From<InnerDetectUpgradeError> for DetectUpgradeError {
    fn from(value: InnerDetectUpgradeError) -> Self {
        match value {
            InnerDetectUpgradeError::CurrentVersionMalformed(s) => Self::CurrentVersionMalformed(s),
            InnerDetectUpgradeError::ReadCursor(e) => Self::ReadCursor(e.to_string()),
            InnerDetectUpgradeError::ReadCurrentSpace(s) => Self::ReadCurrentSpace(s),
        }
    }
}

/// `UpgradeFacade::acknowledge` 的错误。
#[derive(Debug, Error)]
pub enum AcknowledgeUpgradeError {
    #[error("current build version is malformed: {0}")]
    CurrentVersionMalformed(String),

    #[error("write app version cursor failed: {0}")]
    WriteCursor(String),
}

impl From<InnerAcknowledgeError> for AcknowledgeUpgradeError {
    fn from(value: InnerAcknowledgeError) -> Self {
        match value {
            InnerAcknowledgeError::CurrentVersionMalformed(s) => Self::CurrentVersionMalformed(s),
            InnerAcknowledgeError::WriteCursor(e) => Self::WriteCursor(e.to_string()),
        }
    }
}

/// 构造 `UpgradeFacade` 所需的端口集合。
pub struct UpgradeFacadeDeps {
    pub app_version_state: Arc<dyn AppVersionStatePort>,
    pub current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
}

/// 升级检测 facade。线程安全，可放入 `Arc`。
pub struct UpgradeFacade {
    detect: DetectUpgradeUseCase,
    acknowledge: AcknowledgeUseCase,
}

impl UpgradeFacade {
    pub fn new(deps: UpgradeFacadeDeps) -> Self {
        let UpgradeFacadeDeps {
            app_version_state,
            current_space_identity,
        } = deps;
        Self {
            detect: DetectUpgradeUseCase::new(app_version_state.clone(), current_space_identity),
            acknowledge: AcknowledgeUseCase::new(app_version_state),
        }
    }

    /// 启动期一次性判定。返回 [`UpgradeStatus`] 由调用方决定后续动作。
    pub async fn detect_on_startup(
        &self,
        current_version: &str,
    ) -> Result<UpgradeStatus, DetectUpgradeError> {
        self.detect
            .execute(current_version)
            .await
            .map_err(Into::into)
    }

    /// 把游标推进到 `current_version`。调用方应在用户确认（点掉弹窗、
    /// 跑完重新配对、CLI 标记已读等）后调用。
    pub async fn acknowledge(&self, current_version: &str) -> Result<(), AcknowledgeUpgradeError> {
        self.acknowledge
            .execute(current_version)
            .await
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use uc_core::ports::AppVersionStateError;

    struct FakeVersionState {
        value: Mutex<Option<String>>,
    }
    impl FakeVersionState {
        fn new(initial: Option<&str>) -> Arc<Self> {
            Arc::new(Self {
                value: Mutex::new(initial.map(|s| s.to_string())),
            })
        }
    }
    #[async_trait]
    impl AppVersionStatePort for FakeVersionState {
        async fn read(&self) -> Result<Option<String>, AppVersionStateError> {
            Ok(self.value.lock().unwrap().clone())
        }
        async fn write(&self, version: &str) -> Result<(), AppVersionStateError> {
            *self.value.lock().unwrap() = Some(version.to_string());
            Ok(())
        }
    }

    struct FakeCurrentSpace(bool);
    #[async_trait]
    impl CurrentSpaceIdentityPort for FakeCurrentSpace {
        async fn current_space_id(
            &self,
        ) -> Result<
            Option<uc_core::ids::SpaceId>,
            crate::space::current_space::CurrentSpaceIdentityError,
        > {
            Ok(self.0.then(|| uc_core::ids::SpaceId::from("space")))
        }
    }

    #[tokio::test]
    async fn detect_then_acknowledge_round_trip() {
        let port = FakeVersionState::new(None);
        let facade = UpgradeFacade::new(UpgradeFacadeDeps {
            app_version_state: port.clone(),
            current_space_identity: Arc::new(FakeCurrentSpace(true)),
        });

        // First call: missing cursor + completed setup → unknown upgrade.
        let to = semver::Version::parse("1.0.0").unwrap();
        assert_eq!(
            facade.detect_on_startup("1.0.0").await.unwrap(),
            UpgradeStatus::Upgraded {
                from: None,
                to: to.clone()
            }
        );

        // Acknowledge → cursor advances.
        facade.acknowledge("1.0.0").await.unwrap();
        assert_eq!(port.value.lock().unwrap().as_deref(), Some("1.0.0"));

        // Re-detect → NoChange.
        assert_eq!(
            facade.detect_on_startup("1.0.0").await.unwrap(),
            UpgradeStatus::NoChange
        );
    }
}
