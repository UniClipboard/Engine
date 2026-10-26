//! 仅测试观察或持有原wire边界；实际结果与确认仍通过同一原生实现。
use super::{fixture_for, ReceiveExports};
use async_trait::async_trait;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectionLease, ConnectivityError, PeerSendStream, ProgressBound, ProgressContactNotice,
    ProgressExchangeDeadline, ProgressReceiveError, ProgressTaskAuthorityPort,
    ProgressTerminalNotice, ProgressUpdateNotice, ProgressWirePort, StreamLease, UniStreamLease,
};
use uc_core::file_transfer::{OutboundProgressSummary, ProgressIdentity, MAX_PROGRESS_ITEMS};

pub(crate) fn decorate_progress_wire(
    path: &Path,
    inner: Arc<dyn ProgressWirePort>,
) -> Arc<dyn ProgressWirePort> {
    match fixture_for(path) {
        Some(fixture) => Arc::new(ObservedWire { inner, fixture }),
        None => inner,
    }
}
struct ObservedWire {
    inner: Arc<dyn ProgressWirePort>,
    fixture: Arc<ReceiveExports>,
}
#[async_trait]
impl ProgressWirePort for ObservedWire {
    async fn summary(
        &self,
        stream: &mut UniStreamLease<Box<dyn PeerSendStream>>,
        connection: &ConnectionLease,
        source: ProgressIdentity,
        summary: OutboundProgressSummary,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        self.inner
            .summary(stream, connection, source, summary, deadline, cancel)
            .await
    }
    async fn contact(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        notice: ProgressContactNotice,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        self.inner
            .contact(stream, connection, notice, deadline, cancel)
            .await
    }
    async fn update(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        notice: ProgressUpdateNotice,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        self.inner
            .update(stream, connection, notice, deadline, cancel)
            .await
    }
    async fn register(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        source: ProgressIdentity,
        authority: &dyn ProgressTaskAuthorityPort,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<ProgressBound, ProgressReceiveError> {
        self.inner
            .register(stream, connection, source, authority, deadline, cancel)
            .await
    }
    async fn terminal(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        notice: ProgressTerminalNotice,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<ProgressTerminalNotice, ProgressReceiveError> {
        if self.fixture.hold_terminals.load(Ordering::SeqCst) {
            self.fixture.terminal_held.notify_one();
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(ConnectivityError::Stopped.into()),
                permit = self.fixture.terminal_release.acquire() => permit.unwrap().forget(),
            }
        }
        let acknowledged = self
            .inner
            .terminal(stream, connection, notice, deadline, cancel)
            .await?;
        if acknowledged == notice {
            let mut observed = self.fixture.terminal_acks.lock().unwrap();
            // 同一结果在重连后可以再次精确确认；记录按业务结果去重，始终有界。
            if observed.len() < MAX_PROGRESS_ITEMS
                && !observed.iter().any(|old| {
                    old.source == acknowledged.source
                        && old.attempt == acknowledged.attempt
                        && old.result == acknowledged.result
                })
            {
                observed.push(acknowledged);
                self.fixture.terminal_ack_changed.notify_waiters();
            }
        }
        Ok(acknowledged)
    }
}
impl ReceiveExports {
    pub async fn matching_acks(
        &self,
        source: ProgressIdentity,
        attempt: ProgressIdentity,
        count: usize,
    ) -> Vec<ProgressTerminalNotice> {
        timeout(Duration::from_secs(20), async {
            loop {
                let changed = self.terminal_ack_changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let values = self
                    .terminal_acks
                    .lock()
                    .unwrap()
                    .iter()
                    .copied()
                    .filter(|notice| notice.source == source && notice.attempt == attempt)
                    .collect::<Vec<_>>();
                if values.len() >= count {
                    return values;
                }
                changed.await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            let observed = self.terminal_acks.lock().unwrap();
            let same_source = observed.iter().filter(|notice| notice.source == source).count();
            let same_attempt = observed.iter().filter(|notice| notice.attempt == attempt).count();
            panic!("expected {count} matching real ACKs; total={}, same_source={same_source}, same_attempt={same_attempt}", observed.len());
        })
    }
}
