use super::super::{
    BlobExchangeError, BlobExchangeFailure, BlobExchangePort, BlobStoreLifecyclePort,
};
use crate::connectivity::{
    ConnectivityBackend, ConnectivityManager, ConnectivityPolicy, IdleRetentionPolicy,
    InboundAdmissionPolicy, PeerConnection, SessionConnectionAdmissionPort,
};
use crate::transfer::policy::{
    ACTIVE_DOWNLOADS, ACTIVE_DOWNLOADS_PER_PEER, ACTIVE_PROVIDES, ACTIVE_PROVIDES_PER_PEER,
    CONNECTION_IDLE_TIMEOUT, INBOUND_REQUEST_TIMEOUT, RETAINED_BLOB_CONNECTIONS,
    UNIDENTIFIED_INBOUND,
};
use futures::future::{ready, Either};
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{oneshot, watch};
use tokio::task::{JoinError, JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::instrument::WithSubscriber;
use tracing::{Instrument, Span};
use uc_core::error_class::ErrorClass;
use uc_observability_contract::diagnostics::{record_task_join_failure, DiagnosticTaskKind};
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

type ShutdownResult = Option<Result<(), Arc<BlobExchangeError>>>;
#[derive(Clone, Copy)]
pub enum BlobShutdownWait {
    Complete,
    Bounded,
}

pub struct BlobTransport {
    pub(super) manager: ConnectivityManager,
    pub(super) exchange: Arc<dyn BlobExchangePort>,
    pub(super) lifecycle: Arc<dyn BlobStoreLifecyclePort>,
    pub(super) stop: CancellationToken,
    pub(super) jobs: Mutex<JoinSet<Result<(), Arc<BlobExchangeError>>>>,
    pub(super) failure: Mutex<Option<Arc<BlobExchangeError>>>,
    pub(super) closing: Mutex<Option<watch::Receiver<ShutdownResult>>>,
}
struct ConnectionWaiter {
    connection: Arc<dyn PeerConnection>,
    cancel: CancellationToken,
}
impl Drop for ConnectionWaiter {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.connection.close();
    }
}
impl BlobTransport {
    pub fn new(
        backend: Arc<dyn ConnectivityBackend>,
        admission: Arc<dyn SessionConnectionAdmissionPort>,
        exchange: Arc<dyn BlobExchangePort>,
        lifecycle: Arc<dyn BlobStoreLifecyclePort>,
    ) -> Arc<Self> {
        let manager = ConnectivityManager::new(
            backend,
            admission,
            ConnectivityPolicy {
                supervise_local_work: true,
                inbound_admission: Some(InboundAdmissionPolicy {
                    unidentified: UNIDENTIFIED_INBOUND,
                    identified_per_peer: ACTIVE_PROVIDES_PER_PEER,
                    timeout: INBOUND_REQUEST_TIMEOUT,
                }),
                connections_per_peer_direction: Some(1),
                outbound_per_peer: ACTIVE_DOWNLOADS_PER_PEER,
                inbound_per_connection: ACTIVE_PROVIDES_PER_PEER,
                inbound_per_session: ACTIVE_PROVIDES,
                waiters_per_peer: ACTIVE_DOWNLOADS_PER_PEER,
                waiters_per_session: ACTIVE_DOWNLOADS,
                flights_per_session: ACTIVE_DOWNLOADS,
                idle_retention: Some(IdleRetentionPolicy {
                    timeout: CONNECTION_IDLE_TIMEOUT,
                    capacity: RETAINED_BLOB_CONNECTIONS,
                }),
                accepted_stream_timeout: Some(INBOUND_REQUEST_TIMEOUT),
                reject_excess_inbound: true,
                admit_on_first_stream: true,
                task_kind: DiagnosticTaskKind::BlobTransportWorker,
                ..ConnectivityPolicy::default()
            },
        );
        Arc::new(Self {
            manager,
            exchange,
            lifecycle,
            stop: CancellationToken::new(),
            jobs: Mutex::new(JoinSet::new()),
            failure: Mutex::new(None),
            closing: Mutex::new(None),
        })
    }
    /// 入口立即冻结准入；协议等待被丢弃时发停止意图，原任务继续结算。
    pub fn serve(
        self: &Arc<Self>,
        connection: Arc<dyn PeerConnection>,
    ) -> impl Future<Output = Result<(), BlobExchangeError>> + Send + 'static {
        if self.stop.is_cancelled() || connection.is_closed() {
            connection.close();
            return Either::Left(ready(Err(BlobExchangeError::refused(
                BlobExchangeFailure::Closed,
            ))));
        }
        let owner = self.clone();
        let cancel = self.stop.child_token();
        let waiter = ConnectionWaiter {
            connection: connection.clone(),
            cancel: cancel.clone(),
        };
        let admitting = self.manager.accept(connection);
        Either::Right(async move {
            let _waiter = waiter;
            let lease = admitting.await.map_err(|source| {
                BlobExchangeError::new(BlobExchangeFailure::Connectivity, source)
            })?;
            let (send, receive) = oneshot::channel();
            {
                let mut jobs = owner.jobs.lock().unwrap_or_else(PoisonError::into_inner);
                while let Some(result) = jobs.try_join_next() {
                    let result = match result {
                        Ok(result) => result,
                        Err(source) => Err(Arc::new(task_error(source))),
                    };
                    if let Err(source) = result {
                        if !shutdown_failure(&source) {
                            continue;
                        }
                        owner
                            .failure
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .get_or_insert(source);
                    }
                }
                if owner.stop.is_cancelled() || cancel.is_cancelled() {
                    return Err(BlobExchangeError::refused(BlobExchangeFailure::Closed));
                }
                let worker_owner = owner.clone();
                let completion = lease.clone();
                // 完成结果保留到关闭收集，不以外部接收者是否仍在为准。
                jobs.spawn(
                    async move {
                        let _completion = completion;
                        let worker = tokio::spawn(
                            async move { worker_owner.run_connection(lease, cancel).await }
                                .instrument(Span::none())
                                .with_current_subscriber(),
                        );
                        let result = match worker.await {
                            Ok(result) => result,
                            Err(source) => Err(task_error(source)),
                        }
                        .map_err(Arc::new);
                        if let Err(error) = &result {
                            uc_warn!(
                                error_kind = "blob_connection_service",
                                error_class = error.class(),
                                io_error_kind = io_error_kind(error.as_ref()),
                                "blob connection service failed"
                            );
                        }
                        // discarded-source[channel]: 等待方已退出，完整负责人仍持实际结果。
                        let _ = send.send(result.clone());
                        result
                    }
                    .instrument(Span::none())
                    .with_current_subscriber(),
                );
            }
            receive
                .await
                .map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Settlement, source))?
                .map_err(|source| BlobExchangeError::new(source.kind, source))
        })
    }
}
pub(super) fn task_error(source: JoinError) -> BlobExchangeError {
    record_task_join_failure(DiagnosticTaskKind::BlobTransportWorker);
    BlobExchangeError::new(BlobExchangeFailure::Task, source)
}

pub(super) fn shutdown_failure(error: &BlobExchangeError) -> bool {
    matches!(
        error.kind,
        BlobExchangeFailure::Task | BlobExchangeFailure::Store | BlobExchangeFailure::Settlement
    )
}
