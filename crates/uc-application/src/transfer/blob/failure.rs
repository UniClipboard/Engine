//! 拉取失败的稳定原因：按既有错误类型分类，持久化与展示只用类别，不用错误文本。
use std::io::ErrorKind;

use uc_core::file_transfer::FileTransferFailureReason;
use uc_core::ports::blob::BlobError;

use super::fetch_blob::FetchBlobError;
use super::transport::{BlobExchangeError, BlobExchangeFailure};
use crate::connectivity::ConnectivityError;

/// 沿错误链找到拉取用例的错误并给出原因类别；找不到或无法判断时为 `Unknown`。
pub(crate) fn fetch_failure_reason(error: &anyhow::Error) -> FileTransferFailureReason {
    match error
        .chain()
        .find_map(|source| source.downcast_ref::<FetchBlobError>())
    {
        Some(FetchBlobError::Transfer(source)) => transfer_reason(source),
        Some(FetchBlobError::Reference(_)) => FileTransferFailureReason::StorageUnavailable,
        Some(FetchBlobError::Cipher(_)) => FileTransferFailureReason::IntegrityCheckFailed,
        Some(FetchBlobError::Cancelled(_) | FetchBlobError::Hash(_)) | None => {
            FileTransferFailureReason::Unknown
        }
    }
}

fn transfer_reason(source: &anyhow::Error) -> FileTransferFailureReason {
    if let Some(error) = source
        .chain()
        .find_map(|source| source.downcast_ref::<BlobExchangeError>())
    {
        use FileTransferFailureReason as Reason;
        return match error.kind {
            BlobExchangeFailure::Integrity => Reason::IntegrityCheckFailed,
            BlobExchangeFailure::RequestExpired => Reason::TimedOut,
            BlobExchangeFailure::Store | BlobExchangeFailure::Settlement => {
                io_reason(error).unwrap_or(Reason::StorageUnavailable)
            }
            BlobExchangeFailure::Connectivity => {
                if error.source.chain().any(|source| {
                    matches!(
                        source.downcast_ref::<ConnectivityError>(),
                        Some(ConnectivityError::Denied)
                    )
                }) {
                    Reason::AccessDenied
                } else {
                    io_reason(error).unwrap_or(Reason::NetworkUnavailable)
                }
            }
            BlobExchangeFailure::Network
            | BlobExchangeFailure::TransientNetwork
            | BlobExchangeFailure::IncompleteExport
            | BlobExchangeFailure::SourceUnavailable => {
                io_reason(error).unwrap_or(Reason::NetworkUnavailable)
            }
            BlobExchangeFailure::AlreadyCompleted
            | BlobExchangeFailure::Busy
            | BlobExchangeFailure::Closed
            | BlobExchangeFailure::Task
            | BlobExchangeFailure::ShutdownIncomplete
            | BlobExchangeFailure::Cancelled
            | BlobExchangeFailure::InvalidRequest => Reason::Unknown,
        };
    }

    match source
        .chain()
        .find_map(|source| source.downcast_ref::<BlobError>())
    {
        Some(BlobError::NotFound) => FileTransferFailureReason::StorageUnavailable,
        Some(BlobError::Unavailable(source)) => {
            io_reason(source.as_ref()).unwrap_or(FileTransferFailureReason::NetworkUnavailable)
        }
        Some(BlobError::Internal(source)) => {
            io_reason(source.as_ref()).unwrap_or(FileTransferFailureReason::Unknown)
        }
        Some(BlobError::InvalidTicket { .. }) | None => FileTransferFailureReason::Unknown,
    }
}

