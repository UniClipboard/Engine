use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{error, warn};
use uc_application::facade::{
    HostClipboardDispatch, LocalClipboardIntent, LocalClipboardOutcome, LocalClipboardRequest,
};
use uc_core::ports::{SelfWriteLedgerPort, SystemClipboardPort};
use uc_core::{ClipboardChangeOrigin, TaskRegistry};

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
    mut changes: Box<dyn HostClipboardChangeStream>,
    runtime: HostClipboardChangeRuntime,
    tasks: Arc<TaskRegistry>,
) {
    let _ = tasks
        .spawn(move |cancel| async move {
            loop {
                let Some(change) = next_change_or_stop(changes.as_mut(), &cancel).await else {
                    if let Err(error) = changes.shutdown().await {
                        warn!(error = %error, "host clipboard change stream shutdown failed");
                    }
                    return;
                };
                match change {
                    Ok(HostClipboardChange::Changed) => {
                        if let Err(error) = runtime
                            .process_change(HostClipboardDispatch::Background)
                            .await
                        {
                            warn!(error = %error, "host clipboard change processing failed");
                        }
                    }
                    Ok(HostClipboardChange::Closed) => return,
                    Err(error) => {
                        warn!(error = %error, "host clipboard change stream failed");
                        return;
                    }
                }
            }
        })
        .await;
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
            error!("host clipboard watcher observed an invalid resend origin");
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
                tracing::info!(
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

fn observe_error(context: &'static str, error: impl std::fmt::Display) -> EngineError {
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
}
