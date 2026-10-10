//! 成员连接的登记：成员被移除或离开 Space 时按设备统一关闭。
//!
//! 入站连接的授权只在开始时判定一次，长连接（进度回传、等待应用结算的剪贴板帧）
//! 之后不会再被检查；出站内容发送则可能停在拨号或等待回执。登记让会话的成员连接
//! 负责人在撤销时一并关闭这些连接，并让进行中的拨号失效，而不是由每个协议各自轮询授权。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use iroh::endpoint::Connection;
use uc_core::ids::DeviceId;

struct Tracked {
    stable_id: usize,
    connection: Connection,
}

#[derive(Default)]
struct State {
    connections: HashMap<DeviceId, Vec<Tracked>>,
    /// 每个设备被撤销过的次数，以及整体撤销（离开 Space）的次数。
    revoked: HashMap<DeviceId, u64>,
    revoked_all: u64,
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
pub(crate) struct AdmittedConnections {
    state: Arc<Mutex<State>>,
}

/// 登记凭据；连接的 handler 结束时丢弃它即自动注销。
pub(crate) struct ConnectionRegistration {
    owner: AdmittedConnections,
    device: DeviceId,
    stable_id: usize,
}

impl AdmittedConnections {
    /// 登记 `connection`。入站先登记再授权，保证撤销与授权并发时至少一方生效。
    pub(crate) fn register(
        &self,
        device: &DeviceId,
        connection: &Connection,
    ) -> ConnectionRegistration {
        let mut state = self.lock();
        self.insert(&mut state, device, connection)
    }

    /// 出站拨号开始前调用，记下此刻的撤销状态。
    pub(crate) fn epoch(&self, device: &DeviceId) -> RevocationEpoch {
        let state = self.lock();
        RevocationEpoch::of(&state, device)
    }

    /// 拨号完成后登记；若拨号期间 `device` 被撤销过，返回 `None`，调用方丢弃连接。
    pub(crate) fn register_since(
        &self,
        device: &DeviceId,
        connection: &Connection,
        epoch: RevocationEpoch,
    ) -> Option<ConnectionRegistration> {
        let mut state = self.lock();
        if RevocationEpoch::of(&state, device) != epoch {
            return None;
        }
        Some(self.insert(&mut state, device, connection))
    }

    /// 关闭并移除 `device` 的全部登记连接，并使进行中的拨号失效。
    pub(crate) fn close_device(&self, device: &DeviceId, reason: &'static [u8]) {
        let closing = {
            let mut state = self.lock();
            *state.revoked.entry(*device).or_default() += 1;
            state.connections.remove(device).unwrap_or_default()
        };
        Self::close(closing, reason);
    }

    /// 关闭并移除所有登记连接，并使进行中的拨号失效。
    pub(crate) fn close_all(&self, reason: &'static [u8]) {
        let closing: Vec<Tracked> = {
            let mut state = self.lock();
            state.revoked_all += 1;
            state
                .connections
                .drain()
                .flat_map(|(_, tracked)| tracked)
                .collect()
        };
        Self::close(closing, reason);
    }

    fn insert(
        &self,
        state: &mut State,
        device: &DeviceId,
        connection: &Connection,
    ) -> ConnectionRegistration {
        let stable_id = connection.stable_id();
        state.connections.entry(*device).or_default().push(Tracked {
            stable_id,
            connection: connection.clone(),
        });
        ConnectionRegistration {
            owner: self.clone(),
            device: *device,
            stable_id,
        }
    }

    fn close(tracked: Vec<Tracked>, reason: &'static [u8]) {
        for entry in tracked {
            entry.connection.close(0u32.into(), reason);
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
