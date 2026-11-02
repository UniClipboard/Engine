//! 只在既有双 Engine 根 fixture 内改变真实 presence 回复半流。
use super::{registered_fixture, ReceiveStreams};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::time::Instant;
use uc_application::deps::{
    PeerReachabilityWireError, PeerReachabilityWirePort, PeerReceiveStream, PeerSendStream,
};
use uc_core::error_class::ErrorClass;

#[derive(Clone)]
struct WireRecord {
    started_at: Instant,
    returned_at: Instant,
    nonce: [u8; 16],
    operation: &'static str,
    reset_succeeded: Option<bool>,
    reset_completed_at: Option<Instant>,
    reset_class: Option<&'static str>,
    result_class: Option<&'static str>,
    succeeded: bool,
}

struct Correlation {
    reset_completed_at: Instant,
    write_returned_at: Instant,
    read_returned_at: Instant,
}

#[derive(Default)]
struct State {
    armed_until: Option<Instant>,
    records: Vec<WireRecord>,
    truncated: bool,
    last_correlation: Option<Correlation>,
}

#[derive(Default)]
pub(crate) struct PresenceFault {
    state: Mutex<State>,
}
impl PresenceFault {
    pub fn arm(&self, deadline: Instant) {
        self.state.lock().unwrap().armed_until = Some(deadline);
    }
    pub fn disarm(&self) {
        self.state.lock().unwrap().armed_until = None;
    }
    fn armed(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .armed_until
            .is_some_and(|deadline| Instant::now() < deadline)
    }
    fn record(&self, record: WireRecord) {
        let mut state = self.state.lock().unwrap();
        if state.records.len() == 64 {
            state.records.remove(0);
            state.truncated = true;
        }
        state.records.push(record);
    }
    pub fn correlated_failure(&self, reader: &Self, after: Instant) -> Option<(Instant, Instant)> {
        // nonce 只用于内存中的真实请求关联，不进入任何工件。
        let writes = self.state.lock().unwrap().records.clone();
        let reads = reader.state.lock().unwrap().records.clone();
        for write in writes.iter().filter(|event| {
            event.started_at >= after
                && event.operation == "write_reply"
                && event.reset_succeeded == Some(true)
                && !event.succeeded
                && event.result_class.is_some()
        }) {
            let Some(reset_completed_at) = write.reset_completed_at else {
                continue;
            };
            if let Some(read) = reads.iter().find(|event| {
                event.returned_at >= reset_completed_at
                    && event.operation == "read_reply"
                    && event.nonce == write.nonce
                    && !event.succeeded
                    && event.result_class.is_some()
            }) {
                self.state.lock().unwrap().last_correlation = Some(Correlation {
                    reset_completed_at,
                    write_returned_at: write.returned_at,
                    read_returned_at: read.returned_at,
                });
                return Some((write.returned_at, read.returned_at));
            }
        }
        None
    }
    pub fn snapshot(&self, origin: Instant) -> Value {
        let state = self.state.lock().unwrap();
        let elapsed = |at: Instant| {
            u64::try_from(at.saturating_duration_since(origin).as_micros()).unwrap_or(u64::MAX)
        };
        let records: Vec<_> = state.records.iter().enumerate().map(|(index, event)| json!({
            "wire_alias": index + 1, "operation": event.operation,
            "started_us": elapsed(event.started_at), "returned_us": elapsed(event.returned_at),
            "reset_succeeded": event.reset_succeeded, "reset_error_class": event.reset_class,
            "reset_completed_us": event.reset_completed_at.map(elapsed),
            "reset_before_write_return": event.reset_completed_at.map(|at| at <= event.returned_at),
            "succeeded": event.succeeded, "error_class": event.result_class,
        })).collect();
        let correlation = state.last_correlation.as_ref().map(|pair| {
            json!({
                "same_challenge": true,
                "reset_completed_us": elapsed(pair.reset_completed_at),
                "write_failed_us": elapsed(pair.write_returned_at),
                "read_failed_us": elapsed(pair.read_returned_at),
                "read_after_reset": pair.read_returned_at >= pair.reset_completed_at,
                "reset_before_write_return": pair.write_returned_at >= pair.reset_completed_at,
            })
        });
        json!({ "records": records, "truncated": state.truncated, "correlation": correlation })
    }
}

