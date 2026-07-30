use std::{collections::HashSet, fmt};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::{DeviceId, SpaceId};

use super::revocation::{GroupEpoch, SpaceKeyMaterial, SpaceSecurityMode};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BootstrapId(String);

impl BootstrapId {
    pub fn from_string(value: impl Into<String>) -> Result<Self, BootstrapError> {
        let value = value.into();
        if value.is_empty() || value.len() > 128 || !value.is_ascii() {
            return Err(BootstrapError::InvalidBootstrapId);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LegacyBootstrapStatus {
    Prepared,
    Staged,
    AwaitingReadmission,
    Complete,
    RecoveryRequired,
}

impl LegacyBootstrapStatus {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Complete | Self::RecoveryRequired)
    }
}

/// Persistent intent to upgrade a Legacy space into its first MLS-protected
/// generation. Retained devices are not current MLS members: they must be
/// explicitly admitted again after local activation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawLegacyBootstrapRecord")]
pub struct LegacyBootstrapRecord {
    bootstrap_id: BootstrapId,
    space_id: SpaceId,
    sponsor_device_id: DeviceId,
    pending_readmission: Vec<DeviceId>,
    previous_epoch: GroupEpoch,
    next_epoch: GroupEpoch,
    status: LegacyBootstrapStatus,
    created_at_ms: i64,
    updated_at_ms: i64,
}

#[derive(Deserialize)]
struct RawLegacyBootstrapRecord {
    bootstrap_id: BootstrapId,
    space_id: SpaceId,
    sponsor_device_id: DeviceId,
    #[serde(default)]
    pending_readmission: Vec<DeviceId>,
    previous_epoch: GroupEpoch,
    next_epoch: GroupEpoch,
    status: LegacyBootstrapStatus,
    created_at_ms: i64,
    updated_at_ms: i64,
}

impl LegacyBootstrapRecord {
    pub fn prepare(
        bootstrap_id: BootstrapId,
        space_id: SpaceId,
        sponsor_device_id: DeviceId,
        retained_members: Vec<DeviceId>,
        now_ms: i64,
    ) -> Result<Self, BootstrapError> {
        let mut seen = HashSet::new();
        let pending_readmission = retained_members
            .into_iter()
            .filter(|device_id| device_id != &sponsor_device_id && seen.insert(device_id.clone()))
            .collect();
        Ok(Self {
            bootstrap_id,
            space_id,
            sponsor_device_id,
            pending_readmission,
            previous_epoch: GroupEpoch::new(0),
            next_epoch: GroupEpoch::new(1),
            status: LegacyBootstrapStatus::Prepared,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
    }

    pub fn transition_to(
        &mut self,
        next: LegacyBootstrapStatus,
        now_ms: i64,
    ) -> Result<(), BootstrapError> {
        if self.status == next {
            self.updated_at_ms = now_ms;
            return Ok(());
        }
        if next == LegacyBootstrapStatus::Complete && !self.pending_readmission.is_empty() {
            return Err(BootstrapError::ReadmissionPending);
        }
        let valid = matches!(
            (self.status, next),
            (
                LegacyBootstrapStatus::Prepared,
                LegacyBootstrapStatus::Staged
            ) | (
                LegacyBootstrapStatus::Staged,
                LegacyBootstrapStatus::AwaitingReadmission
            ) | (
                LegacyBootstrapStatus::Staged,
                LegacyBootstrapStatus::Complete
            ) | (
                LegacyBootstrapStatus::AwaitingReadmission,
                LegacyBootstrapStatus::Complete
            ) | (
                LegacyBootstrapStatus::Prepared
                    | LegacyBootstrapStatus::Staged
                    | LegacyBootstrapStatus::AwaitingReadmission,
                LegacyBootstrapStatus::RecoveryRequired
            )
        );
        if !valid {
            return Err(BootstrapError::InvalidTransition {
                from: self.status,
                to: next,
            });
        }
        self.status = next;
        self.updated_at_ms = now_ms;
        Ok(())
    }

    pub fn mark_readmitted(
        &mut self,
        device_id: &DeviceId,
        now_ms: i64,
    ) -> Result<bool, BootstrapError> {
        if self.status != LegacyBootstrapStatus::AwaitingReadmission {
            return Err(BootstrapError::ReadmissionNotExpected);
        }
        let before = self.pending_readmission.len();
        self.pending_readmission
            .retain(|member| member != device_id);
        if self.pending_readmission.len() == before {
            return Ok(false);
        }
        self.updated_at_ms = now_ms;
        if self.pending_readmission.is_empty() {
            self.transition_to(LegacyBootstrapStatus::Complete, now_ms)?;
        }
        Ok(true)
    }

    pub fn bootstrap_id(&self) -> &BootstrapId {
        &self.bootstrap_id
    }

    pub fn space_id(&self) -> &SpaceId {
        &self.space_id
    }

    pub fn sponsor_device_id(&self) -> &DeviceId {
        &self.sponsor_device_id
    }

    pub fn pending_readmission(&self) -> &[DeviceId] {
        &self.pending_readmission
    }

    pub const fn previous_epoch(&self) -> GroupEpoch {
        self.previous_epoch
    }

    pub const fn next_epoch(&self) -> GroupEpoch {
        self.next_epoch
    }

    pub const fn status(&self) -> LegacyBootstrapStatus {
        self.status
    }

    pub const fn created_at_ms(&self) -> i64 {
        self.created_at_ms
    }

    pub const fn updated_at_ms(&self) -> i64 {
        self.updated_at_ms
    }
}

impl fmt::Debug for LegacyBootstrapRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyBootstrapRecord")
            .field("bootstrap_id", &self.bootstrap_id)
            .field("previous_epoch", &self.previous_epoch)
            .field("next_epoch", &self.next_epoch)
            .field("status", &self.status)
            .field("pending_readmission_count", &self.pending_readmission.len())
            .field("created_at_ms", &self.created_at_ms)
            .field("updated_at_ms", &self.updated_at_ms)
            .finish()
    }
}

impl TryFrom<RawLegacyBootstrapRecord> for LegacyBootstrapRecord {
    type Error = BootstrapError;

