//! 普通重投与手动重试共用原材料读取、成功输出证明和选择校验。
use super::{
    BeginReceiveRetryPort, PreparedReceiveRetry, ReceiveCancellationBarrier,
    RetryEntryReceiveError, RetryEntryReceiveOutcome, RetrySnapshotReader,
};
use crate::space::{CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort};
use crate::transfer::receive::materialization::{validate_selection, ReceiveSelection};
use crate::transfer::receive::query::ReceiveProgressQuery;
use crate::transfer::receive::work::{ReceiveAction, ReceiveWorkRegistry};
use anyhow::anyhow;
use std::{collections::HashSet, path::PathBuf, sync::Arc};
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::{AttemptState, TrackedFileTransferStatus};
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

/// 保存准入拒绝的实际业务结果，公开错误文本不含条目或尝试身份。
#[derive(Debug, thiserror::Error)]
#[error("original receive is not ready for redelivery")]
pub(crate) struct ReceiveRedeliveryRefusal(pub(crate) RetryEntryReceiveOutcome);

pub(crate) struct ReceiveRetryPreparation {
    pub(crate) begin: Arc<dyn BeginReceiveRetryPort>,
    pub(crate) cache_dir: PathBuf,
    pub(crate) query: Arc<ReceiveProgressQuery>,
    pub(crate) work: Arc<ReceiveWorkRegistry>,
    pub(crate) cancellation: Arc<dyn ReceiveCancellationBarrier>,
    pub(crate) snapshots: RetrySnapshotReader,
    pub(crate) scope: Arc<dyn CurrentSpaceMemberScopePort>,
}

impl ReceiveRetryPreparation {
    /// 普通重投只触发原接收单元的失败项，不用新信封的位置或票据替换原材料。
    pub(crate) async fn prepare_redelivery(
        &self,
        entry: &EntryId,
        expected_hash: &str,
        sender: DeviceId,
        action: Arc<ReceiveAction>,
    ) -> Result<PreparedReceiveRetry, RetryEntryReceiveError> {
        let progress = self
            .query
            .get(entry)
            .await
            .map_err(|source| RetryEntryReceiveError::Source(source.into()))?
            .ok_or_else(|| {
                RetryEntryReceiveError::Unavailable(anyhow!(
                    "original receive attempt is unavailable"
                ))
            })?;
        let source = self
            .work
            .retry_material(entry, &progress.attempt_id)
            .ok_or_else(|| {
                RetryEntryReceiveError::Unavailable(anyhow!(
                    "original receive material is unavailable"
                ))
            })?
            .0;
        if source.expected_hash != expected_hash {
            return Err(RetryEntryReceiveError::Unavailable(anyhow!(
                "original receive identity does not match"
            )));
        }
        let items = progress
            .items
            .iter()
            .filter(|item| {
                matches!(
                    item.status,
                    TrackedFileTransferStatus::Failed | TrackedFileTransferStatus::Cancelled
                )
            })
            .map(|item| item.item_id.clone())
            .collect::<Vec<_>>();
        if items.is_empty() {
            return Err(RetryEntryReceiveError::Unavailable(anyhow!(
                "original receive has no reusable failed selection"
            )));
        }
        match self
            .prepare(
                entry.clone(),
                progress.attempt_id,
                items,
                action,
                Some(sender),
            )
            .await?
        {
            Ok(prepared) => Ok(prepared),
            Err(outcome) => Err(RetryEntryReceiveError::Unavailable(
                ReceiveRedeliveryRefusal(outcome).into(),
            )),
        }
    }

