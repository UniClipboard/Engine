//! 接收完整结果的固定本地详情，不进入协议或远程摘要。
use std::future::Future;
use std::sync::{Arc, Mutex};

use super::super::{complete_operation, CompletionResult, OperationCompletion};
use super::complete_clipboard_receive_failure;
use crate::wire_enum;

tokio::task_local! {
    static RECEIVE: ClipboardReceiveObservation;
}

/// 跨原接收队列与处理任务携带诊断事实，不提供阶段查询，也不参与业务回执。
#[derive(Clone, Default)]
pub struct ClipboardReceiveObservation(Arc<Mutex<ReceiveFacts>>);

#[derive(Default)]
struct ReceiveFacts {
    failure: Option<ClipboardReceiveFailure>,
    partial: bool,
    cancelled: bool,
    retry_completion: Option<Box<dyn FnOnce(ClipboardRetryOutcome) + Send>>,
}

/// 只有完整流程负责人能提交结局；这些固定诊断类别不参与业务结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardRetryOutcome {
    Succeeded,
    Partial,
    Cancelled,
    Deferred,
    Rejected,
    Skipped,
    Failed(ClipboardReceiveFailure),
}

impl Drop for ReceiveFacts {
    fn drop(&mut self) {
        // 任务异常退出或准入未交接时仍结束在线记录，不能留下永远运行的root。
        if let Some(completion) = self.retry_completion.take() {
            completion(ClipboardRetryOutcome::Failed(
                ClipboardReceiveFailure::ApplyFailed,
            ));
        }
    }
}

impl ClipboardReceiveObservation {
    /// Engine提供完成记录，原Application任务持有唯一实际结算责任。
    pub fn with_retry_completion(
        completion: impl FnOnce(ClipboardRetryOutcome) + Send + 'static,
    ) -> Self {
        Self(Arc::new(Mutex::new(ReceiveFacts {
            retry_completion: Some(Box::new(completion)),
            failure: None,
            partial: false,
            cancelled: false,
        })))
    }

    pub fn finish_retry(&self, outcome: ClipboardRetryOutcome) {
        let (completion, outcome) = {
            let mut facts = self.0.lock().unwrap_or_else(|error| error.into_inner());
            let outcome = match outcome {
                ClipboardRetryOutcome::Succeeded if facts.cancelled => {
                    ClipboardRetryOutcome::Cancelled
                }
                ClipboardRetryOutcome::Succeeded if facts.partial => ClipboardRetryOutcome::Partial,
                other => other,
            };
            (facts.retry_completion.take(), outcome)
        };
        if let Some(completion) = completion {
            completion(outcome);
        }
    }

    pub fn capture() -> Self {
        RECEIVE.try_with(Clone::clone).unwrap_or_default()
    }

    pub async fn scope<F: Future>(&self, future: F) -> F::Output {
        RECEIVE.scope(self.clone(), future).await
    }

    /// 回执保持原值；完整接收负责人已描述的部分／取消事实只影响记录。
    pub fn finish_success(&self, mut completion: OperationCompletion) {
        let facts = self.0.lock().unwrap_or_else(|error| error.into_inner());
        completion.result = if facts.cancelled {
            CompletionResult::Cancelled
        } else if facts.partial {
            CompletionResult::Partial
        } else {
            completion.result
        };
        drop(facts);
        complete_operation(completion);
    }

    pub fn finish_failure(
        &self,
        fallback: ClipboardReceiveFailure,
        completion: OperationCompletion,
    ) {
        let failure = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .failure
            .take();
        let failure = if fallback == ClipboardReceiveFailure::ApplicationRejected {
            failure.unwrap_or(fallback)
        } else {
            fallback
        };
        complete_clipboard_receive_failure(failure, completion);
    }
}

pub fn describe_clipboard_receive_failure(failure: ClipboardReceiveFailure) {
    let _ = RECEIVE.try_with(|observation| {
        let mut slot = observation
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if slot.failure.is_none() {
            slot.failure = Some(failure);
        }
    });
}

/// 由原接收负责人在业务提交后描述事实，不读取或暴露业务状态。
pub fn describe_clipboard_receive_partial(cancelled: bool) {
    let _ = RECEIVE.try_with(|observation| {
        let mut facts = observation
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        facts.partial = true;
        facts.cancelled |= cancelled;
    });
}

