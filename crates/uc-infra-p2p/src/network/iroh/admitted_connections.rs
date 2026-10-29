//! 成员连接的登记：成员被移除或离开 Space 时按设备统一关闭。
//!
//! 既有协议在连接开始时授权，普通剪贴板还在开流及交接前重检。长连接（进度回传、等待应用结算的剪贴板帧）
//! 之后不会再被检查；出站内容发送则可能停在拨号或等待回执。登记让会话的成员连接
//! 负责人在撤销时一并关闭这些连接，并让进行中的拨号失效，而不是由每个协议各自轮询授权。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use uc_application::deps::PeerConnection;
use uc_core::ids::DeviceId;

struct Tracked {
    stable_id: usize,
    connection: Arc<dyn PeerConnection>,
}

#[derive(Default)]
struct State {
    connections: HashMap<DeviceId, Vec<Tracked>>,
    /// 每个设备被撤销过的次数，以及整体撤销（离开 Space）的次数。
    revoked: HashMap<DeviceId, u64>,
    revoked_all: u64,
    revision: u64,
}

/// 开始拨号前取得的撤销快照；撤销发生过则迟到的连接不得登记。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct RevocationEpoch {
    device: u64,
    all: u64,
}

impl RevocationEpoch {
    fn of(state: &State, device: &DeviceId) -> Self {
        Self {
            device: state.revoked.get(device).copied().unwrap_or_default(),
            all: state.revoked_all,
        }
    }
}

/// 同一会话内各协议共享的登记表（入站 handler 与出站发送），克隆共享同一份状态。
#[derive(Clone, Default)]
pub struct AdmittedConnections {
    state: Arc<Mutex<State>>,
}

/// 登记凭据；连接的 handler 结束时丢弃它即自动注销。
pub(crate) struct ConnectionRegistration {
    owner: AdmittedConnections,
    device: DeviceId,
    stable_id: usize,
}

impl AdmittedConnections {
    /// 出站拨号开始前调用，记下此刻的撤销状态。
    pub(crate) fn epoch(&self, device: &DeviceId) -> RevocationEpoch {
        let state = self.lock();
        RevocationEpoch::of(&state, device)
    }

    pub(crate) fn revision(&self) -> u64 {
        self.lock().revision
    }
    pub(crate) fn current_since(&self, peer: &DeviceId, revision: u64) -> bool {
        let state = self.lock();
        state.revoked_all <= revision
            && state.revoked.get(peer).copied().unwrap_or_default() <= revision
    }
    pub(crate) fn current(&self, device: &DeviceId, epoch: RevocationEpoch) -> bool {
        RevocationEpoch::of(&self.lock(), device) == epoch
    }

    pub(crate) fn register_managed_since(
        &self,
        device: &DeviceId,
        connection: Arc<dyn PeerConnection>,
        epoch: RevocationEpoch,
    ) -> Option<ConnectionRegistration> {
        let mut state = self.lock();
        if RevocationEpoch::of(&state, device) != epoch {
            return None;
        }
        let stable_id = connection.id();
        state.connections.entry(*device).or_default().push(Tracked {
            stable_id,
            connection,
        });
        Some(ConnectionRegistration {
            owner: self.clone(),
            device: *device,
            stable_id,
        })
    }

    /// 关闭并移除 `device` 的全部登记连接，并使进行中的拨号失效。
    pub(crate) fn close_device(&self, device: &DeviceId) {
        let closing = {
            let mut state = self.lock();
            state.revision += 1;
            let revision = state.revision;
            state.revoked.insert(*device, revision);
            state.connections.remove(device).unwrap_or_default()
        };
        Self::close(closing);
    }

    /// 关闭并移除所有登记连接，并使进行中的拨号失效。
    pub(crate) fn close_all(&self) {
        let closing: Vec<Tracked> = {
            let mut state = self.lock();
            state.revision += 1;
            state.revoked_all = state.revision;
            state
                .connections
                .drain()
                .flat_map(|(_, tracked)| tracked)
                .collect()
        };
        Self::close(closing);
    }

    fn close(tracked: Vec<Tracked>) {
        for entry in tracked {
            entry.connection.close();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for ConnectionRegistration {
    fn drop(&mut self) {
        let mut state = self.owner.lock();
        if let Some(tracked) = state.connections.get_mut(&self.device) {
            tracked.retain(|entry| entry.stable_id != self.stable_id);
            if tracked.is_empty() {
                state.connections.remove(&self.device);
            }
        }
    }
}