    pub(crate) async fn prepare(
        &self,
        entry: EntryId,
        expected_attempt: String,
        item_ids: Vec<String>,
        action: Arc<ReceiveAction>,
        sender: Option<DeviceId>,
    ) -> Result<Result<PreparedReceiveRetry, RetryEntryReceiveOutcome>, RetryEntryReceiveError>
    {
        let requested = item_ids.iter().collect::<HashSet<_>>();
        let Some(progress) =
            self.query.get(&entry).await.map_err(|source| {
                preflight_failure(RetryEntryReceiveError::Source(source.into()))
            })?
        else {
            return Ok(Err(RetryEntryReceiveOutcome::Unavailable));
        };
        if progress.attempt_id != expected_attempt {
            return Ok(Err(RetryEntryReceiveOutcome::Superseded));
        }
        if self.cancellation.is_settling(&entry, &expected_attempt)
            || (progress.state.is_terminal() && self.work.owns_entry(entry.as_ref()))
            || matches!(
                progress.state,
                AttemptState::Cancelling | AttemptState::Failing
            )
        {
            return Ok(Err(RetryEntryReceiveOutcome::CancellationIncomplete));
        }
        if !progress.state.is_terminal() {
            return Ok(Err(RetryEntryReceiveOutcome::AlreadyReceiving));
        }
        if requested.iter().any(|id| {
            !progress.items.iter().any(|item| {
                &item.item_id == *id
                    && matches!(
                        item.status,
                        TrackedFileTransferStatus::Failed | TrackedFileTransferStatus::Cancelled
                    )
            })
        }) {
            return Err(RetryEntryReceiveError::InvalidInput);
        }
        let Some((mut source, landed)) = self.work.retry_material(&entry, &expected_attempt) else {
            return Ok(Err(RetryEntryReceiveOutcome::Unavailable));
        };
        if let Some(sender) = sender {
            source.from_device = sender;
        }
        let scope = match self.scope.snapshot().await {
            Ok(scope) => scope,
            Err(
                CurrentSpaceMemberScopeError::Locked | CurrentSpaceMemberScopeError::NoCurrentSpace,
            ) => return Ok(Err(RetryEntryReceiveOutcome::AccessDenied)),
            Err(source) => {
                return Err(preflight_failure(RetryEntryReceiveError::Source(
                    source.into(),
                )))
            }
        };
        if !scope.local_member_active || !scope.usable_peer_device_ids.contains(&source.from_device)
        {
            return Ok(Err(RetryEntryReceiveOutcome::AccessDenied));
        }
        let planned = source.planned(&entry, &expected_attempt);
        let selected = if source.manifest.is_some() {
            if requested.len() != 1 || !requested.iter().any(|id| id.as_str() == "directory_unit") {
                return Err(RetryEntryReceiveError::InvalidInput);
            }
            (0..source.blob_refs.len()).collect()
        } else {
            planned
                .iter()
                .filter(|item| requested.contains(&item.id))
                .map(|item| item.blob_index)
                .collect::<HashSet<_>>()
        };
        if (source.manifest.is_none() && (selected.is_empty() || selected.len() != requested.len()))
            || selected.iter().any(|index| landed.contains_key(index))
        {
            return Err(RetryEntryReceiveError::InvalidInput);
        }
        let unselected = planned
            .iter()
            .filter(|item| {
                !selected.contains(&item.blob_index) && !landed.contains_key(&item.blob_index)
            })
            .filter_map(|item| {
                progress
                    .items
                    .iter()
                    .find(|previous| previous.item_id == item.id)
                    .map(|previous| (item.blob_index, previous.clone()))
            })
            .collect();
        let Some(snapshot) = self
            .snapshots
            .read(&entry, &source)
            .await
            .map_err(preflight_failure)?
        else {
            return Ok(Err(RetryEntryReceiveOutcome::Unavailable));
        };
        let selection = ReceiveSelection {
            selected,
            landed,
            unselected,
        };
        validate_selection(
            &selection,
            &source.blob_refs,
            source.manifest.is_some(),
            &snapshot,
            &self.cache_dir,
            &action.cancellation(),
        )
        .await
        .map_err(|source| preflight_failure(RetryEntryReceiveError::Unavailable(source)))?;
        Ok(Ok(PreparedReceiveRetry {
            begin: self.begin.clone(),
            cancellation: self.cancellation.clone(),
            identity: self.snapshots.identity.clone(),
            scope: self.scope.clone(),
            entry,
            expected_attempt,
            source,
            snapshot,
            selection: Some(selection),
            action,
            accepted: None,
        }))
    }
}

fn preflight_failure(source: RetryEntryReceiveError) -> RetryEntryReceiveError {
    uc_warn!(
        error_kind = "receive_retry_preflight",
        io_error_kind = io_error_kind(&source),
        "receive retry could not validate its original source"
    );
    source
}
