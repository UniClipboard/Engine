use super::{
    PreparedReceiveRetry, ReceiveCancellationBarrier, RetryEntryReceiveError,
    RetryEntryReceiveOutcome, RetrySnapshotReader,
};
use crate::clipboard::sync::apply_inbound::ApplyInboundClipboardUseCase;
use crate::space::{CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort};
use crate::transfer::policy::{ACTIVE_DOWNLOADS, QUEUED_DOWNLOADS};
use crate::transfer::receive::materialization::{validate_selection, ReceiveSelection};
use crate::transfer::receive::query::ReceiveProgressQuery;
use crate::transfer::receive::work::ReceiveWorkRegistry;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::oneshot;
use uc_core::ids::EntryId;
use uc_core::ports::{AttemptState, TrackedFileTransferStatus};
use uc_observability_contract::diagnostics::ObservationContext;
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

pub(crate) struct RetryEntryReceiveUseCase {
    pub(crate) cache_dir: PathBuf,
    pub(crate) apply: Arc<ApplyInboundClipboardUseCase>,
    pub(crate) query: Arc<ReceiveProgressQuery>,
    pub(crate) work: Arc<ReceiveWorkRegistry>,
    pub(crate) cancellation: Arc<dyn ReceiveCancellationBarrier>,
    pub(crate) snapshots: RetrySnapshotReader,
    pub(crate) scope: Arc<dyn CurrentSpaceMemberScopePort>,
    pub(crate) requests: Arc<Mutex<HashSet<EntryId>>>,
}

struct RequestGuard {
    requests: Arc<Mutex<HashSet<EntryId>>>,
    entry: EntryId,
}
impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.entry);
    }
}

impl RetryEntryReceiveUseCase {
    pub(crate) async fn execute(
        &self,
        entry: EntryId,
        expected_attempt: String,
        item_ids: Vec<String>,
    ) -> Result<RetryEntryReceiveOutcome, RetryEntryReceiveError> {
        if item_ids.is_empty() || expected_attempt.is_empty() {
            return Err(RetryEntryReceiveError::InvalidInput);
        }
        let requested = item_ids.iter().collect::<HashSet<_>>();
        if requested.len() != item_ids.len() {
            return Err(RetryEntryReceiveError::InvalidInput);
        }
        let guard = {
            let mut requests = self.requests.lock().unwrap_or_else(PoisonError::into_inner);
            if requests.contains(&entry) {
                return Ok(RetryEntryReceiveOutcome::AlreadyReceiving);
            }
            if requests.len() >= ACTIVE_DOWNLOADS + QUEUED_DOWNLOADS {
                return Err(RetryEntryReceiveError::Busy);
            }
            requests.insert(entry.clone());
            RequestGuard {
                requests: self.requests.clone(),
                entry: entry.clone(),
            }
        };
        // 出生世代先于所有异步读取；删除或关闭后旧请求不得重新登记。
        let action = self.work.action();
        let Some(progress) =
            self.query.get(&entry).await.map_err(|source| {
                preflight_failure(RetryEntryReceiveError::Source(source.into()))
            })?
        else {
            return Ok(RetryEntryReceiveOutcome::Unavailable);
        };
        if progress.attempt_id != expected_attempt {
            return Ok(RetryEntryReceiveOutcome::Superseded);
        }
        if self.cancellation.is_settling(&entry, &expected_attempt)
            || (progress.state.is_terminal() && self.work.owns_entry(entry.as_ref()))
            || progress.state == AttemptState::Cancelling
        {
            return Ok(RetryEntryReceiveOutcome::CancellationIncomplete);
        }
        if !progress.state.is_terminal() {
            return Ok(RetryEntryReceiveOutcome::AlreadyReceiving);
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
        let Some((source, landed)) = self.work.retry_material(&entry, &expected_attempt) else {
            return Ok(RetryEntryReceiveOutcome::Unavailable);
        };
        let scope = match self.scope.snapshot().await {
            Ok(scope) => scope,
            Err(
                CurrentSpaceMemberScopeError::Locked | CurrentSpaceMemberScopeError::NoCurrentSpace,
            ) => return Ok(RetryEntryReceiveOutcome::AccessDenied),
            Err(source) => {
                return Err(preflight_failure(RetryEntryReceiveError::Source(
                    source.into(),
                )))
            }
        };
        if !scope.local_member_active || !scope.usable_peer_device_ids.contains(&source.from_device)
        {
            return Ok(RetryEntryReceiveOutcome::AccessDenied);
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
            return Ok(RetryEntryReceiveOutcome::Unavailable);
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
        let (accepted, reply) = oneshot::channel();
        let prepared = PreparedReceiveRetry {
            cancellation: self.cancellation.clone(),
            identity: self.snapshots.identity.clone(),
            scope: self.scope.clone(),
            entry,
            expected_attempt,
            source,
            snapshot,
            selection: Some(selection),
            action,
            accepted: Some(accepted),
        };
        let apply = self.apply.clone();
        let observation = ObservationContext::capture();
        let task = tokio::spawn(observation.scope(async move {
            // 调用者离开只结束等待；完整接收监督和防重入一直持有到结算结束。
            let _guard = guard;
            let result = apply.execute_retry(prepared).await;
            if let Err(source) = &result {
                uc_warn!(
                    error_kind = "receive_retry_settlement",
                    io_error_kind = io_error_kind(source),
                    "receive retry action failed during settlement"
                );
            }
            result
        }));
        match reply.await {
            Ok(outcome) => Ok(outcome),
            // discarded-source[channel]: 原流程错误保留在实际接收任务结果中。
            Err(_) => match task.await.map_err(RetryEntryReceiveError::Task)? {
                Err(source) => Err(RetryEntryReceiveError::Receive(source.into())),
                Ok(_) => Ok(RetryEntryReceiveOutcome::Unavailable),
            },
        }
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
