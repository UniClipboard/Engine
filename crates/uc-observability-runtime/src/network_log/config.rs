//! 独立网络日志的配置：目录根、进程角色与容量预算。

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use crate::ConfigError;

/// 承载 Engine 的进程角色。固定枚举，目录名与导出来源都只使用这些值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkProcessRole {
    Daemon,
    MobileApp,
    KeyboardExtension,
    ShareExtension,
    BackgroundService,
}

impl NetworkProcessRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daemon => "daemon",
            Self::MobileApp => "mobile_app",
            Self::KeyboardExtension => "keyboard_extension",
            Self::ShareExtension => "share_extension",
            Self::BackgroundService => "background_service",
        }
    }
}

/// 网络日志的容量预算。没有默认值：数值由宿主显式给出，产品默认值须经用户确认后才能固定。
///
/// 单条记录由规则表构造且远小于 `record_bytes`；队列内存上限因此是 `queue_records * record_bytes`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkLogBudget {
    /// 单条编码后的字节上限；超出的记录整条丢弃并计数。
    pub record_bytes: usize,
    /// 写入队列的记录容量；队列满时新记录立即丢弃，业务线程不等待磁盘。
    pub queue_records: usize,
    /// 单个分段文件的字节上限。
    pub file_bytes: u64,
    /// 单次运行目录的字节上限；超出时从最旧的分段起淘汰。
    pub run_bytes: u64,
    /// 网络日志根目录的字节上限，只回收没有活动写入者的旧运行目录。
    pub root_bytes: u64,
    /// 旧运行目录的保留时间。
    pub retention: Duration,
}

impl NetworkLogBudget {
    fn is_valid(&self) -> bool {
        self.record_bytes >= 512
            && self.queue_records > 0
            && self.file_bytes >= self.record_bytes as u64
            && self.run_bytes >= self.file_bytes
            && self.root_bytes >= self.run_bytes
            && !self.retention.is_zero()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct NetworkLogConfig {
    pub(crate) root: PathBuf,
    pub(crate) role: NetworkProcessRole,
    pub(crate) budget: NetworkLogBudget,
    pub(crate) known_alpns: Vec<String>,
}

impl NetworkLogConfig {
    /// `root` 是宿主批准的日志根；网络日志只写其下的 `network/<role>/<run>/`。
    pub fn new(
        root: impl Into<PathBuf>,
        role: NetworkProcessRole,
        budget: NetworkLogBudget,
    ) -> Result<Self, ConfigError> {
        if !budget.is_valid() {
            return Err(ConfigError::InvalidNetworkLogBudget);
        }
        Ok(Self {
            root: root.into(),
            role,
            budget,
            known_alpns: Vec::new(),
        })
    }

    /// 登记产品使用的 ALPN。日志只输出登记过的取值，其余统一记为 `unrecognized`，
    /// 不保留对端发来的任意字符串。
    pub fn with_known_alpns(mut self, alpns: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.known_alpns = alpns.into_iter().map(Into::into).collect();
        self
    }
}

impl fmt::Debug for NetworkLogConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NetworkLogConfig")
            .field("root", &"REDACTED")
            .field("role", &self.role)
            .field("budget", &self.budget)
            .finish()
    }
}
