//! Query只做既有字节与完整Application能力的映射，不产生下载或第二套结果判断。
use super::mapping::{wire_active, wire_descriptor, wire_result};
use uc_application::deps::{
    ProgressPageReply, ProgressReceiveError, ProgressSnapshotPage, ProgressSnapshotProviderPort,
    ProgressSnapshotQuery,
};
use uc_core::file_transfer::ProgressItemState;
use uc_core::ids::DeviceId;
use uc_sync_protocol::transfer_progress::v2::{
    ItemState, Message, ProducerDescriptor, QueryResult, RejectionReason, SnapshotPage, WireId,
};

pub(super) async fn reply(
    owner: &dyn ProgressSnapshotProviderPort,
    peer: &DeviceId,
    challenge: WireId,
    query: ProgressSnapshotQuery,
) -> Result<Message, ProgressReceiveError> {
    let result = match owner.snapshot_page(peer, query).await? {
        ProgressPageReply::Current(value) => {
            let (descriptor, page) = page(value);
            QueryResult::Current { descriptor, page }
        }
        ProgressPageReply::Superseded(value) => {
            let (descriptor, page) = page(value);
            QueryResult::Superseded { descriptor, page }
        }
        ProgressPageReply::NotRetained => QueryResult::NotRetained,
        ProgressPageReply::Unsupported => QueryResult::Unsupported,
        ProgressPageReply::AccessDenied => QueryResult::AccessDenied,
        ProgressPageReply::Busy => {
            return Ok(Message::Rejected {
                challenge,
                reason: RejectionReason::Busy,
            })
        }
    };
    Ok(Message::QueryResult { challenge, result })
}
fn page(value: ProgressSnapshotPage) -> (ProducerDescriptor, SnapshotPage) {
    (
        wire_descriptor(value.descriptor),
        SnapshotPage {
            snapshot_revision: value.revision,
            offset: value.offset,
            total: value.total,
            items: value
                .items
                .into_iter()
                .map(|value| match value {
                    ProgressItemState::Active(value) => ItemState::Active(wire_active(value)),
                    ProgressItemState::Terminal(value) => ItemState::Terminal(wire_result(value)),
                })
                .collect(),
        },
    )
}
