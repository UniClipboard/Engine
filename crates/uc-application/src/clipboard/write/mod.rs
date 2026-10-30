//! 系统剪贴板写入负责人及相关能力。

mod active_register;
mod coordinator;
mod mobile_consumability;
mod restore_broadcast;

pub use active_register::LocalActiveRegisterAdvancer;
pub use coordinator::{ClipboardWriteCoordinator, ClipboardWriteIntent};
pub use mobile_consumability::{
    MobileConsumabilityProbe, MobileConsumableBackfill, MobileConsumableRefBackfill,
};
pub use restore_broadcast::{
    restore_broadcast_channel, RestoreBroadcastReceiver, RestoreBroadcastRequest,
    RestoreBroadcastTrigger,
};
