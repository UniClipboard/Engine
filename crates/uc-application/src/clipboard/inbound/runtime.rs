use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use thiserror::Error;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

use uc_core::clipboard::{ClipboardContentCategory, ClipboardContentCategorySet};
use uc_core::ids::DeviceId;
use uc_core::ports::security::TransferCipherPort;
use uc_core::ports::{
    ClipboardReceiverPort, ClockPort, ConnectionChannel, InboundClipboard,
    InboundClipboardDisposition, InboundClipboardReceipt, SettingsPort,
};
use uc_core::MemberRepositoryPort;
use uc_observability_contract::analytics::{
    Direction, PayloadSizeBucket, PayloadType, TransportType,
};
use uc_observability_contract::otlp::{
    log_clipboard_sync_stage, ClipboardSyncStage, ClipboardSyncTiming,
};
use uc_observability_contract::FlowId;

use crate::clipboard::sync::decode_v3_bytes_to_snapshot;
use crate::clipboard::sync::receive_gate::MemberReceiveGate;
use crate::clipboard::write::ClipboardWriteIntent;
use crate::deps::CurrentSpaceMemberScopePort;

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
    pub member_scope: Arc<dyn CurrentSpaceMemberScopePort>,
    pub transfer_cipher: Arc<dyn TransferCipherPort>,
    pub settings: Arc<dyn SettingsPort>,
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
    settings: Arc<dyn SettingsPort>,
    transfer_cipher: Arc<dyn TransferCipherPort>,
    clock: Arc<dyn ClockPort>,
    apply: Arc<dyn InboundClipboardApplyPort>,
    events: Arc<dyn ClipboardInboundEventPort>,
}

struct InboundFlow {
    application_flow_id: Option<FlowId>,
    log_flow_id: FlowId,
    synthetic: bool,
}

struct PreparedInbound {
    from_device: DeviceId,
    snapshot_hash: String,
    plaintext: Bytes,
    flow: InboundFlow,
    at_ms: i64,
    transport: ConnectionChannel,
    payload_type: PayloadType,
    payload_size_bucket: PayloadSizeBucket,
    timing: InboundTiming,
    receipt: InboundClipboardReceipt,
}

#[derive(Debug, Default)]
struct InboundTiming {
    receiver_queue_ms: u32,
    receiver_policy_ms: u32,
    receiver_decrypt_ms: u32,
    receiver_preflight_decode_ms: u32,
}

