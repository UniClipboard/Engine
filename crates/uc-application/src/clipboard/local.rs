//! 本机 Clipboard capture 的完整 Application 动作。
//!
//! 调用方只表达宿主观察或显式发送意图；capture、active register、
//! best-effort live index 与 dispatch 的顺序由本模块唯一负责。

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use thiserror::Error;
use uc_core::ids::{DeviceId, EntryId};
use uc_core::{ClipboardChangeOrigin, SystemClipboardSnapshot};

use crate::clipboard::outbound::{
    ClipboardOutboundError, ClipboardOutboundInput, ClipboardOutboundOutcome,
};
use crate::clipboard::sync::sync_runtime::ClipboardSyncRuntime;
use crate::clipboard::write::LocalActiveRegisterAdvancer;
use crate::facade::{
    CapturedClipboardEntryView, ClipboardCaptureFacadeError, ClipboardCapturePort,
    ClipboardHostEvent, ClipboardOriginKind, HostEvent, HostEventBus,
};
use crate::search::live_index::{
    ClipboardLiveIndexError, ClipboardLiveIndexInput, ClipboardLiveIndexOutcome,
    ClipboardLiveIndexPort,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostClipboardDispatch {
    Background,
    AwaitReport,
    CaptureOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalClipboardIntent {
    ObservedHostChange { dispatch: HostClipboardDispatch },
    ExplicitSend { targets: Vec<DeviceId> },
}

#[derive(Debug, Clone)]
pub struct LocalClipboardRequest {
    pub snapshot: SystemClipboardSnapshot,
    pub origin: ClipboardChangeOrigin,
    pub intent: LocalClipboardIntent,
    pub source_started_at: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalClipboardIndexStatus {
    NotAttempted,
    Indexed,
    Skipped { reason: String },
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalClipboardCompletion {
    pub entry_id: String,
    pub snapshot_hash: String,
    pub deduplicated: bool,
    pub index: LocalClipboardIndexStatus,
    pub dispatch: Option<ClipboardOutboundOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalClipboardOutcome {
    Empty,
    Completed(LocalClipboardCompletion),
}

#[derive(Debug, Error)]
pub enum LocalClipboardProcessError {
    #[error("clipboard capture failed")]
    Capture {
        #[source]
        source: ClipboardCaptureFacadeError,
    },
    #[error("clipboard dispatch failed")]
    Dispatch {
        #[source]
        source: ClipboardOutboundError,
    },
}

#[async_trait]
pub(crate) trait LocalClipboardActivationPort: Send + Sync {
    async fn advance_local(&self, snapshot_hash: String, entry_id: EntryId);
}

#[async_trait]
impl LocalClipboardActivationPort for LocalActiveRegisterAdvancer {
    async fn advance_local(&self, snapshot_hash: String, entry_id: EntryId) {
        LocalActiveRegisterAdvancer::advance_local(self, snapshot_hash, entry_id).await;
    }
}

#[async_trait]
pub(crate) trait LocalClipboardDispatchPort: Send + Sync {
    async fn dispatch_local_capture(
        &self,
        input: ClipboardOutboundInput,
        targets: Option<Vec<DeviceId>>,
    ) -> Result<ClipboardOutboundOutcome, ClipboardOutboundError>;
}

#[async_trait]
impl LocalClipboardDispatchPort for ClipboardSyncRuntime {
    async fn dispatch_local_capture(
        &self,
        input: ClipboardOutboundInput,
        targets: Option<Vec<DeviceId>>,
    ) -> Result<ClipboardOutboundOutcome, ClipboardOutboundError> {
        self.dispatch_local_capture_to_targets(input, targets).await
    }
}

pub(crate) struct LocalClipboardProcessorDeps {
    pub capture: Arc<dyn ClipboardCapturePort>,
    pub live_index: Arc<dyn ClipboardLiveIndexPort>,
    pub activation: Arc<dyn LocalClipboardActivationPort>,
    pub dispatch: Arc<dyn LocalClipboardDispatchPort>,
    pub host_events: Arc<HostEventBus>,
}

pub(crate) struct LocalClipboardProcessor {
    capture: Arc<dyn ClipboardCapturePort>,
    live_index: Arc<dyn ClipboardLiveIndexPort>,
    activation: Arc<dyn LocalClipboardActivationPort>,
    dispatch: Arc<dyn LocalClipboardDispatchPort>,
    host_events: Arc<HostEventBus>,
}

impl LocalClipboardProcessor {
    pub(crate) fn new(deps: LocalClipboardProcessorDeps) -> Self {
        Self {
            capture: deps.capture,
            live_index: deps.live_index,
            activation: deps.activation,
            dispatch: deps.dispatch,
            host_events: deps.host_events,
        }
    }

    pub(crate) async fn process(
        &self,
        request: LocalClipboardRequest,
    ) -> Result<LocalClipboardOutcome, LocalClipboardProcessError> {
        let LocalClipboardRequest {
            snapshot,
            origin,
            intent,
            source_started_at,
        } = request;
        let captured = self
            .capture
            .capture(snapshot.clone(), origin, None)
            .await
            .map_err(|source| LocalClipboardProcessError::Capture { source })?;
        let Some(CapturedClipboardEntryView {
            entry_id,
            deduplicated,
            snapshot_hash,
        }) = captured
        else {
            return Ok(LocalClipboardOutcome::Empty);
        };

        if matches!(intent, LocalClipboardIntent::ObservedHostChange { .. }) {
            self.activation
                .advance_local(snapshot_hash.clone(), EntryId::from(entry_id.as_str()))
                .await;
            self.host_events
                .emit_or_warn(HostEvent::Clipboard(ClipboardHostEvent::NewContent {
                    entry_id: entry_id.clone(),
                    attempt_id: None,
                    preview: "New clipboard content".to_owned(),
                    origin: ClipboardOriginKind::Local,
                }));
        }

        let shared_snapshot = Arc::new(snapshot);
        let index = if deduplicated {
            LocalClipboardIndexStatus::NotAttempted
        } else {
            match self
                .live_index
                .index_capture(ClipboardLiveIndexInput {
                    entry_id: entry_id.clone(),
                    snapshot: Arc::clone(&shared_snapshot),
                })
                .await
            {
                Ok(ClipboardLiveIndexOutcome::Indexed) => LocalClipboardIndexStatus::Indexed,
                Ok(ClipboardLiveIndexOutcome::Skipped { reason }) => {
                    LocalClipboardIndexStatus::Skipped { reason }
                }
                Err(ClipboardLiveIndexError::Internal(_)) => {
                    tracing::warn!(
                        error_kind = "live_index",
                        "local clipboard live index failed"
                    );
                    LocalClipboardIndexStatus::Failed
                }
            }
        };

        let target_filter = match &intent {
            LocalClipboardIntent::ObservedHostChange { .. } => None,
            LocalClipboardIntent::ExplicitSend { targets } if targets.is_empty() => None,
            LocalClipboardIntent::ExplicitSend { targets } => Some(targets.clone()),
        };
        let should_dispatch = !matches!(
            intent,
            LocalClipboardIntent::ObservedHostChange {
                dispatch: HostClipboardDispatch::CaptureOnly
            }
        );
        let dispatch = if should_dispatch {
            let snapshot = match Arc::try_unwrap(shared_snapshot) {
                Ok(snapshot) => snapshot,
                Err(shared) => (*shared).clone(),
            };
            Some(
                self.dispatch
                    .dispatch_local_capture(
                        ClipboardOutboundInput {
                            entry_id: entry_id.clone(),
                            snapshot,
                            origin,
                            source_started_at,
                        },
                        target_filter,
                    )
                    .await
                    .map_err(|source| LocalClipboardProcessError::Dispatch { source })?,
            )
        } else {
            None
        };

        Ok(LocalClipboardOutcome::Completed(LocalClipboardCompletion {
            entry_id,
            snapshot_hash,
            deduplicated,
            index,
            dispatch,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use uc_core::ids::{DeviceId, EntryId};
    use uc_core::{ClipboardChangeOrigin, SystemClipboardSnapshot};

    use crate::clipboard::outbound::{
        ClipboardOutboundError, ClipboardOutboundInput, ClipboardOutboundOutcome,
    };
    use crate::facade::{
        CapturedClipboardEntryView, ClipboardCaptureFacadeError, ClipboardCapturePort, EmitError,
        HostEvent, HostEventBus, HostEventEmitterPort,
    };
    use crate::search::live_index::{
        ClipboardLiveIndexError, ClipboardLiveIndexInput, ClipboardLiveIndexOutcome,
        ClipboardLiveIndexPort,
    };

    use super::*;

    struct FixedCapture;

    #[async_trait]
    impl ClipboardCapturePort for FixedCapture {
        async fn capture(
            &self,
            _snapshot: SystemClipboardSnapshot,
            _origin: ClipboardChangeOrigin,
            _preset_entry_id: Option<EntryId>,
        ) -> Result<Option<CapturedClipboardEntryView>, ClipboardCaptureFacadeError> {
            Ok(Some(CapturedClipboardEntryView {
                entry_id: "entry-a".to_owned(),
                deduplicated: false,
                snapshot_hash: "blake3v1:capture".to_owned(),
            }))
        }
    }

    struct DeduplicatedCapture;

    #[async_trait]
    impl ClipboardCapturePort for DeduplicatedCapture {
        async fn capture(
            &self,
            _snapshot: SystemClipboardSnapshot,
            _origin: ClipboardChangeOrigin,
            _preset_entry_id: Option<EntryId>,
        ) -> Result<Option<CapturedClipboardEntryView>, ClipboardCaptureFacadeError> {
            Ok(Some(CapturedClipboardEntryView {
                entry_id: "entry-existing".to_owned(),
                deduplicated: true,
                snapshot_hash: "blake3v1:existing".to_owned(),
            }))
        }
    }

    struct EmptyCapture;

    #[async_trait]
    impl ClipboardCapturePort for EmptyCapture {
        async fn capture(
            &self,
            _snapshot: SystemClipboardSnapshot,
            _origin: ClipboardChangeOrigin,
            _preset_entry_id: Option<EntryId>,
        ) -> Result<Option<CapturedClipboardEntryView>, ClipboardCaptureFacadeError> {
            Ok(None)
        }
    }

    #[derive(Default)]
    struct RecordingIndex(AtomicUsize);

    #[async_trait]
    impl ClipboardLiveIndexPort for RecordingIndex {
        async fn index_capture(
            &self,
            _input: ClipboardLiveIndexInput,
        ) -> Result<ClipboardLiveIndexOutcome, ClipboardLiveIndexError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ClipboardLiveIndexOutcome::Indexed)
        }
    }

    struct FailingIndex;

    #[async_trait]
    impl ClipboardLiveIndexPort for FailingIndex {
        async fn index_capture(
            &self,
            _input: ClipboardLiveIndexInput,
        ) -> Result<ClipboardLiveIndexOutcome, ClipboardLiveIndexError> {
            Err(ClipboardLiveIndexError::Internal(
                "sensitive index detail".to_owned(),
            ))
        }
    }

    #[derive(Default)]
    struct RecordingActivation(AtomicUsize);

    #[async_trait]
    impl LocalClipboardActivationPort for RecordingActivation {
        async fn advance_local(&self, _snapshot_hash: String, _entry_id: EntryId) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[derive(Default)]
    struct RecordingDispatch(Mutex<Vec<Option<Vec<DeviceId>>>>);

    #[async_trait]
    impl LocalClipboardDispatchPort for RecordingDispatch {
        async fn dispatch_local_capture(
            &self,
            _input: ClipboardOutboundInput,
            targets: Option<Vec<DeviceId>>,
        ) -> Result<ClipboardOutboundOutcome, ClipboardOutboundError> {
            self.0.lock().unwrap().push(targets);
            Ok(ClipboardOutboundOutcome::Skipped {
                reason: "test".to_owned(),
            })
        }
    }

    #[derive(Default)]
    struct RecordingHostEvents(Mutex<Vec<HostEvent>>);

    struct FailingDispatch;

    #[async_trait]
    impl LocalClipboardDispatchPort for FailingDispatch {
        async fn dispatch_local_capture(
            &self,
            _input: ClipboardOutboundInput,
            _targets: Option<Vec<DeviceId>>,
        ) -> Result<ClipboardOutboundOutcome, ClipboardOutboundError> {
            Err(ClipboardOutboundError::Internal("test failure".to_owned()))
        }
    }

    impl HostEventEmitterPort for RecordingHostEvents {
        fn emit(&self, event: HostEvent) -> Result<(), EmitError> {
            self.0.lock().unwrap().push(event);
            Ok(())
        }
    }

    #[tokio::test]
    async fn observed_host_change_completes_the_local_clipboard_action() {
        let index = Arc::new(RecordingIndex::default());
        let activation = Arc::new(RecordingActivation::default());
        let dispatch = Arc::new(RecordingDispatch::default());
        let events = Arc::new(RecordingHostEvents::default());
        let event_bus = Arc::new(HostEventBus::new());
        event_bus.register("local-test", events.clone());
        let processor = LocalClipboardProcessor::new(LocalClipboardProcessorDeps {
            capture: Arc::new(FixedCapture),
            live_index: index.clone(),
            activation: activation.clone(),
            dispatch: dispatch.clone(),
            host_events: event_bus,
        });

        let outcome = processor
            .process(LocalClipboardRequest {
                snapshot: SystemClipboardSnapshot {
                    representations: Vec::new(),
                    ts_ms: 0,
                    file_content_digests: Vec::new(),
                    file_set_v1_component: None,
                },
                origin: ClipboardChangeOrigin::LocalCapture,
                intent: LocalClipboardIntent::ObservedHostChange {
                    dispatch: HostClipboardDispatch::AwaitReport,
                },
                source_started_at: None,
            })
            .await
            .unwrap();

        let LocalClipboardOutcome::Completed(completion) = outcome else {
            panic!("expected completed local clipboard action");
        };
        assert_eq!(completion.entry_id, "entry-a");
        assert_eq!(completion.snapshot_hash, "blake3v1:capture");
        assert!(!completion.deduplicated);
        assert_eq!(completion.index, LocalClipboardIndexStatus::Indexed);
        assert!(completion.dispatch.is_some());
        assert_eq!(activation.0.load(Ordering::SeqCst), 1);
        assert_eq!(index.0.load(Ordering::SeqCst), 1);
        assert_eq!(dispatch.0.lock().unwrap().as_slice(), &[None]);
        assert_eq!(events.0.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn explicit_send_targets_without_advancing_or_emitting_host_change() {
        let index = Arc::new(RecordingIndex::default());
        let activation = Arc::new(RecordingActivation::default());
        let dispatch = Arc::new(RecordingDispatch::default());
        let events = Arc::new(RecordingHostEvents::default());
        let event_bus = Arc::new(HostEventBus::new());
        event_bus.register("local-test", events.clone());
        let processor = LocalClipboardProcessor::new(LocalClipboardProcessorDeps {
            capture: Arc::new(FixedCapture),
            live_index: index.clone(),
            activation: activation.clone(),
            dispatch: dispatch.clone(),
            host_events: event_bus,
        });
        let targets = vec![DeviceId::new("peer-a"), DeviceId::new("peer-b")];

        let outcome = processor
            .process(LocalClipboardRequest {
                snapshot: empty_snapshot(),
                origin: ClipboardChangeOrigin::LocalCapture,
                intent: LocalClipboardIntent::ExplicitSend {
                    targets: targets.clone(),
                },
                source_started_at: None,
            })
            .await
            .unwrap();

        assert!(matches!(outcome, LocalClipboardOutcome::Completed(_)));
        assert_eq!(activation.0.load(Ordering::SeqCst), 0);
        assert_eq!(events.0.lock().unwrap().len(), 0);
        assert_eq!(index.0.load(Ordering::SeqCst), 1);
        assert_eq!(dispatch.0.lock().unwrap().as_slice(), &[Some(targets)]);
    }

    #[tokio::test]
    async fn deduplicated_capture_skips_index_but_still_dispatches() {
        let index = Arc::new(RecordingIndex::default());
        let dispatch = Arc::new(RecordingDispatch::default());
        let processor = LocalClipboardProcessor::new(LocalClipboardProcessorDeps {
            capture: Arc::new(DeduplicatedCapture),
            live_index: index.clone(),
            activation: Arc::new(RecordingActivation::default()),
            dispatch: dispatch.clone(),
            host_events: Arc::new(HostEventBus::new()),
        });

        let outcome = processor
            .process(LocalClipboardRequest {
                snapshot: empty_snapshot(),
                origin: ClipboardChangeOrigin::LocalCapture,
                intent: LocalClipboardIntent::ObservedHostChange {
                    dispatch: HostClipboardDispatch::Background,
                },
                source_started_at: None,
            })
            .await
            .unwrap();

        let LocalClipboardOutcome::Completed(completion) = outcome else {
            panic!("expected completed local clipboard action");
        };
        assert_eq!(completion.index, LocalClipboardIndexStatus::NotAttempted);
        assert_eq!(index.0.load(Ordering::SeqCst), 0);
        assert_eq!(dispatch.0.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn index_failure_is_a_best_effort_status_and_dispatch_continues() {
        let dispatch = Arc::new(RecordingDispatch::default());
        let processor = LocalClipboardProcessor::new(LocalClipboardProcessorDeps {
            capture: Arc::new(FixedCapture),
            live_index: Arc::new(FailingIndex),
            activation: Arc::new(RecordingActivation::default()),
            dispatch: dispatch.clone(),
            host_events: Arc::new(HostEventBus::new()),
        });

        let outcome = processor
            .process(LocalClipboardRequest {
                snapshot: empty_snapshot(),
                origin: ClipboardChangeOrigin::LocalCapture,
                intent: LocalClipboardIntent::ObservedHostChange {
                    dispatch: HostClipboardDispatch::AwaitReport,
                },
                source_started_at: None,
            })
            .await
            .unwrap();

        let LocalClipboardOutcome::Completed(completion) = outcome else {
            panic!("expected completed local clipboard action");
        };
        assert_eq!(completion.index, LocalClipboardIndexStatus::Failed);
        assert_eq!(dispatch.0.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn empty_capture_stops_before_side_effects() {
        let index = Arc::new(RecordingIndex::default());
        let activation = Arc::new(RecordingActivation::default());
        let dispatch = Arc::new(RecordingDispatch::default());
        let processor = LocalClipboardProcessor::new(LocalClipboardProcessorDeps {
            capture: Arc::new(EmptyCapture),
            live_index: index.clone(),
            activation: activation.clone(),
            dispatch: dispatch.clone(),
            host_events: Arc::new(HostEventBus::new()),
        });

        let outcome = processor
            .process(LocalClipboardRequest {
                snapshot: empty_snapshot(),
                origin: ClipboardChangeOrigin::LocalCapture,
                intent: LocalClipboardIntent::ObservedHostChange {
                    dispatch: HostClipboardDispatch::Background,
                },
                source_started_at: None,
            })
            .await
            .unwrap();

        assert!(matches!(outcome, LocalClipboardOutcome::Empty));
        assert_eq!(activation.0.load(Ordering::SeqCst), 0);
        assert_eq!(index.0.load(Ordering::SeqCst), 0);
        assert!(dispatch.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn capture_only_skips_dispatch_after_indexing() {
        let dispatch = Arc::new(RecordingDispatch::default());
        let processor = LocalClipboardProcessor::new(LocalClipboardProcessorDeps {
            capture: Arc::new(FixedCapture),
            live_index: Arc::new(RecordingIndex::default()),
            activation: Arc::new(RecordingActivation::default()),
            dispatch: dispatch.clone(),
            host_events: Arc::new(HostEventBus::new()),
        });

        let outcome = processor
            .process(LocalClipboardRequest {
                snapshot: empty_snapshot(),
                origin: ClipboardChangeOrigin::LocalCapture,
                intent: LocalClipboardIntent::ObservedHostChange {
                    dispatch: HostClipboardDispatch::CaptureOnly,
                },
                source_started_at: None,
            })
            .await
            .unwrap();

        let LocalClipboardOutcome::Completed(completion) = outcome else {
            panic!("expected completed capture-only action");
        };
        assert!(completion.dispatch.is_none());
        assert!(dispatch.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn dispatch_failure_preserves_typed_source() {
        let processor = LocalClipboardProcessor::new(LocalClipboardProcessorDeps {
            capture: Arc::new(FixedCapture),
            live_index: Arc::new(RecordingIndex::default()),
            activation: Arc::new(RecordingActivation::default()),
            dispatch: Arc::new(FailingDispatch),
            host_events: Arc::new(HostEventBus::new()),
        });

        let error = processor
            .process(LocalClipboardRequest {
                snapshot: empty_snapshot(),
                origin: ClipboardChangeOrigin::LocalCapture,
                intent: LocalClipboardIntent::ObservedHostChange {
                    dispatch: HostClipboardDispatch::Background,
                },
                source_started_at: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(error, LocalClipboardProcessError::Dispatch { .. }));
        assert!(std::error::Error::source(&error).is_some());
    }

    fn empty_snapshot() -> SystemClipboardSnapshot {
        SystemClipboardSnapshot {
            representations: Vec::new(),
            ts_ms: 0,
            file_content_digests: Vec::new(),
            file_set_v1_component: None,
        }
    }
}
