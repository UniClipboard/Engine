//! 只有真实旧协议能力才读取完整原事实并发送粗粒度通知，不按单项回调假冒批次。
use super::TransferProgressProducer;
use crate::connectivity::{ConnectionLease, ConnectivityError};
use crate::transfer::progress::{
    ProgressExchangeDeadline, ProgressPageReply, ProgressReceiveError, ProgressSnapshotQuery,
    ProgressTaskDescriptor,
};
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{
    summarize_outbound_progress, ProgressItemState, ProgressSnapshotCoverage,
    ProgressSnapshotEffect, ProgressSnapshotInput, ProgressSnapshotRefusal,
    MAX_SNAPSHOT_PAGE_ITEMS,
};
use uc_core::ids::DeviceId;

impl TransferProgressProducer {
    pub(super) async fn report_legacy_summary(
        &self,
        connection: &ConnectionLease,
        peer: DeviceId,
        expected: ProgressTaskDescriptor,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        let current = self
            .authority
            .current_task(&peer, expected.source)
            .ok_or(ProgressReceiveError::UnknownTask)?;
        if current.descriptor != expected {
            return Err(ProgressReceiveError::Stale);
        }
        let mut coverage = ProgressSnapshotCoverage::new(expected, current.items.to_vec())?;
        let mut states = Vec::with_capacity(current.items.len());
        let mut offset = 0;
        let mut revision = None;
        loop {
            connection.check_current().await?;
            deadline.check(cancel)?;
            // 本地权威读取一旦开始就等待真实结束，不能用网络超时丢弃它。
            let reply = self
                .snapshots
                .snapshot_page(
                    &peer,
                    ProgressSnapshotQuery {
                        source: expected.source,
                        attempt: Some(expected.attempt),
                        offset,
                        revision,
                    },
                )
                .await?;
            deadline.check(cancel)?;
            let page = match reply {
                ProgressPageReply::Current(page) => page,
                ProgressPageReply::Superseded(_) => return Err(ProgressReceiveError::Stale),
                ProgressPageReply::NotRetained => return Err(ProgressReceiveError::UnknownTask),
                ProgressPageReply::Unsupported => return Err(ProgressReceiveError::Unsupported),
                ProgressPageReply::AccessDenied => return Err(ConnectivityError::Denied.into()),
                ProgressPageReply::Busy => return Err(ProgressReceiveError::Busy),
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
                    revision: next_revision,
                } => {
                    states.extend(page.items);
                    offset = next_offset;
                    revision = Some(next_revision);
                }
                ProgressSnapshotEffect::Complete { .. } => {
                    states.extend(page.items);
                    break;
                }
                ProgressSnapshotEffect::Refused(reason) => return Err(reason.into()),
            }
        }
        let summary = summarize_outbound_progress(&states)?;
        if self
            .authority
            .current_task(&peer, expected.source)
            .is_none_or(|value| value.descriptor != expected)
        {
            return Err(ProgressReceiveError::Stale);
        }
        connection.check_current().await?;
        deadline.check(cancel)?;
        let mut stream = connection.open_uni(deadline.instant(), cancel).await?;
        if self
            .authority
            .current_task(&peer, expected.source)
            .is_none_or(|value| value.descriptor != expected)
        {
            return Err(ProgressReceiveError::Stale);
        }
        self.wire
            .summary(
                &mut stream,
                connection,
                expected.source,
                summary,
                deadline,
                cancel,
            )
            .await?;
        if self
            .authority
            .current_task(&peer, expected.source)
            .is_none_or(|value| value.descriptor != expected)
        {
            return Err(ProgressReceiveError::Stale);
        }
        deadline.check(cancel)
    }
}
