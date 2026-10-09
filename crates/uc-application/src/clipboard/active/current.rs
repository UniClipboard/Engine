//! 活动剪贴板当前状态查询：读取寄存器并由本模块记录失败分类。
//!
//! 失败原样返回给调用方（公开契约边界会丢弃来源），因此在这里取得固定分类：
//! 变体级 `error_class` 与来源链上的 `io_error_kind`，不输出错误正文。

use uc_core::clipboard::ActiveClipboardState;
use uc_core::error_class::ErrorClass;
use uc_core::ports::clipboard::{ActiveClipboardRegisterError, LoadActiveClipboardPort};
use uc_observability_contract::{error_source::io_error_kind, uc_info, uc_warn};

pub(super) async fn load_current(
    register: &dyn LoadActiveClipboardPort,
) -> Result<Option<ActiveClipboardState>, ActiveClipboardRegisterError> {
    let result = register.load().await;
    if let Err(error) = &result {
        match error {
            // 空间未解锁是可预期状态，不是存储故障。
            ActiveClipboardRegisterError::NotUnlocked => uc_info!(
                operation = "query_active_clipboard",
                outcome = "rejected",
                error_class = error.class(),
                "active clipboard current query rejected"
            ),
            ActiveClipboardRegisterError::Storage(_) => uc_warn!(
                operation = "query_active_clipboard",
                outcome = "failed",
                error_kind = "register_load",
                error_class = error.class(),
                io_error_kind = io_error_kind(error),
                "active clipboard current query failed"
            ),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use std::io;

    use async_trait::async_trait;
    use uc_testkit::log_capture::CapturedLogs;

    use super::*;

    struct FailingRegister(fn() -> ActiveClipboardRegisterError);

    #[async_trait]
    impl LoadActiveClipboardPort for FailingRegister {
        async fn load(&self) -> Result<Option<ActiveClipboardState>, ActiveClipboardRegisterError> {
            Err((self.0)())
        }
    }

    struct EmptyRegister;

    #[async_trait]
    impl LoadActiveClipboardPort for EmptyRegister {
        async fn load(&self) -> Result<Option<ActiveClipboardState>, ActiveClipboardRegisterError> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn a_storage_failure_records_only_fixed_classification() {
        let logs = CapturedLogs::default();
        let _guard = logs.install();
        let register = FailingRegister(|| {
            ActiveClipboardRegisterError::Storage(
                anyhow::Error::new(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "SECRET_REGISTER_PATH",
                ))
                .context("load active clipboard register"),
            )
        });

        let error = load_current(&register).await.unwrap_err();

        assert!(matches!(error, ActiveClipboardRegisterError::Storage(_)));
        let output = logs.output();
        assert_eq!(
            logs.count("active clipboard current query failed"),
            1,
            "{output}"
        );
        assert!(output.contains("error_kind=\"register_load\""), "{output}");
        assert!(output.contains("error_class=\"storage\""), "{output}");
        assert!(
            output.contains("io_error_kind=PermissionDenied"),
            "{output}"
        );
        assert!(!output.contains("SECRET_REGISTER_PATH"), "{output}");
    }

    #[tokio::test]
    async fn a_locked_space_is_recorded_as_a_rejection_not_a_failure() {
        let logs = CapturedLogs::default();
        let _guard = logs.install();
        let register = FailingRegister(|| ActiveClipboardRegisterError::NotUnlocked);

        load_current(&register).await.unwrap_err();

        let output = logs.output();
        assert_eq!(
            logs.count("active clipboard current query rejected"),
            1,
            "{output}"
        );
        assert!(output.contains("error_class=\"not_unlocked\""), "{output}");
        assert_eq!(logs.count("active clipboard current query failed"), 0);
    }

    #[tokio::test]
    async fn a_successful_query_writes_no_record() {
        let logs = CapturedLogs::default();
        let _guard = logs.install();

        assert!(load_current(&EmptyRegister).await.unwrap().is_none());

        assert_eq!(logs.output(), "", "{}", logs.output());
    }
}
