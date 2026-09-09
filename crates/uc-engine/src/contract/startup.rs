use serde::{Deserialize, Serialize};

/// 一次启动的只读状态；资料升级完成不等于 Engine 已就绪。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupState {
    Preparing,
    Upgrading,
    StartingServices,
    Ready,
    Failed,
    Interrupted,
}

impl StartupState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Ready | Self::Failed | Self::Interrupted)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupUpgradeStep {
    Checking,
    ConvertingContents,
    ConvertingLargeContents,
    ConvertingRelatedRecords,
    Verifying,
    Preparing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupProgressUnit {
    ContentRepresentations,
    LargeContents,
    RelatedRecords,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupStepProgress {
    pub step: StartupUpgradeStep,
    pub processed: u64,
    pub total: Option<u64>,
    pub unit: Option<StartupProgressUnit>,
    /// 未完成恢复核对时为 None，不把未知警告写成零。
    pub warning_count: Option<u64>,
    /// 只有负责人完成必要验证与耐久提交后才为 true。
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupUpgradeProgress {
    pub required: bool,
    pub recovering: bool,
    pub completed: bool,
    pub current_step: Option<StartupUpgradeStep>,
    /// 每种步骤最多一项，快照合并不会删除已完成步骤。
    pub steps: Vec<StartupStepProgress>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupFailureReason {
    StorageFull,
    PermissionDenied,
    StorageUnavailable,
    ProtectionUnavailable,
    CorruptData,
    SourceChanged,
    AlreadyRunning,
    StartupFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupFailure {
    pub reason: StartupFailureReason,
    pub retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupAllowedActions {
    pub retry: bool,
    pub export_diagnostics: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupSnapshot {
    pub attempt_id: String,
    pub sequence: u64,
    pub state: StartupState,
    /// 自通道创建起的耗时；仅随真实状态更新，不代表心跳。
    pub elapsed_ms: u64,
    pub upgrade: Option<StartupUpgradeProgress>,
    pub failure: Option<StartupFailure>,
    pub allowed_actions: StartupAllowedActions,
}
