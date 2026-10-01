//! [`AcknowledgeUseCase`] —— 把版本游标推进到当前版本。
//!
//! 调用方（UI 弹窗确认 / CLI `--acknowledge` / 自动确认）执行后，
//! 下次启动 `DetectUpgradeUseCase` 将得到 `UpgradeStatus::NoChange`。

use std::sync::Arc;

use thiserror::Error;

use uc_core::error_class::ErrorClass;
use uc_core::ports::{AppVersionStateError, AppVersionStatePort};
use uc_observability_contract::{
    error_source::io_error_kind, log_fields::log_vocab, uc_info, uc_warn,
};

#[derive(Debug, Error)]
pub(crate) enum AcknowledgeError {
    #[error("current build version is malformed")]
    CurrentVersionMalformed(#[source] semver::Error),

    #[error("write app version cursor failed: {0}")]
    WriteCursor(#[from] AppVersionStateError),
}

impl ErrorClass for AcknowledgeError {
    fn class(&self) -> &'static str {
        match self {
            Self::CurrentVersionMalformed(_) => "current_version_malformed",
            Self::WriteCursor(_) => "write_cursor",
        }
    }
}

pub(crate) struct AcknowledgeUseCase {
    app_version_state: Arc<dyn AppVersionStatePort>,
}

impl AcknowledgeUseCase {
    pub(crate) fn new(app_version_state: Arc<dyn AppVersionStatePort>) -> Self {
        Self { app_version_state }
    }

    /// 把游标推进到 `current_version_str`。先用 semver 校验合法性，
    /// 避免把无效字符串写回磁盘污染游标。
    ///
    /// 失败原样返回，同时由本负责人记录一次固定分类（公开契约边界会丢弃来源）。
    #[tracing::instrument(name = "usecase.acknowledge_settings_upgrade.execute", skip_all)]
    pub(crate) async fn execute(&self, current_version_str: &str) -> Result<(), AcknowledgeError> {
        let result = self.acknowledge(current_version_str).await;
        if let Err(error) = &result {
            uc_warn!(
                target: "upgrade",
                operation = "acknowledge_upgrade",
                outcome = "failed",
                error_kind = "upgrade_acknowledge",
                error_class = error.class(),
                io_error_kind = io_error_kind(error),
                "upgrade acknowledgement failed"
            );
        }
        result
    }

    async fn acknowledge(&self, current_version_str: &str) -> Result<(), AcknowledgeError> {
        let _validated = semver::Version::parse(current_version_str)
            .map_err(AcknowledgeError::CurrentVersionMalformed)?;

        self.app_version_state.write(current_version_str).await?;
        uc_info!(
            target: "upgrade",
            version = log_vocab(&current_version_str),
            "app version cursor advanced"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct FakeVersionState {
        value: Mutex<Option<String>>,
        write_should_fail: bool,
    }
    impl FakeVersionState {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                value: Mutex::new(None),
                write_should_fail: false,
            })
        }
        fn failing() -> Arc<Self> {
            Arc::new(Self {
                value: Mutex::new(None),
                write_should_fail: true,
            })
        }
    }
    #[async_trait]
    impl AppVersionStatePort for FakeVersionState {
        async fn read(&self) -> Result<Option<String>, AppVersionStateError> {
            Ok(self.value.lock().unwrap().clone())
        }
        async fn write(&self, version: &str) -> Result<(), AppVersionStateError> {
            if self.write_should_fail {
                return Err(AppVersionStateError::Write(Box::new(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "SECRET_CURSOR_PATH",
                ))));
            }
            *self.value.lock().unwrap() = Some(version.to_string());
            Ok(())
        }
    }

    #[tokio::test]
    async fn acknowledge_writes_cursor_and_round_trips() {
        let port = FakeVersionState::new();
        let uc = AcknowledgeUseCase::new(port.clone());
        uc.execute("1.0.0-alpha.1").await.unwrap();
        assert_eq!(port.value.lock().unwrap().as_deref(), Some("1.0.0-alpha.1"));
    }

    #[tokio::test]
    async fn acknowledge_rejects_invalid_version() {
        let uc = AcknowledgeUseCase::new(FakeVersionState::new());
        let err = uc.execute("not-semver").await.unwrap_err();
        assert!(matches!(err, AcknowledgeError::CurrentVersionMalformed(_)));
    }

    #[tokio::test]
    async fn acknowledge_propagates_write_failure() {
        let uc = AcknowledgeUseCase::new(FakeVersionState::failing());
        let err = uc.execute("1.0.0").await.unwrap_err();
        assert!(matches!(err, AcknowledgeError::WriteCursor(_)));
    }

    #[tokio::test]
    async fn a_failed_cursor_write_records_only_fixed_classification() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let uc = AcknowledgeUseCase::new(FakeVersionState::failing());

        uc.execute("1.0.0").await.unwrap_err();

        let output = logs.output();
        assert_eq!(logs.count("upgrade acknowledgement failed"), 1, "{output}");
        assert!(
            output.contains("error_kind=\"upgrade_acknowledge\""),
            "{output}"
        );
        assert!(output.contains("error_class=\"write_cursor\""), "{output}");
        assert!(
            output.contains("io_error_kind=PermissionDenied"),
            "{output}"
        );
        assert!(!output.contains("SECRET_CURSOR_PATH"), "{output}");
    }

    #[tokio::test]
    async fn a_malformed_version_records_its_class_and_a_success_records_no_failure() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();

        AcknowledgeUseCase::new(FakeVersionState::new())
            .execute("1.0.0")
            .await
            .unwrap();
        assert_eq!(logs.count("upgrade acknowledgement failed"), 0);

        AcknowledgeUseCase::new(FakeVersionState::new())
            .execute("not-semver")
            .await
            .unwrap_err();
        let output = logs.output();
        assert_eq!(logs.count("upgrade acknowledgement failed"), 1, "{output}");
        assert!(
            output.contains("error_class=\"current_version_malformed\""),
            "{output}"
        );
    }
}
