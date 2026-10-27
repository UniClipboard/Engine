use super::io::{invalid, Exchange};
use super::mapping::{active, descriptor, terminal_result};
use super::TransferProgressWire;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectionLease, ProgressExchangeDeadline, ProgressPageReply, ProgressPageRequest,
    ProgressReceiveError, ProgressSnapshotPage, ProgressSnapshotWirePort, StreamLease,
};
use uc_core::file_transfer::ProgressItemState;
use uc_sync_protocol::transfer_progress::v2::{
    ItemState, Message, ProducerDescriptor, QueryResult, RejectionReason, SnapshotPage, WireId,
};

#[async_trait]
impl ProgressSnapshotWirePort for TransferProgressWire {
    async fn read_page(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        request: ProgressPageRequest,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<ProgressPageReply, ProgressReceiveError> {
        let mut exchange = Exchange::new(stream, connection, deadline, cancel);
        exchange
            .write(Message::Query {
                challenge: WireId(request.challenge.bytes()),
                source_entry: WireId(request.source.bytes()),
                expected_attempt: request.attempt.map(|attempt| WireId(attempt.bytes())),
                offset: request.offset,
                snapshot_revision: request.revision,
            })
            .await?;
        exchange.finish_send().await?;
        let response = exchange.read().await?;
        exchange.end_request().await?;
        let (challenge, result) = match response {
            Message::QueryResult { challenge, result } => (challenge, Ok(result)),
            Message::Rejected { challenge, reason } => (challenge, Err(reason)),
            _ => return Err(invalid()),
        };
        if challenge.0 != request.challenge.bytes() {
            return Err(ProgressReceiveError::Stale);
        }
        Ok(match result {
            Ok(QueryResult::Current { descriptor, page }) => {
                ProgressPageReply::Current(snapshot_page(descriptor, page))
            }
            Ok(QueryResult::Superseded { descriptor, page }) => {
                ProgressPageReply::Superseded(snapshot_page(descriptor, page))
            }
            Ok(QueryResult::NotRetained) => ProgressPageReply::NotRetained,
            Ok(QueryResult::Unsupported) => ProgressPageReply::Unsupported,
            Ok(QueryResult::AccessDenied) | Err(RejectionReason::AccessDenied) => {
                ProgressPageReply::AccessDenied
            }
            Err(RejectionReason::Busy) => ProgressPageReply::Busy,
            Err(RejectionReason::Stale) => return Err(ProgressReceiveError::Stale),
            Err(RejectionReason::UnknownTask) => return Err(ProgressReceiveError::UnknownTask),
            Err(RejectionReason::InvalidScope) => return Err(ProgressReceiveError::InvalidPlan),
        })
    }
}
fn snapshot_page(current: ProducerDescriptor, page: SnapshotPage) -> ProgressSnapshotPage {
    ProgressSnapshotPage {
        descriptor: descriptor(current),
        revision: page.snapshot_revision,
        offset: page.offset,
        total: page.total,
        items: page
            .items
            .into_iter()
            .map(|value| match value {
                ItemState::Terminal(value) => ProgressItemState::Terminal(terminal_result(value)),
                ItemState::Active(value) => ProgressItemState::Active(active(value)),
            })
            .collect(),
    }
}
