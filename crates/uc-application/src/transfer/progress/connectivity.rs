//! 进度专属连接策略；只组装既有受管连接能力，不拥有第二套拨号或发送流程。
use crate::connectivity::{
    ConnectivityPolicy, ElapsedClockPort, IdleRetentionPolicy, InboundAdmissionPolicy,
};
use std::sync::Arc;
use uc_core::file_transfer::{
    EXCHANGE_GLOBAL_ACTIVE, EXCHANGE_GLOBAL_QUEUED, EXCHANGE_PEER_QUEUED, PROGRESS_IDLE_RETENTION,
    PROGRESS_RETAINED_CONNECTIONS, TERMINAL_ROUND_LIMIT,
};
use uc_observability_contract::diagnostics::DiagnosticTaskKind;

pub fn transfer_progress_connectivity_policy(
    clock: Arc<dyn ElapsedClockPort>,
) -> ConnectivityPolicy {
    ConnectivityPolicy {
        supervise_local_work: true,
        connections_per_peer_direction: Some(1),
        outbound_per_peer: 1,
        inbound_per_connection: 1,
        inbound_per_session: EXCHANGE_GLOBAL_ACTIVE,
        waiters_per_peer: EXCHANGE_PEER_QUEUED,
        waiters_per_session: EXCHANGE_GLOBAL_QUEUED,
        flights_per_session: EXCHANGE_GLOBAL_ACTIVE,
        inbound_admission: Some(InboundAdmissionPolicy {
            unidentified: EXCHANGE_GLOBAL_ACTIVE,
            identified_per_peer: 1,
            timeout: TERMINAL_ROUND_LIMIT,
        }),
        idle_retention: Some(IdleRetentionPolicy {
            elapsed_clock: Some(clock),
            timeout: PROGRESS_IDLE_RETENTION,
            capacity: PROGRESS_RETAINED_CONNECTIONS,
        }),
        accepted_stream_timeout: Some(TERMINAL_ROUND_LIMIT),
        reject_excess_inbound: true,
        task_kind: DiagnosticTaskKind::TransferProgressConnectivityWorker,
        // 旧/1首流是单向；认证身份可直接从原生连接解析，不在这里假定首流双向。
        admit_on_first_stream: false,
        ..ConnectivityPolicy::default()
    }
}
