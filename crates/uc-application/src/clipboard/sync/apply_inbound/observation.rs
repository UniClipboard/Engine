//! 完整接收共享的固定失败分类，不输出原始错误。
use std::error::Error as StdError;
use std::io::{Error as IoError, ErrorKind};
use uc_observability_contract::diagnostics::connectivity::ClipboardReceiveFailure;

pub(crate) fn classify_apply_failure(error: &(dyn StdError + 'static)) -> ClipboardReceiveFailure {
    let mut source = Some(error);
    for _ in 0..32 {
        let Some(error) = source else {
            break;
        };
        if let Some(error) = error.downcast_ref::<IoError>() {
            return match error.kind() {
                ErrorKind::PermissionDenied => ClipboardReceiveFailure::ApplyPermissionDenied,
                ErrorKind::StorageFull => ClipboardReceiveFailure::ApplyStorageFull,
                ErrorKind::ReadOnlyFilesystem => ClipboardReceiveFailure::ApplyReadOnly,
                _ => ClipboardReceiveFailure::ApplyIoFailed,
            };
        }
        source = error.source();
    }
    ClipboardReceiveFailure::ApplyFailed
}
