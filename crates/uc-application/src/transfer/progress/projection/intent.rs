//! 本机条目拥有发送意图；删除封住旧意图，协议源身份始终保持原值。
use super::handle::Inner;
use super::{ProgressProjectionAdmission, ProgressProjectionKey, ProgressProjectionRegistry};
use crate::transfer::progress::ProgressReceiveError;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, Weak};
use uc_core::file_transfer::ProgressItem;
use uc_core::ids::EntryId;

pub(super) struct IntentGeneration {
    retired: AtomicBool,
}
#[derive(Clone)]
pub(super) struct IntentOwner {
    pub entry: EntryId,
    generation: Arc<IntentGeneration>,
}
impl IntentOwner {
    pub fn retired(&self) -> bool {
        self.generation.retired.load(Ordering::Acquire)
    }
}
#[derive(Default)]
pub(super) struct IntentLifetimes {
    generations: HashMap<EntryId, Weak<IntentGeneration>>,
    deleting: HashMap<EntryId, usize>,
}
impl IntentLifetimes {
    pub fn begin(&mut self, entry: EntryId, closed: bool) -> IntentOwner {
        // 没有活动意图或保留投影时不保留本机条目标识；失败/淘汰不积累墓碑。
        self.generations
            .retain(|_, generation| generation.strong_count() > 0);
        let blocked = closed || self.deleting.contains_key(&entry);
        let generation = if blocked {
            None
        } else {
            self.generations
                .get(&entry)
                .and_then(Weak::upgrade)
                .filter(|value| !value.retired.load(Ordering::Acquire))
        }
        .unwrap_or_else(|| {
            Arc::new(IntentGeneration {
                retired: AtomicBool::new(blocked),
            })
        });
        if !blocked {
            self.generations
                .insert(entry.clone(), Arc::downgrade(&generation));
        }
        IntentOwner { entry, generation }
    }
    pub fn delete(&mut self, entry: &EntryId) {
        *self.deleting.entry(entry.clone()).or_default() += 1;
        if let Some(generation) = self
            .generations
            .remove(entry)
            .and_then(|generation| generation.upgrade())
        {
            generation.retired.store(true, Ordering::Release);
        }
    }
    pub fn finish_delete(&mut self, entry: &EntryId) {
        if let Some(count) = self.deleting.get_mut(entry) {
            *count -= 1;
            if *count == 0 {
                self.deleting.remove(entry);
            }
        }
    }
    pub fn retire_all(&mut self) {
        for generation in self.generations.values().filter_map(Weak::upgrade) {
            generation.retired.store(true, Ordering::Release);
        }
        self.generations.clear();
    }
}

/// 原发送动作在第一次等待前取得；只接受该动作所属世代的原始计划。
pub struct ProgressIntent {
    registry: ProgressProjectionRegistry,
    owner: IntentOwner,
}
impl ProgressIntent {
    pub fn accept(
        &self,
        key: ProgressProjectionKey,
        plan: Vec<ProgressItem>,
    ) -> Result<ProgressProjectionAdmission, ProgressReceiveError> {
        self.registry.retain_intent(&self.owner, key, plan)
    }
}
pub(crate) struct ProgressEntryDeletion {
    inner: Arc<Inner>,
    entry: EntryId,
}
impl Drop for ProgressEntryDeletion {
    fn drop(&mut self) {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .intents
            .finish_delete(&self.entry);
    }
}
impl ProgressProjectionRegistry {
    pub fn begin_intent(&self, owner: EntryId) -> ProgressIntent {
        let owner = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let closed = state.capacity.is_closed();
            state.intents.begin(owner, closed)
        };
        ProgressIntent {
            registry: self.clone(),
            owner,
        }
    }
    /// 删除/保留清理使用同一原动作；围栏持续到原数据库与文件清理实际返回。
    pub(crate) fn deleting_entry(&self, entry: &EntryId) -> ProgressEntryDeletion {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.intents.delete(entry);
        let keys = state
            .slots
            .iter()
            .filter(|(_, slot)| slot.owner.entry == *entry)
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        for key in keys {
            super::handle::remove_slot(&self.inner, &mut state, key);
        }
        self.inner.wake.notify_one();
        ProgressEntryDeletion {
            inner: self.inner.clone(),
            entry: entry.clone(),
        }
    }
}
