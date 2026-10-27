//! 进度连接的有界空闲保留策略；活动使用权不按空闲回收。
use std::time::Duration;
pub const PROGRESS_IDLE_RETENTION: Duration = Duration::from_secs(30);
pub const PROGRESS_RETAINED_CONNECTIONS: usize = 16;