pub(crate) fn decorate_presence(
    path: &Path,
    inner: Arc<dyn PeerReachabilityWirePort>,
) -> Arc<dyn PeerReachabilityWirePort> {
    match registered_fixture(path) {
        Some(fixture) => Arc::new(ObservedPresence { inner, fixture }),
        None => inner,
    }
}

struct ObservedPresence {
    inner: Arc<dyn PeerReachabilityWirePort>,
    fixture: Arc<ReceiveStreams>,
}
#[async_trait]
impl PeerReachabilityWirePort for ObservedPresence {
    async fn read_admission(
        &self,
        receive: &mut dyn PeerReceiveStream,
        deadline: Instant,
    ) -> Result<(), PeerReachabilityWireError> {
        self.inner.read_admission(receive, deadline).await
    }
    async fn write_admission(
        &self,
        send: &mut dyn PeerSendStream,
        deadline: Instant,
    ) -> Result<(), PeerReachabilityWireError> {
        self.inner.write_admission(send, deadline).await
    }
    async fn read_confirmation(
        &self,
        receive: &mut dyn PeerReceiveStream,
        deadline: Instant,
    ) -> Result<bool, PeerReachabilityWireError> {
        self.inner.read_confirmation(receive, deadline).await
    }
    async fn write_confirmation(
        &self,
        send: &mut dyn PeerSendStream,
        admitted: bool,
        deadline: Instant,
    ) -> Result<(), PeerReachabilityWireError> {
        self.inner
            .write_confirmation(send, admitted, deadline)
            .await
    }
    async fn read_challenge(
        &self,
        receive: &mut dyn PeerReceiveStream,
        deadline: Instant,
    ) -> Result<[u8; 16], PeerReachabilityWireError> {
        self.inner.read_challenge(receive, deadline).await
    }
    async fn write_challenge(
        &self,
        send: &mut dyn PeerSendStream,
        nonce: &[u8; 16],
        deadline: Instant,
    ) -> Result<(), PeerReachabilityWireError> {
        self.inner.write_challenge(send, nonce, deadline).await
    }
    async fn read_reply(
        &self,
        receive: &mut dyn PeerReceiveStream,
        nonce: &[u8; 16],
        deadline: Instant,
    ) -> Result<bool, PeerReachabilityWireError> {
        let observed = self.fixture.presence.armed();
        let started_at = Instant::now();
        let result = self.inner.read_reply(receive, nonce, deadline).await;
        let returned_at = Instant::now();
        if observed || self.fixture.presence.armed() {
            self.fixture.presence.record(WireRecord {
                started_at,
                returned_at,
                nonce: *nonce,
                operation: "read_reply",
                reset_succeeded: None,
                reset_completed_at: None,
                reset_class: None,
                result_class: result.as_ref().err().map(ErrorClass::class),
                succeeded: result.is_ok(),
            });
        }
        result
    }
    async fn write_reply(
        &self,
        send: &mut dyn PeerSendStream,
        nonce: &[u8; 16],
        admitted: bool,
        deadline: Instant,
    ) -> Result<(), PeerReachabilityWireError> {
        let observed = admitted && self.fixture.presence.armed();
        let started_at = Instant::now();
        let reset = observed.then(|| {
            let result = send.reset(0);
            // 同步 reset 成功后立即计时；不把后续写入完成当作 reset 的时间。
            let reset_completed_at = result.is_ok().then(Instant::now);
            (result, reset_completed_at)
        });
        // reset 后仍执行原写入与 FIN；只返回原 wire 的真实结果。
        let result = self
            .inner
            .write_reply(send, nonce, admitted, deadline)
            .await;
        let returned_at = Instant::now();
        if let Some((reset, reset_completed_at)) = reset {
            self.fixture.presence.record(WireRecord {
                started_at,
                returned_at,
                nonce: *nonce,
                operation: "write_reply",
                reset_succeeded: Some(reset.is_ok()),
                reset_completed_at,
                reset_class: reset.as_ref().err().map(ErrorClass::class),
                result_class: result.as_ref().err().map(ErrorClass::class),
                succeeded: result.is_ok(),
            });
        }
        result
    }
}
