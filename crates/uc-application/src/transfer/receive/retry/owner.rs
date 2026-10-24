use super::{ReceiveRetryPreparation, RetryEntryReceiveError, RetryEntryReceiveOutcome};
use crate::clipboard::sync::apply_inbound::ApplyInboundClipboardUseCase;
use crate::transfer::policy::{ACTIVE_DOWNLOADS, QUEUED_DOWNLOADS};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::oneshot;
use uc_core::ids::EntryId;
use uc_observability_contract::diagnostics::{
    connectivity::ClipboardReceiveObservation, ObservationContext,
};
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

pub(crate) struct RetryEntryReceiveUseCase {
    pub(crate) preparation: Arc<ReceiveRetryPreparation>,
    pub(crate) apply: Arc<ApplyInboundClipboardUseCase>,
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
                return Ok(
                    if self
                        .preparation
                        .cancellation
                        .is_settling(&entry, &expected_attempt)
                    {
                        RetryEntryReceiveOutcome::CancellationIncomplete
                    } else {
                        RetryEntryReceiveOutcome::AlreadyReceiving
                    },
                );
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
        let action = self.preparation.work.action();
        let mut prepared = match self
            .preparation
            .prepare(entry, expected_attempt, item_ids, action, None)
            .await?
        {
            Ok(prepared) => prepared,
            Err(outcome) => return Ok(outcome),
        };
        let (accepted, reply) = oneshot::channel();
        prepared.accepted = Some(accepted);
        let apply = self.apply.clone();
        let observation = ObservationContext::capture();
        let receive_observation = ClipboardReceiveObservation::capture();
        let processing = Box::pin(async move {
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
        });
        let task = tokio::spawn(
            observation.scope(async move { receive_observation.scope(processing).await }),
        );
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
