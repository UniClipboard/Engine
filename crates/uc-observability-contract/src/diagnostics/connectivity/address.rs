//! 地址事实只携带固定来源和数量；候选签名仅作为内存中的匿名关联材料。
use super::{connection::peer_context, emit_local, record::LocalEvent};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateSummary {
    pub direct_count: u32,
    pub relay_count: u32,
    pub other_count: u32,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressInputSource {
    Stored,
    AdmissionRoute,
    Provided,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySource {
    Dns,
    Mdns,
    Pkarr,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum AddressEvent {
    Loaded {
        summary: CandidateSummary,
        observed_at_ms: i64,
    },
    Used {
        source: AddressInputSource,
        summary: CandidateSummary,
        #[serde(skip_serializing_if = "Option::is_none")]
        connect_id: Option<uuid::Uuid>,
    },
    Discovered {
        source: DiscoverySource,
        summary: CandidateSummary,
        observed_at_ms: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        lookup_id: Option<uuid::Uuid>,
    },
    PublishRequested {
        source: DiscoverySource,
        summary: CandidateSummary,
    },
    LookupStarted {
        source: DiscoverySource,
        lookup_id: uuid::Uuid,
    },
    LookupFinished {
        source: DiscoverySource,
        lookup_id: uuid::Uuid,
        interrupted: bool,
        result_count: u32,
        error_count: u32,
        duration_ms: u64,
    },
}

impl AddressEvent {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Loaded { .. } => "address.loaded",
            Self::Used { .. } => "address.used",
            Self::Discovered { .. } => "address.discovered",
            Self::PublishRequested { .. } => "address.publish_requested",
            Self::LookupStarted { .. } => "address.lookup.started",
            Self::LookupFinished { .. } => "address.lookup.finished",
        }
    }

    pub(super) fn level(&self) -> &'static str {
        if matches!(self, Self::LookupFinished { error_count, .. } if *error_count > 0) {
            "WARN"
        } else {
            "INFO"
        }
    }

    pub(super) fn fields(&self) -> Map<String, Value> {
        let Value::Object(mut fields) = json!(self) else {
            return Map::new();
        };
        fields.remove("event");
        if let Some(Value::Object(summary)) = fields.remove("summary") {
            fields.extend(summary);
        }
        if matches!(self, Self::Loaded { .. }) {
            fields.insert("source".into(), json!("stored"));
        }
        fields
    }
}

#[derive(Clone)]
struct CandidateFingerprint([u8; 32]);

/// 只供本地运行时比较内存中的候选集合，不得序列化或发送此签名。
#[doc(hidden)]
pub fn local_candidate_fingerprint() -> Option<[u8; 32]> {
    opentelemetry::Context::current()
        .get::<CandidateFingerprint>()
        .map(|value| value.0)
}

/// 捕获输出位置而不捕获业务 span；供底层发现回调在 NoSubscriber 下发出安全事实。
#[derive(Clone)]
pub struct NetworkRecorder {
    pub(super) dispatcher: tracing::Dispatch,
}

impl Default for NetworkRecorder {
    fn default() -> Self {
        Self::current()
    }
}

impl NetworkRecorder {
    pub fn is_enabled(&self) -> bool {
        tracing::dispatcher::with_default(&self.dispatcher, super::local_events_enabled)
    }
    pub fn current() -> Self {
        Self {
            dispatcher: tracing::dispatcher::get_default(Clone::clone),
        }
    }

    pub fn address_loaded(
        &self,
        peer: [u8; 32],
        fingerprint: Option<[u8; 32]>,
        summary: CandidateSummary,
        observed_at_ms: i64,
    ) {
        self.emit(
            peer,
            fingerprint,
            AddressEvent::Loaded {
                summary,
                observed_at_ms,
            },
        );
    }

    pub fn address_used(
        &self,
        peer: [u8; 32],
        fingerprint: Option<[u8; 32]>,
        source: AddressInputSource,
        summary: CandidateSummary,
    ) {
        self.emit(
            peer,
            fingerprint,
            AddressEvent::Used {
                source,
                summary,
                connect_id: None,
            },
        );
    }

