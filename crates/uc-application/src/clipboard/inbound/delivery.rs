//! 接收队列的任务信封；业务负载与在线关联分开持有。

use tokio::sync::broadcast;
use uc_core::ports::InboundClipboard;
use uc_observability_contract::diagnostics::connectivity::ClipboardReceiveObservation;
use uc_observability_contract::diagnostics::ObservationContext;

/// 已认证接收任务。关联不可读取，不参与授权、去重或回执。
#[derive(Clone)]
pub struct ClipboardDelivery {
    pub message: InboundClipboard,
    pub(crate) observation: ObservationContext,
    pub(crate) receive_observation: ClipboardReceiveObservation,
}

impl ClipboardDelivery {
    pub fn new(message: InboundClipboard) -> Self {
        Self {
            message,
            observation: ObservationContext::capture(),
            receive_observation: ClipboardReceiveObservation::capture(),
        }
    }
}

impl std::fmt::Debug for ClipboardDelivery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipboardDelivery").finish_non_exhaustive()
    }
}

/// 订阅有界接收队列；串行处理、回执与关闭仍由接收运行期负责。
pub trait ClipboardReceiverPort: Send + Sync {
    fn subscribe(&self) -> broadcast::Receiver<ClipboardDelivery>;
}
