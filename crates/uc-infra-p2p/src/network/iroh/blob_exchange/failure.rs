use iroh::endpoint::{ConnectionError, ReadError, WriteError};
use iroh_blobs::get::fsm::{AtBlobHeaderNextError, ConnectedNextError, DecodeError};
use std::io;
use uc_application::deps::{BlobExchangeError, BlobExchangeFailure};

pub(super) fn network(source: impl Into<anyhow::Error>) -> BlobExchangeError {
    let source = source.into();
    let temporary = source.chain().any(|cause| {
        if let Some(error) = cause.downcast_ref::<ConnectionError>() {
            return matches!(error, ConnectionError::TimedOut | ConnectionError::Reset);
        }
        if let Some(error) = cause.downcast_ref::<io::Error>() {
            // QUIC reset／应用拒绝也可能映射为 ConnectionReset，必须优先查看具体来源。
            if error.get_ref().is_some() {
                return false;
            }
            return matches!(
                error.kind(),
                io::ErrorKind::TimedOut
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::BrokenPipe
            );
        }
        false
    });
    // 显式流停止／应用关闭不能因为某个 IO 外层分类而成为网络重试。
    let stopped = source.chain().any(|cause| {
        matches!(cause.downcast_ref::<ReadError>(), Some(ReadError::Reset(_)))
            || matches!(
                cause.downcast_ref::<WriteError>(),
                Some(WriteError::Stopped(_))
            )
            || matches!(
                cause.downcast_ref::<ConnectionError>(),
                Some(ConnectionError::ApplicationClosed(_) | ConnectionError::LocallyClosed)
            )
    });
    BlobExchangeError::new(
        if temporary && !stopped {
            BlobExchangeFailure::TransientNetwork
        } else {
            BlobExchangeFailure::Network
        },
        source,
    )
}

pub(super) fn request_write(source: ConnectedNextError) -> BlobExchangeError {
    match &source {
        ConnectedNextError::Write { .. } => network(source),
        _ => BlobExchangeError::new(BlobExchangeFailure::InvalidRequest, source),
    }
}
pub(super) fn header(source: AtBlobHeaderNextError) -> BlobExchangeError {
    match &source {
        AtBlobHeaderNextError::NotFound { .. } => {
            BlobExchangeError::new(BlobExchangeFailure::SourceUnavailable, source)
        }
        AtBlobHeaderNextError::Read { .. } => network(source),
    }
}
pub(super) fn content(source: DecodeError) -> BlobExchangeError {
    match &source {
        DecodeError::Read { .. } => network(source),
        DecodeError::ChunkNotFound { .. }
        | DecodeError::ParentNotFound { .. }
        | DecodeError::LeafNotFound { .. } => {
            BlobExchangeError::new(BlobExchangeFailure::SourceUnavailable, source)
        }
        _ => BlobExchangeError::new(BlobExchangeFailure::Integrity, source),
    }
}

pub(super) fn combine<T>(
    operation: Result<T, BlobExchangeError>,
    settlement: Result<(), BlobExchangeError>,
) -> Result<T, BlobExchangeError> {
    match (operation, settlement) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(primary), Err(settlement)) => Err(primary.with_settlement(settlement)),
    }
}

#[cfg(test)]
mod tests {
    use super::{combine, network};
    use iroh::endpoint::{ConnectionError, ReadError, VarInt, WriteError};
    use std::error::Error;
    use std::io;
    use uc_application::deps::{BlobExchangeError, BlobExchangeFailure};
    use uc_core::error_class::ErrorClass;

    #[test]
    fn retry_whitelist_requires_a_specific_network_source() {
        for kind in [io::ErrorKind::ConnectionReset, io::ErrorKind::TimedOut] {
            let error = network(io::Error::from(kind));
            assert_eq!(error.kind, BlobExchangeFailure::TransientNetwork);
            assert!(error.source().is_some());
            assert_eq!(
                error
                    .source
                    .downcast_ref::<io::Error>()
                    .expect("IO source")
                    .kind(),
                kind
            );
        }
        for kind in [
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::InvalidData,
            io::ErrorKind::Other,
        ] {
            assert_eq!(
                network(io::Error::from(kind)).kind,
                BlobExchangeFailure::Network
            );
        }
        let unknown = io::Error::new(
            io::ErrorKind::ConnectionReset,
            io::Error::other("synthetic opaque cause"),
        );
        assert_eq!(network(unknown).kind, BlobExchangeFailure::Network);
        assert_eq!(
            network(ConnectionError::TimedOut).kind,
            BlobExchangeFailure::TransientNetwork
        );
        assert_eq!(
            network(ConnectionError::Reset).kind,
            BlobExchangeFailure::TransientNetwork
        );
        assert_eq!(
            network(ConnectionError::LocallyClosed).kind,
            BlobExchangeFailure::Network
        );
    }

    #[test]
    fn peer_stream_reset_and_stop_are_permanent_even_with_connection_reset_io_kind() {
        let reset = ReadError::Reset(VarInt::from_u32(0));
        let error = network(io::Error::new(io::ErrorKind::ConnectionReset, reset));
        assert_eq!(error.kind, BlobExchangeFailure::Network);
        let stop = WriteError::Stopped(VarInt::from_u32(0));
        assert_eq!(network(stop).kind, BlobExchangeFailure::Network);
    }

    #[test]
    fn settlement_failure_preserves_both_sources_and_prevents_network_retry() {
        let primary = network(io::Error::from(io::ErrorKind::ConnectionReset));
        let secondary = BlobExchangeError::new(
            BlobExchangeFailure::Store,
            io::Error::from(io::ErrorKind::PermissionDenied),
        );
        let Err(error) = combine::<()>(Err(primary), Err(secondary)) else {
            panic!("settlement failure was swallowed");
        };
        assert_eq!(error.class(), "blob_exchange_settlement_failed");
        let primary = error
            .source
            .downcast_ref::<BlobExchangeError>()
            .expect("primary retained");
        assert_eq!(
            primary
                .source
                .downcast_ref::<io::Error>()
                .expect("network source")
                .kind(),
            io::ErrorKind::ConnectionReset
        );
        assert_eq!(error.additional.len(), 1);
        assert_eq!(
            error.additional[0]
                .source
                .downcast_ref::<io::Error>()
                .expect("store source")
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn error_debug_and_display_keep_only_fixed_categories() {
        let error = BlobExchangeError::new(
            BlobExchangeFailure::Store,
            io::Error::other("synthetic-private-source"),
        );
        assert!(!format!("{error:?}").contains("synthetic-private-source"));
        assert!(!format!("{error}").contains("synthetic-private-source"));
        assert_eq!(
            error
                .source
                .downcast_ref::<io::Error>()
                .expect("source")
                .kind(),
            io::ErrorKind::Other
        );
    }
}
