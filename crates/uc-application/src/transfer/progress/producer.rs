//! 已结算结果的一次完整通知；当前登记、终态与精确ACK共用原预算。
use super::{
    ElapsedClockPort, ProgressBound, ProgressExchangeDeadline, ProgressReceiveError,
    ProgressTaskAuthorityPort, ProgressTaskDescriptor, ProgressTerminalNotice,
    ProgressTerminalRoundPort, ProgressWirePort,
};
use crate::connectivity::{ConnectionLease, ConnectionProtocol, ConnectivityManager};
use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{ProgressIdentity, ProgressTerminal, TERMINAL_ROUND_LIMIT};
use uc_core::ids::DeviceId;

pub struct TransferProgressProducer {
    manager: ConnectivityManager,
    wire: Arc<dyn ProgressWirePort>,
    authority: Arc<dyn ProgressTaskAuthorityPort>,
    clock: Arc<dyn ElapsedClockPort>,
}
impl TransferProgressProducer {
    pub fn new(
        manager: ConnectivityManager,
        wire: Arc<dyn ProgressWirePort>,
        authority: Arc<dyn ProgressTaskAuthorityPort>,
        clock: Arc<dyn ElapsedClockPort>,
    ) -> Self {
        Self {
            manager,
            wire,
            authority,
            clock,
        }
    }
    pub async fn register_current(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
        cancel: &CancellationToken,
    ) -> Result<ProgressBound, ProgressReceiveError> {
        let deadline = ProgressExchangeDeadline::new(
            self.clock.clone(),
            self.clock.elapsed()?.saturating_add(TERMINAL_ROUND_LIMIT),
        )?;
        if self.authority.current_task(peer, source).is_none() {
            return Err(ProgressReceiveError::UnknownTask);
        }
        let connection = self.connect(peer, &deadline, cancel).await?;
        self.register_on(&connection, source, &deadline, cancel)
            .await
    }
    async fn connect(
        &self,
        peer: &DeviceId,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<ConnectionLease, ProgressReceiveError> {
        deadline.check(cancel)?;
        let connection = self
            .manager
            .request(*peer)
            .connect(deadline.instant(), cancel)
            .await?;
        deadline.check(cancel)?;
        match connection.protocol() {
            ConnectionProtocol::TransferProgressV2 => Ok(connection),
            ConnectionProtocol::TransferProgressV1 => Err(ProgressReceiveError::Unsupported),
            _ => Err(ProgressReceiveError::Protocol),
        }
    }
    async fn register_on(
        &self,
        connection: &ConnectionLease,
        source: ProgressIdentity,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<ProgressBound, ProgressReceiveError> {
        let mut stream = connection
            .open_bi_with_cancel(deadline.instant(), cancel)
            .await?;
        deadline.check(cancel)?;
        let bound = self
            .wire
            .register(
                &mut stream,
                connection,
                source,
                self.authority.as_ref(),
                deadline,
                cancel,
            )
            .await?;
        self.manager.retain(connection).await?;
        if bound.descriptor.source != source {
            return Err(ProgressReceiveError::Stale);
        }
        self.check_current(&connection.peer(), bound, deadline, cancel)?;
        Ok(bound)
    }
    fn check_current(
        &self,
        peer: &DeviceId,
        bound: ProgressBound,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        let current = self.authority.current_task(peer, bound.descriptor.source);
        deadline.check(cancel)?;
        if current.is_none_or(|current| current.descriptor != bound.descriptor) {
            return Err(ProgressReceiveError::Stale);
        }
        Ok(())
    }
}

#[async_trait]
impl ProgressTerminalRoundPort for TransferProgressProducer {
    async fn report_settled(
        &self,
        peer: &DeviceId,
        expected: ProgressTaskDescriptor,
        result: ProgressTerminal,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        let source = expected.source;
        let attempt = expected.attempt;
        let current = self
            .authority
            .current_task(peer, source)
            .ok_or(ProgressReceiveError::UnknownTask)?;
        if current.descriptor != expected {
            return Err(ProgressReceiveError::Stale);
        }
        if !current.items.contains(&result.item) {
            return Err(ProgressReceiveError::InvalidPlan);
        }
        let connection = self.connect(peer, deadline, cancel).await?;
        let bound = self
            .register_on(&connection, source, deadline, cancel)
            .await?;
        if bound.descriptor != current.descriptor {
            return Err(ProgressReceiveError::Stale);
        }
        let notice = ProgressTerminalNotice {
            binding: bound.binding,
            source,
            attempt,
            revision: bound.descriptor.revision,
            result,
        };
        let mut stream = connection
            .open_bi_with_cancel(deadline.instant(), cancel)
            .await?;
        self.check_current(peer, bound, deadline, cancel)?;
        let adopted = self
            .wire
            .terminal(&mut stream, &connection, notice, deadline, cancel)
            .await?;
        // 流仍由本动作持有；保留读取与后置权威核对不提前释放许可。
        self.manager.retain(&connection).await?;
        self.check_current(peer, bound, deadline, cancel)?;
        if adopted != notice {
            return Err(ProgressReceiveError::Conflict);
        }
        Ok(())
    }
}