/// 只看 `std::io::Error` 的种类；其余来源不解释。
fn io_reason(source: &(dyn std::error::Error + 'static)) -> Option<FileTransferFailureReason> {
    let mut current = Some(source);
    while let Some(error) = current {
        if let Some(io) = error.downcast_ref::<std::io::Error>() {
            return match io.kind() {
                ErrorKind::TimedOut => Some(FileTransferFailureReason::TimedOut),
                ErrorKind::PermissionDenied => Some(FileTransferFailureReason::AccessDenied),
                ErrorKind::StorageFull
                | ErrorKind::ReadOnlyFilesystem
                | ErrorKind::QuotaExceeded => Some(FileTransferFailureReason::StorageUnavailable),
                ErrorKind::ConnectionRefused
                | ErrorKind::ConnectionReset
                | ErrorKind::ConnectionAborted
                | ErrorKind::NotConnected
                | ErrorKind::BrokenPipe
                | ErrorKind::UnexpectedEof
                | ErrorKind::HostUnreachable
                | ErrorKind::NetworkUnreachable
                | ErrorKind::NetworkDown => Some(FileTransferFailureReason::NetworkUnavailable),
                _ => None,
            };
        }
        current = error.source();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fetch(error: FetchBlobError) -> anyhow::Error {
        anyhow::Error::new(error)
    }

    fn io(kind: ErrorKind) -> Box<dyn std::error::Error + Send + Sync> {
        Box::new(std::io::Error::new(kind, "PRIVATE_DETAIL"))
    }

    #[test]
    fn each_fetch_failure_maps_to_one_stable_reason() {
        use FileTransferFailureReason as Reason;
        let transfer =
            |error: BlobError| fetch(FetchBlobError::Transfer(anyhow::Error::from(error)));
        for (error, expected) in [
            (
                transfer(BlobError::Unavailable(io(ErrorKind::ConnectionReset))),
                Reason::NetworkUnavailable,
            ),
            (
                transfer(BlobError::Unavailable(io(ErrorKind::TimedOut))),
                Reason::TimedOut,
            ),
            (
                transfer(BlobError::Unavailable(io(ErrorKind::Other))),
                Reason::NetworkUnavailable,
            ),
            (
                transfer(BlobError::Internal(io(ErrorKind::PermissionDenied))),
                Reason::AccessDenied,
            ),
            (
                transfer(BlobError::Internal(io(ErrorKind::StorageFull))),
                Reason::StorageUnavailable,
            ),
            (
                transfer(BlobError::Internal(io(ErrorKind::Other))),
                Reason::Unknown,
            ),
            (transfer(BlobError::NotFound), Reason::StorageUnavailable),
            (transfer(BlobError::invalid_ticket()), Reason::Unknown),
            (
                fetch(FetchBlobError::Reference(anyhow::anyhow!("PRIVATE_DETAIL"))),
                Reason::StorageUnavailable,
            ),
            (
                fetch(FetchBlobError::Cipher(anyhow::anyhow!("PRIVATE_DETAIL"))),
                Reason::IntegrityCheckFailed,
            ),
            (
                fetch(FetchBlobError::Hash(anyhow::anyhow!("PRIVATE_DETAIL"))),
                Reason::Unknown,
            ),
            (anyhow::anyhow!("PRIVATE_DETAIL"), Reason::Unknown),
        ] {
            assert_eq!(fetch_failure_reason(&error), expected);
        }
    }

    #[test]
    fn the_reason_is_found_through_the_facade_error_the_callers_actually_hold() {
        use crate::transfer::blob::facade::BlobTransferError;
        let fetch = FetchBlobError::Transfer(anyhow::Error::from(BlobError::Unavailable(io(
            ErrorKind::TimedOut,
        ))));
        let error = anyhow::Error::from(BlobTransferError::Fetch(anyhow::Error::new(fetch)));
        assert_eq!(
            fetch_failure_reason(&error),
            FileTransferFailureReason::TimedOut
        );
    }

    #[test]
    fn the_reason_is_found_through_added_context() {
        let error = anyhow::Error::new(FetchBlobError::Transfer(anyhow::Error::from(
            BlobError::Unavailable(io(ErrorKind::TimedOut)),
        )))
        .context("outer layer");
        assert_eq!(
            fetch_failure_reason(&error),
            FileTransferFailureReason::TimedOut
        );
    }
}
