//! 原生宿主的固定事件合同；不接受日志正文、业务身份或自由属性。
use crate::local_recording::LocalRecordingState;
use crate::{LocalDiagnosticSource, OperatingSystem, SourceCollection};
use serde::Serialize;
use serde_json::json;
use std::time::Instant;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostDiagnosticSource {
    Application,
    ShareExtension,
    KeyboardExtension,
    BackgroundService,
}
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostDiagnosticAction {
    RuntimeStart,
    RuntimeStop,
    OwnershipAcquire,
    SecurityPrepare,
}
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostDiagnosticFailure {
    Unavailable,
    PermissionDenied,
    Locked,
    Busy,
    Unknown,
}
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostDiagnosticOutcome {
    Completed,
    Failed(HostDiagnosticFailure),
    Interrupted,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostLifecycleState {
    Foreground,
    Background,
}
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostNetworkKind {
    Wifi,
    Cellular,
    Ethernet,
    Other,
    Unknown,
}

pub enum HostDiagnosticEvent {
    Begin {
        action: HostDiagnosticAction,
    },
    Finish {
        token: String,
        outcome: HostDiagnosticOutcome,
    },
    Lifecycle {
        state: HostLifecycleState,
    },
    NetworkChanged {
        kind: HostNetworkKind,
        available: bool,
    },
    OwnershipReleased,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostDiagnosticRecordStatus {
    Accepted,
    PolicyFiltered,
    CapacityExceeded,
    InvalidToken,
    InvalidSource,
    NotRegistered,
    Unavailable,
    AlreadyShutdown,
}
#[derive(Debug, Clone, Serialize)]
pub struct HostDiagnosticReceipt {
    pub status: HostDiagnosticRecordStatus,
    pub token: Option<String>,
}

impl HostDiagnosticReceipt {
    pub(crate) fn status(status: HostDiagnosticRecordStatus) -> Self {
        Self {
            status,
            token: None,
        }
    }
}

impl HostDiagnosticSource {
    pub(crate) fn allowed(self, platform: OperatingSystem) -> bool {
        match self {
            Self::ShareExtension | Self::KeyboardExtension => platform == OperatingSystem::Ios,
            Self::BackgroundService => {
                matches!(platform, OperatingSystem::Android | OperatingSystem::Ohos)
            }
            Self::Application => true,
        }
    }
    pub(crate) fn source(self) -> LocalDiagnosticSource {
        match self {
            Self::Application => LocalDiagnosticSource::HostApplication,
            Self::ShareExtension => LocalDiagnosticSource::HostShareExtension,
            Self::KeyboardExtension => LocalDiagnosticSource::HostKeyboardExtension,
            Self::BackgroundService => LocalDiagnosticSource::HostBackgroundService,
        }
    }
}

pub(crate) struct HostPending {
    source: HostDiagnosticSource,
    action: HostDiagnosticAction,
    started: Instant,
    capture_id: Option<String>,
}

impl LocalRecordingState {
    pub(crate) fn finish_run(&self) {
        let pending = {
            let mut policy = self
                .capture
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            policy.shutdown();
            let status = policy.status(Instant::now());
            self.capture_checkpoint(&mut policy, &status);
            policy
                .host_pending
                .iter()
                .map(|(id, pending)| (*id, pending.source))
                .collect::<Vec<_>>()
        };
        for (id, source) in pending {
            self.record_host(
                source,
                HostDiagnosticEvent::Finish {
                    token: id.to_string(),
                    outcome: HostDiagnosticOutcome::Interrupted,
                },
            );
        }
        self.checkpoint(
            "diagnostics.run.ended",
            json!({"reason":"runtime_shutdown"}),
        );
    }

    pub(crate) fn record_host(
        &self,
        source: HostDiagnosticSource,
        event: HostDiagnosticEvent,
    ) -> HostDiagnosticReceipt {
        if !source.allowed(self.platform()) {
            return HostDiagnosticReceipt::status(HostDiagnosticRecordStatus::InvalidSource);
        }
        let mut policy = self
            .capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !policy
            .sources
            .get(&source.source())
            .is_some_and(|entry| entry.collection == SourceCollection::Enabled)
        {
            return HostDiagnosticReceipt::status(HostDiagnosticRecordStatus::NotRegistered);
        }
        let now = Instant::now();
        let level = if matches!(
            &event,
            HostDiagnosticEvent::Finish {
                outcome: HostDiagnosticOutcome::Failed(_) | HostDiagnosticOutcome::Interrupted,
                ..
            }
        ) {
            "WARN"
        } else {
            "INFO"
        };
        let status = policy.status(now);
        let mut capture_id = status.capture_id.clone();
        let mut token = None;
        let mut fields = match event {
            HostDiagnosticEvent::Begin { action } => {
                if policy.host_pending.len() >= 1024 {
                    return HostDiagnosticReceipt::status(
                        HostDiagnosticRecordStatus::CapacityExceeded,
                    );
                }
                let id = Uuid::new_v4();
                policy.host_pending.insert(
                    id,
                    HostPending {
                        source,
                        action,
                        started: now,
                        capture_id: capture_id.clone(),
                    },
                );
                token = Some(id.to_string());
                json!({"event.name":"host.action.started", "action": action, "action_id": id.to_string()})
            }
            HostDiagnosticEvent::Finish { token, outcome } => {
                let Ok(id) = Uuid::parse_str(&token) else {
                    return HostDiagnosticReceipt::status(HostDiagnosticRecordStatus::InvalidToken);
                };
                if !policy
                    .host_pending
                    .get(&id)
                    .is_some_and(|pending| pending.source == source)
                {
                    return HostDiagnosticReceipt::status(HostDiagnosticRecordStatus::InvalidToken);
                }
                let Some(pending) = policy.host_pending.remove(&id) else {
                    return HostDiagnosticReceipt::status(HostDiagnosticRecordStatus::InvalidToken);
                };
                capture_id = pending.capture_id;
                json!({"event.name":"host.action.finished", "action":pending.action,"action_id":id.to_string(),"outcome":outcome,
                    "duration_ms":u64::try_from(pending.started.elapsed().as_millis()).unwrap_or(u64::MAX)})
            }
            HostDiagnosticEvent::Lifecycle { state } => {
                let previous = policy.host_lifecycle.insert(source, state);
                if state == HostLifecycleState::Foreground
                    && previous == Some(HostLifecycleState::Background)
                {
                    policy.resumed();
                }
                json!({"event.name":"host.lifecycle.changed", "state":state})
            }
            HostDiagnosticEvent::NetworkChanged { kind, available } => {
                json!({"event.name":"host.network.changed", "kind":kind,"available":available})
            }
            HostDiagnosticEvent::OwnershipReleased => {
                json!({"event.name":"host.ownership.released"})
            }
        };
        fields["source"] = json!(source.source());
        let mut record = json!({"timestamp":chrono::Utc::now().to_rfc3339(), "level":level, "target":"uc.host.diagnostics", "fields":fields});
        if let Some(id) = capture_id {
            record["capture_id"] = json!(id);
        }
        let _context = opentelemetry::Context::new().attach();
        self.annotate(&mut record);
        let include = policy.include(&mut record, now);
        let status = policy.status(now);
        self.capture_checkpoint(&mut policy, &status);
        drop(policy);
        if include {
            self.write_value(&record, source.source());
        }
        HostDiagnosticReceipt {
            status: if include {
                HostDiagnosticRecordStatus::Accepted
            } else {
                HostDiagnosticRecordStatus::PolicyFiltered
            },
            token,
        }
    }
}
