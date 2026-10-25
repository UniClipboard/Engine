use super::super::{ProgressExchangeDeadline, ProgressReceiveError, ProgressSnapshotTarget};
use super::{
    ProgressPageReply, ProgressPageRequest, ProgressSnapshot, ProgressSnapshotRead,
    ProgressSnapshotRound, ProgressSnapshotRoundPort, ProgressSnapshotWirePort,
};
use crate::connectivity::{ConnectionProtocol, ConnectivityManager};
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
impl ProgressSnapshotRoundPort for TransferProgressSnapshotReader {
    fn read_snapshot(
        self: Arc<Self>,
        peer: DeviceId,
        target: ProgressSnapshotTarget,
        deadline: Arc<ProgressExchangeDeadline>,
        cancel: CancellationToken,
    ) -> ProgressSnapshotRound {
        let request = self.manager.request(peer);
        Box::pin(async move {
            let mut descriptor = target.expected;
            if !valid_progress_plan(&target.items)
                || descriptor.is_some_and(|descriptor| {
                    descriptor.source != target.source
                        || usize::from(descriptor.item_count) != target.items.len()
                })
            {
                return Err(ProgressSnapshotRefusal::InvalidPlan.into());
            }
            let mut coverage = descriptor
                .map(|descriptor| ProgressSnapshotCoverage::new(descriptor, target.items.to_vec()))
                .transpose()?;
            deadline.check(&cancel)?;
            let connection = request.connect(deadline.instant(), &cancel).await?;
            deadline.check(&cancel)?;
            match connection.protocol() {
                ConnectionProtocol::TransferProgressV2 => {}
                ConnectionProtocol::TransferProgressV1 => {
                    return Ok(ProgressSnapshotRead::Unsupported)
                }
                _ => return Err(ProgressReceiveError::Protocol),
            }
            let mut items = Vec::with_capacity(target.items.len());
            let mut offset = 0;
            let mut revision = None;
            loop {
                let mut stream = connection
                    .open_bi_with_cancel(deadline.instant(), &cancel)
                    .await?;
                let reply = self
                    .wire
                    .read_page(
                        &mut stream,
                        &connection,
                        ProgressPageRequest {
                            challenge: ProgressIdentity::new(*Uuid::new_v4().as_bytes()),
                            source: target.source,
                            attempt: descriptor.map(|descriptor| descriptor.attempt),
                            offset,
                            revision,
                        },
                        &deadline,
                        &cancel,
                    )
                    .await?;
                self.manager.retain(&connection).await?;
                deadline.check(&cancel)?;
                let page = match reply {
                    ProgressPageReply::Current(page) => page,
                    ProgressPageReply::Superseded(page) => {
                        let Some(expected) = descriptor else {
                            return Err(ProgressReceiveError::Protocol);
                        };
                        if page.descriptor.source != expected.source
                            || page.descriptor.attempt == expected.attempt
                        {
                            return Err(ProgressReceiveError::Protocol);
                        }
                        page.descriptor
                            .check_revision_after(&expected)
                            // discarded-source[core-pure-validation]: 纯修订拒绝不含下层异常，旧或冲突关联统一拒为过期。
                            .map_err(|_| ProgressReceiveError::Stale)?;
                        return Ok(ProgressSnapshotRead::Superseded(page.descriptor));
                    }
                    ProgressPageReply::NotRetained => return Ok(ProgressSnapshotRead::NotRetained),
                    ProgressPageReply::Unsupported => return Ok(ProgressSnapshotRead::Unsupported),
                    ProgressPageReply::AccessDenied => {
                        return Ok(ProgressSnapshotRead::AccessDenied)
                    }
                    ProgressPageReply::Busy => return Ok(ProgressSnapshotRead::Busy),
                };
                if descriptor.is_none() {
                    if page.descriptor.source != target.source
                        || usize::from(page.descriptor.item_count) != target.items.len()
                    {
                        return Err(ProgressSnapshotRefusal::DescriptorChanged.into());
                    }
                    descriptor = Some(page.descriptor);
                    coverage = Some(ProgressSnapshotCoverage::new(
                        page.descriptor,
                        target.items.to_vec(),
                    )?);
                }
                let current = descriptor.ok_or(ProgressReceiveError::Protocol)?;
                let coverage = coverage.as_mut().ok_or(ProgressReceiveError::Protocol)?;
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
                            descriptor: current,
                            revision,
                            items,
                        }));
                    }
                    ProgressSnapshotEffect::Refused(reason) => return Err(reason.into()),
                }
            }
        })
    }
}
