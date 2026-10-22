//! 已知原始计划补齐尚未启动的成员；完成判定只使用现有业务项结果。
use super::PlannedReceiveItem;
use anyhow::{Context, Error as SourceError};
use std::collections::HashSet;
use uc_core::ports::{EntryReceiveProgress, TrackedFileTransferStatus};

pub(super) fn project_totals(
    planned: &[PlannedReceiveItem],
    progress: &mut EntryReceiveProgress,
) -> Result<(), SourceError> {
    let completed = progress
        .items
        .iter()
        .filter(|item| item.status == TrackedFileTransferStatus::Completed)
        .map(|item| item.item_id.as_str())
        .collect::<HashSet<_>>();
    // 计划只有一个目录单元，或每个普通成员各一项；用宽类型累加后检查接口范围。
    let mut total_members = 0u128;
    let mut total_bytes = 0u128;
    let mut completed_members = 0u128;
    let mut completed_bytes = 0u128;
    for item in planned {
        let members = u128::try_from(item.members).context("read planned receive member count")?;
        let bytes = u128::from(item.bytes);
        total_members += members;
        total_bytes += bytes;
        if completed.contains(item.id.as_str()) {
            completed_members += members;
            completed_bytes += bytes;
        }
    }
    let items_total = u32::try_from(total_members).context("project receive member total")?;
    let items_completed =
        u32::try_from(completed_members).context("project completed receive members")?;
    let total_bytes = i64::try_from(total_bytes).context("project receive byte total")?;
    let completed_bytes =
        i64::try_from(completed_bytes).context("project completed receive bytes")?;
    progress.items_total = items_total;
    progress.items_completed = items_completed;
    progress.total_bytes = total_bytes;
    progress.completed_bytes = completed_bytes;
    Ok(())
}
