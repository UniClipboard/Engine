use super::{EngineError, EngineErrorCategory};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineState {
    Running,
    Quiescing,
    Quiesced,
    Suspended,
    ShuttingDown,
    Stopped,
}

impl EngineState {
    pub fn accepts_operations(self) -> bool {
        self == Self::Running
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Running, Self::Quiescing)
                | (Self::Quiescing, Self::Quiesced)
                | (Self::Quiesced, Self::Suspended)
                | (Self::Suspended, Self::Quiesced)
                | (Self::Quiesced, Self::Running)
                | (Self::Suspended, Self::Running)
                | (Self::Running, Self::ShuttingDown)
                | (Self::Quiescing, Self::ShuttingDown)
                | (Self::Quiesced, Self::ShuttingDown)
                | (Self::Suspended, Self::ShuttingDown)
                | (Self::ShuttingDown, Self::Stopped)
        )
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleAction {
    Suspend,
    Resume,
}

/// 单次 Engine 资源释放的完整结果；进程观测由宿主独立负责。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceReleaseOutcome {
    Pending,
    Released { error: Option<EngineError> },
    Failed { error: EngineError },
}

impl ResourceReleaseOutcome {
    pub fn into_result(self) -> Result<(), EngineError> {
        match self {
            Self::Released { error: None } => Ok(()),
            Self::Released { error: Some(error) } | Self::Failed { error } => Err(error),
            Self::Pending => Err(EngineError::new(
                1002,
                EngineErrorCategory::DeadlineExceeded,
                true,
            )),
        }
    }
}

impl From<Result<(), EngineError>> for ResourceReleaseOutcome {
    fn from(result: Result<(), EngineError>) -> Self {
        match result {
            Ok(()) => Self::Released { error: None },
            Err(error) => Self::Failed { error },
        }
    }
}