impl ClipboardInboundRuntime {
    pub fn start(deps: ClipboardInboundRuntimeDeps) -> Self {
        let mut receiver = deps.receiver.subscribe();
        let processor = InboundProcessor {
            receive_gate: MemberReceiveGate::new(deps.member_repo, deps.member_scope),
            settings: deps.settings,
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
            snapshot_hash = %inbound.header.snapshot_hash,
            flow.id = tracing::field::Empty,
            flow.kind = "clipboard_sync",
            flow.synthetic = tracing::field::Empty,
        ),
    )]
    async fn handle_one(&self, inbound: InboundClipboard) {
        let processing_started_at = Instant::now();
        let Some(mut prepared) = self.prepare(inbound, processing_started_at).await else {
            return;
        };
        let receiver_preflight_decode_started_at = Instant::now();
        let (text_preview, representations) = summarize_plaintext(&prepared.plaintext);
        prepared.timing.receiver_preflight_decode_ms = prepared
            .timing
            .receiver_preflight_decode_ms
            .saturating_add(duration_ms(receiver_preflight_decode_started_at.elapsed()));
        let receiver_apply_started_at = Instant::now();
        let result = self
            .apply
            .apply(InboundClipboardApplyInput {
                from_device: prepared.from_device.as_str().to_owned(),
                snapshot_hash: prepared.snapshot_hash.clone(),
                plaintext: prepared.plaintext.clone(),
                flow_id: prepared.flow.application_flow_id.clone(),
                provisional: None,
                resurface_intent: ClipboardWriteIntent::RemotePush,
            })
            .await;
        let receiver_apply_ms = duration_ms(receiver_apply_started_at.elapsed());
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
        if matches!(
            disposition,
            InboundClipboardDisposition::Applied | InboundClipboardDisposition::Duplicate
        ) {
            self.capture_completed_stages(&prepared, receiver_apply_ms);
        }
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

    async fn prepare(
        &self,
        inbound: InboundClipboard,
        processing_started_at: Instant,
    ) -> Option<PreparedInbound> {
        let receipt = inbound.receipt.clone();
        let flow = record_flow_id(inbound.header.flow_id.as_deref());
        let mut timing = InboundTiming {
            receiver_queue_ms: duration_ms(
                processing_started_at.saturating_duration_since(inbound.received_at),
            ),
            ..InboundTiming::default()
        };
        let receiver_policy_started_at = Instant::now();
        if !inbound_sync_enabled(self.settings.as_ref()).await {
            receipt.finish(InboundClipboardDisposition::Rejected);
            return None;
        }
        let Some(receive_permit) = self.receive_gate.authorize(&inbound.peer_device_id).await
        else {
            receipt.finish(InboundClipboardDisposition::Rejected);
            return None;
        };
        timing.receiver_policy_ms = duration_ms(receiver_policy_started_at.elapsed());
        let receiver_decrypt_started_at = Instant::now();
        let plaintext = match self.transfer_cipher.decrypt(&inbound.ciphertext).await {
            Ok(bytes) => Bytes::from(bytes),
            Err(_) => {
                warn!(
                    snapshot_hash = %inbound.header.snapshot_hash,
                    error_kind = "inbound_clipboard_decrypt_failed",
                    "inbound clipboard decrypt failed"
                );
                receipt.finish(InboundClipboardDisposition::Rejected);
                return None;
            }
        };
        timing.receiver_decrypt_ms = duration_ms(receiver_decrypt_started_at.elapsed());
        let receiver_preflight_decode_started_at = Instant::now();
        let categories = match decode_v3_bytes_to_snapshot(plaintext.as_ref()) {
            Ok(snapshot) => ClipboardContentCategorySet::from_snapshot(&snapshot),
            Err(_) => {
                warn!(
                    snapshot_hash = %inbound.header.snapshot_hash,
                    error_kind = "inbound_clipboard_classification_failed",
                    "inbound clipboard classification failed open"
                );
                ClipboardContentCategorySet::empty()
            }
        };
        timing.receiver_preflight_decode_ms =
            duration_ms(receiver_preflight_decode_started_at.elapsed());
        let receiver_category_policy_started_at = Instant::now();
        if !self
            .receive_gate
            .is_receive_category_allowed(&receive_permit, &categories)
        {
            receipt.finish(InboundClipboardDisposition::Rejected);
            return None;
        }
        timing.receiver_policy_ms = timing
            .receiver_policy_ms
            .saturating_add(duration_ms(receiver_category_policy_started_at.elapsed()));
        let payload_size_bucket = PayloadSizeBucket::from_bytes(plaintext.len() as u64);
        Some(PreparedInbound {
            from_device: inbound.peer_device_id,
            snapshot_hash: inbound.header.snapshot_hash,
            plaintext,
            flow,
            at_ms: self.clock.now_ms(),
            transport: inbound.transport,
            payload_type: payload_type_from_categories(&categories),
            payload_size_bucket,
            timing,
            receipt,
        })
    }

    fn capture_completed_stages(&self, prepared: &PreparedInbound, receiver_apply_ms: u32) {
        let transport_type = transport_type_from_channel(prepared.transport);
        let log = |stage, duration_ms| {
            log_clipboard_sync_stage(ClipboardSyncTiming {
                flow_id: &prepared.flow.log_flow_id,
                flow_synthetic: prepared.flow.synthetic,
                direction: Direction::Inbound,
                payload_type: prepared.payload_type,
                payload_size_bucket: prepared.payload_size_bucket,
                transport_type,
                stage,
                duration_ms,
            });
        };

        log(
            ClipboardSyncStage::ReceiverQueue,
            prepared.timing.receiver_queue_ms,
        );
        log(
            ClipboardSyncStage::ReceiverPolicy,
            prepared.timing.receiver_policy_ms,
        );
        log(
            ClipboardSyncStage::ReceiverDecrypt,
            prepared.timing.receiver_decrypt_ms,
        );
        log(
            ClipboardSyncStage::ReceiverPreflightDecode,
            prepared.timing.receiver_preflight_decode_ms,
        );
        log(ClipboardSyncStage::ReceiverApply, receiver_apply_ms);
    }
}

