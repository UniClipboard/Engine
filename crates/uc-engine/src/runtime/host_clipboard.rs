use std::error::Error;
use std::future::Future;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use uc_application::facade::{
    HostClipboardDispatch, LocalClipboardIntent, LocalClipboardOutcome, LocalClipboardRequest,
};
use uc_core::ports::{SelfWriteLedgerPort, SystemClipboardPort};
use uc_core::{ClipboardChangeOrigin, TaskRegistry};
use uc_observability_contract::{error_source::io_error_kind, uc_error, uc_info, uc_warn};

use super::host_operations::send_report_summary;
use super::operation_error_with_code;
use super::session_supervisor::SessionSupervisor;
use crate::{
    EngineError, HostCapabilityError, HostClipboardChange, HostClipboardChangeStream,
    SendReportSummary,
};

const OBSERVE_CLIPBOARD_FAILED_CODE: u32 = 1254;

#[derive(Clone)]
pub(super) struct HostClipboardChangeRuntime {
    pub(super) session_supervisor: Arc<SessionSupervisor>,
    pub(super) system_clipboard: Arc<dyn SystemClipboardPort>,
    pub(super) change_origin: Arc<dyn SelfWriteLedgerPort>,
}

pub(super) async fn spawn_host_clipboard_change_task(
    changes: Box<dyn HostClipboardChangeStream>,
    runtime: HostClipboardChangeRuntime,
    tasks: Arc<TaskRegistry>,
) {
    let _ = tasks
        .spawn(move |cancel| async move {
            watch_host_clipboard_changes(changes, cancel, || {
                runtime.process_change(HostClipboardDispatch::Background)
            })
            .await;
        })
        .await;
}

/// 监听循环本身：流的结束方式（取消、关闭、失败）与处理失败都在这里记录一次。
/// 单次变化的处理由调用方注入，循环不依赖会话与应用。
async fn watch_host_clipboard_changes<Process, Processing>(
    mut changes: Box<dyn HostClipboardChangeStream>,
    cancel: CancellationToken,
    process: Process,
) where
    Process: Fn() -> Processing,
    Processing: Future<Output = Result<Option<SendReportSummary>, EngineError>>,
{
    loop {
        let Some(change) = next_change_or_stop(changes.as_mut(), &cancel).await else {
            if let Err(error) = changes.shutdown().await {
                uc_warn!(
                    error_kind = "change_stream_shutdown",
                    io_error_kind = io_error_kind(&error),
                    "host clipboard change stream shutdown failed"
                );
            }
            return;
        };
        match change {
            Ok(HostClipboardChange::Changed) => {
                if let Err(error) = process().await {
                    uc_warn!(
                        error_kind = "change_processing",
                        io_error_kind = io_error_kind(&error),
                        "host clipboard change processing failed"
                    );
                }
            }
            Ok(HostClipboardChange::Closed) => {
                uc_warn!(
                    error_kind = "change_stream_closed",
                    "host clipboard change stream closed; watcher stopped"
                );
                return;
            }
            Err(error) => {
                uc_warn!(
                    error_kind = "change_stream",
                    io_error_kind = io_error_kind(&error),
                    "host clipboard change stream failed"
                );
                return;
            }
        }
    }
}

async fn next_change_or_stop(
    changes: &mut dyn HostClipboardChangeStream,
    cancel: &CancellationToken,
) -> Option<Result<HostClipboardChange, HostCapabilityError>> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => None,
        change = changes.next() => Some(change),
    }
}

impl HostClipboardChangeRuntime {
    pub(super) async fn observe_change(
        &self,
        dispatch: bool,
    ) -> Result<Option<SendReportSummary>, EngineError> {
        self.process_change(if dispatch {
            HostClipboardDispatch::AwaitReport
        } else {
            HostClipboardDispatch::CaptureOnly
        })
        .await
    }

    async fn process_change(
        &self,
        dispatch_mode: HostClipboardDispatch,
    ) -> Result<Option<SendReportSummary>, EngineError> {
        let lease = self.session_supervisor.acquire_operation().await?;
        // 取得租约后，这次剪贴板处理已经开始。暂停由租约排空负责等待，
        // 不能在这里再用同一停止信号丢弃正在提交的完整动作。
        let result = self.process_change_while_leased(dispatch_mode).await;
        drop(lease);
        result
    }

