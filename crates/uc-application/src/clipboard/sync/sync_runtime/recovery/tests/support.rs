//! 本模块场景共用的测试支撑：可切换在线状态、计时边界与两类补发脚本。

mod real_delivery_path;
mod scripted_delivery;

pub(super) use real_delivery_path::*;
pub(super) use scripted_delivery::*;

use super::*;

/// 可控的在线状态：测试中途改为离线时不发事件，模拟“状态已变但事件未到”。
pub(super) struct SwitchablePeerReachability {
    pub(super) state: Mutex<ReachabilityState>,
    pub(super) tx: tokio::sync::broadcast::Sender<PeerReachabilityChanged>,
}

impl SwitchablePeerReachability {
    pub(super) fn new(state: ReachabilityState) -> Arc<Self> {
        let (tx, _) = tokio::sync::broadcast::channel(8);
        Arc::new(Self {
            state: Mutex::new(state),
            tx,
        })
    }

    pub(super) fn set(&self, state: ReachabilityState) {
        *self.state.lock().unwrap() = state;
    }

    pub(super) fn announce(&self, device_id: DeviceId, state: ReachabilityState) {
        self.set(state);
        let _ = self.tx.send(PeerReachabilityChanged {
            device_id,
            state,
            at: chrono::Utc::now(),
        });
    }
}

#[async_trait]
impl PeerReachabilityPort for SwitchablePeerReachability {
    async fn ensure_reachable(
        &self,
        _device: &DeviceId,
    ) -> Result<ReachabilityState, PeerReachabilityError> {
        Ok(*self.state.lock().unwrap())
    }

    async fn current_state(&self, _device: &DeviceId) -> ReachabilityState {
        *self.state.lock().unwrap()
    }

    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<PeerReachabilityChanged> {
        self.tx.subscribe()
    }
}

pub(super) const JUST_BEFORE: Duration = Duration::from_millis(1);
