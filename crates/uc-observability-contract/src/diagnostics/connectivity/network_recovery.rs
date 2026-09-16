//! 中转状态与恢复动作分别结算；提交 network_change 不代表中转已恢复。
use super::{
    connection::peer_context, emit_local, millis, record::LocalEvent, NetworkRecorder,
    ObservationContext,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::time::Instant;

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkRecoveryTrigger {
    Demand,
    ConfirmedPathFailure,
    Watchdog,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkRecoveryResult {
    Submitted,
    TimedOut,
    Interrupted,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DnsProbeStage {
    Startup,
    AfterReset,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum DnsProbeResult {
    Resolved { address_count: u32 },
    Failed,
    Unavailable,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum NetworkRecoveryEvent {
    Started {
        action_id: uuid::Uuid,
        trigger: NetworkRecoveryTrigger,
    },
    Finished {
        action_id: uuid::Uuid,
        outcome: NetworkRecoveryResult,
        duration_ms: u64,
    },
    RelayStatus {
        connected_count: u32,
        known_count: u32,
    },
    DnsProbe {
        stage: DnsProbeStage,
        result: DnsProbeResult,
    },
}

impl NetworkRecoveryEvent {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Started { .. } => "network.recovery.started",
            Self::Finished { .. } => "network.recovery.finished",
            Self::RelayStatus { .. } => "relay.status.observed",
            Self::DnsProbe { .. } => "network.dns_probe.finished",
        }
    }
    pub(super) fn level(&self) -> &'static str {
        if matches!(
            self,
            Self::Finished {
                outcome: NetworkRecoveryResult::TimedOut | NetworkRecoveryResult::Interrupted,
                ..
            } | Self::DnsProbe {
                result: DnsProbeResult::Failed | DnsProbeResult::Unavailable,
                ..
            }
        ) {
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
        fields.insert("subject".into(), json!("local_endpoint"));
        fields
    }
}

impl NetworkRecorder {
    pub fn recovery_action(
        &self,
        peer: [u8; 32],
        trigger: NetworkRecoveryTrigger,
    ) -> RecoveryActionObservation {
        let observation = RecoveryActionObservation {
            recorder: self.clone(),
            context: peer_context(peer),
            id: uuid::Uuid::new_v4(),
            started: Instant::now(),
            finished: false,
        };
        self.recovery_event(
            &observation.context,
            NetworkRecoveryEvent::Started {
                action_id: observation.id,
                trigger,
            },
        );
        observation
    }
    pub fn local_relay_status(&self, peer: [u8; 32], connected_count: u32, known_count: u32) {
        self.recovery_event(
            &peer_context(peer),
            NetworkRecoveryEvent::RelayStatus {
                connected_count,
                known_count,
            },
        );
    }
    pub fn dns_probe(&self, peer: [u8; 32], stage: DnsProbeStage, result: DnsProbeResult) {
        self.recovery_event(
            &peer_context(peer),
            NetworkRecoveryEvent::DnsProbe { stage, result },
        );
    }
    fn recovery_event(&self, context: &ObservationContext, record: NetworkRecoveryEvent) {
        tracing::dispatcher::with_default(&self.dispatcher, || {
            emit_local(LocalEvent::NetworkRecovery { record }, context)
        });
    }
}

pub struct RecoveryActionObservation {
    recorder: NetworkRecorder,
    context: ObservationContext,
    id: uuid::Uuid,
    started: Instant,
    finished: bool,
}

impl RecoveryActionObservation {
    pub fn finish(mut self, outcome: NetworkRecoveryResult) {
        self.finished = true;
        self.emit_finish(outcome);
    }
    fn emit_finish(&self, outcome: NetworkRecoveryResult) {
        self.recorder.recovery_event(
            &self.context,
            NetworkRecoveryEvent::Finished {
                action_id: self.id,
                outcome,
                duration_ms: millis(self.started.elapsed()),
            },
        );
    }
}

impl Drop for RecoveryActionObservation {
    fn drop(&mut self) {
        if !self.finished {
            self.emit_finish(NetworkRecoveryResult::Interrupted);
        }
    }
}
