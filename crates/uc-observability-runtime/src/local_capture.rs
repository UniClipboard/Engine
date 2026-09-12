//! 本地采集策略与状态；不负责业务重试，也不控制远程许可。
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};
pub use uc_observability_contract::diagnostics::connectivity::{
    LocalDiagnosticSource, SourceCapability, SourceCollection,
};
use uuid::Uuid;

use crate::host_diagnostics::HostPending;
use crate::{
    FileSourceCounts, HostDiagnosticSource, HostLifecycleState, SetupStatus, SignalResult,
};

#[derive(Debug, Clone, Serialize)]
pub struct SourceCoverage {
    pub source: LocalDiagnosticSource,
    pub capability: SourceCapability,
    pub collection: SourceCollection,
    pub observed_count: u64,
    pub policy_filtered_count: u64,
}

impl SourceCoverage {
    fn unregistered(source: LocalDiagnosticSource) -> Self {
        Self {
            source,
            capability: SourceCapability::Unknown,
            collection: SourceCollection::NotRegistered,
            observed_count: 0,
            policy_filtered_count: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalCaptureMode {
    Standard,
    Detailed,
}

#[derive(Debug, Clone, Copy)]
pub struct DetailedCaptureRequest {
    pub duration: Duration,
}
impl Default for DetailedCaptureRequest {
    fn default() -> Self {
        Self {
            duration: Duration::from_secs(600),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureEndReason {
    Expired,
    Requested,
    SuspensionExpiryUnknown,
    RuntimeShutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalCaptureStatus {
    pub mode: LocalCaptureMode,
    pub capture_id: Option<String>,
    pub remaining_ms: u64,
    pub started_at_utc: Option<String>,
    pub end_reason: Option<CaptureEndReason>,
    pub last_capture_id: Option<String>,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopCaptureResult {
    Stopped,
    AlreadyStopped,
    DifferentCapture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LocalDiagnosticError {
    #[error("local diagnostics are not installed")]
    NotInstalled,
    #[error("local diagnostic files are unavailable")]
    LocalSinkUnavailable,
    #[error("local diagnostics are already shut down")]
    AlreadyShutdown,
    #[error("capture duration must be between one second and fifteen minutes")]
    InvalidDuration,
    #[error("capture identifier is invalid")]
    InvalidCaptureId,
    #[error("export deadline must be between one millisecond and five seconds")]
    InvalidDeadline,
    #[error("host diagnostic source is invalid for this platform")]
    InvalidHostSource,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalDiagnosticStatus {
    pub run_id: String,
    pub capture: LocalCaptureStatus,
    pub observed_records: u64,
    pub policy_filtered_records: u64,
    pub schema_rejected_records: u64,
    pub correlation_limited_records: u64,
    pub engine_version: String,
    pub source_commit: String,
    pub counter_scope: &'static str,
    pub sources: Vec<SourceCoverage>,
    pub local_file: SetupStatus,
    pub closed: bool,
}

#[derive(Debug, Clone)]
pub struct LocalDiagnosticExportReport {
    pub flush: SignalResult,
    pub status: LocalDiagnosticStatus,
    pub requested_at_utc: String,
    pub completed_at_utc: String,
    pub other_processes_flushed: bool,
    pub files: Vec<FileSourceCounts>,
}

pub(crate) fn source_for(record: &Value) -> LocalDiagnosticSource {
    use LocalDiagnosticSource as Source;
    let fields = &record["fields"];
    if record["target"] == "uc.host.diagnostics" {
        return serde_json::from_value(fields["source"].clone()).unwrap_or(Source::Runtime);
    }
    let name = fields["event.name"].as_str().unwrap_or_default();
    if name.starts_with("connection.path.") {
        return Source::ConnectionPaths;
    }
    if name.starts_with("connection.") || name == "address.used" {
        return Source::Connections;
    }
    if name.starts_with("address.") {
        return match fields["source"].as_str().unwrap_or_default() {
            "dns" => Source::DnsDiscovery,
            "mdns" => Source::MdnsDiscovery,
            "pkarr" => Source::PkarrDiscovery,
            _ => Source::AddressStorage,
        };
    }
    if name.starts_with("network.") || name.starts_with("relay.") {
        return Source::RelayRecovery;
    }
    if name.starts_with("session.") || name == "pairing.recovery.decided" {
        return Source::Sessions;
    }
    if fields["uc.operation"] == "membership_group_update" {
        return Source::MembershipUpdates;
    }
    Source::Runtime
}

struct CaptureSession {
    id: Uuid,
    deadline: Instant,
    started: DateTime<Utc>,
}

#[derive(Default)]
pub(crate) struct CapturePolicy {
    pub(crate) host_pending: HashMap<Uuid, HostPending>,
    pub(crate) host_lifecycle: HashMap<HostDiagnosticSource, HostLifecycleState>,
    pub(crate) sources: HashMap<LocalDiagnosticSource, SourceCoverage>,
    active: Option<CaptureSession>,
    closed: bool,
    last_end: Option<CaptureEndReason>,
    last_id: Option<Uuid>,
    pub(crate) revision: u64,
    pub(crate) checkpointed_revision: u64,
    pending: HashMap<String, Uuid>,
    pub(crate) observed: u64,
    pub(crate) filtered: u64,
    pub(crate) rejected: u64,
    pub(crate) limited: u64,
}

impl CapturePolicy {
    pub(crate) fn shutdown(&mut self) {
        self.closed = true;
        if let Some(active) = self.active.take() {
            self.last_id = Some(active.id);
            self.last_end = Some(CaptureEndReason::RuntimeShutdown);
            self.revision = self.revision.saturating_add(1);
        }
    }
    pub(crate) fn resumed(&mut self) {
        if let Some(active) = self.active.take() {
            self.last_id = Some(active.id);
            self.last_end = Some(CaptureEndReason::SuspensionExpiryUnknown);
            self.revision = self.revision.saturating_add(1);
        }
    }
    pub(crate) fn new() -> Self {
        let mut policy = Self::default();
        policy.sources.insert(
            LocalDiagnosticSource::Runtime,
            SourceCoverage {
                source: LocalDiagnosticSource::Runtime,
                capability: SourceCapability::Partial,
                collection: SourceCollection::Enabled,
                observed_count: 0,
                policy_filtered_count: 0,
            },
        );
        policy
    }
    fn expire(&mut self, now: Instant) {
        if self
            .active
            .as_ref()
            .is_some_and(|session| session.deadline <= now)
        {
            self.last_id = self.active.take().map(|s| s.id);
            self.last_end = Some(CaptureEndReason::Expired);
            self.revision = self.revision.saturating_add(1);
        }
    }

    pub(crate) fn start(
        &mut self,
        request: DetailedCaptureRequest,
        now: Instant,
    ) -> Result<LocalCaptureStatus, LocalDiagnosticError> {
        // 与关闭共同受采集锁保护，拒绝已通过外层检查但迟到的开启请求。
        if self.closed {
            return Err(LocalDiagnosticError::AlreadyShutdown);
        }
        if request.duration < Duration::from_secs(1) || request.duration > Duration::from_secs(900)
        {
            return Err(LocalDiagnosticError::InvalidDuration);
        }
        self.expire(now);
        if self.active.is_none() {
            self.active = Some(CaptureSession {
                id: Uuid::new_v4(),
                deadline: now
                    .checked_add(request.duration)
                    .ok_or(LocalDiagnosticError::InvalidDuration)?,
                started: Utc::now(),
            });
            self.revision = self.revision.saturating_add(1);
        }
        Ok(self.status(now))
    }

    pub(crate) fn stop(
        &mut self,
        id: &str,
        now: Instant,
    ) -> Result<StopCaptureResult, LocalDiagnosticError> {
        let id = Uuid::parse_str(id).map_err(|_| LocalDiagnosticError::InvalidCaptureId)?;
        self.expire(now);
        match &self.active {
            None => Ok(StopCaptureResult::AlreadyStopped),
            Some(active) if active.id != id => Ok(StopCaptureResult::DifferentCapture),
            Some(_) => {
                self.last_id = self.active.take().map(|s| s.id);
                self.last_end = Some(CaptureEndReason::Requested);
                self.revision = self.revision.saturating_add(1);
                Ok(StopCaptureResult::Stopped)
            }
        }
    }

    pub(crate) fn status(&mut self, now: Instant) -> LocalCaptureStatus {
        self.expire(now);
        LocalCaptureStatus {
            mode: if self.active.is_some() {
                LocalCaptureMode::Detailed
            } else {
                LocalCaptureMode::Standard
            },
            capture_id: self.active.as_ref().map(|s| s.id.to_string()),
            remaining_ms: self
                .active
                .as_ref()
                .map_or(0, |s| millis(s.deadline.saturating_duration_since(now))),
            started_at_utc: self.active.as_ref().map(|s| s.started.to_rfc3339()),
            end_reason: self.last_end,
            last_capture_id: self.last_id.map(|id| id.to_string()),
            revision: self.revision,
        }
    }

    pub(crate) fn include(&mut self, record: &mut Value, now: Instant) -> bool {
        self.expire(now);
        let source = source_for(record);
        let fields = &record["fields"];
        if fields["event.name"] == "diagnostics.source.status" {
            let (Ok(source), Ok(capability), Ok(collection)) = (
                serde_json::from_value::<LocalDiagnosticSource>(fields["source"].clone()),
                serde_json::from_value::<SourceCapability>(fields["capability"].clone()),
                serde_json::from_value::<SourceCollection>(fields["collection"].clone()),
            ) else {
                self.rejected = self.rejected.saturating_add(1);
                return false;
            };
            let status = self
                .sources
                .entry(source)
                .or_insert_with(|| SourceCoverage::unregistered(source));
            if status.capability == capability && status.collection == collection {
                return false;
            }
            status.capability = capability;
            status.collection = collection;
            return true;
        }
        let status = self
            .sources
            .entry(source)
            .or_insert_with(|| SourceCoverage::unregistered(source));
        status.observed_count = status.observed_count.saturating_add(1);
        self.observed = self.observed.saturating_add(1);
        if ["correlation_status", "record_correlation_status"]
            .iter()
            .any(|key| {
                record[*key].as_str().is_some_and(|status| {
                    matches!(
                        status,
                        "capacity_exceeded"
                            | "candidate_capacity_exceeded"
                            | "generation_exhausted"
                            | "path_capacity_exceeded"
                    )
                })
            })
        {
            self.limited = self.limited.saturating_add(1);
        }
        let fields = &record["fields"];
        let name = fields["event.name"].as_str().unwrap_or_default();
        let key = pending_key(record);
        let starting = matches!(
            name,
            "connection.started"
                | "connection.attempt.started"
                | "connection.established"
                | "address.lookup.started"
                | "network.recovery.started"
        );
        let finishing = matches!(
            name,
            "connection.finished"
                | "connection.attempt.finished"
                | "connection.closed"
                | "connection.observer.stopped"
                | "address.lookup.finished"
                | "network.recovery.finished"
        );
        let tail = if finishing {
            key.as_ref().and_then(|key| self.pending.remove(key))
        } else {
            None
        };
        let detail_only = match name {
            "connection.attempt.started" | "address.lookup.started" => true,
            "connection.attempt.finished" => {
                fields["outcome"] == "connected" || fields["outcome"] == "cancelled_by_winner"
            }
            "address.lookup.finished" => fields["error_count"].as_u64() == Some(0),
            "address.loaded" | "address.discovered" | "address.publish_requested" => {
                record["candidate_values_changed"] == false
            }
            "address.record.loaded" => true,
            "connection.path.observed" => fields["observation"] != "selected",
            _ => false,
        };
        if self.active.is_none() && detail_only && tail.is_none() {
            self.filtered = self.filtered.saturating_add(1);
            if let Some(status) = self.sources.get_mut(&source) {
                status.policy_filtered_count = status.policy_filtered_count.saturating_add(1);
            }
            return false;
        }
        let capture = tail
            .or_else(|| {
                record["capture_id"]
                    .as_str()
                    .and_then(|id| Uuid::parse_str(id).ok())
            })
            .or_else(|| self.active.as_ref().map(|s| s.id));
        if starting {
            if let (Some(key), Some(capture)) = (key, capture) {
                if self.pending.len() < 4096 {
                    self.pending.insert(key, capture);
                } else {
                    self.limited = self.limited.saturating_add(1);
                }
            }
        }
        record["capture_mode"] = json!(if self.active.is_some() {
            LocalCaptureMode::Detailed
        } else {
            LocalCaptureMode::Standard
        });
        if let Some(id) = capture {
            record["capture_id"] = json!(id.to_string());
        }
        if tail.is_some() && self.active.is_none() {
            record["capture_tail_completion"] = json!(true);
        }
        true
    }

    pub(crate) fn coverage(&self) -> Vec<SourceCoverage> {
        LocalDiagnosticSource::ALL
            .into_iter()
            .map(|source| {
                self.sources
                    .get(&source)
                    .cloned()
                    .unwrap_or_else(|| SourceCoverage::unregistered(source))
            })
            .collect()
    }
}

fn pending_key(record: &Value) -> Option<String> {
    let fields = &record["fields"];
    let name = fields["event.name"].as_str()?;
    if name.starts_with("connection.attempt.") {
        return Some(format!(
            "attempt:{}:{}",
            fields["connect_id"].as_str()?,
            fields["attempt_index"].as_u64()?
        ));
    }
    for (prefix, key, root) in [
        ("connection.", "connect_id", false),
        ("address.lookup.", "lookup_id", false),
        ("network.recovery.", "action_id", false),
        ("connection.", "connection_id", true),
    ] {
        let value = if root { &record[key] } else { &fields[key] };
        if name.starts_with(prefix) {
            if let Some(id) = value.as_str() {
                return Some(format!("{key}:{id}"));
            }
        }
    }
    None
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
