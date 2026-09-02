use async_trait::async_trait;
use bytes::Bytes;
use thiserror::Error;
use uc_core::ids::DeviceId;
use uc_observability_contract::FlowId;

use crate::clipboard::sync::apply_inbound::{
    ApplyInboundClipboardUseCase, ApplyInboundError, ApplyInboundInput, ApplyOutcome,
};
use crate::clipboard::write::ClipboardWriteIntent;

mod runtime;

pub use runtime::{
    ClipboardInboundEvent, ClipboardInboundEventAction, ClipboardInboundEventPort,
    ClipboardInboundRepresentationSummary, ClipboardInboundRuntime, ClipboardInboundRuntimeDeps,
    ClipboardInboundRuntimeError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundClipboardApplyOutcome {
    Applied {
        entry_id: String,
    },
    /// Content already held locally; the existing entry was re-activated
    /// instead of duplicated. Mirrors [`ApplyOutcome::Resurfaced`].
    Resurfaced {
        snapshot_hash: String,
        existing_entry_id: String,
        os_write_succeeded: bool,
    },
    DuplicateSkipped {
        snapshot_hash: String,
        existing_entry_id: String,
    },
    DecodeFailed {
        reason: String,
    },
}

#[derive(Debug, Error)]
pub enum InboundClipboardApplyError {
    #[error("inbound clipboard apply failed")]
    Internal(#[source] ApplyInboundError),
}

#[async_trait]
pub trait InboundClipboardApplyPort: Send + Sync {
    async fn apply(
        &self,
        input: InboundClipboardApplyInput,
    ) -> Result<InboundClipboardApplyOutcome, InboundClipboardApplyError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundClipboardApplyInput {
    pub from_device: String,
    pub snapshot_hash: String,
    pub plaintext: Bytes,
    pub flow_id: Option<FlowId>,
    /// 可选的 LAN provisional receive 认领上下文；由同一完整 intent 在
    /// 成功或失败尾部完成结算，调用方不接触 receive attempt 步骤。
    pub provisional: Option<InboundProvisionalReceive>,
    /// See [`ApplyInboundInput::resurface_intent`] — only consulted when the
    /// delivery resolves to an already-held entry.
    pub resurface_intent: ClipboardWriteIntent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundProvisionalReceive {
    pub transfer_id: String,
    pub role: uc_core::ports::ReceiveItemRole,
}

#[async_trait]
impl InboundClipboardApplyPort for ApplyInboundClipboardUseCase {
    async fn apply(
        &self,
        input: InboundClipboardApplyInput,
    ) -> Result<InboundClipboardApplyOutcome, InboundClipboardApplyError> {
        let provisional = input.provisional;
        let input = ApplyInboundInput {
            from_device: DeviceId::new(input.from_device),
            snapshot_hash: input.snapshot_hash,
            plaintext: input.plaintext,
            flow_id: input.flow_id,
            resurface_intent: input.resurface_intent,
        };
        let outcome = match provisional {
            Some(provisional) => {
                self.execute_with_provisional(input, provisional.transfer_id, provisional.role)
                    .await
            }
            None => self.execute(input).await,
        }
        .map_err(InboundClipboardApplyError::Internal)?;
        Ok(apply_outcome_to_view(outcome))
    }
}

fn apply_outcome_to_view(outcome: ApplyOutcome) -> InboundClipboardApplyOutcome {
    match outcome {
        ApplyOutcome::Applied { entry_id } => InboundClipboardApplyOutcome::Applied {
            entry_id: entry_id.to_string(),
        },
        ApplyOutcome::Resurfaced {
            snapshot_hash,
            existing_entry_id,
            os_write_succeeded,
        } => InboundClipboardApplyOutcome::Resurfaced {
            snapshot_hash,
            existing_entry_id: existing_entry_id.to_string(),
            os_write_succeeded,
        },
        ApplyOutcome::DuplicateSkipped {
            snapshot_hash,
            existing_entry_id,
        } => InboundClipboardApplyOutcome::DuplicateSkipped {
            snapshot_hash,
            existing_entry_id: existing_entry_id.to_string(),
        },
        ApplyOutcome::DecodeFailed { reason } => {
            InboundClipboardApplyOutcome::DecodeFailed { reason }
        }
    }
}
