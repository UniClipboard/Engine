//! [`DetectUpgradeUseCase`] —— 启动期版本游标比较。
//!
//! 判定矩阵（来自 P1 设计共识）：
//!
//! | last_seen_version       | has_completed | 结果                                |
//! |-------------------------|---------------|-------------------------------------|
//! | None                    | false         | FreshInstall                        |
//! | None                    | true          | Upgraded { from: None, to: current }|
//! | Some(v), v == current   | 任意          | NoChange                            |
//! | Some(v), v <  current   | 任意          | Upgraded { from: Some(v), to: current } |
//! | Some(v), v >  current   | 任意          | Downgraded { from: v, to: current } |
//!
//! 解析失败兜底：
//! * 当前版本（构建期常量）解析失败 —— 视作内部错误，返回 [`DetectUpgradeError::CurrentVersionMalformed`]。
//! * 游标版本解析失败 —— 视作"未知旧版本"，归到 `Upgraded { from: None, to: current }`，
//!   并打 warn 日志；与"非 fresh 即老用户"策略一致。

use std::sync::Arc;

use thiserror::Error;

use uc_core::error_class::ErrorClass;
use uc_core::ports::{AppVersionStateError, AppVersionStatePort};
use uc_observability_contract::{
    error_source::io_error_kind, log_fields::log_vocab, uc_debug, uc_warn,
};

#[cfg(test)]
use crate::space::CurrentSpaceIdentityError;
use crate::space::CurrentSpaceIdentityPort;

use super::status::UpgradeStatus;

