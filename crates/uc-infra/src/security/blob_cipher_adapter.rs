//! `BlobCipherPort` 的基础设施适配器（V1 加密：XChaCha20-Poly1305）。
//!
//! 端到端会话管理：内部持有 `EncryptionSessionPort`，自己完成
//! "会话就绪检查 + 取出 MasterKey + AEAD 加解密 + EncryptedBlob 序列化"。
//! 调用方只看到 `Plaintext` / `Ciphertext` / `Aad` 的进出。
//!
//! # Wire format
//!
//! `Ciphertext` 字节 = `serde_json::to_vec(&EncryptedBlob)`。这正是历史上
//! `EncryptionPort::encrypt_blob` 输出后被 4 个 decorators 经
//! `serde_json::to_vec(&encrypted_blob)` 写入 SQL inline_data 的字节布局——
//! 保留这个格式让 SQL 中既有的密文仍可被新 adapter 解开（V1 数据兼容
//! ironclad 不变量）。
//!
//! adapter 内部不依赖旧 `EncryptionPort`——AEAD 调用通过 `super::v1_aead`
//! 私有 helper 直接落地，跟 `EncryptionRepository` / `EncryptedBlobStore`
//! 共用同一份算法实现，杜绝行为漂移。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use uc_core::crypto::domain::{Aad, ActiveSpace, Ciphertext, Plaintext};
use uc_core::membership::{ContentKeyId, ContentKeyPurpose, GroupEpoch};
use uc_core::ports::security::blob_cipher::{BlobCipherError, BlobCipherPort};

use super::crypto_model::EncryptedBlob;
use super::key_epoch_aad;
use super::session::InMemorySession;
use super::v1_aead;

pub struct BlobCipherAdapter {
    session: Arc<InMemorySession>,
}

#[derive(Serialize, Deserialize)]
struct KeyedEncryptedBlob {
    version: String,
    aead: String,
    content_key_id: String,
    group_epoch: u64,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    aad_fingerprint: Option<Vec<u8>>,
}

#[derive(Deserialize)]
struct BlobVersion {
    version: String,
}

