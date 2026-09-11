//! 只依据 Iroh 的结构化错误分类，不读取错误正文。
use iroh::endpoint::{ConnectError, ConnectWithOptsError, ConnectingError, ConnectionError};
use uc_observability_contract::diagnostics::connectivity::{
    CandidateSummary, ConnectionFailurePhase as Phase, ConnectionFailureReason as Reason,
    ConnectionOutcome,
};

pub(super) fn candidate_summary(addr: &iroh::EndpointAddr) -> (CandidateSummary, Option<[u8; 32]>) {
    use sha2::Digest;
    let mut summary = CandidateSummary {
        direct_count: 0,
        relay_count: 0,
        other_count: 0,
    };
    for candidate in &addr.addrs {
        match candidate {
            iroh::TransportAddr::Ip(_) => {
                summary.direct_count = summary.direct_count.saturating_add(1)
            }
            iroh::TransportAddr::Relay(_) => {
                summary.relay_count = summary.relay_count.saturating_add(1)
            }
            _ => summary.other_count = summary.other_count.saturating_add(1),
        }
    }
    // EndpointAddr 使用有序集合。摘要仅在内存中比较，输出由运行时另分配随机编号。
    let signature = postcard::to_stdvec(&addr.addrs)
        .ok()
        .map(|bytes| sha2::Sha256::digest(bytes).into());
    (summary, signature)
}

pub(super) fn connect_failure(error: &ConnectError) -> ConnectionOutcome {
    match error {
        ConnectError::Connect { source, .. } => preparation_failure(source),
        ConnectError::Connecting { source, .. } => handshake_failure(source),
        ConnectError::Connection { source, .. } => {
            failed(Phase::Handshake, transport_reason(source))
        }
        _ => failed(Phase::Unknown, Reason::Unknown),
    }
}

pub(super) fn preparation_failure(error: &ConnectWithOptsError) -> ConnectionOutcome {
    match error {
        ConnectWithOptsError::NoAddress { .. } => failed(Phase::AddressLookup, Reason::NoAddress),
        ConnectWithOptsError::EndpointClosed { .. } => {
            failed(Phase::Establish, Reason::EndpointClosed)
        }
        ConnectWithOptsError::SelfConnect { .. } => failed(Phase::Establish, Reason::SelfConnect),
        ConnectWithOptsError::LocallyRejected { .. } => {
            failed(Phase::Establish, Reason::LocallyRejected)
        }
        ConnectWithOptsError::InternalConsistencyError { .. } => {
            failed(Phase::Establish, Reason::Internal)
        }
        ConnectWithOptsError::Noq { .. } => failed(Phase::Establish, Reason::Transport),
        _ => failed(Phase::Establish, Reason::Unknown),
    }
}

pub(super) fn handshake_failure(error: &ConnectingError) -> ConnectionOutcome {
    let reason = match error {
        ConnectingError::HandshakeFailure { .. } => Reason::AuthenticationRejected,
        ConnectingError::LocallyRejected { .. } => Reason::LocallyRejected,
        ConnectingError::ConnectionError { source, .. } => transport_reason(source),
        ConnectingError::InternalConsistencyError { .. } => Reason::Internal,
        _ => Reason::Unknown,
    };
    failed(Phase::Handshake, reason)
}

fn transport_reason(error: &ConnectionError) -> Reason {
    match error {
        ConnectionError::TimedOut => Reason::TimedOut,
        ConnectionError::LocallyClosed => Reason::LocallyClosed,
        _ => Reason::Transport,
    }
}

pub(super) fn failed(phase: Phase, reason: Reason) -> ConnectionOutcome {
    ConnectionOutcome::Failed { phase, reason }
}