wire_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ClipboardReceiveFailure: (&'static str, &'static str) {
        NoConsumer => ("handoff", "no_consumer"),
        ReceiptDropped => ("settlement", "receipt_dropped"),
        SettlementTimeout => ("settlement", "application_timeout"),
        ApplicationRejected => ("application", "rejected"),
        SyncDisabled => ("policy", "sync_disabled"),
        SettingsUnavailable => ("policy", "settings_unavailable"),
        MembershipScopeBlocked => ("policy", "membership_scope_blocked"),
        MembershipScopeUnavailable => ("policy", "membership_scope_unavailable"),
        MemberMissing => ("policy", "member_missing"),
        MemberLookupFailed => ("policy", "member_lookup_failed"),
        ReceiveDisabled => ("policy", "receive_disabled"),
        ContentTypeDisabled => ("classification", "content_type_disabled"),
        SessionLocked => ("decrypt", "session_locked"),
        ContentKeyMissing => ("decrypt", "content_key_missing"),
        ContentKeyEpochMismatch => ("decrypt", "content_key_epoch_mismatch"),
        InvalidTransferFormat => ("decrypt", "invalid_transfer_format"),
        DecryptionFailed => ("decrypt", "decryption_failed"),
        CipherUnavailable => ("decrypt", "cipher_unavailable"),
        DecodeFailed => ("decode", "invalid_content"),
        ApplyFailed => ("apply", "apply_failed"),
        ApplyPermissionDenied => ("apply", "permission_denied"),
        ApplyStorageFull => ("apply", "storage_full"),
        ApplyReadOnly => ("apply", "read_only_filesystem"),
        ApplyIoFailed => ("apply", "io_failed"),
        /// 收完内容后的准入复核发现该连接已不再被准入（成员被移除或世代已退出）。
        AdmissionRevoked => ("admission", "revoked"),
        /// 准入复核在接收期限内没有给出结果。
        AdmissionCheckTimedOut => ("admission", "check_timed_out"),
        /// 准入复核因其他原因失败（连接所有者已停止等）。
        AdmissionCheckFailed => ("admission", "check_failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn retry_completion_follows_the_original_task_and_is_settled_once() {
        let outcomes = Arc::new(Mutex::new(Vec::new()));
        let recorded = outcomes.clone();
        let observation = ClipboardReceiveObservation::with_retry_completion(move |outcome| {
            recorded.lock().unwrap().push(outcome);
        });
        let owner = observation.clone();
        tokio::spawn(async move {
            owner
                .scope(async {
                    describe_clipboard_receive_partial(true);
                })
                .await;
            // 准入调用方已经离开；原任务仍持有实际结算权。
            owner.finish_retry(ClipboardRetryOutcome::Succeeded);
            owner.finish_retry(ClipboardRetryOutcome::Succeeded);
        })
        .await
        .unwrap();
        drop(observation);
        assert_eq!(
            *outcomes.lock().unwrap(),
            vec![ClipboardRetryOutcome::Cancelled]
        );
    }

    #[test]
    fn abandoning_the_last_owner_finishes_the_record_without_claiming_success() {
        let outcomes = Arc::new(Mutex::new(Vec::new()));
        let recorded = outcomes.clone();
        let observation = ClipboardReceiveObservation::with_retry_completion(move |outcome| {
            recorded.lock().unwrap().push(outcome);
        });
        let owner = observation.clone();
        drop(observation);
        assert!(outcomes.lock().unwrap().is_empty());
        drop(owner);
        assert_eq!(
            *outcomes.lock().unwrap(),
            vec![ClipboardRetryOutcome::Failed(
                ClipboardReceiveFailure::ApplyFailed
            )]
        );
    }

    #[test]
    fn admission_check_failures_have_fixed_local_fields() {
        for (failure, expected) in [
            (ClipboardReceiveFailure::AdmissionRevoked, "revoked"),
            (
                ClipboardReceiveFailure::AdmissionCheckTimedOut,
                "check_timed_out",
            ),
            (
                ClipboardReceiveFailure::AdmissionCheckFailed,
                "check_failed",
            ),
        ] {
            assert_eq!(failure.wire(), ("admission", expected));
        }
    }
}
