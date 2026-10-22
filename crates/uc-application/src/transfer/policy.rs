//! 第七片的资源与单调时间预算；下载、恢复和提供端共用这一份默认值。
use std::time::Duration;

pub(super) const PROGRESS_REPORT_INTERVAL: Duration = Duration::from_millis(200);

pub(super) const ACTIVE_DOWNLOADS: usize = 4;
pub(super) const ACTIVE_DOWNLOADS_PER_PEER: usize = 2;
pub(super) const QUEUED_DOWNLOADS: usize = 64;
pub(super) const QUEUED_DOWNLOADS_PER_PEER: usize = 16;
pub(super) const QUEUE_TIMEOUT: Duration = Duration::from_secs(60);
pub(super) const FIRST_DATA_TIMEOUT: Duration = Duration::from_secs(60);
pub(super) const DATA_STALL_TIMEOUT: Duration = Duration::from_secs(300);
pub(super) const CANCEL_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
/// 同一接收动作的本地结算修复，不消耗或重置网络恢复预算。
pub(crate) const LOCAL_SETTLEMENT_RETRY_INITIAL: Duration = Duration::from_secs(1);
pub(crate) const LOCAL_SETTLEMENT_RETRY_MAX: Duration = Duration::from_secs(30);
pub(super) const MAX_FETCH_ATTEMPTS: u32 = 4;
pub(super) const FETCH_BACKOFFS: [Duration; 3] = [
    Duration::from_millis(200),
    Duration::from_millis(800),
    Duration::from_millis(2000),
];
pub(super) const RECOVERY_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const ACTIVE_PROVIDES: usize = 4;
pub(super) const ACTIVE_PROVIDES_PER_PEER: usize = 2;
pub(super) const UNIDENTIFIED_INBOUND: usize = 16;
pub(super) const INBOUND_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub(super) const CONNECTION_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

// 仅限制跨请求保留集合；新对端可淘汰空闲连接，不作为服务任务总数门槛。
pub(super) const RETAINED_BLOB_CONNECTIONS: usize = 16;
