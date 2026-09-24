use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::DeviceId;

use super::SpaceSecurityMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpaceProtectionMode {
    Legacy,
    Migrating,
    Ready,
}

impl From<SpaceSecurityMode> for SpaceProtectionMode {
    fn from(value: SpaceSecurityMode) -> Self {
        match value {
            SpaceSecurityMode::Legacy => Self::Legacy,
            SpaceSecurityMode::Migrating => Self::Migrating,
            SpaceSecurityMode::Ready => Self::Ready,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemberProtectionStatus {
    LegacyUnprotected,
    Protected,
    AwaitingReadmission,
    RequiresReadmission,
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberProtection {
    pub device_id: DeviceId,
    pub status: MemberProtectionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceProtectionSnapshot {
    pub mode: SpaceProtectionMode,
    pub members: Vec<MemberProtection>,
}

#[async_trait]
pub trait SpaceProtectionStatusPort: Send + Sync {
    async fn query_space_protection(
        &self,
        members: &[DeviceId],
    ) -> Result<SpaceProtectionSnapshot, SpaceProtectionError>;
}

#[derive(Debug, Error)]
pub enum SpaceProtectionError {
    #[error("space security state is unavailable")]
    Unavailable {
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("space security state is corrupted")]
    Corrupted {
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("failed to query space security state")]
    Repository(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl SpaceProtectionError {
    pub fn corrupted() -> Self {
        Self::Corrupted { source: None }
    }

    pub fn corrupted_from(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Corrupted {
            source: Some(Box::new(source)),
        }
    }

    pub fn unavailable() -> Self {
        Self::Unavailable { source: None }
    }

    pub fn unavailable_from(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Unavailable {
            source: Some(Box::new(source)),
        }
    }
}
