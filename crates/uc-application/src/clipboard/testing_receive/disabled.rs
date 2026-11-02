//! 未启用验收功能时的同步空观测；不保存身份，不提供故障控制。
use crate::clipboard::inbound::InboundClipboardApplyOutcome;
use uc_core::ids::DeviceId;

pub(crate) enum ReceivePath {
    ClipboardPush,
}

pub(crate) enum ReceiveStage {
    ApplyStarted,
    Failed,
}

#[derive(Clone, Copy)]
pub(crate) struct ReceiveAttempt;

impl ReceiveAttempt {
    pub(crate) fn begin(_: ReceivePath, _: DeviceId, _: &str) -> Option<Self> {
        None
    }

    pub(crate) fn record(self, _: ReceiveStage, _: Option<&str>) {}

    pub(crate) fn apply_outcome(self, _: &InboundClipboardApplyOutcome) {}
}

pub(crate) fn current_stage(_: ReceiveStage) {}

pub(crate) fn current_apply_outcome(_: &InboundClipboardApplyOutcome) {}
