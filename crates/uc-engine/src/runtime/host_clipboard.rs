use std::sync::Arc;
use std::time::Instant;

use tracing::{error, warn};
use uc_application::facade::{
    HostClipboardDispatch, LocalClipboardIntent, LocalClipboardOutcome, LocalClipboardRequest,
};
use uc_core::ports::{SelfWriteLedgerPort, SystemClipboardPort};
use uc_core::{ClipboardChangeOrigin, TaskRegistry};

use super::host_operations::send_report_summary;
use super::operation_error_with_code;
use super::session_supervisor::SessionSupervisor;
use crate::{EngineError, HostClipboardChange, HostClipboardChangeStream, SendReportSummary};

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
    tasks
        .spawn("host_clipboard_changes", move |cancel| async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        if let Err(error) = changes.shutdown().await {
                            warn!(error = %error, "host clipboard change stream shutdown failed");
                        }
                        return;
                    }
                    change = changes.next() => match change {
                        Ok(HostClipboardChange::Changed) => {
                            if let Err(error) = runtime
                                .process_change(HostClipboardDispatch::Background, Some(Instant::now()))
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
            }
        })
        .await;
}

impl HostClipboardChangeRuntime {
    pub(super) async fn observe_change(
        &self,
        dispatch: bool,
    ) -> Result<Option<SendReportSummary>, EngineError> {
        self.process_change(
            if dispatch {
                HostClipboardDispatch::AwaitReport
            } else {
                HostClipboardDispatch::CaptureOnly
            },
            None,
        )
        .await
    }

    async fn process_change(
        &self,
        dispatch_mode: HostClipboardDispatch,
        source_started_at: Option<Instant>,
    ) -> Result<Option<SendReportSummary>, EngineError> {
        let lease = self.session_supervisor.acquire_operation().await?;
        let cancellation = lease.cancellation();
        let result = tokio::select! {
            _ = cancellation.cancelled() => Err(super::operation_unavailable_error()),
            result = self.process_change_while_leased(dispatch_mode, source_started_at) => result,
        };
        drop(lease);
        result
    }

    async fn process_change_while_leased(
        &self,
        dispatch_mode: HostClipboardDispatch,
        source_started_at: Option<Instant>,
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

        let outcome = application
            .process_local_clipboard(LocalClipboardRequest {
                snapshot,
                origin,
                intent: LocalClipboardIntent::ObservedHostChange {
                    dispatch: dispatch_mode,
                },
                source_started_at,
            })
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
