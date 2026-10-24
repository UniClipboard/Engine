use super::super::ProgressReceiveError;
use crate::connectivity::ConnectivityError;
pub(super) enum RetryClass {
    Retry,
    Cancelled,
    Stop,
}
pub(super) fn retry_class(source: &ProgressReceiveError) -> RetryClass {
    match source {
        ProgressReceiveError::Connection(source) => connection_retry(source),
        ProgressReceiveError::Exchange(_) | ProgressReceiveError::Busy => RetryClass::Retry,
        ProgressReceiveError::Snapshot(_)
        | ProgressReceiveError::Clock(_)
        | ProgressReceiveError::Unsupported
        | ProgressReceiveError::Protocol
        | ProgressReceiveError::UnknownTask
        | ProgressReceiveError::InvalidPlan
        | ProgressReceiveError::Stale
        | ProgressReceiveError::Conflict => RetryClass::Stop,
    }
}
fn connection_retry(source: &ConnectivityError) -> RetryClass {
    match source {
        ConnectivityError::AddressMissing
        | ConnectivityError::Offline
        | ConnectivityError::Busy(_)
        | ConnectivityError::DeadlineExpired
        | ConnectivityError::TimedOut(_)
        | ConnectivityError::AdmissionDeadlineExpired
        | ConnectivityError::AdmissionTimedOut(_)
        | ConnectivityError::AddressLookup(_)
        | ConnectivityError::Backend(_)
        | ConnectivityError::Admission(_)
        | ConnectivityError::Stream(_) => RetryClass::Retry,
        ConnectivityError::Stopped => RetryClass::Cancelled,
        ConnectivityError::Shared(source) => connection_retry(source),
        ConnectivityError::Clock(_)
        | ConnectivityError::ResourceClosed(_)
        | ConnectivityError::ResultClosed(_)
        | ConnectivityError::InboundResultClosed(_)
        | ConnectivityError::Task(_)
        | ConnectivityError::ExchangeAlreadyUsed
        | ConnectivityError::Capability(_)
        | ConnectivityError::Denied
        | ConnectivityError::Settlement { .. } => RetryClass::Stop,
    }
}
