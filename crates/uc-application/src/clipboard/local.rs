//! 本机 Clipboard capture 的完整 Application 动作。
//!
//! 调用方只表达宿主观察或显式发送意图；capture、active register、
//! best-effort live index 与 dispatch 的顺序由本模块唯一负责。

use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use uc_core::clipboard::ActiveClipboardState;
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::clipboard::{ActiveClipboardRegisterError, LoadActiveClipboardPort};
use uc_core::{ClipboardChangeOrigin, SystemClipboardSnapshot};
use uc_observability_contract::diagnostics::connectivity::{
    LocalWorkObservation, LocalWorkOutcome, LocalWorkStep,
};

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
use uc_observability_contract::uc_warn;

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
    /// Point-in-time read of the cross-device active register. A local
    /// observation that resolves to the content already recorded here is the
    /// OS echo of our own programmatic write (or a redundant re-copy), not a
    /// new activation.
    pub active_register_load: Arc<dyn LoadActiveClipboardPort>,
    pub dispatch: Arc<dyn LocalClipboardDispatchPort>,
    pub host_events: Arc<HostEventBus>,
}

pub(crate) struct LocalClipboardProcessor {
    capture: Arc<dyn ClipboardCapturePort>,
    live_index: Arc<dyn ClipboardLiveIndexPort>,
    activation: Arc<dyn LocalClipboardActivationPort>,
    active_register_load: Arc<dyn LoadActiveClipboardPort>,
    dispatch: Arc<dyn LocalClipboardDispatchPort>,
    host_events: Arc<HostEventBus>,
}

