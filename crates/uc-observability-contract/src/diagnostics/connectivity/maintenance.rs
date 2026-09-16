//! 维护轮次编号只在本进程有效，不携带业务身份，不参与触发去重或调度。
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::{
    emit_local, millis, record::LocalEvent, LocalWorkOutcome, ObservationContext, RecoveryTrigger,
};

static NEXT_ROUND: AtomicU64 = AtomicU64::new(1);
tokio::task_local! { static ROUND: MaintenanceContext; }

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MaintenanceContext {
    round: u64,
    trigger: RecoveryTrigger,
}

impl MaintenanceContext {
    pub(super) fn capture() -> Option<Self> {
        ROUND.try_with(|value| *value).ok()
    }
    pub(super) fn fields(self, fields: &mut Map<String, Value>) {
        fields.insert("maintenance_round".into(), json!(self.round));
        fields.insert("trigger".into(), json!(self.trigger));
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceDisposition {
    Paused,
    Closed,
    Interrupted,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum MaintenanceEvent {
    Requested {
        context: MaintenanceContext,
    },
    Queued {
        context: MaintenanceContext,
    },
    Started {
        context: MaintenanceContext,
        queue_wait_ms: u64,
    },
    Coalesced {
        context: MaintenanceContext,
        into_round: u64,
        queue_wait_ms: u64,
    },
    NotExecuted {
        context: MaintenanceContext,
        reason: MaintenanceDisposition,
        queue_wait_ms: u64,
    },
    Finished {
        context: MaintenanceContext,
        outcome: LocalWorkOutcome,
        duration_ms: u64,
    },
    Updates {
        context: Option<MaintenanceContext>,
        pending_count: u64,
        selected_count: u64,
    },
}

impl MaintenanceEvent {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Requested { .. } => "membership.maintenance.requested",
            Self::Queued { .. } => "membership.maintenance.queued",
            Self::Started { .. } => "membership.maintenance.started",
            Self::Coalesced { .. } => "membership.maintenance.coalesced",
            Self::NotExecuted { .. } => "membership.maintenance.not_executed",
            Self::Finished { .. } => "membership.maintenance.finished",
            Self::Updates { .. } => "membership.maintenance.updates",
        }
    }
    pub(super) fn level(&self) -> &'static str {
        match self {
            Self::Finished {
                outcome: LocalWorkOutcome::Corrupt,
                ..
            } => "ERROR",
            Self::Finished {
                outcome:
                    LocalWorkOutcome::Error | LocalWorkOutcome::Rejected | LocalWorkOutcome::Interrupted,
                ..
            } => "WARN",
            Self::NotExecuted {
                reason: MaintenanceDisposition::Closed | MaintenanceDisposition::Interrupted,
                ..
            } => "WARN",
            _ => "INFO",
        }
    }
    pub(super) fn fields(&self) -> Map<String, Value> {
        let mut fields = Map::new();
        let context = match self {
            Self::Requested { context }
            | Self::Queued { context }
            | Self::Started { context, .. }
            | Self::Coalesced { context, .. }
            | Self::NotExecuted { context, .. }
            | Self::Finished { context, .. } => Some(*context),
            Self::Updates { context, .. } => *context,
        };
        if let Some(context) = context {
            context.fields(&mut fields);
        }
        match self {
            Self::Started { queue_wait_ms, .. } => {
                fields.insert("queue_wait_ms".into(), json!(queue_wait_ms));
            }
            Self::Coalesced {
                into_round,
                queue_wait_ms,
                ..
            } => {
                fields.insert("coalesced_into_round".into(), json!(into_round));
                fields.insert("queue_wait_ms".into(), json!(queue_wait_ms));
            }
            Self::NotExecuted {
                reason,
                queue_wait_ms,
                ..
            } => {
                fields.insert("reason".into(), json!(reason));
                fields.insert("queue_wait_ms".into(), json!(queue_wait_ms));
            }
            Self::Finished {
                outcome,
                duration_ms,
                ..
            } => {
                fields.insert("uc.outcome".into(), json!(outcome));
                fields.insert("duration_ms".into(), json!(duration_ms));
            }
            Self::Updates {
                pending_count,
                selected_count,
                ..
            } => {
                fields.insert("pending_count".into(), json!(pending_count));
                fields.insert("selected_count".into(), json!(selected_count));
            }
            _ => {}
        }
        fields
    }
}

