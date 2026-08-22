use std::sync::Arc;

use super::{
    ClearProfileStatePort, FactoryResetPhase, ProfileFactoryResetError, ProfileFactoryResetOutcome,
    ProfileFactoryResetRequest, ProfileGeneration, ProfileLifecycleRepositoryPort,
    ProfileLifecycleState, StopProfileRuntimePort, WipeProfileKeysPort,
};

pub struct ProfileFactoryResetUseCase {
    lifecycle_repository: Arc<dyn ProfileLifecycleRepositoryPort>,
    runtime: Arc<dyn StopProfileRuntimePort>,
    keys: Arc<dyn WipeProfileKeysPort>,
    state: Arc<dyn ClearProfileStatePort>,
    operation_lock: tokio::sync::Mutex<()>,
}

impl ProfileFactoryResetUseCase {
    pub fn new(
        lifecycle_repository: Arc<dyn ProfileLifecycleRepositoryPort>,
        runtime: Arc<dyn StopProfileRuntimePort>,
        keys: Arc<dyn WipeProfileKeysPort>,
        state: Arc<dyn ClearProfileStatePort>,
    ) -> Self {
        Self {
            lifecycle_repository,
            runtime,
            keys,
            state,
            operation_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub async fn execute(
        &self,
        request: ProfileFactoryResetRequest,
    ) -> Result<ProfileFactoryResetOutcome, ProfileFactoryResetError> {
        let _guard = self.operation_lock.lock().await;
        let mut lifecycle = self
            .lifecycle_repository
            .load()?
            .ok_or(ProfileFactoryResetError::LifecycleMissing)?;

        if request == ProfileFactoryResetRequest::ResumeIfNeeded
            && lifecycle.state() == ProfileLifecycleState::Ready
        {
            return Ok(ProfileFactoryResetOutcome::NotNeeded);
        }

        self.runtime
            .stop_profile_runtime()
            .await
            .map_err(|_| ProfileFactoryResetError::StopRuntime)?;

        if lifecycle.state() == ProfileLifecycleState::Ready {
            let previous = lifecycle.clone();
            let generation = lifecycle.generation();
            lifecycle.begin_factory_reset(generation)?;
            self.lifecycle_repository
                .compare_and_swap(Some(&previous), &lifecycle)?;
        }

        let generation = lifecycle.generation();
        if lifecycle.state() == ProfileLifecycleState::FactoryReset(FactoryResetPhase::Started) {
            self.keys
                .wipe_and_verify_profile_keys(generation)
                .await
                .map_err(|_| ProfileFactoryResetError::WipeKeys)?;

            let previous = lifecycle.clone();
            lifecycle.mark_keys_wiped(generation)?;
            self.lifecycle_repository
                .compare_and_swap(Some(&previous), &lifecycle)?;
        }

        if lifecycle.state() == ProfileLifecycleState::FactoryReset(FactoryResetPhase::KeysWiped) {
            self.state
                .clear_and_verify_profile_state()
                .await
                .map_err(|_| ProfileFactoryResetError::ClearState)?;

            let previous = lifecycle.clone();
            lifecycle.complete_state_clear(generation, next_generation(generation))?;
            self.lifecycle_repository
                .compare_and_swap(Some(&previous), &lifecycle)?;
        }

        Ok(ProfileFactoryResetOutcome::Completed)
    }
}

fn next_generation(previous: ProfileGeneration) -> ProfileGeneration {
    loop {
        let candidate = ProfileGeneration::new();
        if candidate != previous {
            return candidate;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;
    use crate::profile::factory_reset::{
        ProfileFactoryResetCapabilityError, ProfileLifecycle, ProfileLifecycleRepositoryError,
    };

    struct LifecycleRepository {
        lifecycle: Mutex<Option<ProfileLifecycle>>,
    }

    impl LifecycleRepository {
        fn ready(generation: ProfileGeneration) -> Self {
            Self {
                lifecycle: Mutex::new(Some(ProfileLifecycle::new(generation))),
            }
        }

        fn current(&self) -> ProfileLifecycle {
            self.lifecycle.lock().unwrap().clone().unwrap()
        }
    }

    impl ProfileLifecycleRepositoryPort for LifecycleRepository {
        fn load(&self) -> Result<Option<ProfileLifecycle>, ProfileLifecycleRepositoryError> {
            Ok(self.lifecycle.lock().unwrap().clone())
        }

        fn compare_and_swap(
            &self,
            expected: Option<&ProfileLifecycle>,
            next: &ProfileLifecycle,
        ) -> Result<(), ProfileLifecycleRepositoryError> {
            let mut current = self.lifecycle.lock().unwrap();
            if current.as_ref() != expected {
                return Err(ProfileLifecycleRepositoryError::Conflict);
            }
            *current = Some(next.clone());
            Ok(())
        }
    }

    struct Capability {
        name: &'static str,
        calls: Arc<Mutex<Vec<&'static str>>>,
        failures_remaining: AtomicUsize,
    }

    impl Capability {
        fn new(name: &'static str, calls: Arc<Mutex<Vec<&'static str>>>, failures: usize) -> Self {
            Self {
                name,
                calls,
                failures_remaining: AtomicUsize::new(failures),
            }
        }

        fn invoke(&self) -> Result<(), ProfileFactoryResetCapabilityError> {
            self.calls.lock().unwrap().push(self.name);
            if self
                .failures_remaining
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                Err(ProfileFactoryResetCapabilityError)
            } else {
                Ok(())
            }
        }
    }

    #[async_trait]
    impl StopProfileRuntimePort for Capability {
        async fn stop_profile_runtime(&self) -> Result<(), ProfileFactoryResetCapabilityError> {
            self.invoke()
        }
    }

    #[async_trait]
    impl WipeProfileKeysPort for Capability {
        async fn wipe_and_verify_profile_keys(
            &self,
            _profile_generation: ProfileGeneration,
        ) -> Result<(), ProfileFactoryResetCapabilityError> {
            self.invoke()
        }
    }

    #[async_trait]
    impl ClearProfileStatePort for Capability {
        async fn clear_and_verify_profile_state(
            &self,
        ) -> Result<(), ProfileFactoryResetCapabilityError> {
            self.invoke()
        }
    }

    #[tokio::test]
    async fn factory_reset_runs_in_key_first_order_without_an_active_space() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let generation = ProfileGeneration::from_bytes([1; 16]);
        let lifecycle = Arc::new(LifecycleRepository::ready(generation));
        let reset = ProfileFactoryResetUseCase::new(
            lifecycle.clone(),
            Arc::new(Capability::new("stop", Arc::clone(&calls), 0)),
            Arc::new(Capability::new("wipe", Arc::clone(&calls), 0)),
            Arc::new(Capability::new("clear", Arc::clone(&calls), 0)),
        );

        let outcome = reset
            .execute(ProfileFactoryResetRequest::Start)
            .await
            .unwrap();

        assert_eq!(outcome, ProfileFactoryResetOutcome::Completed);
        assert_eq!(lifecycle.current().state(), ProfileLifecycleState::Ready);
        assert_ne!(lifecycle.current().generation(), generation);
        assert_eq!(*calls.lock().unwrap(), ["stop", "wipe", "clear"]);
    }

    #[tokio::test]
    async fn restart_resumes_each_persisted_phase_without_going_backwards() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let generation = ProfileGeneration::from_bytes([1; 16]);
        let lifecycle = Arc::new(LifecycleRepository::ready(generation));
        let reset = ProfileFactoryResetUseCase::new(
            lifecycle.clone(),
            Arc::new(Capability::new("stop", Arc::clone(&calls), 0)),
            Arc::new(Capability::new("wipe", Arc::clone(&calls), 1)),
            Arc::new(Capability::new("clear", Arc::clone(&calls), 1)),
        );

        assert!(matches!(
            reset.execute(ProfileFactoryResetRequest::Start).await,
            Err(ProfileFactoryResetError::WipeKeys)
        ));
        assert_eq!(
            lifecycle.current().state(),
            ProfileLifecycleState::FactoryReset(FactoryResetPhase::Started)
        );

        assert!(matches!(
            reset
                .execute(ProfileFactoryResetRequest::ResumeIfNeeded)
                .await,
            Err(ProfileFactoryResetError::ClearState)
        ));
        assert_eq!(
            lifecycle.current().state(),
            ProfileLifecycleState::FactoryReset(FactoryResetPhase::KeysWiped)
        );

        assert_eq!(
            reset
                .execute(ProfileFactoryResetRequest::ResumeIfNeeded)
                .await
                .unwrap(),
            ProfileFactoryResetOutcome::Completed
        );
        assert_eq!(lifecycle.current().state(), ProfileLifecycleState::Ready);
        assert_eq!(
            *calls.lock().unwrap(),
            ["stop", "wipe", "stop", "wipe", "clear", "stop", "clear"]
        );
        assert_eq!(
            reset
                .execute(ProfileFactoryResetRequest::ResumeIfNeeded)
                .await
                .unwrap(),
            ProfileFactoryResetOutcome::NotNeeded
        );
    }
}