async fn inbound_sync_enabled(settings: &dyn SettingsPort) -> bool {
    match settings.load().await {
        Ok(settings) if settings.sync.sync_enabled => true,
        Ok(_) => {
            info!(
                reason = "sync_disabled",
                "clipboard inbound: delivery rejected by global sync setting"
            );
            false
        }
        Err(_) => {
            warn!(
                error_kind = "settings_load",
                "clipboard inbound: delivery rejected"
            );
            false
        }
    }
}

fn duration_ms(duration: Duration) -> u32 {
    duration.as_millis().min(u32::MAX as u128) as u32
}

fn payload_type_from_categories(categories: &ClipboardContentCategorySet) -> PayloadType {
    if categories
        .iter()
        .any(|category| matches!(category, ClipboardContentCategory::File))
    {
        PayloadType::File
    } else if categories
        .iter()
        .any(|category| matches!(category, ClipboardContentCategory::Image))
    {
        PayloadType::Image
    } else {
        PayloadType::Text
    }
}

fn transport_type_from_channel(channel: ConnectionChannel) -> TransportType {
    match channel {
        ConnectionChannel::Direct => TransportType::P2pDirect,
        ConnectionChannel::Relay => TransportType::Relay,
        ConnectionChannel::Offline | ConnectionChannel::Unknown => TransportType::Unknown,
    }
}

