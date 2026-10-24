use super::super::ProgressExchangeDeadline;
use super::handle::Inner;
use super::model::{Action, Completion, Work};
use super::retry::{retry_class, RetryClass};
use crate::connectivity::ConnectivityError;
use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use std::error::Error;
use std::sync::{Arc, PoisonError};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uc_core::error_class::ErrorClass;
use uc_observability_contract::uc_debug;

/// 生命周期负责人必须持有run到返回；封口后仍监督全部已启动future。
pub struct ProgressExchangeWorker {
    pub(super) inner: Arc<Inner>,
}
impl ProgressExchangeWorker {
    pub async fn run(self, cancel: CancellationToken) {
        let mut running: FuturesUnordered<BoxFuture<'static, (Work, Completion)>> =
            FuturesUnordered::new();
        let mut clock_unavailable = false;
        loop {
            let sample = self.inner.clock.sample();
            if let Err(source) = &sample {
                if !clock_unavailable {
                    uc_debug!(
                        error_class = source.class(),
                        error = source as &dyn Error,
                        "progress exchange clock unavailable"
                    );
                }
            }
            clock_unavailable = sample.is_err();
            let epoch = self.inner.clock.epoch();
            let jobs = {
                let mut state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if cancel.is_cancelled() {
                    state.close();
                }
                match sample {
                    Ok(sample) if !state.closed => state.dispatch(sample),
                    Err(source) => {
                        state.retire_before(epoch, Some(Arc::new(source)));
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            };
            for job in jobs {
                let inner = self.inner.clone();
                running.push(Box::pin(async move {
                    let clock = inner.clock.lease(job.epoch);
                    let result = match job.action {
                        Action::Terminal { descriptor, result } => Completion::Terminal(
                            match ProgressExchangeDeadline::new(clock, job.deadline) {
                                Ok(deadline) => {
                                    inner
                                        .round
                                        .report_settled(
                                            &job.work.peer(),
                                            descriptor,
                                            result,
                                            &deadline,
                                            &job.cancel,
                                        )
                                        .await
                                }
                                Err(source) => Err(source),
                            },
                        ),
                        Action::Snapshot(expected) => {
                            let interested = match job.work {
                                Work::Snapshot(key) => inner
                                    .state
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner)
                                    .query_interested(key),
                                _ => false,
                            };
                            Completion::Snapshot(if !interested {
                                Err(ConnectivityError::Stopped.into())
                            } else {
                                match ProgressExchangeDeadline::new(clock, job.deadline) {
                                    Ok(deadline) => {
                                        inner
                                            .snapshot
                                            .read_snapshot(
                                                &job.work.peer(),
                                                expected,
                                                &deadline,
                                                &job.cancel,
                                            )
                                            .await
                                    }
                                    Err(source) => Err(source),
                                }
                            })
                        }
                    };
                    (job.work, result)
                }));
            }
            let (closed, has_work) = {
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                (state.closed, state.has_work())
            };
            if closed && running.is_empty() {
                break;
            }
            tokio::select! {
                biased;
                _=cancel.cancelled(),if !closed=>{},
                Some((work,result))=running.next(),if !running.is_empty()=>{
                    match result {
                        Completion::Terminal(result)=>{
                            let class=match &result {Ok(())=>RetryClass::Stop,Err(source)=>retry_class(source)};
                            if let Err(source)=&result {if !closed {uc_debug!(error_class=source.class(),error=source as &dyn Error,"progress terminal notification round ended without acknowledgment");}}
                            if let Work::Terminal(key)=work {self.inner.state.lock().unwrap_or_else(PoisonError::into_inner).finish(key,result.is_ok(),matches!(class,RetryClass::Retry),matches!(class,RetryClass::Cancelled));}
                        },
                        Completion::Snapshot(result)=>{
                            if let Err(source)=&result {if !closed {uc_debug!(error_class=source.class(),error=source as &dyn Error,"progress snapshot exchange ended without complete view");}}
                            // 最后一次本机读取可能晚于期限；发布前先检查同一可信时钟，不能返回迟到成功。
                            let sample=self.inner.clock.sample();
                            let epoch=self.inner.clock.epoch();
                            let mut state=self.inner.state.lock().unwrap_or_else(PoisonError::into_inner);
                            match sample {Ok(sample)=>state.refresh(Some(sample)),Err(source)=>state.retire_before(epoch,Some(Arc::new(source)))}
                            if let Work::Snapshot(key)=work {state.finish_query(key,result);}
                        },
                    }
                    tokio::task::yield_now().await;
                },
                _=self.inner.wake.notified()=>{},
                _=tokio::time::sleep(Duration::from_millis(100)),if has_work=>{},
            }
        }
    }
}
impl Drop for ProgressExchangeWorker {
    fn drop(&mut self) {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .close();
    }
}
