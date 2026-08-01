use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::DeviceId;
use crate::space_access::GroupAdmission;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LegacyUpgradeId([u8; 32]);

impl LegacyUpgradeId {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for LegacyUpgradeId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LegacyUpgradeId([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProtectionGroupId(String);

impl ProtectionGroupId {
    pub fn from_string(value: impl Into<String>) -> Result<Self, LegacyUpgradeError> {
        let value = value.into();
        if value.is_empty() || value.len() > 128 || !value.is_ascii() {
            return Err(LegacyUpgradeError::InvalidProtectionGroupId);
        }
        Ok(Self(value))
    }

    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AdmissionReplayId([u8; 32]);

impl AdmissionReplayId {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectionGroupAdmission {
    pub protection_group_id: ProtectionGroupId,
    pub admission: GroupAdmission,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyProtectionSnapshot {
    pub descriptor: LegacyUpgradeDescriptor,
    pub protected_members: Vec<DeviceId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyRequestInspection {
    Invalid,
    Verified,
    Replay(ProtectionGroupAdmission),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyProtectionCommand {
    CreateGroup {
        sponsor: DeviceId,
        retained_members: Vec<DeviceId>,
    },
    AdmitMember {
        sponsor: DeviceId,
        existing_members: Vec<DeviceId>,
        request: LegacyUpgradeRequest,
    },
    JoinGroup {
        peer: DeviceId,
        admission: ProtectionGroupAdmission,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyProtectionResult {
    GroupReady(LegacyUpgradeDescriptor),
    MemberAdmitted(ProtectionGroupAdmission),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyUpgradeDescriptor {
    upgrade_id: LegacyUpgradeId,
    protection_group_id: Option<ProtectionGroupId>,
}

impl LegacyUpgradeDescriptor {
    pub const fn legacy(upgrade_id: LegacyUpgradeId) -> Self {
        Self {
            upgrade_id,
            protection_group_id: None,
        }
    }

    pub const fn ready(
        upgrade_id: LegacyUpgradeId,
        protection_group_id: ProtectionGroupId,
    ) -> Self {
        Self {
            upgrade_id,
            protection_group_id: Some(protection_group_id),
        }
    }

    pub const fn upgrade_id(&self) -> LegacyUpgradeId {
        self.upgrade_id
    }

    pub fn protection_group_id(&self) -> Option<&ProtectionGroupId> {
        self.protection_group_id.as_ref()
    }

    pub const fn is_ready(&self) -> bool {
        self.protection_group_id.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyUpgradeAction {
    CreateLocalGroup,
    AwaitRemote,
    AdmitRemote,
    JoinRemote,
    NoAction,
    Reject,
}

#[derive(Clone, PartialEq, Eq)]
pub struct LegacyUpgradeRequest {
    source_device_id: DeviceId,
    target_device_id: DeviceId,
    descriptor: LegacyUpgradeDescriptor,
    key_package: Vec<u8>,
    proof: Vec<u8>,
}

impl LegacyUpgradeRequest {
    pub fn unsigned(
        source_device_id: DeviceId,
        target_device_id: DeviceId,
        descriptor: LegacyUpgradeDescriptor,
        key_package: Vec<u8>,
    ) -> Self {
        Self {
            source_device_id,
            target_device_id,
            descriptor,
            key_package,
            proof: Vec::new(),
        }
    }

    pub fn with_proof(mut self, proof: Vec<u8>) -> Self {
        self.proof = proof;
        self
    }

    pub const fn source_device_id(&self) -> &DeviceId {
        &self.source_device_id
    }

    pub const fn target_device_id(&self) -> &DeviceId {
        &self.target_device_id
    }

    pub const fn descriptor(&self) -> &LegacyUpgradeDescriptor {
        &self.descriptor
    }

    pub fn key_package(&self) -> &[u8] {
        &self.key_package
    }

    pub fn proof(&self) -> &[u8] {
        &self.proof
    }
}

impl std::fmt::Debug for LegacyUpgradeRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LegacyUpgradeRequest")
            .field("source_device_id", &self.source_device_id)
            .field("target_device_id", &self.target_device_id)
            .field("descriptor", &self.descriptor)
            .field("key_package_len", &self.key_package.len())
            .field("proof", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyUpgradeResponseKind {
    UpToDate,
    Retry,
    Admission(ProtectionGroupAdmission),
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyUpgradeResponse {
    pub descriptor: LegacyUpgradeDescriptor,
    pub kind: LegacyUpgradeResponseKind,
}

#[async_trait]
pub trait LegacyProtectionPort: Send + Sync {
    async fn snapshot(
        &self,
        member_ids: &[DeviceId],
    ) -> Result<LegacyProtectionSnapshot, LegacyUpgradeError>;

    async fn begin_attempt(
        &self,
        source_device_id: &DeviceId,
        target_device_id: &DeviceId,
    ) -> Result<LegacyUpgradeRequest, LegacyUpgradeError>;

    async fn inspect_request(
        &self,
        request: &LegacyUpgradeRequest,
    ) -> Result<LegacyRequestInspection, LegacyUpgradeError>;

    async fn execute(
        &self,
        command: LegacyProtectionCommand,
    ) -> Result<LegacyProtectionResult, LegacyUpgradeError>;
}

#[async_trait]
pub trait LegacyUpgradeEndpointPort: Send + Sync {
    async fn handle_legacy_upgrade_request(
        &self,
        authenticated_peer: &DeviceId,
        request: LegacyUpgradeRequest,
    ) -> Result<LegacyUpgradeResponse, LegacyUpgradeError>;
}

#[async_trait]
pub trait LegacyUpgradeDispatchPort: Send + Sync {
    async fn exchange_legacy_upgrade(
        &self,
        peer: &DeviceId,
        request: &LegacyUpgradeRequest,
    ) -> Result<LegacyUpgradeResponse, LegacyUpgradeDispatchError>;
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum LegacyUpgradeDispatchError {
    #[error("legacy upgrade peer is offline")]
    Offline,

    #[error("legacy upgrade was rejected")]
    Rejected,

    #[error("legacy upgrade transport failed")]
    Transport,
}

pub fn decide_legacy_upgrade(
    local_device_id: &DeviceId,
    local: &LegacyUpgradeDescriptor,
    remote_device_id: &DeviceId,
    remote: &LegacyUpgradeDescriptor,
) -> LegacyUpgradeAction {
    if local.upgrade_id != remote.upgrade_id {
        return LegacyUpgradeAction::Reject;
    }

    match (
        local.protection_group_id.as_ref(),
        remote.protection_group_id.as_ref(),
    ) {
        (None, None) => {
            if local_device_id.as_str() < remote_device_id.as_str() {
                LegacyUpgradeAction::CreateLocalGroup
            } else {
                LegacyUpgradeAction::AwaitRemote
            }
        }
        (Some(_), None) => LegacyUpgradeAction::AdmitRemote,
        (None, Some(_)) => LegacyUpgradeAction::JoinRemote,
        (Some(local_group), Some(remote_group)) if local_group == remote_group => {
            LegacyUpgradeAction::NoAction
        }
        (Some(local_group), Some(remote_group)) if local_group < remote_group => {
            LegacyUpgradeAction::AdmitRemote
        }
        (Some(_), Some(_)) => LegacyUpgradeAction::JoinRemote,
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LegacyUpgradeError {
    #[error("invalid protection group id")]
    InvalidProtectionGroupId,

    #[error("legacy upgrade state is unavailable")]
    Unavailable,

    #[error("legacy upgrade request is invalid")]
    InvalidRequest,

    #[error("legacy upgrade request is not authorized")]
    Unauthorized,

    #[error("legacy upgrade failed: {0}")]
    Internal(String),
}
