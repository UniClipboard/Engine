use std::sync::Arc;

use bytes::Bytes;
use thiserror::Error;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

use uc_core::clipboard::ClipboardContentCategorySet;
use uc_core::ids::DeviceId;
use uc_core::ports::security::TransferCipherPort;
use uc_core::ports::{
    ClipboardReceiverPort, ClockPort, InboundClipboard, InboundClipboardDisposition,
    InboundClipboardReceipt,
};
use uc_core::MemberRepositoryPort;
use uc_observability_contract::FlowId;

use crate::clipboard_write::ClipboardWriteIntent;
use crate::usecases::clipboard_sync::decode_v3_bytes_to_snapshot;
use crate::usecases::clipboard_sync::receive_gate::MemberReceiveGate;

use super::{InboundClipboardApplyInput, InboundClipboardApplyOutcome, InboundClipboardApplyPort};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardInboundEventAction {
    NewEntry,
    DuplicateIgnored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardInboundRepresentationSummary {
    pub mime_type: Option<String>,
    pub size_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardInboundEvent {
    pub from_device: DeviceId,
    pub snapshot_hash: String,
    pub text_preview: Option<String>,
    pub representations: Vec<ClipboardInboundRepresentationSummary>,
    pub action: ClipboardInboundEventAction,
    pub disposition: InboundClipboardDisposition,
    pub at_ms: i64,
}

pub trait ClipboardInboundEventPort: Send + Sync {
    fn emit(&self, event: ClipboardInboundEvent);
}

pub struct ClipboardInboundRuntimeDeps {
    pub receiver: Arc<dyn ClipboardReceiverPort>,
    pub member_repo: Arc<dyn MemberRepositoryPort>,
    pub transfer_cipher: Arc<dyn TransferCipherPort>,
    pub clock: Arc<dyn ClockPort>,
    pub apply: Arc<dyn InboundClipboardApplyPort>,
    pub events: Arc<dyn ClipboardInboundEventPort>,
}

#[derive(Debug, Error)]
pub enum ClipboardInboundRuntimeError {
    #[error("clipboard inbound task failed: {0}")]
    Task(String),
}

pub struct ClipboardInboundRuntime {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

struct InboundProcessor {
    receive_gate: MemberReceiveGate,
    transfer_cipher: Arc<dyn TransferCipherPort>,
    clock: Arc<dyn ClockPort>,
    apply: Arc<dyn InboundClipboardApplyPort>,
    events: Arc<dyn ClipboardInboundEventPort>,
}

struct PreparedInbound {
    from_device: DeviceId,
    snapshot_hash: String,
    plaintext: Bytes,
    flow_id: Option<FlowId>,
    at_ms: i64,
    receipt: InboundClipboardReceipt,
}

impl ClipboardInboundRuntime {
    pub fn start(deps: ClipboardInboundRuntimeDeps) -> Self {
        let mut receiver = deps.receiver.subscribe();
        let processor = InboundProcessor {
            receive_gate: MemberReceiveGate::new(deps.member_repo),
            transfer_cipher: deps.transfer_cipher,
            clock: deps.clock,
            apply: deps.apply,
            events: deps.events,
        };
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => return,
                    inbound = receiver.recv() => match inbound {
                        Ok(inbound) => processor.handle_one(inbound).await,
                        Err(broadcast::error::RecvError::Lagged(missed)) => {
                            warn!(missed, "clipboard inbound receiver lagged; dropped frames");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("clipboard inbound receiver closed; exiting runtime");
                            return;
                        }
                    }
                }
            }
        });
        Self {
            cancel,
            task: Some(task),
        }
    }

    pub async fn shutdown(mut self) -> Result<(), ClipboardInboundRuntimeError> {
        self.cancel.cancel();
        let Some(task) = self.task.take() else {
            return Ok(());
        };
        task.await
            .map_err(|error| ClipboardInboundRuntimeError::Task(error.to_string()))
    }
}

