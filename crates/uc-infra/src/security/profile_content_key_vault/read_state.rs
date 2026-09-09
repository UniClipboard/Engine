//! 受可撤销运行期许可约束的私有目录，不向调用方借出整份密钥快照。
use std::collections::HashMap;
use std::fs::File;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use uc_core::membership::{ContentKeyId, GroupEpoch, ProtectionGroupId};

use super::{
    model::PersistedVault, MasterKey, ProfileContentKeyVaultError, ResolvedProfileContentKey,
};

pub(super) struct ReadView {
    pub(super) vault: PersistedVault,
    pub(super) root: MasterKey,
    index: HashMap<String, (usize, usize)>,
}

impl ReadView {
    pub(super) fn new(vault: PersistedVault, root: MasterKey) -> Self {
        let index = vault
            .groups
            .iter()
            .enumerate()
            .flat_map(|(g, group)| {
                group
                    .entries
                    .iter()
                    .enumerate()
                    .map(move |(e, entry)| (entry.content_key_id.clone(), (g, e)))
            })
            .collect();
        Self { vault, root, index }
    }

    pub(super) fn resolve(
        &self,
        id: &ContentKeyId,
        epoch: GroupEpoch,
    ) -> Result<ResolvedProfileContentKey, ProfileContentKeyVaultError> {
        let &(g, e) = self
            .index
            .get(id.as_str())
            .ok_or(ProfileContentKeyVaultError::KeyNotFound)?;
        let group = &self.vault.groups[g];
        let entry = &group.entries[e];
        if entry.epoch != epoch.value() {
            return Err(ProfileContentKeyVaultError::EpochMismatch);
        }
        Ok(ResolvedProfileContentKey {
            protection_group_id: ProtectionGroupId::from_string(group.protection_group_id.clone())
                .map_err(|source| ProfileContentKeyVaultError::Corrupt {
                    source: anyhow::Error::new(source),
                })?,
            content_key_id: id.clone(),
            epoch,
            key: MasterKey::from_bytes(&entry.key).map_err(|source| {
                ProfileContentKeyVaultError::Corrupt {
                    source: anyhow::Error::new(source),
                }
            })?,
        })
    }
}

impl Drop for ReadView {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        for (mut id, _) in self.index.drain() {
            id.zeroize();
        }
    }
}

#[derive(Default)]
pub(super) struct ReadState {
    pub(super) generation: Arc<()>,
    pub(super) reusable: bool,
    pub(super) closed: bool,
    pub(super) view: Option<ReadView>,
    pub(super) lease: Option<Arc<File>>,
}

impl ReadState {
    pub(super) fn check_open(&self) -> Result<(), ProfileContentKeyVaultError> {
        if self.closed {
            Err(ProfileContentKeyVaultError::Closed)
        } else {
            Ok(())
        }
    }

    fn revoke(&mut self) {
        self.generation = Arc::new(());
        self.reusable = false;
        self.view = None;
        self.lease = None;
    }

    pub(super) fn close(&mut self) {
        self.revoke();
        self.closed = true;
    }
}

pub(super) fn lock(state: &Mutex<ReadState>) -> MutexGuard<'_, ReadState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 只关联有效期；不携带密钥，旧许可不能撤销新运行期。
pub(crate) struct ProfileKeyReadLease {
    state: Weak<Mutex<ReadState>>,
    generation: Arc<()>,
}

impl ProfileKeyReadLease {
    pub(super) fn begin(
        state: &Arc<Mutex<ReadState>>,
    ) -> Result<Self, ProfileContentKeyVaultError> {
        let mut current = lock(state);
        current.check_open()?;
        if current.reusable {
            return Err(ProfileContentKeyVaultError::Conflict);
        }
        // 开始复用也是新代次，不能接收此前临时读取的无驻留租约结果。
        current.generation = Arc::new(());
        current.reusable = true;
        Ok(Self {
            state: Arc::downgrade(state),
            generation: current.generation.clone(),
        })
    }
}

impl Drop for ProfileKeyReadLease {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            let mut state = lock(&state);
            if Arc::ptr_eq(&state.generation, &self.generation) {
                state.revoke();
            }
        }
    }
}

/// 写入错误与取消都会失效；不能从失败返回推断 rename 尚未提交。
pub(super) struct InvalidateOnDrop<'a>(pub(super) &'a Mutex<ReadState>);
impl Drop for InvalidateOnDrop<'_> {
    fn drop(&mut self) {
        lock(self.0).view = None;
    }
}
