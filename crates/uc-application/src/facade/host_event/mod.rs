//! 宿主事件对外入口(ADR-018 阶段 4)。
//!
//! 事件总线、出站缓存和发布器实现在 `crate::support`;本模块只做对外
//! 白名单再导出。稳定事件类型直接来自 `uc-core` 的规范定义。

pub use crate::support::host_event_bus::HostEventBus;
pub use crate::support::host_event_publisher::FileTransferHostEventPublisher;
pub use crate::support::outbound_entry_cache::OutboundEntryIdCache;
pub use uc_core::ports::host_event::{
    ClipboardHostEvent, ClipboardOriginKind, DeliveryHostEvent, EmitError, HostEvent,
    HostEventEmitterPort, TransferHostEvent,
};