impl Drop for ClipboardInboundRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl InboundProcessor {
    #[instrument(
        skip_all,
        fields(
            peer.device_id = %inbound.peer_device_id.as_str(),
            snapshot_hash = %inbound.header.snapshot_hash,
            flow.id = tracing::field::Empty,
            flow.kind = "clipboard_sync",
            flow.synthetic = tracing::field::Empty,
        ),
    )]
    async fn handle_one(&self, inbound: InboundClipboard) {
        let Some(prepared) = self.prepare(inbound).await else {
            return;
        };
        let (text_preview, representations) = summarize_plaintext(&prepared.plaintext);
        let result = self
            .apply
            .apply(InboundClipboardApplyInput {
                from_device: prepared.from_device.as_str().to_owned(),
                snapshot_hash: prepared.snapshot_hash.clone(),
                plaintext: prepared.plaintext,
                flow_id: prepared.flow_id,
                resurface_intent: ClipboardWriteIntent::RemotePush,
            })
            .await;
        let (action, disposition) = match &result {
            Ok(InboundClipboardApplyOutcome::Applied { entry_id }) => {
                info!(entry_id = %entry_id, "inbound clipboard applied");
                (
                    ClipboardInboundEventAction::NewEntry,
                    InboundClipboardDisposition::Applied,
                )
            }
            Ok(InboundClipboardApplyOutcome::Resurfaced {
                existing_entry_id,
                os_write_succeeded,
                ..
            }) => {
                debug!(
                    entry_id = %existing_entry_id,
                    os_write_succeeded,
                    "inbound clipboard resurfaced"
                );
                (
                    ClipboardInboundEventAction::NewEntry,
                    InboundClipboardDisposition::Applied,
                )
            }
            Ok(InboundClipboardApplyOutcome::DuplicateSkipped { .. }) => {
                debug!("inbound clipboard duplicate skipped");
                (
                    ClipboardInboundEventAction::DuplicateIgnored,
                    InboundClipboardDisposition::Duplicate,
                )
            }
            Ok(InboundClipboardApplyOutcome::DecodeFailed { .. }) => {
                debug!("inbound clipboard decode failed");
                (
                    ClipboardInboundEventAction::NewEntry,
                    InboundClipboardDisposition::Rejected,
                )
            }
            Err(_) => {
                warn!(
                    error_kind = "inbound_clipboard_apply_failed",
                    "inbound clipboard apply failed"
                );
                (
                    ClipboardInboundEventAction::NewEntry,
                    InboundClipboardDisposition::Rejected,
                )
            }
        };
        self.events.emit(ClipboardInboundEvent {
            from_device: prepared.from_device,
            snapshot_hash: prepared.snapshot_hash,
            text_preview,
            representations,
            action,
            disposition,
            at_ms: prepared.at_ms,
        });
        prepared.receipt.finish(disposition);
    }

    async fn prepare(&self, inbound: InboundClipboard) -> Option<PreparedInbound> {
        let receipt = inbound.receipt.clone();
        let flow_id = record_flow_id(inbound.header.flow_id.as_deref());
        if !self
            .receive_gate
            .is_receive_allowed(&inbound.peer_device_id)
            .await
        {
            receipt.finish(InboundClipboardDisposition::Rejected);
            return None;
        }
        let plaintext = match self.transfer_cipher.decrypt(&inbound.ciphertext).await {
            Ok(bytes) => Bytes::from(bytes),
            Err(_) => {
                warn!(
                    peer = %inbound.peer_device_id.as_str(),
                    snapshot_hash = %inbound.header.snapshot_hash,
                    error_kind = "inbound_clipboard_decrypt_failed",
                    "inbound clipboard decrypt failed"
                );
                receipt.finish(InboundClipboardDisposition::Rejected);
                return None;
            }
        };
        let categories = match decode_v3_bytes_to_snapshot(plaintext.as_ref()) {
            Ok(snapshot) => ClipboardContentCategorySet::from_snapshot(&snapshot),
            Err(_) => {
                warn!(
                    peer = %inbound.peer_device_id.as_str(),
                    snapshot_hash = %inbound.header.snapshot_hash,
                    error_kind = "inbound_clipboard_classification_failed",
                    "inbound clipboard classification failed open"
                );
                ClipboardContentCategorySet::empty()
            }
        };
        if !self
            .receive_gate
            .is_receive_category_allowed(&inbound.peer_device_id, &categories)
            .await
        {
            receipt.finish(InboundClipboardDisposition::Rejected);
            return None;
        }
        Some(PreparedInbound {
            from_device: inbound.peer_device_id,
            snapshot_hash: inbound.header.snapshot_hash,
            plaintext,
            flow_id,
            at_ms: self.clock.now_ms(),
            receipt,
        })
    }
}

