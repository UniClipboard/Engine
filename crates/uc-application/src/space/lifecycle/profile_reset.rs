use std::sync::Arc;

use uc_core::ports::{
    ClearProfileStatePort, FactoryResetPhaseV1, ProfileLifecycleError, ProfileLifecycleMarkerV1,
    ProfileLifecyclePort, StopProfileRuntimePort, WipeProfileKeysPort,
};

#[derive(Debug, thiserror::Error)]
pub enum ProfileFactoryResetError {
    #[error("profile lifecycle state could not be persisted")]
    Lifecycle(#[from] ProfileLifecycleError),
    #[error("profile runtime could not be stopped")]
    StopRuntime,
    #[error("profile keys could not be wiped")]
    WipeKeys,
    #[error("profile state could not be cleared")]
    ClearState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileFactoryResetResult {
    pub previous_generation: [u8; 16],
    pub new_generation: [u8; 16],
}

/// Owns the forward-only profile destruction sequence. Callers invoke one
/// operation and never coordinate lifecycle phases themselves.
pub struct ProfileFactoryReset {
    lifecycle: Arc<dyn ProfileLifecyclePort>,
    runtime: Arc<dyn StopProfileRuntimePort>,
    keys: Arc<dyn WipeProfileKeysPort>,
    state: Arc<dyn ClearProfileStatePort>,
    operation_lock: tokio::sync::Mutex<()>,
}

impl ProfileFactoryReset {
    pub fn new(
        lifecycle: Arc<dyn ProfileLifecyclePort>,
        runtime: Arc<dyn StopProfileRuntimePort>,
        keys: Arc<dyn WipeProfileKeysPort>,
        state: Arc<dyn ClearProfileStatePort>,
    ) -> Self {
        Self {
            lifecycle,
            runtime,
            keys,
            state,
            operation_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub async fn factory_reset(
        &self,
    ) -> Result<ProfileFactoryResetResult, ProfileFactoryResetError> {
        let _guard = self.operation_lock.lock().await;
        let marker = self.lifecycle.load_or_initialize()?;
        self.advance(marker, true)
            .await?
            .ok_or_else(|| ProfileLifecycleError::PhaseConflict.into())
    }

    pub async fn recover_if_needed(
        &self,
    ) -> Result<Option<ProfileFactoryResetResult>, ProfileFactoryResetError> {
        let _guard = self.operation_lock.lock().await;
        let marker = self.lifecycle.load_or_initialize()?;
        self.advance(marker, false).await
    }

    async fn advance(
        &self,
        mut marker: ProfileLifecycleMarkerV1,
        begin_if_idle: bool,
    ) -> Result<Option<ProfileFactoryResetResult>, ProfileFactoryResetError> {
        if marker.factory_reset_phase == FactoryResetPhaseV1::None && !begin_if_idle {
            return Ok(None);
        }
        let previous_generation = marker.profile_generation;
        self.runtime
            .stop_profile_runtime()
            .await
            .map_err(|_| ProfileFactoryResetError::StopRuntime)?;

        if marker.factory_reset_phase == FactoryResetPhaseV1::None {
            marker = self.lifecycle.begin_factory_reset(previous_generation)?;
        }
        if marker.factory_reset_phase == FactoryResetPhaseV1::WipingKeys {
            self.keys
                .wipe_and_verify_profile_keys(previous_generation)
                .await
                .map_err(|_| ProfileFactoryResetError::WipeKeys)?;
            marker = self.lifecycle.mark_keys_wiped(previous_generation)?;
        }
        if marker.factory_reset_phase == FactoryResetPhaseV1::ClearingState {
            self.state
                .clear_and_verify_profile_state(previous_generation)
                .await
                .map_err(|_| ProfileFactoryResetError::ClearState)?;
            marker = self.lifecycle.complete_state_clear(previous_generation)?;
        }
        if marker.factory_reset_phase != FactoryResetPhaseV1::None
            || marker.profile_generation == previous_generation
        {
            return Err(ProfileLifecycleError::PhaseConflict.into());
        }
        Ok(Some(ProfileFactoryResetResult {
            previous_generation,
            new_generation: marker.profile_generation,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use async_trait::async_trait;
    use uc_core::ports::{ProfileFactoryResetCapabilityError, PROFILE_LIFECYCLE_MARKER_FORMAT_V1};

    use super::*;

    struct Lifecycle {
        marker: Mutex<ProfileLifecycleMarkerV1>,
    }

    impl Lifecycle {
        fn new() -> Self {
            Self {
                marker: Mutex::new(ProfileLifecycleMarkerV1 {
                    marker_format_version: PROFILE_LIFECYCLE_MARKER_FORMAT_V1,
                    profile_generation: [1; 16],
                    factory_reset_phase: FactoryResetPhaseV1::None,
                }),
            }
        }

        fn phase(&self) -> FactoryResetPhaseV1 {
            self.marker.lock().unwrap().factory_reset_phase
        }
    }

    impl ProfileLifecyclePort for Lifecycle {
        fn load_or_initialize(&self) -> Result<ProfileLifecycleMarkerV1, ProfileLifecycleError> {
            Ok(*self.marker.lock().unwrap())
        }

        fn begin_factory_reset(
            &self,
            expected_generation: [u8; 16],
        ) -> Result<ProfileLifecycleMarkerV1, ProfileLifecycleError> {
            let mut marker = self.marker.lock().unwrap();
            if marker.profile_generation != expected_generation
                || marker.factory_reset_phase != FactoryResetPhaseV1::None
            {
                return Err(ProfileLifecycleError::PhaseConflict);
            }
            marker.factory_reset_phase = FactoryResetPhaseV1::WipingKeys;
            Ok(*marker)
        }

        fn mark_keys_wiped(
            &self,
            expected_generation: [u8; 16],
        ) -> Result<ProfileLifecycleMarkerV1, ProfileLifecycleError> {
            let mut marker = self.marker.lock().unwrap();
            if marker.profile_generation != expected_generation
                || marker.factory_reset_phase != FactoryResetPhaseV1::WipingKeys
            {
                return Err(ProfileLifecycleError::PhaseConflict);
            }
            marker.factory_reset_phase = FactoryResetPhaseV1::ClearingState;
            Ok(*marker)
        }

        fn complete_state_clear(
            &self,
            expected_generation: [u8; 16],
        ) -> Result<ProfileLifecycleMarkerV1, ProfileLifecycleError> {
            let mut marker = self.marker.lock().unwrap();
            if marker.profile_generation != expected_generation
                || marker.factory_reset_phase != FactoryResetPhaseV1::ClearingState
            {
                return Err(ProfileLifecycleError::PhaseConflict);
            }
            marker.profile_generation = [2; 16];
            marker.factory_reset_phase = FactoryResetPhaseV1::None;
            Ok(*marker)
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
            _profile_generation: [u8; 16],
        ) -> Result<(), ProfileFactoryResetCapabilityError> {
            self.invoke()
        }
    }

    #[async_trait]
    impl ClearProfileStatePort for Capability {
        async fn clear_and_verify_profile_state(
            &self,
            _profile_generation: [u8; 16],
        ) -> Result<(), ProfileFactoryResetCapabilityError> {
            self.invoke()
        }
    }

    #[tokio::test]
    async fn factory_reset_runs_in_key_first_order_without_an_active_space() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let lifecycle = Arc::new(Lifecycle::new());
        let reset = ProfileFactoryReset::new(
            lifecycle,
            Arc::new(Capability::new("stop", Arc::clone(&calls), 0)),
            Arc::new(Capability::new("wipe", Arc::clone(&calls), 0)),
            Arc::new(Capability::new("clear", Arc::clone(&calls), 0)),
        );

        let result = reset.factory_reset().await.unwrap();

        assert_eq!(result.previous_generation, [1; 16]);
        assert_eq!(result.new_generation, [2; 16]);
        assert_eq!(*calls.lock().unwrap(), ["stop", "wipe", "clear"]);
    }

    #[tokio::test]
    async fn restart_resumes_wiping_keys_and_clearing_state_without_going_backwards() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let lifecycle = Arc::new(Lifecycle::new());
        let reset = ProfileFactoryReset::new(
            Arc::clone(&lifecycle) as Arc<dyn ProfileLifecyclePort>,
            Arc::new(Capability::new("stop", Arc::clone(&calls), 0)),
            Arc::new(Capability::new("wipe", Arc::clone(&calls), 1)),
            Arc::new(Capability::new("clear", Arc::clone(&calls), 1)),
        );

        assert!(matches!(
            reset.factory_reset().await,
            Err(ProfileFactoryResetError::WipeKeys)
        ));
        assert_eq!(lifecycle.phase(), FactoryResetPhaseV1::WipingKeys);
        assert!(matches!(
            reset.recover_if_needed().await,
            Err(ProfileFactoryResetError::ClearState)
        ));
        assert_eq!(lifecycle.phase(), FactoryResetPhaseV1::ClearingState);
        let recovered = reset.recover_if_needed().await.unwrap().unwrap();

        assert_eq!(recovered.new_generation, [2; 16]);
        assert_eq!(lifecycle.phase(), FactoryResetPhaseV1::None);
        assert_eq!(
            *calls.lock().unwrap(),
            ["stop", "wipe", "stop", "wipe", "clear", "stop", "clear"]
        );
        assert!(reset.recover_if_needed().await.unwrap().is_none());
    }
}