impl BlobCipherAdapter {
    pub fn new(session: Arc<InMemorySession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl BlobCipherPort for BlobCipherAdapter {
    async fn encrypt(
        &self,
        _space: &ActiveSpace,
        plaintext: &Plaintext,
        aad: &Aad,
    ) -> Result<Ciphertext, BlobCipherError> {
        if !self.session.is_ready() {
            return Err(BlobCipherError::NotUnlocked);
        }
        let space_id = self
            .session
            .current_space_id()
            .map_err(|e| BlobCipherError::Internal(e.to_string()))?;
        let resolved = self
            .session
            .current_content_key(&space_id, ContentKeyPurpose::Content)
            .map_err(|e| BlobCipherError::Internal(e.to_string()))?;
        let bound_aad = key_epoch_aad::bind(
            b"blob-json-v2",
            &space_id,
            resolved.epoch(),
            resolved.content_key_id(),
            ContentKeyPurpose::Content,
            aad.as_bytes(),
        );

        let blob = v1_aead::encrypt_blob_xchacha(resolved.key(), plaintext.as_bytes(), &bound_aad)
            .map_err(|e| BlobCipherError::Internal(e.to_string()))?;
        let keyed = KeyedEncryptedBlob {
            version: "V2".to_owned(),
            aead: blob.aead,
            content_key_id: resolved.content_key_id().as_str().to_owned(),
            group_epoch: resolved.epoch().value(),
            nonce: blob.nonce,
            ciphertext: blob.ciphertext,
            aad_fingerprint: blob.aad_fingerprint,
        };

        let bytes = serde_json::to_vec(&keyed)
            .map_err(|e| BlobCipherError::Internal(format!("serialize keyed blob: {e}")))?;
        Ok(Ciphertext::new(bytes))
    }

    async fn decrypt(
        &self,
        _space: &ActiveSpace,
        ciphertext: &Ciphertext,
        aad: &Aad,
    ) -> Result<Plaintext, BlobCipherError> {
        if !self.session.is_ready() {
            return Err(BlobCipherError::NotUnlocked);
        }

        let version: BlobVersion = serde_json::from_slice(ciphertext.as_bytes())
            .map_err(|_| BlobCipherError::InvalidCiphertext)?;
        let space_id = self
            .session
            .current_space_id()
            .map_err(|_| BlobCipherError::NotUnlocked)?;
        let plain = match version.version.as_str() {
            "V1" => {
                let blob: EncryptedBlob = serde_json::from_slice(ciphertext.as_bytes())
                    .map_err(|_| BlobCipherError::InvalidCiphertext)?;
                if blob.aead != "XChaCha20Poly1305" {
                    return Err(BlobCipherError::InvalidCiphertext);
                }
                let master_key = self
                    .session
                    .legacy_content_key()
                    .map_err(|e| BlobCipherError::Internal(e.to_string()))?;
                decrypt(&master_key, &blob.nonce, &blob.ciphertext, aad.as_bytes())?
            }
            "V2" => {
                let blob: KeyedEncryptedBlob = serde_json::from_slice(ciphertext.as_bytes())
                    .map_err(|_| BlobCipherError::InvalidCiphertext)?;
                if blob.aead != "XChaCha20Poly1305" || blob.nonce.len() != 24 {
                    return Err(BlobCipherError::InvalidCiphertext);
                }
                let content_key_id = ContentKeyId::from_string(blob.content_key_id)
                    .map_err(|_| BlobCipherError::InvalidCiphertext)?;
                let epoch = GroupEpoch::new(blob.group_epoch);
                let resolved = self
                    .session
                    .content_key(&space_id, &content_key_id, ContentKeyPurpose::Content)
                    .map_err(|_| BlobCipherError::InvalidCiphertext)?;
                if resolved.epoch() != epoch {
                    return Err(BlobCipherError::InvalidCiphertext);
                }
                let bound_aad = key_epoch_aad::bind(
                    b"blob-json-v2",
                    &space_id,
                    epoch,
                    &content_key_id,
                    ContentKeyPurpose::Content,
                    aad.as_bytes(),
                );
                decrypt(resolved.key(), &blob.nonce, &blob.ciphertext, &bound_aad)?
            }
            _ => return Err(BlobCipherError::InvalidCiphertext),
        };
        Ok(Plaintext::new(plain))
    }
}

fn decrypt(
    key: &super::secrets::MasterKey,
    nonce: &[u8],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, BlobCipherError> {
    v1_aead::decrypt_blob_xchacha(key, nonce, ciphertext, aad).map_err(|error| match error {
        v1_aead::AeadError::InvalidKey => BlobCipherError::Internal(error.to_string()),
        v1_aead::AeadError::DecryptFailed => BlobCipherError::InvalidCiphertext,
        v1_aead::AeadError::EncryptFailed => BlobCipherError::Internal(error.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use uc_core::ids::SpaceId;

    use super::*;
    use crate::security::secrets::MasterKey;

    fn ready_session() -> (Arc<InMemorySession>, ActiveSpace, MasterKey) {
        let root = MasterKey::from_bytes(&[9u8; 32]).unwrap();
        let space_id = SpaceId::from_str("space-v2");
        let session = Arc::new(InMemorySession::new());
        session.set_master_key_for_space(space_id.clone(), root.clone());
        let material = session
            .create_migrated_space_material(&space_id, 100)
            .unwrap();
        session.install_space_material(&material).unwrap();
        (session, ActiveSpace::new(space_id), root)
    }

    #[tokio::test]
    async fn new_ciphertext_is_v2_and_round_trips() {
        let (session, space, _root) = ready_session();
        let adapter = BlobCipherAdapter::new(session);
        let aad = Aad::new(b"entry-aad".to_vec());
        let plaintext = Plaintext::new(b"secret".to_vec());

        let ciphertext = adapter.encrypt(&space, &plaintext, &aad).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(ciphertext.as_bytes()).unwrap();

        assert_eq!(value["version"], "V2");
        assert_eq!(value["group_epoch"], 1);
        assert_ne!(value["content_key_id"], "legacy-v1");
        assert_eq!(
            adapter
                .decrypt(&space, &ciphertext, &aad)
                .await
                .unwrap()
                .as_bytes(),
            b"secret"
        );
    }

    #[tokio::test]
    async fn legacy_v1_ciphertext_remains_readable() {
        let (session, space, root) = ready_session();
        let adapter = BlobCipherAdapter::new(session);
        let aad = Aad::new(b"legacy-aad".to_vec());
        let legacy = v1_aead::encrypt_blob_xchacha(&root, b"legacy", aad.as_bytes()).unwrap();
        let ciphertext = Ciphertext::new(serde_json::to_vec(&legacy).unwrap());

        assert_eq!(
            adapter
                .decrypt(&space, &ciphertext, &aad)
                .await
                .unwrap()
                .as_bytes(),
            b"legacy"
        );
    }

    #[tokio::test]
    async fn portable_catalog_keeps_legacy_content_key_separate_from_local_protection_root() {
        let sponsor_root = MasterKey::from_bytes(&[7u8; 32]).unwrap();
        let local_root = MasterKey::from_bytes(&[8u8; 32]).unwrap();
        let space_id = SpaceId::from_str("shared-space");

        let sponsor_session = Arc::new(InMemorySession::new());
        sponsor_session.set_master_key_for_space(space_id.clone(), sponsor_root.clone());
        let material = sponsor_session
            .create_migrated_space_material(&space_id, 100)
            .unwrap();

        let joiner_session = Arc::new(InMemorySession::new());
        joiner_session.set_master_key_for_space(space_id.clone(), local_root.clone());
        joiner_session.install_space_material(&material).unwrap();
        assert_eq!(
            joiner_session.get_master_key().unwrap().as_bytes(),
            local_root.as_bytes(),
            "installing a shared catalog must not replace the local protection root"
        );

        let aad = Aad::new(b"legacy-aad".to_vec());
        let legacy =
            v1_aead::encrypt_blob_xchacha(&sponsor_root, b"shared history", aad.as_bytes())
                .unwrap();
        let ciphertext = Ciphertext::new(serde_json::to_vec(&legacy).unwrap());
        let reader = BlobCipherAdapter::new(joiner_session);

        assert_eq!(
            reader
                .decrypt(&ActiveSpace::new(space_id), &ciphertext, &aad)
                .await
                .unwrap()
                .as_bytes(),
            b"shared history"
        );
    }

    #[tokio::test]
    async fn missing_v2_key_does_not_fall_back_to_legacy() {
        let (writer_session, space, root) = ready_session();
        let writer = BlobCipherAdapter::new(writer_session);
        let aad = Aad::new(b"entry-aad".to_vec());
        let ciphertext = writer
            .encrypt(&space, &Plaintext::new(b"secret".to_vec()), &aad)
            .await
            .unwrap();

        let reader_session = Arc::new(InMemorySession::new());
        reader_session.set_master_key_for_space(space.space_id().clone(), root);
        let reader = BlobCipherAdapter::new(reader_session);

        assert!(matches!(
            reader.decrypt(&space, &ciphertext, &aad).await,
            Err(BlobCipherError::InvalidCiphertext)
        ));
    }
}