    pub fn address_discovered(
        &self,
        peer: [u8; 32],
        fingerprint: Option<[u8; 32]>,
        source: DiscoverySource,
        summary: CandidateSummary,
        observed_at_ms: Option<i64>,
    ) {
        self.emit(
            peer,
            fingerprint,
            AddressEvent::Discovered {
                source,
                summary,
                observed_at_ms,
                lookup_id: None,
            },
        );
    }

    pub fn lookup(&self, peer: [u8; 32], source: DiscoverySource) -> LookupObservation {
        let observation = LookupObservation {
            recorder: self.clone(),
            peer,
            source,
            id: uuid::Uuid::new_v4(),
            started: std::time::Instant::now(),
            results: 0,
            errors: 0,
            finished: false,
        };
        self.emit(
            peer,
            None,
            AddressEvent::LookupStarted {
                source,
                lookup_id: observation.id,
            },
        );
        observation
    }

    pub fn address_publish_requested(
        &self,
        peer: [u8; 32],
        fingerprint: Option<[u8; 32]>,
        source: DiscoverySource,
        summary: CandidateSummary,
    ) {
        self.emit(
            peer,
            fingerprint,
            AddressEvent::PublishRequested { source, summary },
        );
    }

    fn emit(&self, peer: [u8; 32], fingerprint: Option<[u8; 32]>, record: AddressEvent) {
        let mut context = peer_context(peer);
        if let Some(fingerprint) = fingerprint {
            context.0 = context.0.with_value(CandidateFingerprint(fingerprint));
        }
        tracing::dispatcher::with_default(&self.dispatcher, || {
            emit_local(LocalEvent::Address { record }, &context)
        });
    }
}

pub struct LookupObservation {
    recorder: NetworkRecorder,
    peer: [u8; 32],
    source: DiscoverySource,
    id: uuid::Uuid,
    started: std::time::Instant,
    results: u32,
    errors: u32,
    finished: bool,
}

impl LookupObservation {
    pub fn result(
        &mut self,
        fingerprint: Option<[u8; 32]>,
        summary: CandidateSummary,
        observed_at_ms: Option<i64>,
    ) {
        self.results = self.results.saturating_add(1);
        self.recorder.emit(
            self.peer,
            fingerprint,
            AddressEvent::Discovered {
                source: self.source,
                summary,
                observed_at_ms,
                lookup_id: Some(self.id),
            },
        );
    }
    pub fn error(&mut self) {
        self.errors = self.errors.saturating_add(1);
    }
    pub fn finish(mut self) {
        self.finished = true;
        self.emit_finish(false);
    }
    fn emit_finish(&self, interrupted: bool) {
        self.recorder.emit(
            self.peer,
            None,
            AddressEvent::LookupFinished {
                source: self.source,
                lookup_id: self.id,
                interrupted,
                result_count: self.results,
                error_count: self.errors,
                duration_ms: super::millis(self.started.elapsed()),
            },
        );
    }
}

impl Drop for LookupObservation {
    fn drop(&mut self) {
        if !self.finished {
            self.emit_finish(true);
        }
    }
}

pub(super) fn emit_used(
    dispatcher: &tracing::Dispatch,
    context: &super::ObservationContext,
    connect_id: uuid::Uuid,
    fingerprint: Option<[u8; 32]>,
    source: AddressInputSource,
    summary: CandidateSummary,
) {
    let mut context = context.clone();
    if let Some(fingerprint) = fingerprint {
        context.0 = context.0.with_value(CandidateFingerprint(fingerprint));
    }
    tracing::dispatcher::with_default(dispatcher, || {
        emit_local(
            LocalEvent::Address {
                record: AddressEvent::Used {
                    source,
                    summary,
                    connect_id: Some(connect_id),
                },
            },
            &context,
        )
    });
}
