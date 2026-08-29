use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::LoadDeviceTrustObservationsPort;
use uc_application::facade::{DeviceTrustObservation, QueryDeviceTrustError};
use uc_core::ids::DeviceId;
use uc_core::membership::MemberRepositoryPort;
use uc_core::ports::PeerReachabilityPort;

/// 从成员投影和可达性缓存加载产品展示所需的观察资料。
///
/// 本适配器不判断成员资格。Application 已经从签名成员账本确定查询范围，
/// 这里仅为该范围补充显示名称和当前可达性。
pub struct DeviceTrustObservationsAdapter {
    members: Arc<dyn MemberRepositoryPort>,
    reachability: Arc<dyn PeerReachabilityPort>,
}

impl DeviceTrustObservationsAdapter {
    pub fn new(
        members: Arc<dyn MemberRepositoryPort>,
        reachability: Arc<dyn PeerReachabilityPort>,
    ) -> Self {
        Self {
            members,
            reachability,
        }
    }
}

#[async_trait]
impl LoadDeviceTrustObservationsPort for DeviceTrustObservationsAdapter {
    async fn load(
        &self,
        device_ids: &[DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError> {
        let mut observations = Vec::with_capacity(device_ids.len());
        for device_id in device_ids {
            let member = self.members.get(device_id).await.map_err(|error| {
                QueryDeviceTrustError::Dependency {
                    source: anyhow::Error::new(error),
                }
            })?;
            let Some(member) = member else {
                continue;
            };
            observations.push(DeviceTrustObservation {
                device_id: device_id.clone(),
                display_name: Some(member.device_name),
                reachability: self.reachability.current_state(device_id).await,
            });
        }
        Ok(observations)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use tokio::sync::broadcast;
    use uc_core::membership::{MembershipError, SpaceMember};
    use uc_core::ports::{PeerReachabilityChanged, PresenceError, ReachabilityState};

    use super::*;

    struct FailingMembers;

    #[async_trait]
    impl MemberRepositoryPort for FailingMembers {
        async fn get(&self, _device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            Err(MembershipError::Repository("test failure".to_owned()))
        }

        async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
            Ok(Vec::new())
        }

        async fn save(&self, _member: &SpaceMember) -> Result<(), MembershipError> {
            Ok(())
        }

        async fn remove(&self, _device_id: &DeviceId) -> Result<bool, MembershipError> {
            Ok(false)
        }
    }

    struct UnknownReachability;

    #[async_trait]
    impl PeerReachabilityPort for UnknownReachability {
        async fn ensure_reachable(
            &self,
            _device: &DeviceId,
        ) -> Result<ReachabilityState, PresenceError> {
            Ok(ReachabilityState::Unknown)
        }

        async fn current_state(&self, _device: &DeviceId) -> ReachabilityState {
            ReachabilityState::Unknown
        }

        fn subscribe(&self) -> broadcast::Receiver<PeerReachabilityChanged> {
            let (_events, receiver) = broadcast::channel(1);
            receiver
        }
    }

    #[tokio::test]
    async fn repository_failure_keeps_stable_classification_and_source() {
        let adapter = DeviceTrustObservationsAdapter::new(
            Arc::new(FailingMembers),
            Arc::new(UnknownReachability),
        );

        let error = adapter
            .load(&[DeviceId::new("device-a")])
            .await
            .expect_err("repository failure must surface");

        assert!(matches!(error, QueryDeviceTrustError::Dependency { .. }));
        assert!(error.source().is_some());
    }
}
