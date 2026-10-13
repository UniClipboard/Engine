use super::budget::ensure_live;
use super::retention::prepare_connection_retention;
use super::{
    membership_history_purpose, HistoryIncoming, HistoryLocalWorkPort, HistoryPeerBindingPort,
    HistoryReceiverPort, HistoryWirePort,
};
use crate::connectivity::{
    ConnectionLease, ConnectionMode, ConnectionProtocol, ConnectivityError, ConnectivityManager,
    PendingInboundLease, StreamLease,
};
use async_trait::async_trait;
use futures::{stream::FuturesUnordered, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{timeout_at, Instant};
use uc_core::error_class::ErrorClass;
use uc_core::membership::{MembershipHistoryExchangeEndpointPort, MembershipHistoryExchangeError};
use uc_observability_contract::diagnostics::{
    describe_membership_exchange, scope_operation_diagnostics,
};
use uc_observability_contract::{error_source::io_error_kind, uc_debug, uc_warn};

pub struct MembershipHistoryReceiver {
    manager: ConnectivityManager,
    wire: Arc<dyn HistoryWirePort>,
    binding: Arc<dyn HistoryPeerBindingPort>,
    local_work: Arc<dyn HistoryLocalWorkPort>,
    endpoint: Arc<dyn MembershipHistoryExchangeEndpointPort>,
}
impl MembershipHistoryReceiver {
    pub fn new(
        manager: ConnectivityManager,
        wire: Arc<dyn HistoryWirePort>,
        binding: Arc<dyn HistoryPeerBindingPort>,
        local_work: Arc<dyn HistoryLocalWorkPort>,
        endpoint: Arc<dyn MembershipHistoryExchangeEndpointPort>,
    ) -> Self {
        Self {
            manager,
            wire,
            binding,
            local_work,
            endpoint,
        }
    }
    async fn process(
        &self,
        connection: &ConnectionLease,
        mut stream: StreamLease,
    ) -> Result<(), MembershipHistoryExchangeError> {
        let deadline = stream.received_at.unwrap_or_else(Instant::now) + Duration::from_secs(10);
        ensure_live(deadline)?;
        timeout_at(deadline, async {
            let incoming = self.wire.read_request(stream.receive.as_mut()).await?;
            let (mut response, retained) =
                self.respond(connection, stream, incoming, deadline).await?;
            if !retained {
                let _drain = timeout_at(deadline, response.send.settled()).await;
                connection.close();
            }
            Ok(())
        })
        .await
        .map_err(MembershipHistoryExchangeError::transport_from)?
    }
    async fn respond(
        &self,
        connection: &ConnectionLease,
        mut stream: StreamLease,
        incoming: HistoryIncoming,
        deadline: Instant,
    ) -> Result<(StreamLease, bool), MembershipHistoryExchangeError> {
        connection
            .check_current()
            .await
            .map_err(MembershipHistoryExchangeError::transport_from)?;
        incoming
            .observation
            .scope(scope_operation_diagnostics(async {
                describe_membership_exchange(membership_history_purpose(&incoming.message), true);
                let endpoint = self.endpoint.clone();
                let peer = connection.peer();
                let working_connection = connection.clone();
                ensure_live(deadline)?;
                let work_hold = stream.hold_for_work();
                let result = self
                    .local_work
                    .execute(Box::pin(async move {
                        let _work_hold = work_hold;
                        working_connection
                            .check_current()
                            .await
                            .map_err(MembershipHistoryExchangeError::transport_from)?;
                        endpoint
                            .handle_membership_history_exchange(&peer, incoming.message)
                            .await
                    }))
                    .await;
                connection
                    .check_current()
                    .await
                    .map_err(MembershipHistoryExchangeError::transport_from)?;
                let retained =
                    prepare_connection_retention(&self.manager, connection, deadline).await;
                self.wire
                    .respond(
                        stream.send.as_mut(),
                        result,
                        connection.protocol(),
                        retained,
                    )
                    .await?;
                Ok((stream, retained))
            }))
            .await
    }
}
#[async_trait]
impl HistoryReceiverPort for MembershipHistoryReceiver {
    async fn receive(
        &self,
        mut pending: PendingInboundLease,
    ) -> Result<(), MembershipHistoryExchangeError> {
        let deadline = pending.deadline;
        ensure_live(deadline)?;
        let protocol = pending.connection().protocol();
        if !matches!(
            protocol,
            ConnectionProtocol::MembershipHistory
                | ConnectionProtocol::MultiplexedMembershipHistory
        ) {
            return Err(MembershipHistoryExchangeError::Rejected);
        }
        let (connection, retained, mut response_stream) = timeout_at(deadline, async {
            let incoming = self.wire.read_request(pending.receive.as_mut()).await?;
            let grant = match self
                .binding
                .bind(pending.connection(), &incoming.message)
                .await
            {
                Ok(grant) => grant,
                Err(source) => {
                    self.wire
                        .respond(
                            pending.send.as_mut(),
                            Err(MembershipHistoryExchangeError::Rejected),
                            protocol,
                            false,
                        )
                        .await?;
                    let _drain = timeout_at(deadline, pending.send.settled()).await;
                    return Err(MembershipHistoryExchangeError::transport_from(source));
                }
            };
            let (connection, stream) = pending
                .bind(grant)
                .await
                .map_err(MembershipHistoryExchangeError::transport_from)?;
            let (response_stream, retained) = self
                .respond(&connection, stream, incoming, deadline)
                .await?;
            Ok::<_, MembershipHistoryExchangeError>((connection, retained, response_stream))
        })
        .await
        .map_err(MembershipHistoryExchangeError::transport_from)??;
        if connection.mode() != ConnectionMode::Multiplexed || !retained {
            // 等待已结束响应的发送结算，再关闭临时连接；不等待对端主动关闭整个连接。
            match timeout_at(deadline, response_stream.send.settled()).await {
                Ok(Ok(())) => {}
                Ok(Err(source)) => uc_debug!(
                    error_kind = source.class(),
                    io_error_kind = io_error_kind(&source),
                    "成员历史临时响应发送已停止"
                ),
                Err(_deadline) => uc_warn!(
                    error_kind = "membership_history_response_drain_timeout",
                    "成员历史临时响应发送结算超时"
                ),
            }
            connection.close();
            return Ok(());
        }
        drop(response_stream);
        let mut exchanges = FuturesUnordered::new();
        loop {
            tokio::select! {
                _ = connection.closed() => break,
                Some(result) = exchanges.next(), if !exchanges.is_empty() => {
                    if let Err(source) = result { uc_warn!(error_kind = "membership_history_stream", io_error_kind = io_error_kind(&source), "成员历史流交换失败"); }
                },
                result = connection.accept_bi(), if exchanges.len() < 4 => match result {
                    Ok(stream) => exchanges.push(self.process(&connection, stream)),
                    Err(source @ (ConnectivityError::TimedOut(_) | ConnectivityError::DeadlineExpired)) if !connection.is_closed() => {
                        uc_warn!(error_kind = "membership_history_inbound_queue_timeout", io_error_kind = io_error_kind(&source), "成员历史过期排队流已释放");
                    }
                    Err(source) => {
                        connection.close();
                        return Err(MembershipHistoryExchangeError::transport_from(source));
                    }
                }
            }
        }
        // 未开始本地工作的 future 可以取消；local-work port 持有已开始提交到结算。
        Ok(())
    }
}
