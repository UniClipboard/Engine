//! 受管连接与进度期限共同消费的包含挂起时间能力；不读取墙钟或持久化计数。
use std::error::Error;
use std::fmt;
use std::time::Duration;
use uc_core::error_class::ErrorClass;

pub trait ElapsedClockPort: Send + Sync {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError>;
}

#[derive(thiserror::Error)]
#[error("elapsed clock unavailable")]
pub struct ElapsedClockError {
    #[source]
    pub source: Box<dyn Error + Send + Sync>,
}
impl fmt::Debug for ElapsedClockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}
impl ErrorClass for ElapsedClockError {
    fn class(&self) -> &'static str {
        "elapsed_clock_unavailable"
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ElapsedClockRefusal {
    #[error("elapsed clock continuity lost")]
    ContinuityLost,
    #[error("elapsed clock reading regressed")]
    NonMonotonicReading,
    #[error("platform has no suspend-inclusive clock")]
    UnsupportedPlatform,
    #[error("native elapsed clock count is invalid")]
    InvalidNativeCount,
}
