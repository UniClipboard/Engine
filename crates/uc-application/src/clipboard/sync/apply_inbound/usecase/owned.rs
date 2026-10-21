use crate::transfer::receive::work::ReceiveWorkError;
use anyhow::Error as SourceError;
use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    sync::Arc,
};

use tracing::Instrument;
use uc_core::ports::ReceiveItemRole;
use uc_observability_contract::diagnostics::ObservationContext;

use super::{ApplyInboundClipboardUseCase, ApplyInboundError, ApplyInboundInput, ApplyOutcome};

impl ApplyInboundClipboardUseCase {
    pub(super) async fn execute_owned(
        &self,
        input: ApplyInboundInput,
        provisional: Option<(String, ReceiveItemRole)>,
    ) -> Result<ApplyOutcome, ApplyInboundError> {
        let work = Arc::new(self.work.begin().ok_or(ApplyInboundError::Stopped)?);
        let action = Arc::clone(&work);
        let receive_action = self.receive_work.action();
        let processing_receive = receive_action.clone();
        let owner = self.clone();
        let observation = ObservationContext::capture();
        let task = tokio::spawn(
            observation.scope(
                async move {
                    owner
                        .execute_internal(input, provisional, &action, &processing_receive)
                        .await
                }
                .in_current_span(),
            ),
        );
        // 同一许可由处理与监督共同持有，异常登记完成前关闭不能返回成功。
        tokio::spawn(async move {
            let outcome = match task.await {
                Ok(Ok(outcome)) => {
                    receive_action.finish(Ok(()));
                    Ok(Ok(outcome))
                }
                Ok(Err(error)) if !receive_action.is_registered() => Ok(Err(error)),
                Ok(Err(error)) => {
                    let (caller_error, shared) = share_failure(error);
                    receive_action.finish(Err(shared));
                    Ok(Err(caller_error))
                }
                Err(source) => {
                    let source = work.failed(source);
                    receive_action.finish(Err(ReceiveWorkError::Task(source.clone())));
                    Err(ApplyInboundError::WorkFailed(source.into()))
                }
            };
            drop(work);
            outcome?
        })
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
