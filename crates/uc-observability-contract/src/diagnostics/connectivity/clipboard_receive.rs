//! 接收完整结果的固定本地详情，不进入协议或远程摘要。
use std::future::Future;
use std::sync::{Arc, Mutex};

use super::super::OperationCompletion;
use super::complete_clipboard_receive_failure;
use crate::wire_enum;

tokio::task_local! {
    static RECEIVE: ClipboardReceiveObservation;
}

/// 仅跨既有接收队列保存第一次失败；不能查询业务阶段或参与回执决定。
#[derive(Clone, Default)]
pub struct ClipboardReceiveObservation(Arc<Mutex<Option<ClipboardReceiveFailure>>>);

impl ClipboardReceiveObservation {
    pub fn capture() -> Self {
        RECEIVE.try_with(Clone::clone).unwrap_or_default()
    }

    pub async fn scope<F: Future>(&self, future: F) -> F::Output {
        RECEIVE.scope(self.clone(), future).await
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
        if slot.is_none() {
            *slot = Some(failure);
        }
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
