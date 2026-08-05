use std::sync::Arc;

use tracing::{error, warn};
use uc_application::clipboard_write::LocalActiveRegisterAdvancer;
use uc_application::facade::{
    ClipboardHostEvent, ClipboardLiveIndexInput, ClipboardLiveIndexOutcome, ClipboardOriginKind,
    ClipboardOutboundInput, HostEvent, HostEventBus,
};
use uc_core::ports::{SelfWriteLedgerPort, SystemClipboardPort};
use uc_core::{ClipboardChangeOrigin, TaskRegistry};

use super::host_operations::send_report_summary;
use super::operation_error_with_code;
use super::session_supervisor::SessionSupervisor;
use crate::{EngineError, HostClipboardChange, HostClipboardChangeStream, SendReportSummary};

const OBSERVE_CLIPBOARD_FAILED_CODE: u32 = 1254;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchMode {
    Background,
    AwaitReport,
    CaptureOnly,
}

#[derive(Clone)]
pub(super) struct HostClipboardChangeRuntime {
    pub(super) session_supervisor: Arc<SessionSupervisor>,
    pub(super) system_clipboard: Arc<dyn SystemClipboardPort>,
    pub(super) change_origin: Arc<dyn SelfWriteLedgerPort>,
    pub(super) active_register: LocalActiveRegisterAdvancer,
    pub(super) host_events: Arc<HostEventBus>,
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
                            if let Err(error) = runtime.process_change(DispatchMode::Background).await {
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
        self.process_change(if dispatch {
            DispatchMode::AwaitReport
        } else {
            DispatchMode::CaptureOnly
        })
        .await
    }

    async fn process_change(
        &self,
        dispatch_mode: DispatchMode,
    ) -> Result<Option<SendReportSummary>, EngineError> {
        let lease = self.session_supervisor.acquire_operation().await?;
        let cancellation = lease.cancellation();
        let processing = self.process_change_while_leased(dispatch_mode);
        tokio::select! {
            _ = cancellation.cancelled() => Ok(None),
            result = processing => result,
        }
    }

    async fn process_change_while_leased(
        &self,
        dispatch_mode: DispatchMode,
    ) -> Result<Option<SendReportSummary>, EngineError> {
        let (facade, capture, live_index, outbound, tasks) = {
            let session_slot = self.session_supervisor.session();
            let session = session_slot.lock().await;
            let Some(session) = session.as_ref() else {
                return Ok(None);
            };
            (
                Arc::clone(&session.facade),
                Arc::clone(&session.clipboard.capture),
                Arc::clone(&session.clipboard.live_index),
                Arc::clone(&session.clipboard.sync),
                Arc::clone(&session.tasks),
            )
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

        let outbound_snapshot = Arc::new(snapshot.clone());
        let Some(captured) = capture
            .capture(snapshot, origin, None)
            .await
            .map_err(|error| observe_error("clipboard capture", error))?
        else {
            return Ok(None);
        };
        let entry_id = uc_core::ids::EntryId::from(captured.entry_id.as_str());
        self.active_register
            .advance_local(captured.snapshot_hash, entry_id)
            .await;
        self.host_events
            .emit_or_warn(HostEvent::Clipboard(ClipboardHostEvent::NewContent {
                entry_id: captured.entry_id.clone(),
                attempt_id: None,
                preview: "New clipboard content".to_string(),
                origin: ClipboardOriginKind::Local,
            }));

        if !captured.deduplicated {
            match live_index
                .index_capture(ClipboardLiveIndexInput {
                    entry_id: captured.entry_id.clone(),
                    snapshot: Arc::clone(&outbound_snapshot),
                })
                .await
            {
                Ok(ClipboardLiveIndexOutcome::Indexed) => {}
                Ok(ClipboardLiveIndexOutcome::Skipped { reason }) => {
                    tracing::debug!(reason, "host clipboard live index skipped");
                }
                Err(error) => warn!(error = %error, "host clipboard live index failed"),
            }
        }

        let dispatch_snapshot =
            Arc::try_unwrap(outbound_snapshot).unwrap_or_else(|shared| (*shared).clone());
        if dispatch_mode == DispatchMode::CaptureOnly {
            return Ok(None);
        }
        let entry_id = captured.entry_id;
        let dispatch = move || async move {
            outbound
                .dispatch_local_capture(ClipboardOutboundInput {
                    entry_id: entry_id.clone(),
                    snapshot: dispatch_snapshot,
                    origin,
                })
                .await
                .map_err(|error| observe_error("clipboard dispatch", error))
                .and_then(|outcome| send_report_summary(entry_id, outcome))
        };
        match dispatch_mode {
            DispatchMode::AwaitReport => dispatch().await.map(Some),
            DispatchMode::Background => {
                tasks
                    .spawn("host_clipboard_outbound", move |cancel| async move {
                        let outcome = tokio::select! {
                            _ = cancel.cancelled() => return,
                                    outcome = dispatch() => outcome,
                        };
                        match outcome {
                            Ok(report) => tracing::info!(
                                accepted = report.total_accepted,
                                duplicate = report.total_duplicate,
                                offline = report.total_offline,
                                errored = report.total_errored,
                                pending = report.total_pending,
                                "host clipboard outbound sync completed"
                            ),
                            Err(error) => {
                                warn!(error = %error, "host clipboard outbound sync failed")
                            }
                        }
                    })
                    .await;
                Ok(None)
            }
            DispatchMode::CaptureOnly => Ok(None),
        }
    }
}

fn observe_error(context: &'static str, error: impl std::fmt::Display) -> EngineError {
    operation_error_with_code(OBSERVE_CLIPBOARD_FAILED_CODE, context, error)
}
