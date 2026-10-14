use crate::connectivity::ConnectivityError;
use uc_core::error_class::ErrorClass;
use uc_core::ports::{ActiveClipboardDispatchError, ActiveClipboardPullClientError};
use uc_observability_contract::{error_source::io_error_kind, uc_debug};

#[derive(Debug, thiserror::Error)]
pub enum ActiveClipboardTransportRefusal {
    #[error("active clipboard activator exceeds identifier bound")]
    InvalidActivator,
    #[error("active clipboard receiver has no subscriber")]
    NoSubscriber,
    #[error("active clipboard peer ended the exchange")]
    PeerClosed,
}
impl ErrorClass for ActiveClipboardTransportRefusal {
    fn class(&self) -> &'static str {
        match self {
            Self::InvalidActivator => "active_invalid_activator",
            Self::NoSubscriber => "active_no_subscriber",
            Self::PeerClosed => "active_peer_closed",
        }
    }
}
#[derive(Debug, thiserror::Error)]
pub enum ActiveClipboardTransportError {
    #[error("active clipboard connectivity failed")]
    Connectivity(#[from] ConnectivityError),
    #[error("active clipboard wire failed")]
    Wire(#[source] anyhow::Error),
    #[error("active clipboard exchange timed out")]
    TimedOut(#[from] tokio::time::error::Elapsed),
    #[error("active clipboard exchange refused")]
    Refused(#[from] ActiveClipboardTransportRefusal),
}
impl ErrorClass for ActiveClipboardTransportError {
    fn class(&self) -> &'static str {
        match self {
            Self::Connectivity(source) => source.class(),
            Self::Wire(_) => "active_wire_failed",
            Self::TimedOut(_) => "active_exchange_timeout",
            Self::Refused(source) => source.class(),
        }
    }
}
impl From<ActiveClipboardTransportError> for ActiveClipboardDispatchError {
    fn from(source: ActiveClipboardTransportError) -> Self {
        match source {
            ActiveClipboardTransportError::Connectivity(source)
                if connectivity_offline(&source) =>
            {
                Self::Offline
            }
            source => Self::Io(Box::new(source)),
        }
    }
}
fn connectivity_offline(source: &ConnectivityError) -> bool {
    match source {
        ConnectivityError::Offline => true,
        ConnectivityError::Shared(source) => connectivity_offline(source),
        _ => false,
    }
}
fn connectivity_timeout(source: &ConnectivityError) -> bool {
    match source {
        ConnectivityError::TimedOut(_) | ConnectivityError::DeadlineExpired => true,
        ConnectivityError::Shared(source) => connectivity_timeout(source),
        _ => false,
    }
}
impl From<ActiveClipboardTransportError> for ActiveClipboardPullClientError {
    fn from(source: ActiveClipboardTransportError) -> Self {
        match source {
            ActiveClipboardTransportError::Connectivity(source)
                if connectivity_offline(&source) =>
            {
                Self::Unreachable
            }
            source
                if matches!(&source, ActiveClipboardTransportError::TimedOut(_))
                    || matches!(&source, ActiveClipboardTransportError::Connectivity(error) if connectivity_timeout(error)) =>
            {
                uc_debug!(
                    error_kind = "pull_timeout",
                    error_class = source.class(),
                    io_error_kind = io_error_kind(&source),
                    "active pull network deadline expired"
                );
                Self::Unreachable
            }
            source => Self::Io(Box::new(source)),
        }
    }
}
#[derive(Debug, thiserror::Error)]
pub(super) enum ActivePullRemoteRefusal {
    #[error("active pull holder does not have the content")]
    NotAvailable,
    #[error("active pull holder is locked")]
    Locked,
    #[error("active pull holder preparation failed")]
    Internal,
}
impl ErrorClass for ActivePullRemoteRefusal {
    fn class(&self) -> &'static str {
        match self {
            Self::NotAvailable => "pull_remote_not_available",
            Self::Locked => "pull_remote_locked",
            Self::Internal => "pull_remote_internal",
        }
    }
}
impl From<ActivePullRemoteRefusal> for ActiveClipboardPullClientError {
    fn from(source: ActivePullRemoteRefusal) -> Self {
        uc_debug!(
            source_class = source.class(),
            "active pull peer cannot serve"
        );
        Self::NotAvailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    use std::io;
    #[test]
    fn address_repository_failure_keeps_class_and_original_io_source() {
        let converted: ActiveClipboardDispatchError =
            ActiveClipboardTransportError::Connectivity(ConnectivityError::AddressLookup(
                Box::new(io::Error::from(io::ErrorKind::PermissionDenied)),
            ))
            .into();
        assert!(matches!(&converted, ActiveClipboardDispatchError::Io(_)));
        let mut source: &dyn Error = &converted;
        loop {
            if let Some(io) = source.downcast_ref::<io::Error>() {
                assert_eq!(io.kind(), io::ErrorKind::PermissionDenied);
                break;
            }
            source = source
                .source()
                .expect("original IO error must remain reachable");
        }
    }
    #[test]
    fn shared_manager_deadline_projects_to_original_unreachable_boundary() {
        let source = ActiveClipboardTransportError::Connectivity(ConnectivityError::Shared(
            std::sync::Arc::new(ConnectivityError::DeadlineExpired),
        ));
        assert!(matches!(
            ActiveClipboardPullClientError::from(source),
            ActiveClipboardPullClientError::Unreachable
        ));
    }
    #[test]
    fn shared_missing_address_preserves_original_unreachable_projection() {
        let source = ActiveClipboardTransportError::Connectivity(ConnectivityError::Shared(
            std::sync::Arc::new(ConnectivityError::Offline),
        ));
        assert!(matches!(
            ActiveClipboardPullClientError::from(source),
            ActiveClipboardPullClientError::Unreachable
        ));
        let source = ActiveClipboardTransportError::Connectivity(ConnectivityError::Shared(
            std::sync::Arc::new(ConnectivityError::Offline),
        ));
        assert!(matches!(
            ActiveClipboardDispatchError::from(source),
            ActiveClipboardDispatchError::Offline
        ));
    }
    #[test]
    fn admission_refusal_is_not_projected_to_offline() {
        let source = ActiveClipboardTransportError::Connectivity(ConnectivityError::Denied);
        let result = ActiveClipboardDispatchError::from(source);
        assert!(matches!(&result, ActiveClipboardDispatchError::Io(_)));
        assert!(result.source().is_some());
    }
}