#[derive(Debug, Error)]
pub(crate) enum DetectUpgradeError {
    #[error("current build version is malformed")]
    CurrentVersionMalformed(#[source] semver::Error),

    #[error("read app version cursor failed: {0}")]
    ReadCursor(#[from] AppVersionStateError),

    #[error("read current Space identity failed")]
    ReadCurrentSpace(#[source] anyhow::Error),
}

impl ErrorClass for DetectUpgradeError {
    fn class(&self) -> &'static str {
        match self {
            Self::CurrentVersionMalformed(_) => "current_version_malformed",
            Self::ReadCursor(_) => "read_cursor",
            Self::ReadCurrentSpace(_) => "read_current_space",
        }
    }
}

pub(crate) struct DetectUpgradeUseCase {
    app_version_state: Arc<dyn AppVersionStatePort>,
    current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
}

impl DetectUpgradeUseCase {
    pub(crate) fn new(
        app_version_state: Arc<dyn AppVersionStatePort>,
        current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
    ) -> Self {
        Self {
            app_version_state,
            current_space_identity,
        }
    }

    /// 执行一次性判定。`current_version_str` 由调用方传入
    /// （通常 = `env!("CARGO_PKG_VERSION")`），保持 use case 不依赖
    /// 构建期常量、利于测试。
    ///
    /// 失败原样返回，同时由本负责人记录一次固定分类（公开契约边界会丢弃来源）。
    pub(crate) async fn execute(
        &self,
        current_version_str: &str,
    ) -> Result<UpgradeStatus, DetectUpgradeError> {
        let result = self.detect(current_version_str).await;
        if let Err(error) = &result {
            uc_warn!(
                target: "upgrade",
                operation = "detect_upgrade",
                outcome = "failed",
                error_kind = "upgrade_detect",
                error_class = error.class(),
                io_error_kind = io_error_kind(error),
                "upgrade detection failed"
            );
        }
        result
    }

    async fn detect(&self, current_version_str: &str) -> Result<UpgradeStatus, DetectUpgradeError> {
        let current = semver::Version::parse(current_version_str)
            .map_err(DetectUpgradeError::CurrentVersionMalformed)?;

        let stored = self.app_version_state.read().await?;

        match stored {
            None => {
                let has_completed = self
                    .current_space_identity
                    .current_space_id()
                    .await
                    .map_err(|e| DetectUpgradeError::ReadCurrentSpace(anyhow::Error::from(e)))?
                    .is_some();

                if has_completed {
                    uc_debug!(
                        target: "upgrade",
                        current = log_vocab(&current),
                        "no version cursor; setup completed → treating as upgraded from unknown"
                    );
                    Ok(UpgradeStatus::Upgraded {
                        from: None,
                        to: current,
                    })
                } else {
                    uc_debug!(
                        target: "upgrade",
                        current = log_vocab(&current),
                        "no version cursor; setup not completed → fresh install"
                    );
                    Ok(UpgradeStatus::FreshInstall)
                }
            }
            Some(raw) => match semver::Version::parse(&raw) {
                Ok(prev) if prev == current => {
                    uc_debug!(
                        target: "upgrade",
                        current = log_vocab(&current),
                        "cursor matches current version"
                    );
                    Ok(UpgradeStatus::NoChange)
                }
                Ok(prev) if prev < current => {
                    uc_debug!(
                        target: "upgrade",
                        "upgrade detected"
                    );
                    Ok(UpgradeStatus::Upgraded {
                        from: Some(prev),
                        to: current,
                    })
                }
                Ok(prev) => {
                    // prev > current —— 回滚。
                    uc_debug!(
                        target: "upgrade",
                        "downgrade detected"
                    );
                    Ok(UpgradeStatus::Downgraded {
                        from: prev,
                        to: current,
                    })
                }
                Err(e) => {
                    uc_warn!(
                        target: "upgrade",
                        error_kind = "cursor_version_parse",
                        io_error_kind = io_error_kind(&e),
                        "cursor content failed to parse as semver; treating as upgrade from unknown"
                    );
                    Ok(UpgradeStatus::Upgraded {
                        from: None,
                        to: current,
                    })
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;

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

    struct FakeCurrentSpace {
        has_completed: bool,
    }
    #[async_trait]
    impl CurrentSpaceIdentityPort for FakeCurrentSpace {
        async fn current_space_id(
            &self,
        ) -> Result<Option<uc_core::ids::SpaceId>, CurrentSpaceIdentityError> {
            Ok(self
                .has_completed
                .then(|| uc_core::ids::SpaceId::from("space")))
        }
    }

    struct FailingReadVersionState;
    #[async_trait]
    impl AppVersionStatePort for FailingReadVersionState {
        async fn read(&self) -> Result<Option<String>, AppVersionStateError> {
            Err(AppVersionStateError::Read(Box::new(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "SECRET_CURSOR_PATH",
            ))))
        }
        async fn write(&self, _version: &str) -> Result<(), AppVersionStateError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_failed_cursor_read_records_only_fixed_classification() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let uc = DetectUpgradeUseCase::new(
            Arc::new(FailingReadVersionState),
            Arc::new(FakeCurrentSpace {
                has_completed: true,
            }),
        );

        let error = uc.execute("1.0.0").await.unwrap_err();

        assert!(matches!(error, DetectUpgradeError::ReadCursor(_)));
        let output = logs.output();
        assert_eq!(logs.count("upgrade detection failed"), 1, "{output}");
        assert!(output.contains("error_kind=\"upgrade_detect\""), "{output}");
        assert!(output.contains("error_class=\"read_cursor\""), "{output}");
        assert!(
            output.contains("io_error_kind=PermissionDenied"),
            "{output}"
        );
        assert!(!output.contains("SECRET_CURSOR_PATH"), "{output}");
    }

    #[tokio::test]
    async fn a_malformed_current_version_records_its_class() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let uc = build(None, false);

        uc.execute("not-a-version").await.unwrap_err();

        let output = logs.output();
        assert_eq!(logs.count("upgrade detection failed"), 1, "{output}");
        assert!(
            output.contains("error_class=\"current_version_malformed\""),
            "{output}"
        );
        assert!(!output.contains("io_error_kind"), "{output}");
    }

    #[tokio::test]
    async fn a_successful_detection_writes_no_failure_record() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();

        build(Some("1.0.0"), true).execute("1.0.0").await.unwrap();

        assert_eq!(logs.count("upgrade detection failed"), 0);
    }

    fn build(cursor: Option<&str>, has_completed: bool) -> DetectUpgradeUseCase {
        DetectUpgradeUseCase::new(
            FakeVersionState::new(cursor),
            Arc::new(FakeCurrentSpace { has_completed }),
        )
    }

    #[tokio::test]
    async fn no_cursor_and_not_completed_is_fresh_install() {
        let uc = build(None, false);
        assert_eq!(
            uc.execute("1.0.0").await.unwrap(),
            UpgradeStatus::FreshInstall
        );
    }

    #[tokio::test]
    async fn no_cursor_but_setup_completed_is_unknown_upgrade() {
        let uc = build(None, true);
        let to = semver::Version::parse("1.0.0").unwrap();
        assert_eq!(
            uc.execute("1.0.0").await.unwrap(),
            UpgradeStatus::Upgraded { from: None, to }
        );
    }

    #[tokio::test]
    async fn cursor_equal_is_no_change() {
        let uc = build(Some("1.0.0"), true);
        assert_eq!(uc.execute("1.0.0").await.unwrap(), UpgradeStatus::NoChange);
    }

    #[tokio::test]
    async fn cursor_lower_is_upgrade() {
        let uc = build(Some("0.9.3"), true);
        let from = semver::Version::parse("0.9.3").unwrap();
        let to = semver::Version::parse("1.0.0").unwrap();
        assert_eq!(
            uc.execute("1.0.0").await.unwrap(),
            UpgradeStatus::Upgraded {
                from: Some(from),
                to
            }
        );
    }

    #[tokio::test]
    async fn cursor_higher_is_downgrade() {
        let uc = build(Some("1.2.0"), true);
        let from = semver::Version::parse("1.2.0").unwrap();
        let to = semver::Version::parse("1.0.0").unwrap();
        assert_eq!(
            uc.execute("1.0.0").await.unwrap(),
            UpgradeStatus::Downgraded { from, to }
        );
    }

    #[tokio::test]
    async fn malformed_cursor_falls_back_to_unknown_upgrade() {
        let uc = build(Some("garbage-not-semver"), true);
        let to = semver::Version::parse("1.0.0").unwrap();
        assert_eq!(
            uc.execute("1.0.0").await.unwrap(),
            UpgradeStatus::Upgraded { from: None, to }
        );
    }

    #[tokio::test]
    async fn malformed_current_is_internal_error() {
        let uc = build(None, false);
        let err = uc.execute("not-a-version").await.unwrap_err();
        assert!(matches!(
            err,
            DetectUpgradeError::CurrentVersionMalformed(_)
        ));
    }

    #[tokio::test]
    async fn prerelease_ordering_matches_semver_spec() {
        // 1.0.0-alpha.1 < 1.0.0 (semver §11)
        let uc = build(Some("1.0.0-alpha.1"), true);
        let from = semver::Version::parse("1.0.0-alpha.1").unwrap();
        let to = semver::Version::parse("1.0.0").unwrap();
        assert_eq!(
            uc.execute("1.0.0").await.unwrap(),
            UpgradeStatus::Upgraded {
                from: Some(from),
                to
            }
        );
    }
}
