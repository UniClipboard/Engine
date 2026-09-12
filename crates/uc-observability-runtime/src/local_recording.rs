//! 进程运行时的本地关联元数据；对端映射有界且只驻留内存。
use crate::config::ObservabilityResource;
use crate::local_capture::CapturePolicy;
use crate::local_file::LocalFileRuntime;
use crate::{
    DetailedCaptureRequest, LocalCaptureStatus, LocalDiagnosticError, LocalDiagnosticSource,
    LocalDiagnosticStatus, OperatingSystem, SetupStatus, SourceCapability, SourceCollection,
    SourceCoverage, StopCaptureResult,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;
use uuid::Uuid;

const MAX_PEERS: usize = 4096;
const MAX_CANDIDATE_SETS: usize = 8;
const MAX_CONNECTIONS: usize = 4096;
const MAX_PATHS: usize = 64;

struct PhysicalRecording {
    reference: Uuid,
    paths: HashMap<[u8; 32], Uuid>,
}

#[derive(Default)]
struct StoredReferences {
    devices: HashMap<[u8; 32], Uuid>,
    records: HashMap<[u8; 32], Uuid>,
}

fn bounded_reference(map: &mut HashMap<[u8; 32], Uuid>, key: [u8; 32]) -> Option<Uuid> {
    if let Some(reference) = map.get(&key) {
        return Some(*reference);
    }
    if map.len() >= MAX_PEERS {
        return None;
    }
    let reference = Uuid::new_v4();
    map.insert(key, reference);
    Some(reference)
}

struct CandidateGeneration {
    fingerprint: [u8; 32],
    generation: u64,
}

struct PeerRecording {
    reference: Uuid,
    candidates: HashMap<[u8; 32], Uuid>,
    generations: HashMap<&'static str, CandidateGeneration>,
}

impl PeerRecording {
    fn new() -> Self {
        Self {
            reference: Uuid::new_v4(),
            candidates: HashMap::new(),
            generations: HashMap::new(),
        }
    }

    fn annotate_candidates(&mut self, record: &mut Value, fingerprint: [u8; 32]) {
        let candidate = if let Some(id) = self.candidates.get(&fingerprint) {
            Some(*id)
        } else if self.candidates.len() < MAX_CANDIDATE_SETS {
            let id = Uuid::new_v4();
            self.candidates.insert(fingerprint, id);
            Some(id)
        } else {
            None
        };
        if let Some(id) = candidate {
            record["candidate_set_ref"] = json!(id.to_string());
        } else {
            record["correlation_status"] = json!("candidate_capacity_exceeded");
        }
        let event = record["fields"]["event.name"].as_str().unwrap_or_default();
        let source = record["fields"]["source"].as_str().unwrap_or_default();
        let key = match (event, source) {
            ("address.loaded" | "address.saved", "stored") => "stored",
            ("address.used", "stored") => "used.stored",
            ("address.used", "admission_route") => "used.admission_route",
            ("address.used", "provided") => "used.provided",
            ("address.discovered", "dns") => "discovered.dns",
            ("address.discovered", "mdns") => "discovered.mdns",
            ("address.discovered", "pkarr") => "discovered.pkarr",
            ("address.publish_requested", "mdns") => "publication.mdns",
            ("address.publish_requested", "pkarr") => "publication.pkarr",
            _ => return,
        };
        if let Some(previous) = self.generations.get_mut(key) {
            let changed = previous.fingerprint != fingerprint;
            if changed {
                let Some(next) = previous.generation.checked_add(1) else {
                    record["correlation_status"] = json!("generation_exhausted");
                    return;
                };
                previous.generation = next;
                previous.fingerprint = fingerprint;
            }
            record["candidate_generation"] = json!(previous.generation);
            record["candidate_values_changed"] = json!(changed);
        } else {
            self.generations.insert(
                key,
                CandidateGeneration {
                    fingerprint,
                    generation: 1,
                },
            );
            record["candidate_generation"] = json!(1);
            record["candidate_values_changed"] = Value::Null;
        }
    }
}

pub(crate) struct LocalRecordingState {
    pub(crate) capture: Mutex<CapturePolicy>,
    run_id: Uuid,
    started: Instant,
    resource: ObservabilityResource,
    peers: Mutex<HashMap<[u8; 32], PeerRecording>>,
    connections: Mutex<HashMap<([u8; 32], u64), PhysicalRecording>>,
    stored: Mutex<StoredReferences>,
    file: Option<Weak<LocalFileRuntime>>,
}

impl LocalRecordingState {
    pub(crate) fn new(
        resource: &ObservabilityResource,
        file: Option<&Arc<LocalFileRuntime>>,
    ) -> Self {
        Self {
            capture: Mutex::new(CapturePolicy::new()),
            run_id: Uuid::new_v4(),
            started: Instant::now(),
            resource: resource.clone(),
            peers: Mutex::new(HashMap::new()),
            connections: Mutex::new(HashMap::new()),
            stored: Mutex::new(StoredReferences::default()),
            file: file.map(Arc::downgrade),
        }
    }

    pub(crate) fn annotate(&self, record: &mut Value) {
        record["local_schema_version"] = json!(2);
        record["run_id"] = json!(self.run_id.to_string());
        record["monotonic_offset_ms"] =
            json!(u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX));
        record["engine_version"] = json!(env!("CARGO_PKG_VERSION"));
        record["host_version"] = json!(self.resource.service_version);
        record["platform"] = json!(self.resource.os.as_str());
        record["environment"] = json!(self.resource.environment.as_str());
        record["source_commit"] = json!(env!("UC_OBSERVABILITY_SOURCE_COMMIT"));
        record["source_state"] = json!(env!("UC_OBSERVABILITY_SOURCE_STATE"));
        if let Some(peer) =
            uc_observability_contract::diagnostics::connectivity::local_connection_peer()
        {
            let mut peers = self
                .peers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !peers.contains_key(&peer) && peers.len() < MAX_PEERS {
                peers.insert(peer, PeerRecording::new());
            }
            if let Some(peer) = peers.get_mut(&peer) {
                record["peer_ref"] = json!(peer.reference.to_string());
                if let Some(fingerprint) = uc_observability_contract::diagnostics::connectivity::local_candidate_fingerprint() {
                    peer.annotate_candidates(record, fingerprint);
                }
            } else {
                record["correlation_status"] = json!("capacity_exceeded");
            }
        }
        self.annotate_connection(record);
        self.annotate_stored(record);
    }

    pub(crate) fn status(&self, local_file: SetupStatus, closed: bool) -> LocalDiagnosticStatus {
        let mut policy = self
            .capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let capture = policy.status(Instant::now());
        self.capture_checkpoint(&mut policy, &capture);
        LocalDiagnosticStatus {
            run_id: self.run_id.to_string(),
            capture,
            observed_records: policy.observed,
            policy_filtered_records: policy.filtered,
            schema_rejected_records: policy.rejected,
            correlation_limited_records: policy.limited,
            engine_version: env!("CARGO_PKG_VERSION").into(),
            source_commit: env!("UC_OBSERVABILITY_SOURCE_COMMIT").into(),
            counter_scope: "typed_events_only",
            sources: policy.coverage(),
            local_file,
            closed,
        }
    }

    pub(crate) fn include(&self, record: &mut Value) -> bool {
        let mut policy = self
            .capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let included = policy.include(record, Instant::now());
        let status = policy.status(Instant::now());
        self.capture_checkpoint(&mut policy, &status);
        included
    }

    pub(crate) fn start_capture(
        &self,
        request: DetailedCaptureRequest,
    ) -> Result<LocalCaptureStatus, LocalDiagnosticError> {
        let mut policy = self
            .capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let status = policy.start(request, Instant::now())?;
        self.capture_checkpoint(&mut policy, &status);
        Ok(status)
    }

    pub(crate) fn stop_capture(&self, id: &str) -> Result<StopCaptureResult, LocalDiagnosticError> {
        let mut policy = self
            .capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = policy.stop(id, Instant::now())?;
        let status = policy.status(Instant::now());
        self.capture_checkpoint(&mut policy, &status);
        Ok(result)
    }

    pub(crate) fn capture_checkpoint(
        &self,
        policy: &mut CapturePolicy,
        status: &LocalCaptureStatus,
    ) {
        if policy.checkpointed_revision != policy.revision {
            self.checkpoint("diagnostics.capture.changed", json!({ "capture": status }));
            policy.checkpointed_revision = policy.revision;
        }
    }

    pub(crate) fn checkpoint(&self, name: &'static str, mut fields: Value) {
        let Some(file) = self.file.as_ref().and_then(std::sync::Weak::upgrade) else {
            return;
        };
        fields["event.name"] = json!(name);
        let mut record = json!({ "timestamp": chrono::Utc::now().to_rfc3339(), "level": "INFO", "target": "uc.diagnostics", "fields": fields });
        let _context = opentelemetry::Context::new().attach();
        self.annotate(&mut record);
        match serde_json::to_vec(&record) {
            Ok(mut bytes) if bytes.len() <= 4096 => {
                bytes.push(b'\n');
                file.writer()
                    .write_record(&bytes, LocalDiagnosticSource::Runtime);
            }
            _ => file.record_rejection(),
        }
    }

    pub(crate) fn platform(&self) -> OperatingSystem {
        self.resource.os
    }

    pub(crate) fn register_source(
        &self,
        source: LocalDiagnosticSource,
        capability: SourceCapability,
        collection: SourceCollection,
    ) {
        let mut policy = self
            .capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let status = policy
            .sources
            .entry(source)
            .or_insert_with(|| SourceCoverage {
                source,
                capability: SourceCapability::Unknown,
                collection: SourceCollection::NotRegistered,
                observed_count: 0,
                policy_filtered_count: 0,
            });
        if status.capability == capability && status.collection == collection {
            return;
        }
        status.capability = capability;
        status.collection = collection;
        self.checkpoint(
            "diagnostics.source.status",
            json!({ "source": source, "capability": capability, "collection": collection }),
        );
    }

    pub(crate) fn write_value(&self, record: &Value, source: LocalDiagnosticSource) {
        let Some(file) = self.file.as_ref().and_then(Weak::upgrade) else {
            return;
        };
        match serde_json::to_vec(record) {
            Ok(mut bytes) if bytes.len() <= 4096 => {
                bytes.push(b'\n');
                file.writer().write_record(&bytes, source);
            }
            _ => {
                self.rejected();
                file.record_rejection();
            }
        }
    }

    pub(crate) fn rejected(&self) {
        let mut capture = self
            .capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        capture.rejected = capture.rejected.saturating_add(1);
    }

    fn annotate_stored(&self, record: &mut Value) {
        let Some((device_key, record_key)) =
            uc_observability_contract::diagnostics::connectivity::local_address_record_keys()
        else {
            return;
        };
        let mut stored = self
            .stored
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(reference) =
            device_key.and_then(|key| bounded_reference(&mut stored.devices, key))
        {
            record["device_ref"] = json!(reference.to_string());
        } else {
            record["record_correlation_status"] = json!("unavailable");
        }
        if let Some(key) = record_key {
            if let Some(reference) = bounded_reference(&mut stored.records, key) {
                record["address_record_ref"] = json!(reference.to_string());
            } else {
                record["record_correlation_status"] = json!("capacity_exceeded");
            }
        }
    }

    fn annotate_connection(&self, record: &mut Value) {
        use uc_observability_contract::diagnostics::connectivity::{
            local_connection_key, local_connection_peer, local_path_key,
        };
        let (Some(peer), Some(key)) = (local_connection_peer(), local_connection_key()) else {
            return;
        };
        let event = record["fields"]["event.name"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let mut connections = self
            .connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = (peer, key);
        if event == "connection.established"
            && !connections.contains_key(&key)
            && connections.len() < MAX_CONNECTIONS
        {
            connections.insert(
                key,
                PhysicalRecording {
                    reference: Uuid::new_v4(),
                    paths: HashMap::new(),
                },
            );
        }
        if let Some(connection) = connections.get_mut(&key) {
            record["connection_id"] = json!(connection.reference.to_string());
            if let Some(path) = local_path_key() {
                if !connection.paths.contains_key(&path) && connection.paths.len() < MAX_PATHS {
                    connection.paths.insert(path, Uuid::new_v4());
                }
                if let Some(reference) = connection.paths.get(&path) {
                    record["path_ref"] = json!(reference.to_string());
                } else {
                    record["correlation_status"] = json!("path_capacity_exceeded");
                }
            }
        } else {
            record["connection_correlation_status"] = json!("unavailable");
        }
        if matches!(
            event.as_str(),
            "connection.closed" | "connection.observer.stopped"
        ) {
            connections.remove(&key);
        }
    }
}
