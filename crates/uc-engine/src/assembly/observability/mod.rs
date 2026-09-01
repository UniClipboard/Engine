//! Engine 私有的跨领域观测装配。
//!
//! 具体业务领域拥有自己的 port decorator 和事件 schema；本模块不提供万能埋点函数。

pub(crate) mod admission;
