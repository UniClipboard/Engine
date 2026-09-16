use rand::RngCore;

use super::ProfileLifecycleError;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ProfileGeneration([u8; 16]);

impl ProfileGeneration {
    pub fn new() -> Self {
        let mut value = [0; 16];
        rand::rng().fill_bytes(&mut value);
        Self(value)
    }

    pub const fn from_bytes(value: [u8; 16]) -> Self {
        Self(value)
    }

    pub const fn into_bytes(self) -> [u8; 16] {
        self.0
    }
}

impl std::fmt::Debug for ProfileGeneration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProfileGeneration([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactoryResetPhase {
    Started,
    KeysWiped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileLifecycleState {
    Ready,
    FactoryReset(FactoryResetPhase),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileLifecycle {
    generation: ProfileGeneration,
    state: ProfileLifecycleState,
}

impl ProfileLifecycle {
    pub const fn new(generation: ProfileGeneration) -> Self {
        Self {
            generation,
            state: ProfileLifecycleState::Ready,
        }
    }

    pub const fn restore(generation: ProfileGeneration, state: ProfileLifecycleState) -> Self {
        Self { generation, state }
    }

    pub const fn generation(&self) -> ProfileGeneration {
        self.generation
    }

    pub const fn state(&self) -> ProfileLifecycleState {
        self.state
    }

    pub fn begin_factory_reset(
        &mut self,
        expected_generation: ProfileGeneration,
    ) -> Result<(), ProfileLifecycleError> {
        self.ensure_generation(expected_generation)?;
        if self.state != ProfileLifecycleState::Ready {
            return Err(ProfileLifecycleError::InvalidTransition);
        }
        self.state = ProfileLifecycleState::FactoryReset(FactoryResetPhase::Started);
        Ok(())
    }

    pub fn mark_keys_wiped(
        &mut self,
        expected_generation: ProfileGeneration,
    ) -> Result<(), ProfileLifecycleError> {
        self.ensure_generation(expected_generation)?;
        if self.state != ProfileLifecycleState::FactoryReset(FactoryResetPhase::Started) {
            return Err(ProfileLifecycleError::InvalidTransition);
        }
        self.state = ProfileLifecycleState::FactoryReset(FactoryResetPhase::KeysWiped);
        Ok(())
    }

    pub fn complete_state_clear(
        &mut self,
        expected_generation: ProfileGeneration,
        new_generation: ProfileGeneration,
    ) -> Result<(), ProfileLifecycleError> {
        self.ensure_generation(expected_generation)?;
        if self.state != ProfileLifecycleState::FactoryReset(FactoryResetPhase::KeysWiped) {
            return Err(ProfileLifecycleError::InvalidTransition);
        }
        if new_generation == expected_generation {
            return Err(ProfileLifecycleError::GenerationConflict);
        }
        self.generation = new_generation;
        self.state = ProfileLifecycleState::Ready;
        Ok(())
    }

    fn ensure_generation(
        &self,
        expected_generation: ProfileGeneration,
    ) -> Result<(), ProfileLifecycleError> {
        if self.generation != expected_generation {
            return Err(ProfileLifecycleError::GenerationConflict);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileFactoryResetRequest {
    Start,
    ResumeIfNeeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileFactoryResetOutcome {
    Completed,
    NotNeeded,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_allows_only_the_forward_factory_reset_sequence() {
        let old_generation = ProfileGeneration::from_bytes([1; 16]);
        let new_generation = ProfileGeneration::from_bytes([2; 16]);
        let mut lifecycle = ProfileLifecycle::new(old_generation);

        lifecycle.begin_factory_reset(old_generation).unwrap();
        assert_eq!(
            lifecycle.state(),
            ProfileLifecycleState::FactoryReset(FactoryResetPhase::Started)
        );

        lifecycle.mark_keys_wiped(old_generation).unwrap();
        assert_eq!(
            lifecycle.state(),
            ProfileLifecycleState::FactoryReset(FactoryResetPhase::KeysWiped)
        );

        lifecycle
            .complete_state_clear(old_generation, new_generation)
            .unwrap();
        assert_eq!(lifecycle.state(), ProfileLifecycleState::Ready);
        assert_eq!(lifecycle.generation(), new_generation);
    }

    #[test]
    fn lifecycle_rejects_skipped_phases_and_stale_generations() {
        let generation = ProfileGeneration::from_bytes([1; 16]);
        let stale = ProfileGeneration::from_bytes([9; 16]);
        let mut lifecycle = ProfileLifecycle::new(generation);

        assert_eq!(
            lifecycle.mark_keys_wiped(generation),
            Err(ProfileLifecycleError::InvalidTransition)
        );
        assert_eq!(
            lifecycle.begin_factory_reset(stale),
            Err(ProfileLifecycleError::GenerationConflict)
        );
    }
}
