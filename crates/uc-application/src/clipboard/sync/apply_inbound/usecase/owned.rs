use std::sync::Arc;

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
        let owner = self.clone();
        let observation = ObservationContext::capture();
        let task = tokio::spawn(
            observation.scope(
                async move { owner.execute_internal(input, provisional, &action).await }
                    .in_current_span(),
            ),
        );
        // 同一许可由处理与监督共同持有，异常登记完成前关闭不能返回成功。
        tokio::spawn(async move {
            let outcome = task
                .await
                .map_err(|source| ApplyInboundError::WorkFailed(work.failed(source).into()));
            drop(work);
            outcome?
        })
        .await
        .map_err(|source| ApplyInboundError::WorkFailed(source.into()))?
    }
}
