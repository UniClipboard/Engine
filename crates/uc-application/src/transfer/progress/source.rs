//! 已接受接收尝试的运行期权威；不持久化来源材料，不按 UUID 大小排序。
use super::{ProgressSourceTask, ProgressTaskDescriptor};
use std::collections::HashMap;
use std::sync::Arc;
use uc_core::file_transfer::{valid_progress_plan, ProgressIdentity, ProgressItem};
use uc_core::ids::{DeviceId, EntryId};
use uuid::Uuid;

const GLOBAL_CAPACITY: usize = 256;
const PEER_CAPACITY: usize = 64;
type SourceKey = (DeviceId, ProgressIdentity);
struct AcceptedTask {
    local_entry: EntryId,
    task: ProgressSourceTask,
}

pub(crate) struct ProgressSourceCatalog {
    producer: ProgressIdentity,
    revision: u64,
    exhausted: bool,
    tasks: HashMap<SourceKey, AcceptedTask>,
}
impl Default for ProgressSourceCatalog {
    fn default() -> Self {
        Self {
            producer: ProgressIdentity::new(*Uuid::new_v4().as_bytes()),
            revision: 0,
            exhausted: false,
            tasks: HashMap::new(),
        }
    }
}
impl ProgressSourceCatalog {
    /// 必须在原接收负责人确认持久 attempt 接受后调用；重复交接不刷新修订。
    pub(crate) fn accept(
        &mut self,
        local_entry: &EntryId,
        peer: DeviceId,
        source: ProgressIdentity,
        attempt: ProgressIdentity,
        items: Vec<ProgressItem>,
    ) {
        if self.exhausted {
            return;
        }
        let key = (peer, source);
        if self.tasks.get(&key).is_some_and(|existing| {
            existing.local_entry == *local_entry
                && existing.task.descriptor.attempt == attempt
                && existing.task.items.as_ref() == items.as_slice()
        }) {
            return;
        }
        self.forget(local_entry);
        if !valid_progress_plan(&items) {
            return;
        }
        let Some(revision) = self.revision.checked_add(1) else {
            self.clear();
            self.exhausted = true;
            return;
        };
        self.revision = revision;
        // 同一个来源在接受新attempt时替换，不为每次尝试保留无限历史。
        self.tasks.remove(&key);
        if self
            .tasks
            .keys()
            .filter(|(device, _)| *device == peer)
            .count()
            >= PEER_CAPACITY
        {
            self.evict_oldest(Some(peer));
        }
        if self.tasks.len() >= GLOBAL_CAPACITY {
            self.evict_oldest(None);
        }
        self.tasks.insert(
            key,
            AcceptedTask {
                local_entry: local_entry.clone(),
                task: ProgressSourceTask {
                    descriptor: ProgressTaskDescriptor {
                        producer: self.producer,
                        revision,
                        source,
                        attempt,
                        item_count: items.len() as u16,
                    },
                    items: Arc::from(items),
                },
            },
        );
    }
    pub(crate) fn current(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Option<ProgressSourceTask> {
        self.tasks
            .get(&(*peer, source))
            .map(|entry| entry.task.clone())
    }
    pub(crate) fn forget(&mut self, entry: &EntryId) {
        self.tasks.retain(|_, task| task.local_entry != *entry);
    }
    pub(crate) fn clear(&mut self) {
        self.tasks.clear();
    }
    /// 会话真正封口才退休producer；普通连接重建不调用此动作。
    pub(crate) fn retire(&mut self) {
        *self = Self::default();
    }
    fn evict_oldest(&mut self, peer: Option<DeviceId>) {
        let oldest = self
            .tasks
            .iter()
            .filter(|((device, _), _)| peer.is_none_or(|peer| peer == *device))
            .min_by_key(|(_, value)| value.task.descriptor.revision)
            .map(|(key, _)| *key);
        if let Some(key) = oldest {
            self.tasks.remove(&key);
        }
    }
}