    async fn process_change_while_leased(
        &self,
        dispatch_mode: HostClipboardDispatch,
    ) -> Result<Option<SendReportSummary>, EngineError> {
        let (facade, application) = match self
            .session_supervisor
            .current_facade_and_application()
            .await
        {
            Ok(current) => current,
            Err(_) => return Ok(None),
        };
        let encryption = facade
            .encryption_state()
            .await
            .map_err(|error| observe_error("clipboard encryption state", error))?;
        if !encryption.session_ready {
            // 在 observe_local_copy 之前返回，没有复制并同步的业务记录；这里补一条，使“复制了但没同步”可归因。
            uc_info!(
                reason = "space_locked",
                "host clipboard change skipped: space is locked"
            );
            return Ok(None);
        }

        let snapshot = self
            .system_clipboard
            .read_snapshot()
            .map_err(|error| observe_error("clipboard snapshot read", error))?;
        if snapshot.is_empty() {
            return Ok(None);
        }
        let origin_guard_key = snapshot.origin_guard_key();
        let origin = self
            .change_origin
            .attribute_observed_change(&origin_guard_key)
            .await;
        if origin.is_remote_push() {
            return Ok(None);
        }
        if origin == ClipboardChangeOrigin::Resend {
            uc_error!("host clipboard watcher observed an invalid resend origin");
            return Ok(None);
        }

        let outcome = crate::assembly::observability::observe_local_copy(
            application.process_local_clipboard(LocalClipboardRequest {
                snapshot,
                origin,
                intent: LocalClipboardIntent::ObservedHostChange {
                    dispatch: dispatch_mode,
                },
            }),
        )
        .await
        .map_err(|error| observe_error("local clipboard", error))?;
        let LocalClipboardOutcome::Completed(completion) = outcome else {
            return Ok(None);
        };
        let Some(dispatch) = completion.dispatch else {
            return Ok(None);
        };
        let report = send_report_summary(completion.entry_id, dispatch)?;
        match dispatch_mode {
            HostClipboardDispatch::AwaitReport => Ok(Some(report)),
            HostClipboardDispatch::Background => {
                uc_info!(
                    accepted = report.total_accepted,
                    duplicate = report.total_duplicate,
                    offline = report.total_offline,
                    errored = report.total_errored,
                    pending = report.total_pending,
                    "host clipboard outbound sync completed"
                );
                Ok(None)
            }
            HostClipboardDispatch::CaptureOnly => Ok(None),
        }
    }
}

fn observe_error(
    context: &'static str,
    error: impl Into<Box<dyn Error + Send + Sync>>,
) -> EngineError {
    operation_error_with_code(OBSERVE_CLIPBOARD_FAILED_CODE, context, error)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use async_trait::async_trait;

    use super::*;

    struct ReadyClipboardChange<'a>(&'a AtomicBool);

    #[async_trait]
    impl HostClipboardChangeStream for ReadyClipboardChange<'_> {
        async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
            self.0.store(true, Ordering::SeqCst);
            Ok(HostClipboardChange::Changed)
        }

        async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn stop_wins_over_a_ready_clipboard_change() {
        let next_called = AtomicBool::new(false);
        let mut changes = ReadyClipboardChange(&next_called);
        let cancel = CancellationToken::new();
        cancel.cancel();

        assert!(next_change_or_stop(&mut changes, &cancel).await.is_none());
        assert!(!next_called.load(Ordering::SeqCst));
    }

    struct ScriptedChanges {
        script: std::collections::VecDeque<Result<HostClipboardChange, HostCapabilityError>>,
        shutdowns: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl HostClipboardChangeStream for ScriptedChanges {
        async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
            match self.script.pop_front() {
                Some(next) => next,
                None => std::future::pending().await,
            }
        }

        async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
            self.shutdowns.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn scripted(
        script: Vec<Result<HostClipboardChange, HostCapabilityError>>,
    ) -> (
        Box<dyn HostClipboardChangeStream>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let shutdowns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stream = ScriptedChanges {
            script: script.into(),
            shutdowns: Arc::clone(&shutdowns),
        };
        (Box::new(stream), shutdowns)
    }

    #[tokio::test]
    async fn a_closed_change_stream_stops_the_watcher_with_one_warning() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let (changes, _) = scripted(vec![Ok(HostClipboardChange::Closed)]);

        watch_host_clipboard_changes(changes, CancellationToken::new(), || async { Ok(None) })
            .await;

        assert_eq!(
            logs.count("host clipboard change stream closed; watcher stopped"),
            1,
            "{}",
            logs.output()
        );
        assert!(logs
            .output()
            .contains("error_kind=\"change_stream_closed\""));
    }

    #[tokio::test]
    async fn a_failed_change_stream_records_its_io_kind_and_stops() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let (changes, _) = scripted(vec![Err(HostCapabilityError::new(
            crate::HostCapabilityErrorCategory::Io,
            "PRIVATE_STREAM_DETAIL",
        ))]);

        watch_host_clipboard_changes(changes, CancellationToken::new(), || async { Ok(None) })
            .await;

        assert_eq!(
            logs.count("host clipboard change stream failed"),
            1,
            "{}",
            logs.output()
        );
        assert!(!logs.output().contains("PRIVATE_STREAM_DETAIL"));
    }

    #[tokio::test]
    async fn a_processing_failure_is_recorded_and_the_watcher_keeps_running_until_stopped() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let (changes, shutdowns) = scripted(vec![
            Ok(HostClipboardChange::Changed),
            Ok(HostClipboardChange::Changed),
        ]);
        let cancel = CancellationToken::new();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let watcher = tokio::spawn({
            let cancel = cancel.clone();
            let calls = Arc::clone(&calls);
            watch_host_clipboard_changes(changes, cancel, move || {
                calls.fetch_add(1, Ordering::SeqCst);
                async {
                    Err(EngineError::new(
                        OBSERVE_CLIPBOARD_FAILED_CODE,
                        crate::EngineErrorCategory::Internal,
                        false,
                    ))
                }
            })
        });
        while calls.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
        cancel.cancel();
        watcher.await.unwrap();

        assert_eq!(
            logs.count("host clipboard change processing failed"),
            2,
            "{}",
            logs.output()
        );
        assert_eq!(shutdowns.load(Ordering::SeqCst), 1);
    }
}
