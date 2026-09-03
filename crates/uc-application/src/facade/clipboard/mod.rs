//! Outbound clipboard dispatch and receive-management facade.

mod cancel_entry_receive;
pub(crate) mod facade;

pub use cancel_entry_receive::{CancelEntryReceiveError, CancelEntryReceiveOutcome};
pub use facade::{
    ClipboardSyncError, ClipboardSyncFacade, DispatchEntryInput, DispatchEntryOutcome,
    DispatchEntryPerTarget,
};

// 投递状态视图相关类型——外部 crate 通过 `ClipboardSyncFacade::get_entry_delivery_view`
// 取得,渲染层使用 view 类型来绘制 UI;失败枚举沿用 `uc_core::clipboard::DeliveryFailureReason`,
// 外部按需直接从 uc-core 引入。
pub use crate::clipboard::sync::get_entry_delivery_view::{
    EntryDeliveryStatusView, EntryDeliveryTargetView, EntryDeliveryView, EntrySource,
    GetEntryDeliveryViewError,
};