fn record_flow_id(wire_flow_id: Option<&str>) -> InboundFlow {
    match wire_flow_id {
        Some(wire_id) => match FlowId::parse_str(wire_id) {
            Ok(flow_id) => {
                tracing::Span::current().record("flow.id", tracing::field::display(&flow_id));
                InboundFlow {
                    application_flow_id: Some(flow_id.clone()),
                    log_flow_id: flow_id,
                    synthetic: false,
                }
            }
            Err(_) => {
                let synthetic = FlowId::generate();
                tracing::Span::current().record("flow.id", tracing::field::display(&synthetic));
                tracing::Span::current().record("flow.synthetic", true);
                warn!(
                    error_kind = "invalid_inbound_flow_id",
                    "inbound clipboard flow id was invalid; using a synthetic trace id"
                );
                InboundFlow {
                    application_flow_id: None,
                    log_flow_id: synthetic,
                    synthetic: true,
                }
            }
        },
        None => {
            let synthetic = FlowId::generate();
            tracing::Span::current().record("flow.id", tracing::field::display(&synthetic));
            tracing::Span::current().record("flow.synthetic", true);
            InboundFlow {
                application_flow_id: None,
                log_flow_id: synthetic,
                synthetic: true,
            }
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
    use std::io::{self, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use async_trait::async_trait;
    use bytes::Bytes;
    use tokio::sync::{broadcast, Notify};
    use tracing_subscriber::fmt::MakeWriter;

    use uc_core::ids::{DeviceId, FormatId, RepresentationId};
    use uc_core::ports::security::{TransferCipherError, TransferCipherPort};
    use uc_core::ports::{
        ClipboardHeader, ClipboardReceiverPort, ClockPort, ConnectionChannel, InboundClipboard,
        InboundClipboardDisposition, InboundClipboardReceipt, InboundClipboardResult, SettingsPort,
    };
    use uc_core::security::IdentityFingerprint;
    use uc_core::{
        MemberRepositoryPort, MemberSyncPreferences, MembershipError, MimeType,
        ObservedClipboardRepresentation, SpaceMember, SystemClipboardSnapshot,
    };

    use super::*;
    use crate::clipboard::sync::encode_snapshot_to_v3_bytes;
    use crate::deps::{
        CurrentSpaceMemberScope, CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort,
    };
    use crate::facade::{
        InboundClipboardApplyError, InboundClipboardApplyInput, InboundClipboardApplyOutcome,
        InboundClipboardApplyPort,
    };
    use uc_observability_contract::FlowId;

    struct FakeReceiver {
        tx: broadcast::Sender<InboundClipboard>,
    }

    struct FixedSettings {
        sync_enabled: bool,
    }

    #[async_trait]
    impl SettingsPort for FixedSettings {
        async fn load(&self) -> anyhow::Result<uc_core::settings::model::Settings> {
            let mut settings = uc_core::settings::model::Settings::default();
            settings.sync.sync_enabled = self.sync_enabled;
            Ok(settings)
        }

        async fn save(&self, _settings: &uc_core::settings::model::Settings) -> anyhow::Result<()> {
            Ok(())
        }
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

    struct AllowAllScope;

    #[async_trait]
    impl CurrentSpaceMemberScopePort for AllowAllScope {
        async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
            Ok(CurrentSpaceMemberScope {
                revision: 1,
                local_member_active: true,
                usable_peer_device_ids: [
                    "peer-1",
                    "peer-relay",
                    "peer-disabled",
                    "peer-unavailable",
                    "peer-text-disabled",
                ]
                .into_iter()
                .map(DeviceId::new)
                .collect(),
                paused_peer_devices: Vec::new(),
            })
        }
    }

    struct BlockedScope;

    #[async_trait]
    impl CurrentSpaceMemberScopePort for BlockedScope {
        async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
            Ok(CurrentSpaceMemberScope {
                revision: 1,
                local_member_active: true,
                usable_peer_device_ids: Vec::new(),
                paused_peer_devices: Vec::new(),
            })
        }
    }

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

    #[derive(Clone, Default)]
    struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

    struct Writer(CapturedWriter);

    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0
                 .0
                .lock()
                .expect("captured log writer lock")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for CapturedWriter {
        type Writer = Writer;

        fn make_writer(&'a self) -> Self::Writer {
            Writer(self.clone())
        }
    }

    impl CapturedWriter {
        fn output(&self) -> String {
            String::from_utf8(self.0.lock().expect("captured log writer lock").clone())
                .expect("UTF-8 log output")
        }
    }

    fn timing_log_writer() -> CapturedWriter {
        static WRITER: OnceLock<CapturedWriter> = OnceLock::new();

        WRITER
            .get_or_init(|| {
                let writer = CapturedWriter::default();
                let subscriber = tracing_subscriber::fmt()
                    .with_ansi(false)
                    .without_time()
                    .with_writer(writer.clone())
                    .finish();
                tracing::subscriber::set_global_default(subscriber)
                    .expect("install timing log test subscriber");
                writer
            })
            .clone()
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
                transport: ConnectionChannel::Unknown,
                received_at: Instant::now(),
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
            member_scope: Arc::new(AllowAllScope),
            transfer_cipher: Arc::new(EchoCipher),
            settings: Arc::new(FixedSettings { sync_enabled: true }),
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
            member_scope: Arc::new(AllowAllScope),
            transfer_cipher,
            settings: Arc::new(FixedSettings { sync_enabled: true }),
            clock: Arc::new(FixedClock),
            apply,
            events,
        }
    }

    fn deps_with_member_scope(
        receiver: Arc<FakeReceiver>,
        member_scope: Arc<dyn CurrentSpaceMemberScopePort>,
        apply: Arc<dyn InboundClipboardApplyPort>,
        events: Arc<dyn ClipboardInboundEventPort>,
    ) -> ClipboardInboundRuntimeDeps {
        ClipboardInboundRuntimeDeps {
            receiver,
            member_repo: Arc::new(AllowAllMembers),
            member_scope,
            transfer_cipher: Arc::new(NeverCipher),
            settings: Arc::new(FixedSettings { sync_enabled: true }),
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

    #[test]
    fn disabled_global_sync_records_the_rejection_reason() {
        // 与 relay timing 测试共用唯一全局 subscriber。并行测试期间临时
        // default 会与全局 callsite interest cache 竞争，偶发丢失本线程日志。
        let writer = timing_log_writer();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test runtime");

        assert!(!runtime.block_on(inbound_sync_enabled(&FixedSettings {
            sync_enabled: false,
        })));

        assert!(
            writer.output().contains("reason=\"sync_disabled\""),
            "disabled sync must expose a non-sensitive rejection reason; logs={}",
            writer.output()
        );
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
                    crate::clipboard::sync::apply_inbound::ApplyInboundError::Internal(
                        "storage unavailable".to_owned(),
                    ),
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
    async fn runtime_writes_relay_stage_timings_to_the_otlp_log_after_receiver_applies() {
        let receiver = Arc::new(FakeReceiver::new());
        let writer = timing_log_writer();
        let runtime = ClipboardInboundRuntime::start(deps(
            Arc::clone(&receiver),
            Arc::new(QueueApply {
                outcomes: Mutex::new(VecDeque::from([Ok(
                    InboundClipboardApplyOutcome::Applied {
                        entry_id: "entry-timed".to_owned(),
                    },
                )])),
            }),
            Arc::new(RecordingEvents::default()),
        ));
        let (mut inbound, result) = text_fixture("peer-relay");
        let flow_id = FlowId::generate();
        inbound.header.flow_id = Some(flow_id.to_string());
        inbound.received_at = Instant::now();
        inbound.transport = ConnectionChannel::Relay;

        receiver.publish(inbound);

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), result.wait())
                .await
                .expect("receipt settled"),
            Some(InboundClipboardDisposition::Applied)
        );

        let output = writer.output();
        let output = output
            .lines()
            .filter(|line| line.contains(&format!("flow_id={flow_id}")))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(output.contains(&format!("flow_id={flow_id}")));
        assert!(output.contains("sync_direction=\"inbound\""));
        assert!(output.contains("sync_transport=\"relay\""));
        assert!(output.contains("sync_stage=\"receiver_queue\""));
        assert!(output.contains("sync_stage=\"receiver_apply\""));
        assert!(!output.contains("sync_stage=\"receiver_commit\""));
        assert!(!output.contains("private text"));
        assert!(!output.contains("peer-relay"));

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
    async fn global_sync_disabled_rejects_before_decrypt_or_apply() {
        let receiver = Arc::new(FakeReceiver::new());
        let mut runtime_deps = deps(
            Arc::clone(&receiver),
            Arc::new(NeverApply),
            Arc::new(RecordingEvents::default()),
        );
        runtime_deps.settings = Arc::new(FixedSettings {
            sync_enabled: false,
        });
        runtime_deps.transfer_cipher = Arc::new(NeverCipher);
        let runtime = ClipboardInboundRuntime::start(runtime_deps);
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
    async fn upgrade_required_peer_is_rejected_before_decrypt_or_apply() {
        let receiver = Arc::new(FakeReceiver::new());
        let runtime = ClipboardInboundRuntime::start(deps_with_member_scope(
            Arc::clone(&receiver),
            Arc::new(BlockedScope),
            Arc::new(NeverApply),
            Arc::new(RecordingEvents::default()),
        ));
        let (inbound, result) = fixture("peer-needs-upgrade", "hash-needs-upgrade");

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
