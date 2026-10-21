use super::budget::{ensure_live, exchange_deadline};
use super::retention::prepare_connection_retention;
use super::HistoryWirePort;
use crate::connectivity::{ConnectionLease, ConnectionProtocol, ConnectivityManager};
use crate::space::membership::{
    RestrictedMembershipDelivery, RestrictedMembershipDeliveryError,
    RestrictedMembershipDeliveryPort,
};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    MembershipHistoryAckV3, MembershipHistoryExchangeError, MembershipHistoryExchangePort,
    MembershipHistoryMessage,
};
use uc_observability_contract::diagnostics::{
    describe_membership_exchange, MembershipExchangePurpose,
};
use uc_observability_contract::uc_debug;

struct ExchangeUse {
    manager: ConnectivityManager,
    connection: ConnectionLease,
    deadline: Instant,
    completed: bool,
}
impl Drop for ExchangeUse {
    fn drop(&mut self) {
        if !self.completed {
            let expired = Instant::now() >= self.deadline;
            uc_debug!(
                error_kind = if expired {
                    "history_budget_exhausted"
                } else {
                    "history_exchange_ended_before_budget"
                },
                "成员历史未完成交换释放使用权"
            );
            if expired {
                self.manager.evict(&self.connection);
            }
        }
    }
}

pub struct MembershipHistoryTransport {
    manager: ConnectivityManager,
    wire: Arc<dyn HistoryWirePort>,
}
impl MembershipHistoryTransport {
    pub fn new(manager: ConnectivityManager, wire: Arc<dyn HistoryWirePort>) -> Self {
        Self { manager, wire }
    }
}
#[async_trait]
impl MembershipHistoryExchangePort for MembershipHistoryTransport {
    async fn exchange_membership_history(
        &self,
        peer: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        describe_membership_exchange(membership_history_purpose(&message), false);
        let deadline = exchange_deadline(Instant::now() + Duration::from_secs(10));
        timeout_at(deadline, async {
            let mut connection = self
                .manager
                .request(*peer)
                .connect(deadline, &CancellationToken::new())
                .await
                .map_err(MembershipHistoryExchangeError::offline_from)?;
            let mut usage = ExchangeUse {
                manager: self.manager.clone(),
                connection: connection.clone(),
                deadline,
                completed: false,
            };
            if !matches!(
                connection.protocol(),
                ConnectionProtocol::MembershipHistory
                    | ConnectionProtocol::MultiplexedMembershipHistory
            ) {
                return Err(MembershipHistoryExchangeError::Rejected);
            }
            // 仅在首次写入前重获一次已失活连接，不能重送可能已发送的消息。
            let mut stream = match connection.open_bi(deadline).await {
                Ok(stream) => stream,
                Err(_before_write_failure) if connection.is_closed() => {
                    connection.close();
                    connection = self
                        .manager
                        .refresh(&connection, deadline)
                        .await
                        .map_err(MembershipHistoryExchangeError::offline_from)?;
                    usage.connection = connection.clone();
                    connection
                        .open_bi(deadline)
                        .await
                        .map_err(MembershipHistoryExchangeError::transport_from)?
                }
                Err(source) => return Err(MembershipHistoryExchangeError::transport_from(source)),
            };
            connection
                .check_current()
                .await
                .map_err(MembershipHistoryExchangeError::transport_from)?;
            ensure_live(deadline)?;
            self.wire
                .write_request(stream.send.as_mut(), message)
                .await?;
            let response = self
                .wire
                .read_response(stream.receive.as_mut(), connection.protocol())
                .await?;
            connection
                .check_current()
                .await
                .map_err(MembershipHistoryExchangeError::transport_from)?;
            if response.reusable {
                prepare_connection_retention(&self.manager, &connection, deadline).await;
            } else {
                self.manager.retire(&connection);
                connection.close();
            }
            usage.completed = true;
            Ok(response.message)
        })
        .await
        .map_err(MembershipHistoryExchangeError::transport_from)?
    }
}
#[async_trait]
impl RestrictedMembershipDeliveryPort for MembershipHistoryTransport {
    async fn deliver_restricted_membership(
        &self,
        peer: &DeviceId,
        delivery: &RestrictedMembershipDelivery,
    ) -> Result<(), RestrictedMembershipDeliveryError> {
        let message = match delivery {
            RestrictedMembershipDelivery::Event(event) => {
                MembershipHistoryMessage::RestrictedEventV3(event.as_ref().clone())
            }
            RestrictedMembershipDelivery::Decision(decision) => {
                MembershipHistoryMessage::RestrictedDecisionV3(decision.as_ref().clone())
            }
        };
        match self.exchange_membership_history(peer, message).await {
            Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::RestrictedApplied
                | MembershipHistoryAckV3::RestrictedConsistent,
            )) => Ok(()),
            Ok(_) | Err(MembershipHistoryExchangeError::Rejected) => {
                Err(RestrictedMembershipDeliveryError::Rejected)
            }
            Err(source) => Err(RestrictedMembershipDeliveryError::deferred_from(source)),
        }
    }
}
pub fn membership_history_purpose(message: &MembershipHistoryMessage) -> MembershipExchangePurpose {
    match message {
        MembershipHistoryMessage::SummaryV3(_) => MembershipExchangePurpose::CompareSummary,
        MembershipHistoryMessage::RequestSuffixV3(_) => MembershipExchangePurpose::RequestHistory,
        MembershipHistoryMessage::SuffixPageV4(_) => MembershipExchangePurpose::SendHistory,
        MembershipHistoryMessage::RequestConflictEvidenceV3(_) => {
            MembershipExchangePurpose::RequestConflictEvidence
        }
        MembershipHistoryMessage::ConflictEvidenceV3(_) => {
            MembershipExchangePurpose::SendConflictEvidence
        }
        MembershipHistoryMessage::AckV3(_) => MembershipExchangePurpose::Acknowledge,
        MembershipHistoryMessage::RestrictedEventV3(_) => {
            MembershipExchangePurpose::DeliverRestrictedEvent
        }
        MembershipHistoryMessage::RestrictedDecisionV3(_) => {
            MembershipExchangePurpose::DeliverRestrictedDecision
        }
    }
}
