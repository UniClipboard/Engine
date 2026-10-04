use serde::{Deserialize, Serialize};
use uc_core::crypto::model::EncryptionError;
use uc_core::membership::{
    AdmissionContentKeyCatalogV1, AdmissionContentKeyEntryV1, SpaceKeyMaterial,
};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::secrets::MasterKey;

/// 内容密钥目录，即 session/repository 唯一接受的持久格式。
///
/// 内容密钥只以不透明的 `MasterKey` 出入本模块；原始字节与 JSON 编码只在本模块内部出现。
pub struct ContentKeyCatalog {
    version: u8,
    entries: Vec<ContentKeyCatalogEntry>,
}

pub struct ContentKeyCatalogEntry {
    content_key_id: String,
    epoch: u64,
    key: MasterKey,
}

impl ContentKeyCatalogEntry {
    pub fn new(content_key_id: impl Into<String>, epoch: u64, key: MasterKey) -> Self {
        Self {
            content_key_id: content_key_id.into(),
            epoch,
            key,
        }
    }

    pub fn content_key_id(&self) -> &str {
        &self.content_key_id
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn key(&self) -> &MasterKey {
        &self.key
    }
}

impl ContentKeyCatalog {
    /// 新目录只写 V2；V1 只作为已持久化资料的读取兼容。
    pub fn v2(entries: Vec<ContentKeyCatalogEntry>) -> Self {
        Self {
            version: 2,
            entries,
        }
    }

    pub fn decode(encoded: &[u8]) -> Result<Self, EncryptionError> {
        let persisted = decode(encoded)?;
        let entries = persisted
            .entries
            .iter()
            .map(|entry| {
                MasterKey::from_bytes(&entry.key)
                    .map(|key| {
                        ContentKeyCatalogEntry::new(entry.content_key_id.clone(), entry.epoch, key)
                    })
                    .map_err(EncryptionError::key_material_corrupt_from)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            version: persisted.version,
            entries,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, EncryptionError> {
        encode(&PersistedContentKeyCatalog {
            version: self.version,
            entries: self
                .entries
                .iter()
                .map(|entry| PersistedContentKeyEntry {
                    content_key_id: entry.content_key_id.clone(),
                    epoch: entry.epoch,
                    key: entry.key.as_bytes().to_vec(),
                })
                .collect(),
        })
    }

    pub fn version(&self) -> u8 {
        self.version
    }

    pub fn entries(&self) -> &[ContentKeyCatalogEntry] {
        &self.entries
    }

    pub fn push(&mut self, entry: ContentKeyCatalogEntry) {
        self.entries.push(entry);
    }
}

/// 目录的磁盘格式。字段名、顺序与类型就是已发布的 JSON 格式，修改必须提升目录版本。
#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct PersistedContentKeyCatalog {
    version: u8,
    entries: Vec<PersistedContentKeyEntry>,
}

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct PersistedContentKeyEntry {
    content_key_id: String,
    epoch: u64,
    key: Vec<u8>,
}

fn decode(encoded: &[u8]) -> Result<PersistedContentKeyCatalog, EncryptionError> {
    serde_json::from_slice(encoded).map_err(EncryptionError::key_material_corrupt_from)
}

fn encode(catalog: &PersistedContentKeyCatalog) -> Result<Vec<u8>, EncryptionError> {
    serde_json::to_vec(catalog).map_err(EncryptionError::key_material_corrupt_from)
}

pub fn export_admission_content_key_catalog(
    material: &SpaceKeyMaterial,
) -> Result<AdmissionContentKeyCatalogV1, EncryptionError> {
    let catalog = decode(material.key_catalog())?;
    if catalog.version != 2 {
        return Err(EncryptionError::UnsupportedVersion);
    }
    let entries = catalog
        .entries
        .iter()
        .map(|entry| {
            AdmissionContentKeyEntryV1::new(
                entry.content_key_id.clone(),
                entry.epoch,
                entry.key.clone(),
            )
            .map_err(EncryptionError::key_material_corrupt_from)
        })
        .collect::<Result<Vec<_>, _>>()?;
    AdmissionContentKeyCatalogV1::new(
        material.state().current_content_key_id().as_str(),
        material.state().epoch().value(),
        entries,
    )
    .map_err(EncryptionError::key_material_corrupt_from)
}

/// 把已经通过 admission commitment 验证的目录转换为 session/repository
/// 唯一接受的 V2 持久格式。控制世代 owner 不复制该私有格式。
pub fn import_admission_content_key_catalog(
    catalog: &AdmissionContentKeyCatalogV1,
) -> Result<Vec<u8>, EncryptionError> {
    catalog
        .validate()
        .map_err(EncryptionError::key_material_corrupt_from)?;
    encode(&PersistedContentKeyCatalog {
        version: 2,
        entries: catalog
            .entries
            .iter()
            .map(|entry| PersistedContentKeyEntry {
                content_key_id: entry.content_key_id.clone(),
                epoch: entry.epoch,
                key: entry.key.clone(),
            })
            .collect(),
    })
}
