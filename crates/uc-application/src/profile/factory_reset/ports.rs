use async_trait::async_trait;

use super::{
    ProfileFactoryResetCapabilityError, ProfileGeneration, ProfileLifecycle,
    ProfileLifecycleRepositoryError,
};

pub trait ProfileLifecycleRepositoryPort: Send + Sync {
    fn load(&self) -> Result<Option<ProfileLifecycle>, ProfileLifecycleRepositoryError>;

    fn compare_and_swap(
        &self,
        expected: Option<&ProfileLifecycle>,
        next: &ProfileLifecycle,
    ) -> Result<(), ProfileLifecycleRepositoryError>;
}

#[async_trait]
pub trait StopProfileRuntimePort: Send + Sync {
    async fn stop_profile_runtime(&self) -> Result<(), ProfileFactoryResetCapabilityError>;
}

#[async_trait]
pub trait WipeProfileKeysPort: Send + Sync {
    async fn wipe_and_verify_profile_keys(
        &self,
        profile_generation: ProfileGeneration,
    ) -> Result<(), ProfileFactoryResetCapabilityError>;
}

#[async_trait]
pub trait ClearProfileStatePort: Send + Sync {
    async fn clear_and_verify_profile_state(
        &self,
    ) -> Result<(), ProfileFactoryResetCapabilityError>;
}
