use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub const PROFILE_LIFECYCLE_MARKER_FORMAT_V1: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactoryResetPhaseV1 {
    None,
    WipingKeys,
    ClearingState,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileLifecycleMarkerV1 {
    pub marker_format_version: u16,
    pub profile_generation: [u8; 16],
    pub factory_reset_phase: FactoryResetPhaseV1,
}

impl std::fmt::Debug for ProfileLifecycleMarkerV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProfileLifecycleMarkerV1")
            .field("marker_format_version", &self.marker_format_version)
            .field("profile_generation", &"[REDACTED]")
            .field("factory_reset_phase", &self.factory_reset_phase)
            .finish()
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ProfileLifecycleError {
    #[error("profile lifecycle storage is unavailable")]
    SecureStorage,
    #[error("profile lifecycle marker is corrupt")]
    Corrupt,
    #[error("profile lifecycle phase conflicts with persisted state")]
    PhaseConflict,
}

pub trait ProfileLifecyclePort: Send + Sync {
    fn load_or_initialize(&self) -> Result<ProfileLifecycleMarkerV1, ProfileLifecycleError>;

    fn begin_factory_reset(
        &self,
        expected_generation: [u8; 16],
    ) -> Result<ProfileLifecycleMarkerV1, ProfileLifecycleError>;

    fn mark_keys_wiped(
        &self,
        expected_generation: [u8; 16],
    ) -> Result<ProfileLifecycleMarkerV1, ProfileLifecycleError>;

    fn complete_state_clear(
        &self,
        expected_generation: [u8; 16],
    ) -> Result<ProfileLifecycleMarkerV1, ProfileLifecycleError>;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("profile factory reset capability failed")]
pub struct ProfileFactoryResetCapabilityError;

#[async_trait]
pub trait StopProfileRuntimePort: Send + Sync {
    async fn stop_profile_runtime(&self) -> Result<(), ProfileFactoryResetCapabilityError>;
}

#[async_trait]
pub trait WipeProfileKeysPort: Send + Sync {
    async fn wipe_and_verify_profile_keys(
        &self,
        profile_generation: [u8; 16],
    ) -> Result<(), ProfileFactoryResetCapabilityError>;
}

#[async_trait]
pub trait ClearProfileStatePort: Send + Sync {
    async fn clear_and_verify_profile_state(
        &self,
        profile_generation: [u8; 16],
    ) -> Result<(), ProfileFactoryResetCapabilityError>;
}
