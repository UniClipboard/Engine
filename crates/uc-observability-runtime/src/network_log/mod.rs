//! 第三方网络库（iroh、noq）的独立日志。
//!
//! 与 Engine 业务日志共用进程唯一的 subscriber，但 schema、隐私规则、目录、队列、写入线程、预算、
//! 健康状态和导出来源都是独立的；业务日志的成败不影响它，它的成败也不影响业务。
//! 通用的有界文件队列来自 `local_file`，这里只提供自己的写入策略和事件规则。

mod config;
mod layer;
pub(crate) mod rules;
mod runtime;
mod status;
mod store;

#[cfg(feature = "test-process-release")]
pub(crate) use store::{
    PROBE_NETWORK_PARK_BLOCK, PROBE_NETWORK_PARK_ENTERED, PROBE_NETWORK_PARK_FAILURE,
};

pub use config::{NetworkLogBudget, NetworkLogConfig, NetworkProcessRole};
pub(crate) use runtime::NetworkLogRuntime;
pub use status::{
    NetworkCaptureMode, NetworkCaptureStatus, NetworkLibraryStatus, NetworkLogCounters,
    NetworkLogError, NetworkLogExportReport, NetworkLogFile, NetworkLogState, NetworkLogStatus,
    NETWORK_LOG_SCHEMA_VERSION,
};
