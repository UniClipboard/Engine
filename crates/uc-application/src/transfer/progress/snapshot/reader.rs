use super::super::{ProgressExchangeDeadline, ProgressReceiveError, ProgressSourceTask};
use super::{
    ProgressPageReply, ProgressPageRequest, ProgressSnapshot, ProgressSnapshotRead,
    ProgressSnapshotRoundPort, ProgressSnapshotWirePort,
};
use crate::connectivity::{ConnectionProtocol, ConnectivityManager};
use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{
    valid_progress_plan, ProgressIdentity, ProgressItemState, ProgressSnapshotCoverage,
    ProgressSnapshotEffect, ProgressSnapshotInput, ProgressSnapshotRefusal,
    MAX_SNAPSHOT_PAGE_ITEMS,
};
use uc_core::ids::DeviceId;
use uuid::Uuid;

pub struct TransferProgressSnapshotReader {
    manager: ConnectivityManager,
    wire: Arc<dyn ProgressSnapshotWirePort>,
}
impl TransferProgressSnapshotReader {
    pub fn new(manager: ConnectivityManager, wire: Arc<dyn ProgressSnapshotWirePort>) -> Self {
        Self { manager, wire }
    }
}
#[async_trait]
impl ProgressSnapshotRoundPort for TransferProgressSnapshotReader {
    async fn read_snapshot(
        &self,
        peer: &DeviceId,
        expected: ProgressSourceTask,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<ProgressSnapshotRead, ProgressReceiveError> {
        let descriptor = expected.descriptor;
        if !valid_progress_plan(&expected.items)
            || usize::from(descriptor.item_count) != expected.items.len()
        {
            return Err(ProgressSnapshotRefusal::InvalidPlan.into());
        }
        let mut coverage = ProgressSnapshotCoverage::new(descriptor, expected.items.to_vec())?;
        deadline.check(cancel)?;
        let connection = self
            .manager
            .request(*peer)
            .connect(deadline.instant(), cancel)
            .await?;
        deadline.check(cancel)?;
        match connection.protocol() {
            ConnectionProtocol::TransferProgressV2 => {}
            ConnectionProtocol::TransferProgressV1 => return Ok(ProgressSnapshotRead::Unsupported),
            _ => return Err(ProgressReceiveError::Protocol),
        }
        let mut items = Vec::with_capacity(expected.items.len());
        let mut offset = 0;
        let mut revision = None;
        loop {
            let mut stream = connection
                .open_bi_with_cancel(deadline.instant(), cancel)
                .await?;
            let reply = self
                .wire
                .read_page(
                    &mut stream,
                    &connection,
                    ProgressPageRequest {
                        challenge: ProgressIdentity::new(*Uuid::new_v4().as_bytes()),
                        source: descriptor.source,
                        attempt: descriptor.attempt,
                        offset,
                        revision,
                    },
                    deadline,
                    cancel,
                )
                .await?;
            // 实际准入／保留读取与本页使用权共存；期限失效后不继续下一页或返回快照。
            self.manager.retain(&connection).await?;
            deadline.check(cancel)?;
            let page = match reply {
                ProgressPageReply::Current(page) => page,
                ProgressPageReply::Superseded(page) => {
                    if page.descriptor.source != descriptor.source
                        || page.descriptor.attempt == descriptor.attempt
                    {
                        return Err(ProgressReceiveError::Protocol);
                    }
                    return Ok(ProgressSnapshotRead::Superseded(page.descriptor));
                }
                ProgressPageReply::NotRetained => return Ok(ProgressSnapshotRead::NotRetained),
                ProgressPageReply::Unsupported => return Ok(ProgressSnapshotRead::Unsupported),
                ProgressPageReply::AccessDenied => return Ok(ProgressSnapshotRead::AccessDenied),
                ProgressPageReply::Busy => return Ok(ProgressSnapshotRead::Busy),
            };
            if page.items.is_empty() || page.items.len() > MAX_SNAPSHOT_PAGE_ITEMS {
                return Err(ProgressSnapshotRefusal::PageSize.into());
            }
            let effect = coverage.apply(ProgressSnapshotInput::Page {
                descriptor: page.descriptor,
                revision: page.revision,
                offset: page.offset,
                total: page.total,
                items: page
                    .items
                    .iter()
                    .copied()
                    .map(ProgressItemState::item)
                    .collect(),
            });
            match effect {
                ProgressSnapshotEffect::Accepted {
                    next_offset,
                    revision: current,
                } => {
                    items.extend(page.items);
                    offset = next_offset;
                    revision = Some(current);
                }
                ProgressSnapshotEffect::Complete { revision } => {
                    items.extend(page.items);
                    return Ok(ProgressSnapshotRead::Complete(ProgressSnapshot {
                        descriptor,
                        revision,
                        items,
                    }));
                }
                ProgressSnapshotEffect::Refused(reason) => return Err(reason.into()),
            }
        }
    }
}
