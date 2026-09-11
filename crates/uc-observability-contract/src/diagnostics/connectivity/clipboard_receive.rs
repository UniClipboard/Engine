//! 接收完整结果的固定本地详情，不进入协议或远程摘要。
use std::future::Future;
use std::sync::{Arc, Mutex};

use super::super::OperationCompletion;
use super::complete_clipboard_receive_failure;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardReceiveFailure {
    NoConsumer,
    ReceiptDropped,
    SettlementTimeout,
    ApplicationRejected,
    SyncDisabled,
    SettingsUnavailable,
    MembershipScopeBlocked,
    MembershipScopeUnavailable,
    MemberMissing,
    MemberLookupFailed,
    ReceiveDisabled,
    ContentTypeDisabled,
    SessionLocked,
    ContentKeyMissing,
    ContentKeyEpochMismatch,
    InvalidTransferFormat,
    DecryptionFailed,
    CipherUnavailable,
    DecodeFailed,
    ApplyFailed,
    ApplyPermissionDenied,
    ApplyStorageFull,
    ApplyReadOnly,
    ApplyIoFailed,
}

impl ClipboardReceiveFailure {
    pub(super) fn local_fields(self) -> (&'static str, &'static str) {
        match self {
            Self::NoConsumer => ("handoff", "no_consumer"),
            Self::ReceiptDropped => ("settlement", "receipt_dropped"),
            Self::SettlementTimeout => ("settlement", "application_timeout"),
            Self::ApplicationRejected => ("application", "rejected"),
            Self::SyncDisabled => ("policy", "sync_disabled"),
            Self::SettingsUnavailable => ("policy", "settings_unavailable"),
            Self::MembershipScopeBlocked => ("policy", "membership_scope_blocked"),
            Self::MembershipScopeUnavailable => ("policy", "membership_scope_unavailable"),
            Self::MemberMissing => ("policy", "member_missing"),
            Self::MemberLookupFailed => ("policy", "member_lookup_failed"),
            Self::ReceiveDisabled => ("policy", "receive_disabled"),
            Self::ContentTypeDisabled => ("classification", "content_type_disabled"),
            Self::SessionLocked => ("decrypt", "session_locked"),
            Self::ContentKeyMissing => ("decrypt", "content_key_missing"),
            Self::ContentKeyEpochMismatch => ("decrypt", "content_key_epoch_mismatch"),
            Self::InvalidTransferFormat => ("decrypt", "invalid_transfer_format"),
            Self::DecryptionFailed => ("decrypt", "decryption_failed"),
            Self::CipherUnavailable => ("decrypt", "cipher_unavailable"),
            Self::DecodeFailed => ("decode", "invalid_content"),
            Self::ApplyFailed => ("apply", "apply_failed"),
            Self::ApplyPermissionDenied => ("apply", "permission_denied"),
            Self::ApplyStorageFull => ("apply", "storage_full"),
            Self::ApplyReadOnly => ("apply", "read_only_filesystem"),
            Self::ApplyIoFailed => ("apply", "io_failed"),
        }
    }

    pub(super) fn source_chain(self) -> Option<[&'static str; 4]> {
        match self {
            Self::ApplyPermissionDenied
            | Self::ApplyStorageFull
            | Self::ApplyReadOnly
            | Self::ApplyIoFailed => {
                Some(["clipboard_receive", "apply", "io", self.local_fields().1])
            }
            _ => None,
        }
    }
}
