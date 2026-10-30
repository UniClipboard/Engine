//! 一次完整通知；当前登记、活动发送或终态精确ACK共用原预算。
//! 本地权威读取之前冻结连接请求；会话退休后不能为旧动作重取新会话资格。
use super::{
    ElapsedClockPort, ProgressBound, ProgressContactNotice, ProgressExchangeDeadline,
    ProgressLiveRoundPort, ProgressReceiveError, ProgressSnapshotProviderPort,
    ProgressTaskAuthorityPort, ProgressTaskDescriptor, ProgressTerminalNotice,
    ProgressTerminalRoundPort, ProgressUpdateNotice, ProgressWirePort,
};
use crate::connectivity::{
    ConnectionLease, ConnectionProtocol, ConnectionRequest, ConnectivityManager,
};
use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{
    ProgressActive, ProgressIdentity, ProgressTerminal, TERMINAL_ROUND_LIMIT,
};
use uc_core::ids::DeviceId;

mod legacy;

pub struct TransferProgressProducer {
    manager: ConnectivityManager,
    wire: Arc<dyn ProgressWirePort>,
    authority: Arc<dyn ProgressTaskAuthorityPort>,
    snapshots: Arc<dyn ProgressSnapshotProviderPort>,
    clock: Arc<dyn ElapsedClockPort>,
}
impl TransferProgressProducer {
    pub fn new(
        manager: ConnectivityManager,
        wire: Arc<dyn ProgressWirePort>,
        authority: Arc<dyn ProgressTaskAuthorityPort>,
        snapshots: Arc<dyn ProgressSnapshotProviderPort>,
        clock: Arc<dyn ElapsedClockPort>,
    ) -> Self {
        Self {
            manager,
            wire,
            authority,
            snapshots,
            clock,
        }
    }
    pub async fn register_current(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
        cancel: &CancellationToken,
    ) -> Result<ProgressBound, ProgressReceiveError> {
        let request = self.manager.request(*peer);
        let deadline = ProgressExchangeDeadline::new(
            self.clock.clone(),
            self.clock.elapsed()?.saturating_add(TERMINAL_ROUND_LIMIT),
        )?;
        if self.authority.current_task(peer, source).is_none() {
            return Err(ProgressReceiveError::UnknownTask);
        }
        let connection = self.connect(request, &deadline, cancel).await?;
        self.register_on(&connection, source, &deadline, cancel)
            .await
    }
    async fn connect(
        &self,
        request: ConnectionRequest,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<ConnectionLease, ProgressReceiveError> {
        deadline.check(cancel)?;
        let connection = request.connect(deadline.instant(), cancel).await?;
        deadline.check(cancel)?;
        match connection.protocol() {
            ConnectionProtocol::TransferProgressV2 | ConnectionProtocol::TransferProgressV1 => {
                Ok(connection)
            }
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
        if connection.protocol() == ConnectionProtocol::TransferProgressV1 {
            return Err(ProgressReceiveError::Unsupported);
        }
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
        let request = self.manager.request(*peer);
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
        let connection = self.connect(request, deadline, cancel).await?;
        if connection.protocol() == ConnectionProtocol::TransferProgressV1 {
            self.report_legacy_summary(&connection, *peer, expected, deadline, cancel)
                .await?;
            // 旧帧完整交付仍没有业务采用ACK，原监督者不能把该结果记为确认送达。
            return Err(ProgressReceiveError::Unsupported);
        }
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

#[async_trait]
impl ProgressLiveRoundPort for TransferProgressProducer {
    async fn report_contact(
        &self,
        peer: &DeviceId,
        expected: ProgressTaskDescriptor,
        sequence: u64,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        let request = self.manager.request(*peer);
        let check = || -> Result<(), ProgressReceiveError> {
            let current = self
                .authority
                .current_task(peer, expected.source)
                .ok_or(ProgressReceiveError::UnknownTask)?;
            if current.descriptor != expected
                || current
                    .items
                    .iter()
                    .all(|item| current.notification_closed.contains(item))
            {
                return Err(ProgressReceiveError::Stale);
            }
            deadline.check(cancel)
        };
        check()?;
        let connection = self.connect(request, deadline, cancel).await?;
        if connection.protocol() == ConnectionProtocol::TransferProgressV1 {
            self.report_legacy_summary(&connection, *peer, expected, deadline, cancel)
                .await?;
            check()?;
            return Ok(());
        }
        let bound = self
            .register_on(&connection, expected.source, deadline, cancel)
            .await?;
        if bound.descriptor != expected {
            return Err(ProgressReceiveError::Stale);
        }
        let mut stream = connection
            .open_bi_with_cancel(deadline.instant(), cancel)
            .await?;
        check()?;
        self.wire
            .contact(
                &mut stream,
                &connection,
                ProgressContactNotice {
                    binding: bound.binding,
                    source: expected.source,
                    attempt: expected.attempt,
                    revision: expected.revision,
                    sequence,
                },
                deadline,
                cancel,
            )
            .await?;
        self.manager.retain(&connection).await?;
        check()?;
        Ok(())
    }

    async fn report_live(
        &self,
        peer: &DeviceId,
        expected: ProgressTaskDescriptor,
        update: ProgressActive,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        let request = self.manager.request(*peer);
        let check = || -> Result<(), ProgressReceiveError> {
            let current = self
                .authority
                .current_task(peer, expected.source)
                .ok_or(ProgressReceiveError::UnknownTask)?;
            if current.descriptor != expected {
                return Err(ProgressReceiveError::Stale);
            }
            if !current.items.contains(&update.item) {
                return Err(ProgressReceiveError::InvalidPlan);
            }
            if current.notification_closed.contains(&update.item) {
                return Err(ProgressReceiveError::Stale);
            }
            deadline.check(cancel)
        };
        check()?;
        let connection = self.connect(request, deadline, cancel).await?;
        if connection.protocol() == ConnectionProtocol::TransferProgressV1 {
            self.report_legacy_summary(&connection, *peer, expected, deadline, cancel)
                .await?;
            check()?;
            return Ok(());
        }
        let bound = self
            .register_on(&connection, expected.source, deadline, cancel)
            .await?;
        if bound.descriptor != expected {
            return Err(ProgressReceiveError::Stale);
        }
        let mut stream = connection
            .open_bi_with_cancel(deadline.instant(), cancel)
            .await?;
        check()?;
        self.wire
            .update(
                &mut stream,
                &connection,
                ProgressUpdateNotice {
                    binding: bound.binding,
                    source: expected.source,
                    attempt: expected.attempt,
                    revision: expected.revision,
                    update,
                },
                deadline,
                cancel,
            )
            .await?;
        self.manager.retain(&connection).await?;
        check()?;
        Ok(())
    }
}