impl LocalClipboardProcessor {
    pub(crate) fn new(deps: LocalClipboardProcessorDeps) -> Self {
        Self {
            capture: deps.capture,
            live_index: deps.live_index,
            activation: deps.activation,
            active_register_load: deps.active_register_load,
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

        let captured_entry_id = EntryId::from(entry_id.as_str());
        // An observed change that resolves to the content already recorded as
        // the active clipboard is not a new activation: it is either the OS
        // echo of our own programmatic write (remote push / restore) or a
        // redundant re-copy of what is already active. Re-advancing the
        // register and re-dispatching it would push the same bytes back to
        // every peer, which is exactly the A→B→A→B echo loop. Only a *new*
        // active value is dispatched; explicit sends stay unconditional.
        let already_active = self
            .is_already_active(&snapshot_hash, &captured_entry_id)
            .await;

        if matches!(intent, LocalClipboardIntent::ObservedHostChange { .. }) && !already_active {
            self.activation
                .advance_local(snapshot_hash.clone(), captured_entry_id)
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
            let observation = LocalWorkObservation::begin(LocalWorkStep::ClipboardLiveIndex);
            let result = self
                .live_index
                .index_capture(ClipboardLiveIndexInput {
                    entry_id: entry_id.clone(),
                    snapshot: Arc::clone(&shared_snapshot),
                })
                .await;
            observation.finish(if result.is_ok() {
                LocalWorkOutcome::Ok
            } else {
                LocalWorkOutcome::Error
            });
            match result {
                Ok(ClipboardLiveIndexOutcome::Indexed) => LocalClipboardIndexStatus::Indexed,
                Ok(ClipboardLiveIndexOutcome::Skipped { reason }) => {
                    LocalClipboardIndexStatus::Skipped { reason }
                }
                Err(ClipboardLiveIndexError::Internal(_)) => {
                    uc_warn!(
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
        ) && !(already_active
            && matches!(intent, LocalClipboardIntent::ObservedHostChange { .. }));
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

    /// Whether this observation already reflects the register's active content.
    ///
    /// Matches on the local entry handle and on the cross-device content hash.
    /// A load failure degrades to "not active" so an unreadable register can
    /// never suppress a genuine user copy.
    async fn is_already_active(&self, snapshot_hash: &str, entry_id: &EntryId) -> bool {
        match self.active_register_load.load().await {
            Ok(Some(state)) => is_same_active_content(&state, snapshot_hash, entry_id),
            Ok(None) => false,
            Err(error) => {
                uc_warn!(
                    error_kind = register_error_kind(&error),
                    "local clipboard: active register load failed; treating observation as a new activation"
                );
                false
            }
        }
    }
}

fn is_same_active_content(
    state: &ActiveClipboardState,
    snapshot_hash: &str,
    entry_id: &EntryId,
) -> bool {
    state.entry_id == *entry_id || state.snapshot_hash == snapshot_hash
}

fn register_error_kind(error: &ActiveClipboardRegisterError) -> &'static str {
    match error {
        ActiveClipboardRegisterError::NotUnlocked => "not_unlocked",
        ActiveClipboardRegisterError::Storage(_) => "storage",
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

    #[derive(Default)]
    struct FixedRegister {
        state: Option<ActiveClipboardState>,
        fail: bool,
    }

    #[async_trait]
    impl LoadActiveClipboardPort for FixedRegister {
        async fn load(&self) -> Result<Option<ActiveClipboardState>, ActiveClipboardRegisterError> {
            if self.fail {
                return Err(ActiveClipboardRegisterError::Storage(anyhow::anyhow!(
                    "register unavailable"
                )));
            }
            Ok(self.state.clone())
        }
    }

    fn empty_register() -> Arc<dyn LoadActiveClipboardPort> {
        Arc::new(FixedRegister::default())
    }

    fn register_at(snapshot_hash: &str, entry_id: &str) -> Arc<dyn LoadActiveClipboardPort> {
        Arc::new(FixedRegister {
            state: Some(ActiveClipboardState::new(
                snapshot_hash,
                EntryId::from(entry_id),
                1,
                DeviceId::new("remote-device"),
            )),
            fail: false,
        })
    }

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
            Err(ClipboardLiveIndexError::Internal(anyhow::anyhow!(
                "sensitive index detail"
            )))
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
            Err(ClipboardOutboundError::Internal(anyhow::anyhow!(
                "test failure"
            )))
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
            active_register_load: empty_register(),
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
            active_register_load: empty_register(),
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
            active_register_load: empty_register(),
            host_events: Arc::new(HostEventBus::new()),
        });

        let outcome = processor
            .process(LocalClipboardRequest {
                snapshot: empty_snapshot(),
                origin: ClipboardChangeOrigin::LocalCapture,
                intent: LocalClipboardIntent::ObservedHostChange {
                    dispatch: HostClipboardDispatch::Background,
                },
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
            active_register_load: empty_register(),
            host_events: Arc::new(HostEventBus::new()),
        });

        let outcome = processor
            .process(LocalClipboardRequest {
                snapshot: empty_snapshot(),
                origin: ClipboardChangeOrigin::LocalCapture,
                intent: LocalClipboardIntent::ObservedHostChange {
                    dispatch: HostClipboardDispatch::AwaitReport,
                },
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
            active_register_load: empty_register(),
            host_events: Arc::new(HostEventBus::new()),
        });

        let outcome = processor
            .process(LocalClipboardRequest {
                snapshot: empty_snapshot(),
                origin: ClipboardChangeOrigin::LocalCapture,
                intent: LocalClipboardIntent::ObservedHostChange {
                    dispatch: HostClipboardDispatch::Background,
                },
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
            active_register_load: empty_register(),
            host_events: Arc::new(HostEventBus::new()),
        });

        let outcome = processor
            .process(LocalClipboardRequest {
                snapshot: empty_snapshot(),
                origin: ClipboardChangeOrigin::LocalCapture,
                intent: LocalClipboardIntent::ObservedHostChange {
                    dispatch: HostClipboardDispatch::CaptureOnly,
                },
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
            active_register_load: empty_register(),
            host_events: Arc::new(HostEventBus::new()),
        });

        let error = processor
            .process(LocalClipboardRequest {
                snapshot: empty_snapshot(),
                origin: ClipboardChangeOrigin::LocalCapture,
                intent: LocalClipboardIntent::ObservedHostChange {
                    dispatch: HostClipboardDispatch::Background,
                },
            })
            .await
            .unwrap_err();

        assert!(matches!(error, LocalClipboardProcessError::Dispatch { .. }));
        assert!(std::error::Error::source(&error).is_some());
    }

    /// A local observation that resolves to the content already recorded as
    /// the active clipboard is the OS echo of our own write (remote push or
    /// restore). It must not re-advance the register, re-notify the UI, or
    /// re-dispatch — re-dispatching identical bytes is what sustains the
    /// A→B→A→B echo loop across paired devices.
    #[tokio::test]
    async fn echo_of_already_active_content_is_not_redispatched() {
        let index = Arc::new(RecordingIndex::default());
        let activation = Arc::new(RecordingActivation::default());
        let dispatch = Arc::new(RecordingDispatch::default());
        let events = Arc::new(RecordingHostEvents::default());
        let event_bus = Arc::new(HostEventBus::new());
        event_bus.register("local-test", events.clone());
        let processor = LocalClipboardProcessor::new(LocalClipboardProcessorDeps {
            capture: Arc::new(DeduplicatedCapture),
            live_index: index.clone(),
            activation: activation.clone(),
            active_register_load: register_at("blake3v1:existing", "entry-existing"),
            dispatch: dispatch.clone(),
            host_events: event_bus,
        });

        let outcome = processor
            .process(LocalClipboardRequest {
                snapshot: empty_snapshot(),
                origin: ClipboardChangeOrigin::LocalCapture,
                intent: LocalClipboardIntent::ObservedHostChange {
                    dispatch: HostClipboardDispatch::AwaitReport,
                },
            })
            .await
            .unwrap();

        let LocalClipboardOutcome::Completed(completion) = outcome else {
            panic!("expected completed local clipboard action");
        };
        assert!(completion.dispatch.is_none());
        assert_eq!(dispatch.0.lock().unwrap().len(), 0);
        assert_eq!(activation.0.load(Ordering::SeqCst), 0);
        assert_eq!(events.0.lock().unwrap().len(), 0);
        assert_eq!(index.0.load(Ordering::SeqCst), 0);
    }

    /// A re-copy of content that is *not* the current active clipboard is a
    /// real re-activation: it still advances the register and re-dispatches so
    /// peers bump it back to the top.
    #[tokio::test]
    async fn deduplicated_capture_of_inactive_content_still_dispatches() {
        let activation = Arc::new(RecordingActivation::default());
        let dispatch = Arc::new(RecordingDispatch::default());
        let processor = LocalClipboardProcessor::new(LocalClipboardProcessorDeps {
            capture: Arc::new(DeduplicatedCapture),
            live_index: Arc::new(RecordingIndex::default()),
            activation: activation.clone(),
            active_register_load: register_at("blake3v1:other", "entry-other"),
            dispatch: dispatch.clone(),
            host_events: Arc::new(HostEventBus::new()),
        });

        processor
            .process(LocalClipboardRequest {
                snapshot: empty_snapshot(),
                origin: ClipboardChangeOrigin::LocalCapture,
                intent: LocalClipboardIntent::ObservedHostChange {
                    dispatch: HostClipboardDispatch::AwaitReport,
                },
            })
            .await
            .unwrap();

        assert_eq!(activation.0.load(Ordering::SeqCst), 1);
        assert_eq!(dispatch.0.lock().unwrap().len(), 1);
    }

    /// Explicit sends are an intentional user action and must still reach the
    /// targets even when the content is already the active clipboard.
    #[tokio::test]
    async fn explicit_send_ignores_already_active_state() {
        let dispatch = Arc::new(RecordingDispatch::default());
        let processor = LocalClipboardProcessor::new(LocalClipboardProcessorDeps {
            capture: Arc::new(DeduplicatedCapture),
            live_index: Arc::new(RecordingIndex::default()),
            activation: Arc::new(RecordingActivation::default()),
            active_register_load: register_at("blake3v1:existing", "entry-existing"),
            dispatch: dispatch.clone(),
            host_events: Arc::new(HostEventBus::new()),
        });

        processor
            .process(LocalClipboardRequest {
                snapshot: empty_snapshot(),
                origin: ClipboardChangeOrigin::LocalCapture,
                intent: LocalClipboardIntent::ExplicitSend {
                    targets: vec![DeviceId::new("peer-a")],
                },
            })
            .await
            .unwrap();

        assert_eq!(dispatch.0.lock().unwrap().len(), 1);
    }

    /// An unreadable register must never suppress a genuine local copy.
    #[tokio::test]
    async fn register_load_failure_still_dispatches_local_copy() {
        let dispatch = Arc::new(RecordingDispatch::default());
        let processor = LocalClipboardProcessor::new(LocalClipboardProcessorDeps {
            capture: Arc::new(DeduplicatedCapture),
            live_index: Arc::new(RecordingIndex::default()),
            activation: Arc::new(RecordingActivation::default()),
            active_register_load: Arc::new(FixedRegister {
                state: None,
                fail: true,
            }),
            dispatch: dispatch.clone(),
            host_events: Arc::new(HostEventBus::new()),
        });

        processor
            .process(LocalClipboardRequest {
                snapshot: empty_snapshot(),
                origin: ClipboardChangeOrigin::LocalCapture,
                intent: LocalClipboardIntent::ObservedHostChange {
                    dispatch: HostClipboardDispatch::AwaitReport,
                },
            })
            .await
            .unwrap();

        assert_eq!(dispatch.0.lock().unwrap().len(), 1);
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
