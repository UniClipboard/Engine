//! 成员连接的登记：成员被移除或离开 Space 时按设备统一关闭。
//!
//! 入站连接的授权只在开始时判定一次，长连接（进度回传、等待应用结算的剪贴板帧）
//! 之后不会再被检查；出站内容发送则可能停在等待回执。登记让会话的成员连接
//! 负责人在撤销时一并关闭这些连接，而不是由每个协议各自轮询授权。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use iroh::endpoint::Connection;
use uc_core::ids::DeviceId;

struct Tracked {
    stable_id: usize,
    connection: Connection,
}

/// 同一会话内各协议共享的登记表（入站 handler 与出站发送），克隆共享同一份状态。
#[derive(Clone, Default)]
pub(crate) struct AdmittedConnections {
    entries: Arc<Mutex<HashMap<DeviceId, Vec<Tracked>>>>,
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
        let stable_id = connection.stable_id();
        self.lock().entry(*device).or_default().push(Tracked {
            stable_id,
            connection: connection.clone(),
        });
        ConnectionRegistration {
            owner: self.clone(),
            device: *device,
            stable_id,
        }
    }

    /// 关闭并移除 `device` 的全部登记连接。
    pub(crate) fn close_device(&self, device: &DeviceId, reason: &'static [u8]) {
        let closing = self.lock().remove(device).unwrap_or_default();
        Self::close(closing, reason);
    }

    /// 关闭并移除所有登记连接。
    pub(crate) fn close_all(&self, reason: &'static [u8]) {
        let closing: Vec<Tracked> = self
            .lock()
            .drain()
            .flat_map(|(_, tracked)| tracked)
            .collect();
        Self::close(closing, reason);
    }

    fn close(tracked: Vec<Tracked>, reason: &'static [u8]) {
        for entry in tracked {
            entry.connection.close(0u32.into(), reason);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<DeviceId, Vec<Tracked>>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for ConnectionRegistration {
    fn drop(&mut self) {
        let mut entries = self.owner.lock();
        if let Some(tracked) = entries.get_mut(&self.device) {
            tracked.retain(|entry| entry.stable_id != self.stable_id);
            if tracked.is_empty() {
                entries.remove(&self.device);
            }
        }
    }
}
