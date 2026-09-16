//! 存储层只观察不透明地址记录，不依赖 Iroh 编码；签名只在本地运行时内存中使用。
use super::{emit_local, record::LocalEvent, NetworkRecorder, ObservationContext};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::Digest;

#[derive(Clone)]
struct RecordKeys {
    device: Option<[u8; 32]>,
    record: Option<[u8; 32]>,
}

pub struct StoredAddressObservation {
    context: ObservationContext,
}

impl StoredAddressObservation {
    pub fn new(device: &str, record: Option<&[u8]>) -> Self {
        let keys = RecordKeys {
            device: (device.len() <= 128).then(|| sha2::Sha256::digest(device.as_bytes()).into()),
            record: record
                .filter(|bytes| bytes.len() <= 16 * 1024)
                .map(|bytes| sha2::Sha256::digest(bytes).into()),
        };
        Self {
            context: ObservationContext(ObservationContext::capture().0.with_value(keys)),
        }
    }

    pub fn in_scope<T>(&self, operation: impl FnOnce() -> T) -> T {
        let _guard = self.context.0.clone().attach();
        operation()
    }
}

pub(super) fn carry_record_keys(mut context: ObservationContext) -> ObservationContext {
    if let Some(keys) = opentelemetry::Context::current()
        .get::<RecordKeys>()
        .cloned()
    {
        context.0 = context.0.with_value(keys);
    }
    context
}

#[doc(hidden)]
pub fn local_address_record_keys() -> Option<(Option<[u8; 32]>, Option<[u8; 32]>)> {
    opentelemetry::Context::current()
        .get::<RecordKeys>()
        .map(|keys| (keys.device, keys.record))
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum AddressRecordResult {
    Loaded { observed_at_ms: i64 },
    Saved { observed_at_ms: i64 },
    Missing,
    ReadFailed,
    SaveFailed,
    InvalidEncoding,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AddressRecordEvent {
    pub result: AddressRecordResult,
}

impl AddressRecordEvent {
    pub(super) fn name(&self) -> &'static str {
        match self.result {
            AddressRecordResult::Loaded { .. } => "address.record.loaded",
            AddressRecordResult::Saved { .. } => "address.record.saved",
            AddressRecordResult::Missing => "address.record.missing",
            AddressRecordResult::ReadFailed => "address.record.read_failed",
            AddressRecordResult::SaveFailed => "address.record.save_failed",
            AddressRecordResult::InvalidEncoding => "address.record.invalid_encoding",
        }
    }
    pub(super) fn level(&self) -> &'static str {
        if matches!(
            self.result,
            AddressRecordResult::ReadFailed
                | AddressRecordResult::SaveFailed
                | AddressRecordResult::InvalidEncoding
        ) {
            "WARN"
        } else {
            "INFO"
        }
    }
    pub(super) fn fields(&self) -> Map<String, Value> {
        let mut fields = Map::new();
        fields.insert("source".into(), json!("stored"));
        if let AddressRecordResult::Loaded { observed_at_ms }
        | AddressRecordResult::Saved { observed_at_ms } = self.result
        {
            fields.insert("observed_at_ms".into(), json!(observed_at_ms));
        }
        fields
    }
}

impl NetworkRecorder {
    pub fn address_record(
        &self,
        observation: &StoredAddressObservation,
        result: AddressRecordResult,
    ) {
        tracing::dispatcher::with_default(&self.dispatcher, || {
            emit_local(
                LocalEvent::AddressRecord {
                    record: AddressRecordEvent { result },
                },
                &observation.context,
            )
        });
    }
}
