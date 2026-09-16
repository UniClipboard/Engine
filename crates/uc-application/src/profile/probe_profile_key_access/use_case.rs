use std::sync::Arc;

use super::{ProbeProfileKeyAccessError, ProbeProfileKeyAccessPort, ProfileKeyAccessProbe};

pub struct ProbeProfileKeyAccessUseCase {
    probe: Arc<dyn ProbeProfileKeyAccessPort>,
}

impl ProbeProfileKeyAccessUseCase {
    pub fn new(probe: Arc<dyn ProbeProfileKeyAccessPort>) -> Self {
        Self { probe }
    }

    pub async fn execute(&self) -> Result<bool, ProbeProfileKeyAccessError> {
        match self.probe.probe_profile_key_access().await? {
            ProfileKeyAccessProbe::Available => Ok(true),
            ProfileKeyAccessProbe::PermissionDenied
            | ProfileKeyAccessProbe::TemporarilyUnavailable => Ok(false),
            ProfileKeyAccessProbe::Missing => Err(ProbeProfileKeyAccessError::NotInitialized),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::*;
    use crate::profile::probe_profile_key_access::ProfileKeyAccessProbePortError;

    struct StubProbe {
        result: Mutex<Result<ProfileKeyAccessProbe, ProfileKeyAccessProbePortError>>,
    }

    #[async_trait]
    impl ProbeProfileKeyAccessPort for StubProbe {
        async fn probe_profile_key_access(
            &self,
        ) -> Result<ProfileKeyAccessProbe, ProfileKeyAccessProbePortError> {
            self.result.lock().unwrap().clone()
        }
    }

    async fn execute(
        result: Result<ProfileKeyAccessProbe, ProfileKeyAccessProbePortError>,
    ) -> Result<bool, ProbeProfileKeyAccessError> {
        ProbeProfileKeyAccessUseCase::new(Arc::new(StubProbe {
            result: Mutex::new(result),
        }))
        .execute()
        .await
    }

    #[tokio::test]
    async fn available_key_returns_true() {
        assert!(execute(Ok(ProfileKeyAccessProbe::Available)).await.unwrap());
    }

    #[tokio::test]
    async fn denied_or_temporarily_unavailable_returns_false() {
        assert!(!execute(Ok(ProfileKeyAccessProbe::PermissionDenied))
            .await
            .unwrap());
        assert!(!execute(Ok(ProfileKeyAccessProbe::TemporarilyUnavailable))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn missing_key_returns_not_initialized() {
        assert_eq!(
            execute(Ok(ProfileKeyAccessProbe::Missing)).await,
            Err(ProbeProfileKeyAccessError::NotInitialized)
        );
    }

    #[tokio::test]
    async fn unexpected_probe_failure_is_preserved() {
        assert_eq!(
            execute(Err(ProfileKeyAccessProbePortError)).await,
            Err(ProbeProfileKeyAccessError::Probe(
                ProfileKeyAccessProbePortError
            ))
        );
    }
}
