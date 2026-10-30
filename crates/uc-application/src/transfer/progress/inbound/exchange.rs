use super::super::{
    ElapsedClockError, ElapsedClockRefusal, ProgressExchangeDeadline, ProgressReceiveError,
};
use super::owner::TransferProgressTransport;
use crate::connectivity::{ConnectionLease, ConnectionProtocol, ConnectivityError, StreamLease};
use std::error::Error;
use std::future::{pending, poll_fn, Future};
use std::sync::Arc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use uc_core::error_class::ErrorClass;
use uc_core::file_transfer::TERMINAL_ROUND_LIMIT;
use uc_observability_contract::uc_debug;

pub(super) fn record_exchange(result: Result<(), ProgressReceiveError>) {
    if let Err(source) = result {
        uc_debug!(
            error_class = source.class(),
            error = &source as &dyn Error,
            "transfer progress inbound exchange failed"
        );
    }
}
/// 截止或取消只封口网络；已经poll的本地能力仍结算至真实结束。
pub(super) async fn settle<T>(
    work: impl Future<Output = Result<T, ProgressReceiveError>>,
    deadline: &ProgressExchangeDeadline,
    cancel: &CancellationToken,
    stop_network: impl Fn(),
) -> Result<T, ProgressReceiveError> {
    let mut started = false;
    tokio::pin!(work);
    let primary = {
        let observed = poll_fn(|cx| {
            started = true;
            work.as_mut().poll(cx)
        });
        tokio::pin!(observed);
        tokio::select! {
            biased;
            expired = deadline.network(pending::<Result<(), ProgressReceiveError>>(), cancel) => match expired {
                Err(source) => source,
                Ok(()) => ProgressReceiveError::Protocol,
            },
            result = &mut observed => return result.and_then(|value| deadline.check(cancel).map(|()| value)),
        }
    };
    cancel.cancel();
    stop_network();
    if started {
        if let Err(additional) = work.await {
            // 同一取消事实没有新的下层异常，不伪造第二项结算失败。
            if matches!(
                &primary,
                ProgressReceiveError::Connection(ConnectivityError::Stopped)
            ) && matches!(
                &additional,
                ProgressReceiveError::Connection(ConnectivityError::Stopped)
            ) {
                return Err(primary);
            }
            return Err(ProgressReceiveError::Settlement {
                primary: Box::new(primary),
                additional: Box::new(additional),
            });
        }
    }
    Err(primary)
}
impl TransferProgressTransport {
    pub(super) async fn receive_connection(
        self: &Arc<Self>,
        lease: ConnectionLease,
        generation: u64,
        first_deadline: ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        match lease.protocol() {
            ConnectionProtocol::TransferProgressV1 => {
                let stream = settle(
                    async { Ok(lease.accept_uni(first_deadline.instant(), cancel).await?) },
                    &first_deadline,
                    cancel,
                    || lease.close(),
                )
                .await?;
                settle(
                    self.wire
                        .legacy(stream, &lease, first_deadline.clone(), cancel),
                    &first_deadline,
                    cancel,
                    || {},
                )
                .await
            }
            ConnectionProtocol::TransferProgressV2 => {
                // 保留资格在同一首轮预算内核对；后续每条流仍由原grant重查当前授权。
                let retained = settle(
                    async { Ok(self.manager.retain(&lease).await?) },
                    &first_deadline,
                    cancel,
                    || lease.close(),
                )
                .await?;
                let first = settle(
                    async { Ok(lease.accept_bi().await?) },
                    &first_deadline,
                    cancel,
                    || lease.close(),
                )
                .await?;
                self.receive_current(lease, generation, first, first_deadline, cancel, retained)
                    .await
            }
            _ => Err(ProgressReceiveError::Protocol),
        }
    }
    fn start_exchange(
        self: &Arc<Self>,
        jobs: &mut JoinSet<Result<(), ProgressReceiveError>>,
        lease: ConnectionLease,
        generation: u64,
        stream: StreamLease,
        deadline: ProgressExchangeDeadline,
        cancel: CancellationToken,
    ) {
        let owner = Arc::clone(self);
        jobs.spawn(async move {
            settle(
                owner
                    .wire
                    .current(stream, &lease, generation, deadline.clone(), &cancel),
                &deadline,
                &cancel,
                || {},
            )
            .await
        });
    }
    async fn receive_current(
        self: &Arc<Self>,
        lease: ConnectionLease,
        generation: u64,
        first: StreamLease,
        deadline: ProgressExchangeDeadline,
        cancel: &CancellationToken,
        retained: bool,
    ) -> Result<(), ProgressReceiveError> {
        let mut jobs = JoinSet::new();
        self.start_exchange(
            &mut jobs,
            lease.clone(),
            generation,
            first,
            deadline,
            cancel.child_token(),
        );
        if !retained {
            while let Some(result) = jobs.join_next().await {
                self.exchange_task(result);
            }
            return Ok(());
        }
        loop {
            let accepting = lease.accept_bi();
            tokio::pin!(accepting);
            let accepted = loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => { lease.close(); break accepting.await; },
                    _ = lease.closed() => break accepting.await,
                    Some(result) = jobs.join_next(), if !jobs.is_empty() => {
                        if self.exchange_task(result) { lease.close(); }
                    }
                    result = &mut accepting => break result,
                }
            };
            match accepted {
                Ok(stream) if !cancel.is_cancelled() && !lease.is_closed() => {
                    let deadline = stream
                        .received_elapsed_at
                        .ok_or_else(|| ElapsedClockError {
                            source: Box::new(ElapsedClockRefusal::ContinuityLost),
                        })
                        .map_err(ProgressReceiveError::from)
                        .and_then(|origin| {
                            ProgressExchangeDeadline::received(
                                self.clock.clone(),
                                origin,
                                TERMINAL_ROUND_LIMIT,
                            )
                        });
                    match deadline {
                        Ok(deadline) => self.start_exchange(
                            &mut jobs,
                            lease.clone(),
                            generation,
                            stream,
                            deadline,
                            cancel.child_token(),
                        ),
                        Err(source) => {
                            drop(stream);
                            record_exchange(Err(source));
                        }
                    }
                }
                Err(ConnectivityError::Busy(source)) => {
                    record_exchange(Err(ConnectivityError::Busy(source).into()))
                }
                accepted => {
                    while let Some(result) = jobs.join_next().await {
                        self.exchange_task(result);
                    }
                    return match accepted {
                        Ok(_) => Err(ConnectivityError::Stopped.into()),
                        Err(source) => Err(source.into()),
                    };
                }
            }
        }
    }
}
