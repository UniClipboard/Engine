//! KeyMaterialStore——keyring (KEK) + 磁盘 (KeySlot) 的统一存取入口。
//!
//! Slice 3 - C8 起作为 uc-infra 内部具体类型存在(原 `KeyMaterialPort` trait
//! 已删除)；唯一消费者是 `RuntimeSpaceAccessAdapter`,后者通过 Arc 共享。

use std::sync::Arc;
use uc_core::{crypto::model::EncryptionError, ports::SecureStoragePort};

use crate::fs::key_slot_store::KeySlotStore;
use crate::security::crypto_model::{validate_kdf, KeyScope, KeySlot, KeySlotFile};
use crate::security::{Kek, SecureStorageAccess};

use super::scope_identifier::scope_identifier;
use crate::security::{v1_aead, MasterKey};
use uc_core::crypto::domain::Passphrase;
use uc_core::crypto::model::Passphrase as LegacyPassphrase;

#[cfg(test)]
mod tests;

pub struct KeyMaterialStore {
    secure_storage: SecureStorageAccess,
    keyslot_store: Arc<dyn KeySlotStore>,
}

impl KeyMaterialStore {
    pub fn new(
        secure_storage: Arc<dyn SecureStoragePort>,
        keyslot_store: Arc<dyn KeySlotStore>,
    ) -> Self {
        Self {
            secure_storage: SecureStorageAccess::new(secure_storage),
            keyslot_store,
        }
    }
}

fn kek_key(scope: &KeyScope) -> String {
    format!("kek:v1:{}", scope_identifier(scope))
}

impl KeyMaterialStore {
    pub async fn load_kek(&self, scope: &KeyScope) -> Result<Kek, EncryptionError> {
        let key = kek_key(scope);
        let secret = self
            .secure_storage
            .execute(move |storage| storage.get(&key)?.ok_or(EncryptionError::KeyNotFound))
            .await?;
        match Kek::from_bytes(&secret) {
            Ok(kek) => Ok(kek),
            Err(_) => Err(EncryptionError::KeyMaterialCorrupt),
        }
    }

    pub async fn store_kek(&self, scope: &KeyScope, kek: &Kek) -> Result<(), EncryptionError> {
        let key = kek_key(scope);
        let kek = kek.clone();
        self.secure_storage
            .execute(move |storage| {
                storage.set(&key, kek.as_bytes())?;
                Ok(())
            })
            .await
    }

    pub(crate) async fn authenticate_and_restore_kek(
        &self,
        scope: &KeyScope,
        passphrase: &Passphrase,
    ) -> Result<MasterKey, EncryptionError> {
        let (master, kek) = self.authenticate_kek(scope, passphrase).await?;
        self.persist_authenticated_kek(scope, &kek).await?;
        Ok(master)
    }

    pub(crate) async fn authenticate_kek(
        &self,
        scope: &KeyScope,
        passphrase: &Passphrase,
    ) -> Result<(MasterKey, Kek), EncryptionError> {
        let slot = self.load_keyslot(scope).await?;
        let wrapped = slot
            .wrapped_master_key
            .as_ref()
            .ok_or(EncryptionError::CorruptedKeySlot)?;
        let legacy = LegacyPassphrase(passphrase.expose().to_owned());
        let kek = match v1_aead::derive_kek_argon2id(&legacy, &slot.salt, &slot.kdf) {
            Ok(kek) => kek,
            Err(_) => return Err(EncryptionError::KdfFailed),
        };
        let master = match v1_aead::unwrap_master_key_xchacha(&kek, &wrapped.blob) {
            Ok(master) => master,
            Err(_) => return Err(EncryptionError::WrongPassphrase),
        };
        Ok((master, kek))
    }

    pub(crate) async fn persist_authenticated_kek(
        &self,
        scope: &KeyScope,
        kek: &Kek,
    ) -> Result<(), EncryptionError> {
        // Never trust a process-local observation after explicit authentication.
        match self.load_kek(scope).await {
            Ok(existing) if existing.as_bytes() == kek.as_bytes() => {}
            Ok(_) | Err(EncryptionError::KeyNotFound | EncryptionError::KeyMaterialCorrupt) => {
                self.store_kek(scope, &kek).await?
            }
            Err(error) => return Err(error),
        }
        let persisted = self.load_kek(scope).await?;
        if persisted.as_bytes() != kek.as_bytes() {
            return Err(EncryptionError::KeyMaterialCorrupt);
        }
        Ok(())
    }

    pub async fn delete_kek(&self, scope: &KeyScope) -> Result<(), EncryptionError> {
        let key = kek_key(scope);
        self.secure_storage
            .execute(move |storage| {
                storage.delete(&key)?;
                Ok(())
            })
            .await
    }

    pub async fn load_keyslot(&self, scope: &KeyScope) -> Result<KeySlot, EncryptionError> {
        let file = self.keyslot_store.load().await?;
        if &file.scope != scope {
            return Err(EncryptionError::KeyMaterialCorrupt);
        }
        if file.version != "V1" {
            return Err(EncryptionError::UnsupportedKeySlotVersion);
        }
        validate_kdf(&file.kdf)?;
        file.wrapped_master_key.validate_basic()?;
        if file.salt.len() < 8 {
            return Err(EncryptionError::CorruptedKeySlot);
        }
        Ok(file.into())
    }

    /// 本机磁盘上是否存在 keyslot 文件(任意 scope)。取代 Phase C 前的
    /// `EncryptionStatePort.load_state() == Initialized` 判断:从"是否写过
    /// marker 文件"改成"是否真的有 keyslot",更精确。
    pub async fn keyslot_exists(&self) -> Result<bool, EncryptionError> {
        Ok(self.keyslot_store.exists().await)
    }

    pub async fn store_keyslot(&self, keyslot: &KeySlot) -> Result<(), EncryptionError> {
        let file = match KeySlotFile::try_from(keyslot) {
            Ok(file) => file,
            Err(_) => return Err(EncryptionError::CorruptedKeySlot),
        };
        self.keyslot_store.store(&file).await
    }

    pub async fn delete_keyslot(&self, scope: &KeyScope) -> Result<(), EncryptionError> {
        let file = self.keyslot_store.load().await?;
        if &file.scope != scope {
            return Err(EncryptionError::KeyMaterialCorrupt);
        }
        self.keyslot_store.delete().await
    }

    pub async fn quarantine_keyslot(&self) -> Result<(), EncryptionError> {
        self.keyslot_store.quarantine().await
    }
}
