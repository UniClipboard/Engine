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

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SpaceProtectionError {
    #[error("space security state is unavailable")]
    Unavailable,

    #[error("space security state is corrupted")]
    Corrupted,

    #[error("failed to query space security state: {0}")]
    Repository(String),
}