fn record_flow_id(wire_flow_id: Option<&str>) -> Option<FlowId> {
    match wire_flow_id {
        Some(wire_id) => match FlowId::parse_str(wire_id) {
            Ok(flow_id) => {
                tracing::Span::current().record("flow.id", tracing::field::display(&flow_id));
                Some(flow_id)
            }
            Err(_) => {
                let synthetic = FlowId::generate();
                tracing::Span::current().record("flow.id", tracing::field::display(&synthetic));
                tracing::Span::current().record("flow.synthetic", true);
                warn!(
                    error_kind = "invalid_inbound_flow_id",
                    "inbound clipboard flow id was invalid; using a synthetic trace id"
                );
                None
            }
        },
        None => {
            let synthetic = FlowId::generate();
            tracing::Span::current().record("flow.id", tracing::field::display(&synthetic));
            tracing::Span::current().record("flow.synthetic", true);
            None
        }
    }
}

fn summarize_plaintext(
    plaintext: &[u8],
) -> (Option<String>, Vec<ClipboardInboundRepresentationSummary>) {
    let Ok(snapshot) = decode_v3_bytes_to_snapshot(plaintext) else {
        return (None, Vec::new());
    };
    let text_preview = snapshot.representations.iter().find_map(|representation| {
        let mime = representation.mime.as_ref()?.as_str();
        let is_text = mime.eq_ignore_ascii_case("text/plain")
            || mime.eq_ignore_ascii_case("public.utf8-plain-text")
            || mime.to_ascii_lowercase().starts_with("text/");
        if !is_text {
            return None;
        }
        let text = std::str::from_utf8(representation.inline_bytes()?).ok()?;
        Some(text.chars().take(200).collect())
    });
    let representations = snapshot
        .representations
        .into_iter()
        .map(|representation| {
            let size_bytes = representation.size_bytes();
            ClipboardInboundRepresentationSummary {
                mime_type: representation.mime.map(|mime| mime.0),
                size_bytes,
            }
        })
        .collect();
    (text_preview, representations)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use bytes::Bytes;
    use tokio::sync::{broadcast, Notify};

    use uc_core::ids::{DeviceId, FormatId, RepresentationId};
    use uc_core::ports::security::{TransferCipherError, TransferCipherPort};
    use uc_core::ports::{
        ClipboardHeader, ClipboardReceiverPort, ClockPort, InboundClipboard,
        InboundClipboardDisposition, InboundClipboardReceipt, InboundClipboardResult,
    };
    use uc_core::security::IdentityFingerprint;
    use uc_core::{
        MemberRepositoryPort, MemberSyncPreferences, MembershipError, MimeType,
        ObservedClipboardRepresentation, SpaceMember, SystemClipboardSnapshot,
    };

    use super::*;
    use crate::facade::{
        InboundClipboardApplyError, InboundClipboardApplyInput, InboundClipboardApplyOutcome,
        InboundClipboardApplyPort,
    };
    use crate::usecases::clipboard_sync::encode_snapshot_to_v3_bytes;

    struct FakeReceiver {
        tx: broadcast::Sender<InboundClipboard>,
    }

    impl FakeReceiver {
        fn new() -> Self {
            let (tx, _) = broadcast::channel(16);
            Self { tx }
        }

        fn publish(&self, inbound: InboundClipboard) {
            self.tx.send(inbound).expect("runtime subscribed");
        }
    }

    #[async_trait]
    impl ClipboardReceiverPort for FakeReceiver {
        fn subscribe(&self) -> broadcast::Receiver<InboundClipboard> {
            self.tx.subscribe()
        }
    }

    struct AllowAllMembers;

    #[async_trait]
    impl MemberRepositoryPort for AllowAllMembers {
        async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            Ok(Some(SpaceMember {
                device_id: device_id.clone(),
                device_name: "Test peer".to_owned(),
                identity_fingerprint: IdentityFingerprint::from_raw_string("AAAABBBBCCCCDDDD")
                    .expect("valid fingerprint"),
                joined_at: chrono::Utc::now(),
                sync_preferences: MemberSyncPreferences::default(),
            }))
        }

        async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
            Ok(Vec::new())
        }

        async fn save(&self, _member: &SpaceMember) -> Result<(), MembershipError> {
            Ok(())
        }

        async fn remove(&self, _device_id: &DeviceId) -> Result<bool, MembershipError> {
            Ok(false)
        }
    }

    struct EchoCipher;

    #[async_trait]
    impl TransferCipherPort for EchoCipher {
        async fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, TransferCipherError> {
            Ok(plaintext.to_vec())
        }

        async fn decrypt(&self, encrypted: &[u8]) -> Result<Vec<u8>, TransferCipherError> {
            Ok(encrypted.to_vec())
        }
    }

    struct NeverCipher;

    #[async_trait]
    impl TransferCipherPort for NeverCipher {
        async fn encrypt(&self, _plaintext: &[u8]) -> Result<Vec<u8>, TransferCipherError> {
            panic!("receive policy must reject before encryption")
        }

        async fn decrypt(&self, _encrypted: &[u8]) -> Result<Vec<u8>, TransferCipherError> {
            panic!("receive policy must reject before decryption")
        }
    }

    struct QueueCipher {
        decrypt_calls: AtomicUsize,
        outcomes: Mutex<VecDeque<Result<Vec<u8>, TransferCipherError>>>,
    }

    #[async_trait]
    impl TransferCipherPort for QueueCipher {
        async fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, TransferCipherError> {
            Ok(plaintext.to_vec())
        }

        async fn decrypt(&self, _encrypted: &[u8]) -> Result<Vec<u8>, TransferCipherError> {
            self.decrypt_calls.fetch_add(1, Ordering::SeqCst);
            self.outcomes
                .lock()
                .expect("cipher outcome queue")
                .pop_front()
                .expect("one cipher outcome per inbound")
        }
    }

    struct FixedClock;

    impl ClockPort for FixedClock {
        fn now_ms(&self) -> i64 {
            42
        }
    }

    struct QueueApply {
        outcomes: Mutex<VecDeque<Result<InboundClipboardApplyOutcome, InboundClipboardApplyError>>>,
    }

    #[async_trait]
    impl InboundClipboardApplyPort for QueueApply {
        async fn apply(
            &self,
            _input: InboundClipboardApplyInput,
        ) -> Result<InboundClipboardApplyOutcome, InboundClipboardApplyError> {
            self.outcomes
                .lock()
                .expect("outcome queue")
                .pop_front()
                .expect("one outcome per inbound")
        }
    }

    #[derive(Default)]
    struct RecordingEvents {
        events: Mutex<Vec<ClipboardInboundEvent>>,
    }

    impl ClipboardInboundEventPort for RecordingEvents {
        fn emit(&self, event: ClipboardInboundEvent) {
            self.events.lock().expect("event recorder").push(event);
        }
    }

    struct BlockingApply {
        started: Arc<Notify>,
        release: Arc<Notify>,
        calls: Arc<AtomicUsize>,
    }

    struct NeverApply;

    #[async_trait]
    impl InboundClipboardApplyPort for NeverApply {
        async fn apply(
            &self,
            _input: InboundClipboardApplyInput,
        ) -> Result<InboundClipboardApplyOutcome, InboundClipboardApplyError> {
            panic!("receive policy must reject before inbound apply")
        }
    }

    enum MemberLookup {
        Found(MemberSyncPreferences),
        Missing,
        Failed,
    }

    struct ConfigurableMembers {
        lookup: MemberLookup,
    }

    #[async_trait]
    impl MemberRepositoryPort for ConfigurableMembers {
        async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            match &self.lookup {
                MemberLookup::Found(preferences) => Ok(Some(SpaceMember {
                    device_id: device_id.clone(),
                    device_name: "Test peer".to_owned(),
                    identity_fingerprint: IdentityFingerprint::from_raw_string("AAAABBBBCCCCDDDD")
                        .expect("valid fingerprint"),
                    joined_at: chrono::Utc::now(),
                    sync_preferences: preferences.clone(),
                })),
                MemberLookup::Missing => Ok(None),
                MemberLookup::Failed => Err(MembershipError::Repository("test failure".to_owned())),
            }
        }

        async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
            Ok(Vec::new())
        }

        async fn save(&self, _member: &SpaceMember) -> Result<(), MembershipError> {
            Ok(())
        }

        async fn remove(&self, _device_id: &DeviceId) -> Result<bool, MembershipError> {
            Ok(false)
        }
    }

    #[async_trait]
    impl InboundClipboardApplyPort for BlockingApply {
        async fn apply(
            &self,
            _input: InboundClipboardApplyInput,
        ) -> Result<InboundClipboardApplyOutcome, InboundClipboardApplyError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            self.release.notified().await;
            Ok(InboundClipboardApplyOutcome::Applied {
                entry_id: "entry-after-release".to_owned(),
            })
        }
    }

    fn fixture(peer: &str, snapshot_hash: &str) -> (InboundClipboard, InboundClipboardResult) {
        fixture_with_ciphertext(
            peer,
            snapshot_hash,
            Bytes::from_static(b"not-a-v3-envelope"),
        )
    }

    fn fixture_with_ciphertext(
        peer: &str,
        snapshot_hash: &str,
        ciphertext: Bytes,
    ) -> (InboundClipboard, InboundClipboardResult) {
        let (receipt, result) = InboundClipboardReceipt::pending();
        (
            InboundClipboard {
                peer_device_id: DeviceId::new(peer),
                header: ClipboardHeader {
                    version: ClipboardHeader::CURRENT_VERSION,
                    snapshot_hash: snapshot_hash.to_owned(),
                    captured_at_ms: 1,
                    origin_device_id: peer.to_owned(),
                    origin_device_name: "Peer".to_owned(),
                    payload_version: 3,
                    flow_id: None,
                },
                ciphertext,
                receipt,
            },
            result,
        )
    }

    fn text_fixture(peer: &str) -> (InboundClipboard, InboundClipboardResult) {
        let snapshot = SystemClipboardSnapshot {
            ts_ms: 1,
            representations: vec![ObservedClipboardRepresentation::new(
                RepresentationId::new(),
                FormatId::from("text"),
                Some(MimeType("text/plain".to_owned())),
                b"private text".to_vec(),
            )],
            file_content_digests: Vec::new(),
            file_set_v1_component: None,
        };
        let (plaintext, snapshot_hash) =
            encode_snapshot_to_v3_bytes(&snapshot).expect("encode text envelope");
        fixture_with_ciphertext(peer, &snapshot_hash, plaintext)
    }

    fn deps(
        receiver: Arc<FakeReceiver>,
        apply: Arc<dyn InboundClipboardApplyPort>,
        events: Arc<dyn ClipboardInboundEventPort>,
    ) -> ClipboardInboundRuntimeDeps {
        ClipboardInboundRuntimeDeps {
            receiver,
            member_repo: Arc::new(AllowAllMembers),
            transfer_cipher: Arc::new(EchoCipher),
            clock: Arc::new(FixedClock),
            apply,
            events,
        }
    }

    fn deps_with_policy(
        receiver: Arc<FakeReceiver>,
        member_repo: Arc<dyn MemberRepositoryPort>,
        transfer_cipher: Arc<dyn TransferCipherPort>,
        apply: Arc<dyn InboundClipboardApplyPort>,
        events: Arc<dyn ClipboardInboundEventPort>,
    ) -> ClipboardInboundRuntimeDeps {
        ClipboardInboundRuntimeDeps {
            receiver,
            member_repo,
            transfer_cipher,
            clock: Arc::new(FixedClock),
            apply,
            events,
        }
    }

    #[test]
    fn runtime_has_one_complete_start_entry() {
        let _: fn(ClipboardInboundRuntimeDeps) -> ClipboardInboundRuntime =
            ClipboardInboundRuntime::start;
    }

    #[tokio::test]
    async fn runtime_settles_every_apply_result_and_emits_from_that_result() {
        let receiver = Arc::new(FakeReceiver::new());
        let events = Arc::new(RecordingEvents::default());
        let apply = Arc::new(QueueApply {
            outcomes: Mutex::new(VecDeque::from([
                Ok(InboundClipboardApplyOutcome::Applied {
                    entry_id: "entry-a".to_owned(),
                }),
                Ok(InboundClipboardApplyOutcome::DuplicateSkipped {
                    snapshot_hash: "hash-b".to_owned(),
                    existing_entry_id: "entry-a".to_owned(),
                }),
                Ok(InboundClipboardApplyOutcome::DecodeFailed {
                    reason: "invalid envelope".to_owned(),
                }),
                Err(InboundClipboardApplyError::Internal(
                    "storage unavailable".to_owned(),
                )),
            ])),
        });
        let runtime =
            ClipboardInboundRuntime::start(deps(Arc::clone(&receiver), apply, events.clone()));

        let mut results = Vec::new();
        for suffix in ["a", "b", "c", "d"] {
            let (inbound, result) = fixture("peer-1", &format!("hash-{suffix}"));
            receiver.publish(inbound);
            results.push(
                tokio::time::timeout(Duration::from_secs(1), result.wait())
                    .await
                    .expect("receipt settled"),
            );
        }

        assert_eq!(
            results,
            vec![
                Some(InboundClipboardDisposition::Applied),
                Some(InboundClipboardDisposition::Duplicate),
                Some(InboundClipboardDisposition::Rejected),
                Some(InboundClipboardDisposition::Rejected),
            ]
        );
        let emitted = events.events.lock().expect("event recorder").clone();
        assert_eq!(emitted.len(), 4);
        assert_eq!(emitted[0].action, ClipboardInboundEventAction::NewEntry);
        assert_eq!(emitted[0].disposition, InboundClipboardDisposition::Applied);
        assert_eq!(
            emitted[1].action,
            ClipboardInboundEventAction::DuplicateIgnored
        );
        assert_eq!(
            emitted[1].disposition,
            InboundClipboardDisposition::Duplicate
        );
        assert_eq!(
            emitted[2].disposition,
            InboundClipboardDisposition::Rejected
        );
        assert_eq!(
            emitted[3].disposition,
            InboundClipboardDisposition::Rejected
        );

        runtime.shutdown().await.expect("runtime shutdown");
    }

    #[tokio::test]
    async fn shutdown_waits_for_the_active_inbound_to_reach_a_receipt() {
        let receiver = Arc::new(FakeReceiver::new());
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = ClipboardInboundRuntime::start(deps(
            Arc::clone(&receiver),
            Arc::new(BlockingApply {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                calls,
            }),
            Arc::new(RecordingEvents::default()),
        ));
        let (inbound, result) = fixture("peer-1", "hash-a");
        receiver.publish(inbound);
        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("apply started");

        let mut shutdown = tokio::spawn(async move { runtime.shutdown().await });
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut shutdown)
                .await
                .is_err(),
            "shutdown returned before the active inbound settled"
        );

        release.notify_one();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), result.wait())
                .await
                .expect("receipt settled"),
            Some(InboundClipboardDisposition::Applied)
        );
        tokio::time::timeout(Duration::from_secs(1), shutdown)
            .await
            .expect("shutdown completed")
            .expect("shutdown task")
            .expect("runtime shutdown");
    }

    #[tokio::test]
    async fn shutdown_does_not_start_an_inbound_that_is_still_queued() {
        let receiver = Arc::new(FakeReceiver::new());
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = ClipboardInboundRuntime::start(deps(
            Arc::clone(&receiver),
            Arc::new(BlockingApply {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                calls: Arc::clone(&calls),
            }),
            Arc::new(RecordingEvents::default()),
        ));
        let (active_inbound, active_result) = fixture("peer-1", "hash-active");
        receiver.publish(active_inbound);
        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("first apply started");
        let (queued_inbound, queued_result) = fixture("peer-1", "hash-queued");
        receiver.publish(queued_inbound);

        let shutdown = tokio::spawn(async move { runtime.shutdown().await });
        release.notify_one();

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), active_result.wait())
                .await
                .expect("active receipt settled"),
            Some(InboundClipboardDisposition::Applied)
        );
        tokio::time::timeout(Duration::from_secs(1), shutdown)
            .await
            .expect("shutdown completed")
            .expect("shutdown task")
            .expect("runtime shutdown");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), queued_result.wait())
                .await
                .expect("queued receipt dropped"),
            None
        );
    }

    #[tokio::test]
    async fn receive_disabled_rejects_before_decrypt_or_apply() {
        let receiver = Arc::new(FakeReceiver::new());
        let mut preferences = MemberSyncPreferences::default();
        preferences.receive_enabled = false;
        let runtime = ClipboardInboundRuntime::start(deps_with_policy(
            Arc::clone(&receiver),
            Arc::new(ConfigurableMembers {
                lookup: MemberLookup::Found(preferences),
            }),
            Arc::new(NeverCipher),
            Arc::new(NeverApply),
            Arc::new(RecordingEvents::default()),
        ));
        let (inbound, result) = fixture("peer-disabled", "hash-disabled");

        receiver.publish(inbound);

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), result.wait())
                .await
                .expect("receipt settled"),
            Some(InboundClipboardDisposition::Rejected)
        );
        runtime.shutdown().await.expect("runtime shutdown");
    }

    #[tokio::test]
    async fn unavailable_member_preferences_reject_before_decrypt_or_apply() {
        for lookup in [MemberLookup::Missing, MemberLookup::Failed] {
            let receiver = Arc::new(FakeReceiver::new());
            let runtime = ClipboardInboundRuntime::start(deps_with_policy(
                Arc::clone(&receiver),
                Arc::new(ConfigurableMembers { lookup }),
                Arc::new(NeverCipher),
                Arc::new(NeverApply),
                Arc::new(RecordingEvents::default()),
            ));
            let (inbound, result) = fixture("peer-unavailable", "hash-unavailable");

            receiver.publish(inbound);

            assert_eq!(
                tokio::time::timeout(Duration::from_secs(1), result.wait())
                    .await
                    .expect("receipt settled"),
                Some(InboundClipboardDisposition::Rejected)
            );
            runtime.shutdown().await.expect("runtime shutdown");
        }
    }

    #[tokio::test]
    async fn decrypt_failure_rejects_one_inbound_and_continues_with_the_next() {
        let receiver = Arc::new(FakeReceiver::new());
        let cipher = Arc::new(QueueCipher {
            decrypt_calls: AtomicUsize::new(0),
            outcomes: Mutex::new(VecDeque::from([
                Err(TransferCipherError::DecryptionFailed),
                Ok(b"not-a-v3-envelope".to_vec()),
            ])),
        });
        let apply = Arc::new(QueueApply {
            outcomes: Mutex::new(VecDeque::from([Ok(
                InboundClipboardApplyOutcome::Applied {
                    entry_id: "entry-after-decrypt-failure".to_owned(),
                },
            )])),
        });
        let runtime = ClipboardInboundRuntime::start(deps_with_policy(
            Arc::clone(&receiver),
            Arc::new(AllowAllMembers),
            cipher.clone(),
            apply,
            Arc::new(RecordingEvents::default()),
        ));
        let (failed_inbound, failed_result) = fixture("peer-1", "hash-failed");
        let (next_inbound, next_result) = fixture("peer-1", "hash-next");

        receiver.publish(failed_inbound);
        receiver.publish(next_inbound);

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), failed_result.wait())
                .await
                .expect("failed receipt settled"),
            Some(InboundClipboardDisposition::Rejected)
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), next_result.wait())
                .await
                .expect("next receipt settled"),
            Some(InboundClipboardDisposition::Applied)
        );
        assert_eq!(cipher.decrypt_calls.load(Ordering::SeqCst), 2);
        runtime.shutdown().await.expect("runtime shutdown");
    }

    #[tokio::test]
    async fn disabled_text_category_rejects_after_decrypt_and_before_apply() {
        let receiver = Arc::new(FakeReceiver::new());
        let mut preferences = MemberSyncPreferences::default();
        preferences.receive_content_types.text = false;
        let runtime = ClipboardInboundRuntime::start(deps_with_policy(
            Arc::clone(&receiver),
            Arc::new(ConfigurableMembers {
                lookup: MemberLookup::Found(preferences),
            }),
            Arc::new(EchoCipher),
            Arc::new(NeverApply),
            Arc::new(RecordingEvents::default()),
        ));
        let (inbound, result) = text_fixture("peer-text-disabled");

        receiver.publish(inbound);

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), result.wait())
                .await
                .expect("receipt settled"),
            Some(InboundClipboardDisposition::Rejected)
        );
        runtime.shutdown().await.expect("runtime shutdown");
    }
}
