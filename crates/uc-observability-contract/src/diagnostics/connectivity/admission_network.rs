//! 配对通信边界的有限快照；计数为连接累计值，不推断重传或慢连接原因。
use super::{
    emit_local, record::LocalEvent, AdmissionExchangeSide, NetworkPathKind, ObservationContext,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionNetworkPoint {
    Connected,
    ExchangeStarted,
    ExchangeFinished,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionNetworkSnapshot {
    pub path: NetworkPathKind,
    pub rtt_us: Option<u64>,
    pub lost_packets_total: u64,
    pub sent_datagrams_total: u64,
    pub received_datagrams_total: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AdmissionNetworkEvent {
    side: AdmissionExchangeSide,
    point: AdmissionNetworkPoint,
    snapshot: AdmissionNetworkSnapshot,
}
impl AdmissionNetworkEvent {
    pub(super) fn fields(&self) -> Map<String, Value> {
        let mut fields = Map::new();
        fields.insert("uc.role".into(), json!(self.side));
        fields.insert("point".into(), json!(self.point));
        if let Value::Object(snapshot) = json!(self.snapshot) {
            fields.extend(snapshot);
        }
        fields.insert(
            "rtt_measurement".into(),
            json!(if self.snapshot.rtt_us.is_some() {
                "available"
            } else {
                "unavailable"
            }),
        );
        fields.insert("retransmission_measurement".into(), json!("unsupported"));
        fields
    }
}

pub fn record_admission_network_snapshot(
    side: AdmissionExchangeSide,
    point: AdmissionNetworkPoint,
    snapshot: AdmissionNetworkSnapshot,
) {
    emit_local(
        LocalEvent::AdmissionNetwork {
            record: AdmissionNetworkEvent {
                side,
                point,
                snapshot,
            },
        },
        &ObservationContext::capture(),
    );
}
