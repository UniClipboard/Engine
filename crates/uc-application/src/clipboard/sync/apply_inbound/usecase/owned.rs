use crate::transfer::receive::retry::PreparedReceiveRetry;
use crate::transfer::receive::work::ReceiveWorkError;
use anyhow::Error as SourceError;
use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    sync::{Arc, Mutex, PoisonError},
};

use tokio::sync::oneshot;
use tracing::Instrument;
use uc_core::ports::ReceiveItemRole;
use uc_observability_contract::diagnostics::ObservationContext;

use super::{ApplyInboundClipboardUseCase, ApplyInboundError, ApplyInboundInput, ApplyOutcome};

impl ApplyInboundClipboardUseCase {
    pub(super) async fn execute_owned(
        &self,
        input: ApplyInboundInput,
        provisional: Option<(String, ReceiveItemRole)>,
        retry: Option<PreparedReceiveRetry>,
    ) -> Result<ApplyOutcome, ApplyInboundError> {
        let work = Arc::new(self.work.begin().ok_or(ApplyInboundError::Stopped)?);
        let action = Arc::clone(&work);
        let receive_action = retry
            .as_ref()
            .map_or_else(|| self.receive_work.action(), |retry| retry.action.clone());
        let processing_receive = receive_action.clone();
        let (sender, receiver) = oneshot::channel();
        let reply = ReceiveReply(Arc::new(Mutex::new(Some(sender))));
        let processing_reply = reply.clone();
        let owner = self.clone();
        let observation = ObservationContext::capture();
        let task = tokio::spawn(
            observation.scope(
                async move {
                    let outcome = owner
                        .execute_internal(
                            input,
                            provisional,
                            &action,
                            &processing_receive,
                            retry,
                            &processing_reply,
                        )
                        .await;
                    match (outcome, processing_receive.take_settlement()) {
                        (Err(error), Some(pending)) => {
                            let (caller, failure) = share_failure(error);
                            let failure = Arc::new(failure);
                            processing_reply.respond(Err(caller));
                            // 原任务继续持有原许可，只修复失败结算，不重试下载或保存。
                            owner
                                .recover_settlement(pending, &processing_receive, failure.clone())
                                .await;
                            Processed::Recovered(failure)
                        }
                        (outcome, None) => Processed::Original(outcome),
                        // 成功路径不应留下失败责任；保留责任并完成结算，不能静默释放。
                        (Ok(_), Some(pending)) => {
                            let (caller, failure) = share_failure(ApplyInboundError::Internal(
                                SourceError::msg("successful receive retained unsettled failure"),
                            ));
                            let failure = Arc::new(failure);
                            processing_reply.respond(Err(caller));
                            owner
                                .recover_settlement(pending, &processing_receive, failure.clone())
                                .await;
                            Processed::Recovered(failure)
                        }
                    }
                }
                .in_current_span(),
            ),
        );
        // 同一许可由处理与监督共同持有，异常登记完成前关闭不能返回成功。
        tokio::spawn(async move {
            let outcome = match task.await {
                Ok(Processed::Original(Ok(outcome))) => {
                    receive_action.finish(Ok(()));
                    Some(Ok(outcome))
                }
                Ok(Processed::Original(Err(error))) if !receive_action.is_registered() => {
                    receive_action.batch.finish(false);
                    Some(Err(error))
                }
                Ok(Processed::Original(Err(error))) => {
                    let (caller_error, shared) = share_failure(error);
                    receive_action.finish(Err(shared));
                    Some(Err(caller_error))
                }
                Ok(Processed::Recovered(source)) => {
                    receive_action.finish_shared(Err(source));
                    None
                }
                Err(source) => {
                    let source = work.failed(source);
                    receive_action.finish(Err(ReceiveWorkError::Task(source.clone())));
                    Some(Err(ApplyInboundError::WorkFailed(source.into())))
                }
            };
            drop(work);
            if let Some(outcome) = outcome {
                reply.respond(outcome);
            }
        });
        receiver
            .await
            .map_err(|source| ApplyInboundError::WorkFailed(source.into()))?
    }
}

#[derive(Debug)]
struct SharedSource(SourceError);

impl Display for SharedSource {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, formatter)
    }
}

impl Error for SharedSource {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

fn share_failure(error: ApplyInboundError) -> (ApplyInboundError, ReceiveWorkError) {
    let (wrap, source): (fn(SourceError) -> ApplyInboundError, SourceError) = match error {
        ApplyInboundError::Stopped => {
            return (ApplyInboundError::Stopped, ReceiveWorkError::Stopped)
        }
        ApplyInboundError::WorkFailed(source) => (ApplyInboundError::WorkFailed, source),
        ApplyInboundError::DedupQuery(source) => (ApplyInboundError::DedupQuery, source),
        ApplyInboundError::Capture(source) => (ApplyInboundError::Capture, source),
        ApplyInboundError::Internal(source) => (ApplyInboundError::Internal, source),
    };
    let shared = Arc::new(SharedSource(source));
    (
        wrap(SourceError::new(shared.clone())),
        ReceiveWorkError::Action(SourceError::new(shared)),
    )
}

enum Processed {
    Original(Result<ApplyOutcome, ApplyInboundError>),
    Recovered(Arc<ReceiveWorkError>),
}

/// 同一个原处理任务可以先报告本地错误，再继续完成资源结算。
#[derive(Clone)]
pub(super) struct ReceiveReply(
    Arc<Mutex<Option<oneshot::Sender<Result<ApplyOutcome, ApplyInboundError>>>>>,
);

impl ReceiveReply {
    pub(super) fn local_error(&self, error: ApplyInboundError) -> Arc<ReceiveWorkError> {
        let (caller, failure) = share_failure(error);
        self.respond(Err(caller));
        Arc::new(failure)
    }

    fn respond(&self, outcome: Result<ApplyOutcome, ApplyInboundError>) {
        if let Some(sender) = self.0.lock().unwrap_or_else(PoisonError::into_inner).take() {
            // discarded-source[channel]: 等待方已退出，原监督任务仍负责实际完成。
            let _ = sender.send(outcome);
        }
    }
}
