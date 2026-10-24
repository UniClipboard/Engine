//! 入站剪贴板处理入口；数据模型、错误、专属能力与完整流程按职责组织。
//! 完整接收仍交给原 transfer/receive 负责人，调用方不编排逐项网络和结算步骤。

mod error;
mod model;
mod observation;
pub(crate) mod ports;
mod timing;
mod usecase;

#[cfg(test)]
mod tests;

pub use error::ApplyInboundError;
pub use model::{ApplyInboundInput, ApplyOutcome, InboundActivationOwner};
pub(crate) use observation::classify_apply_failure;
pub use ports::{InboundCapture, InboundSnapshotRebuild, InboundWrite};
pub use usecase::ApplyInboundClipboardUseCase;
pub(crate) use usecase::{
    InboundApplyCommonDeps, InboundReceiveAttemptDeps, InteractiveReceiveDeps, StoreOnlyPullDeps,
};