pub struct MaintenanceObservation {
    context: MaintenanceContext,
    parent: ObservationContext,
    requested: Instant,
    started: Option<Instant>,
    finished: bool,
}

impl MaintenanceObservation {
    pub fn request(trigger: RecoveryTrigger) -> Self {
        let observation = Self {
            context: MaintenanceContext {
                round: NEXT_ROUND.fetch_add(1, Ordering::Relaxed),
                trigger,
            },
            parent: ObservationContext::capture(),
            requested: Instant::now(),
            started: None,
            finished: false,
        };
        observation.emit(MaintenanceEvent::Requested {
            context: observation.context,
        });
        observation
    }
    pub fn queued(&self) {
        self.emit(MaintenanceEvent::Queued {
            context: self.context,
        });
    }
    pub fn coalesce(mut self, target: &Self) {
        self.finished = true;
        self.emit(MaintenanceEvent::Coalesced {
            context: self.context,
            into_round: target.context.round,
            queue_wait_ms: millis(self.requested.elapsed()),
        });
    }
    pub fn not_executed(mut self, reason: MaintenanceDisposition) {
        self.finished = true;
        self.emit(MaintenanceEvent::NotExecuted {
            context: self.context,
            reason,
            queue_wait_ms: millis(self.requested.elapsed()),
        });
    }
    pub fn start(&mut self) {
        self.started = Some(Instant::now());
        self.emit(MaintenanceEvent::Started {
            context: self.context,
            queue_wait_ms: millis(self.requested.elapsed()),
        });
    }
    pub async fn scope<T>(&self, work: impl Future<Output = T>) -> T {
        self.parent
            .clone()
            .scope(ROUND.scope(self.context, work))
            .await
    }
    pub fn finish(mut self, outcome: LocalWorkOutcome) {
        self.finished = true;
        if let Some(started) = self.started {
            self.emit(MaintenanceEvent::Finished {
                context: self.context,
                outcome,
                duration_ms: millis(started.elapsed()),
            });
        }
    }
    fn emit(&self, record: MaintenanceEvent) {
        emit_local(LocalEvent::Maintenance { record }, &self.parent);
    }
}

impl Drop for MaintenanceObservation {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        match self.started {
            Some(started) => self.emit(MaintenanceEvent::Finished {
                context: self.context,
                outcome: LocalWorkOutcome::Interrupted,
                duration_ms: millis(started.elapsed()),
            }),
            None => self.emit(MaintenanceEvent::NotExecuted {
                context: self.context,
                reason: MaintenanceDisposition::Interrupted,
                queue_wait_ms: millis(self.requested.elapsed()),
            }),
        }
    }
}

pub fn record_pending_group_updates(pending: usize, selected: usize) {
    emit_local(
        LocalEvent::Maintenance {
            record: MaintenanceEvent::Updates {
                context: MaintenanceContext::capture(),
                pending_count: pending as u64,
                selected_count: selected as u64,
            },
        },
        &ObservationContext::capture(),
    );
}

#[cfg(test)]
mod tests {
    use super::super::decode_local_record;
    use super::*;

    #[test]
    fn maintenance_rejects_identity_free_text_and_unknown_fields() {
        let valid = json!({"kind": "maintenance", "record": {"event": "coalesced", "context": {"round": 3, "trigger": "state_changed"}, "into_round": 2, "queue_wait_ms": 9}});
        let name = "membership.maintenance.coalesced";
        assert!(decode_local_record(name, &valid.to_string(), "INFO").is_some());
        for (field, value) in [
            ("peer", json!("PRIVATE")),
            ("queue_wait_ms", json!(-1)),
            ("into_round", json!("PRIVATE")),
            (
                "context",
                json!({"round": 3, "trigger": "peer_online", "device": "PRIVATE"}),
            ),
        ] {
            let mut invalid = valid.clone();
            invalid["record"][field] = value;
            assert!(decode_local_record(name, &invalid.to_string(), "INFO").is_none());
        }
    }
}