    fn try_from(raw: RawLegacyBootstrapRecord) -> Result<Self, Self::Error> {
        if raw.previous_epoch != GroupEpoch::new(0) || raw.next_epoch != GroupEpoch::new(1) {
            return Err(BootstrapError::InvalidRecord);
        }
        let mut record = Self::prepare(
            raw.bootstrap_id,
            raw.space_id,
            raw.sponsor_device_id,
            raw.pending_readmission,
            raw.created_at_ms,
        )?;
        match raw.status {
            LegacyBootstrapStatus::Prepared => {}
            LegacyBootstrapStatus::Staged => {
                record.transition_to(LegacyBootstrapStatus::Staged, raw.updated_at_ms)?;
            }
            LegacyBootstrapStatus::AwaitingReadmission => {
                record.transition_to(LegacyBootstrapStatus::Staged, raw.updated_at_ms)?;
                record.transition_to(
                    LegacyBootstrapStatus::AwaitingReadmission,
                    raw.updated_at_ms,
                )?;
            }
            LegacyBootstrapStatus::Complete => {
                record.transition_to(LegacyBootstrapStatus::Staged, raw.updated_at_ms)?;
                record.transition_to(LegacyBootstrapStatus::Complete, raw.updated_at_ms)?;
            }
            LegacyBootstrapStatus::RecoveryRequired => {
                record.transition_to(LegacyBootstrapStatus::RecoveryRequired, raw.updated_at_ms)?;
            }
        }
        Ok(record)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyBootstrapStage {
    record: LegacyBootstrapRecord,
    material: SpaceKeyMaterial,
}

impl LegacyBootstrapStage {
    pub fn new(
        record: LegacyBootstrapRecord,
        material: SpaceKeyMaterial,
    ) -> Result<Self, BootstrapError> {
        if record.status() != LegacyBootstrapStatus::Staged
            || material.state().space_id() != record.space_id()
            || material.state().mode() != SpaceSecurityMode::Ready
            || material.state().epoch() != record.next_epoch()
            || material.group_state().is_empty()
        {
            return Err(BootstrapError::InvalidStage);
        }
        Ok(Self { record, material })
    }

    pub fn record(&self) -> &LegacyBootstrapRecord {
        &self.record
    }

    pub fn material(&self) -> &SpaceKeyMaterial {
        &self.material
    }
}

impl fmt::Debug for LegacyBootstrapStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyBootstrapStage")
            .field("bootstrap_id", self.record.bootstrap_id())
            .field("status", &self.record.status())
            .field("epoch", &self.material.state().epoch())
            .field("group_state_len", &self.material.group_state().len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupBootstrapResult {
    AwaitingReadmission {
        bootstrap_id: BootstrapId,
        pending_members: usize,
    },
    Complete {
        bootstrap_id: BootstrapId,
    },
    RecoveryRequired {
        bootstrap_id: BootstrapId,
    },
}

#[async_trait]
pub trait LegacyBootstrapRepositoryPort: Send + Sync {
    async fn begin_legacy_bootstrap(
        &self,
        prepared: &LegacyBootstrapRecord,
    ) -> Result<LegacyBootstrapRecord, BootstrapError>;

    async fn stage_legacy_bootstrap(
        &self,
        stage: &LegacyBootstrapStage,
    ) -> Result<(), BootstrapError>;

    /// Atomically persists the first real MLS material and its activated
    /// bootstrap state. Activation cannot be rolled back to Legacy.
    async fn activate_legacy_bootstrap(
        &self,
        bootstrap_id: &BootstrapId,
        now_ms: i64,
    ) -> Result<LegacyBootstrapRecord, BootstrapError>;

    async fn load_legacy_bootstrap_stage(
        &self,
        bootstrap_id: &BootstrapId,
    ) -> Result<Option<LegacyBootstrapStage>, BootstrapError>;

    async fn get_legacy_bootstrap(
        &self,
        bootstrap_id: &BootstrapId,
    ) -> Result<Option<LegacyBootstrapRecord>, BootstrapError>;

    async fn list_incomplete_legacy_bootstraps(
        &self,
    ) -> Result<Vec<LegacyBootstrapRecord>, BootstrapError>;

    async fn list_legacy_bootstraps(&self) -> Result<Vec<LegacyBootstrapRecord>, BootstrapError>;

    async fn acknowledge_legacy_readmission(
        &self,
        bootstrap_id: &BootstrapId,
        member: &DeviceId,
        now_ms: i64,
    ) -> Result<LegacyBootstrapRecord, BootstrapError>;
}

#[async_trait]
pub trait GroupBootstrapPort: Send + Sync {
    async fn bootstrap_legacy_space(
        &self,
        sponsor: &DeviceId,
        retained_members: &[DeviceId],
        now_ms: i64,
    ) -> Result<GroupBootstrapResult, BootstrapError>;

    async fn acknowledge_legacy_readmission(
        &self,
        bootstrap_id: &BootstrapId,
        member: &DeviceId,
        now_ms: i64,
    ) -> Result<GroupBootstrapResult, BootstrapError>;

    async fn query_legacy_bootstrap(
        &self,
        bootstrap_id: &BootstrapId,
    ) -> Result<Option<GroupBootstrapResult>, BootstrapError>;

    async fn resume_legacy_bootstraps(
        &self,
        now_ms: i64,
    ) -> Result<Vec<GroupBootstrapResult>, BootstrapError>;
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum BootstrapError {
    #[error("invalid legacy bootstrap id")]
    InvalidBootstrapId,

    #[error("invalid persisted legacy bootstrap record")]
    InvalidRecord,

    #[error("invalid staged legacy bootstrap material")]
    InvalidStage,

    #[error("invalid legacy bootstrap transition from {from:?} to {to:?}")]
    InvalidTransition {
        from: LegacyBootstrapStatus,
        to: LegacyBootstrapStatus,
    },

    #[error("member is not awaiting legacy readmission")]
    ReadmissionNotExpected,

    #[error("legacy bootstrap still has members awaiting readmission")]
    ReadmissionPending,

    #[error("legacy bootstrap could not create cryptographic material")]
    CryptographicState,

    #[error("legacy bootstrap could not install activated session material")]
    SessionMaterial,

    #[error("legacy bootstrap repository failure: {0}")]
    Repository(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::membership::{ContentKeyId, SpaceKeyState};

    #[test]
    fn stage_rejects_ready_material_without_group_state() {
        let mut record = LegacyBootstrapRecord::prepare(
            BootstrapId::generate(),
            SpaceId::from("space-a"),
            DeviceId::new("sponsor"),
            Vec::new(),
            1,
        )
        .unwrap();
        record
            .transition_to(LegacyBootstrapStatus::Staged, 2)
            .unwrap();
        let mut state = SpaceKeyState::legacy(SpaceId::from("space-a"));
        state.mark_migrating().unwrap();
        state.mark_ready(ContentKeyId::generate()).unwrap();

        assert_eq!(
            LegacyBootstrapStage::new(record, SpaceKeyMaterial::new(state, Vec::new(), vec![1], 2))
                .unwrap_err(),
            BootstrapError::InvalidStage
        );
    }

    #[test]
    fn bootstrap_cannot_complete_while_readmission_is_pending() {
        let mut record = LegacyBootstrapRecord::prepare(
            BootstrapId::generate(),
            SpaceId::from("space-a"),
            DeviceId::new("sponsor"),
            vec![DeviceId::new("retained")],
            1,
        )
        .unwrap();
        record
            .transition_to(LegacyBootstrapStatus::Staged, 2)
            .unwrap();

        assert_eq!(
            record.transition_to(LegacyBootstrapStatus::Complete, 3),
            Err(BootstrapError::ReadmissionPending)
        );
    }

    #[test]
    fn activation_waits_for_each_retained_member_to_be_readmitted() {
        let mut record = LegacyBootstrapRecord::prepare(
            BootstrapId::generate(),
            SpaceId::from("space-a"),
            DeviceId::new("sponsor"),
            vec![DeviceId::new("retained"), DeviceId::new("sponsor")],
            1,
        )
        .unwrap();
        record
            .transition_to(LegacyBootstrapStatus::Staged, 2)
            .unwrap();
        record
            .transition_to(LegacyBootstrapStatus::AwaitingReadmission, 3)
            .unwrap();

        assert!(record
            .mark_readmitted(&DeviceId::new("retained"), 4)
            .unwrap());
        assert_eq!(record.status(), LegacyBootstrapStatus::Complete);
        assert!(record.pending_readmission().is_empty());
    }
}
