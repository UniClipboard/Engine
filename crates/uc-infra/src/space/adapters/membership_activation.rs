use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::{
    ActivateMembershipEffectPort, MembershipEffectExecutionError, MembershipEffectKind,
    PendingMembershipEffect,
};
use uc_core::ports::PeerReachabilityPort;

/// 在账本开放最终成员 scope 前清理与旧资格绑定的网络观察。
pub struct MembershipActivationAdapter {
    reachability: Arc<dyn PeerReachabilityPort>,
}

impl MembershipActivationAdapter {
    pub fn new(reachability: Arc<dyn PeerReachabilityPort>) -> Self {
        Self { reachability }
    }
}

#[async_trait]
impl ActivateMembershipEffectPort for MembershipActivationAdapter {
    async fn activate_membership_effect(
        &self,
        effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        if effect.affected_device_ids.is_empty() {
            return Err(MembershipEffectExecutionError::Corrupt);
        }
        if effect.kind == MembershipEffectKind::RemoveDevice {
            for device_id in &effect.affected_device_ids {
                self.reachability.forget(device_id).await;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tokio::sync::broadcast;
    use uc_application::deps::MembershipEffectPhase;
    use uc_core::ids::DeviceId;
    use uc_core::ports::{PeerReachabilityChanged, PresenceError, ReachabilityState};

    use super::*;

    #[derive(Default)]
    struct RecordingReachability {
        forgotten: Mutex<Vec<DeviceId>>,
    }

    #[async_trait]
    impl PeerReachabilityPort for RecordingReachability {
        async fn ensure_reachable(
            &self,
            _device: &DeviceId,
        ) -> Result<ReachabilityState, PresenceError> {
            Ok(ReachabilityState::Unknown)
        }

        async fn forget(&self, device: &DeviceId) {
            self.forgotten.lock().unwrap().push(device.clone());
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
    async fn removal_forgets_stale_reachability_before_scope_activation() {
        let reachability = Arc::new(RecordingReachability::default());
        let adapter = MembershipActivationAdapter::new(reachability.clone());

        adapter
            .activate_membership_effect(&PendingMembershipEffect {
                event_id: [1; 32],
                kind: MembershipEffectKind::RemoveDevice,
                phase: MembershipEffectPhase::SecurityApplied,
                affected_device_ids: vec![DeviceId::new("removed")],
                payload: Vec::new(),
            })
            .await
            .unwrap();

        assert_eq!(
            reachability.forgotten.lock().unwrap().as_slice(),
            &[DeviceId::new("removed")]
        );
    }
}
