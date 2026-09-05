//! Engine 私有的跨领域观测装配。
//!
//! 具体业务领域拥有自己的 port decorator 和事件 schema；本模块不提供万能埋点函数。

mod admission;
mod clipboard;
mod membership;
mod storage_upgrade;

pub(crate) use admission::observe_admission_endpoint;
pub(crate) use clipboard::observe_clipboard;
pub(crate) use clipboard::{observe_clipboard_dependencies, observe_local_copy};
pub(crate) use membership::observe_membership;
pub(crate) use storage_upgrade::{profile_storage_upgrade_span, record_profile_storage_upgrade};
