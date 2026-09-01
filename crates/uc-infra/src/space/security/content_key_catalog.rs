use serde::{Deserialize, Serialize};
use uc_core::crypto::model::EncryptionError;
use uc_core::membership::{
    AdmissionContentKeyCatalogV1, AdmissionContentKeyEntryV1, SpaceKeyMaterial,
};
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub(super) struct PersistedContentKeyCatalog {
    pub(super) version: u8,
    pub(super) entries: Vec<PersistedContentKeyEntry>,
}

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub(super) struct PersistedContentKeyEntry {
    pub(super) content_key_id: String,
    pub(super) epoch: u64,
    pub(super) key: Vec<u8>,
}

pub(super) fn decode(encoded: &[u8]) -> Result<PersistedContentKeyCatalog, EncryptionError> {
    serde_json::from_slice(encoded).map_err(|_| EncryptionError::KeyMaterialCorrupt)
}

pub(super) fn encode(catalog: &PersistedContentKeyCatalog) -> Result<Vec<u8>, EncryptionError> {
    serde_json::to_vec(catalog).map_err(|_| EncryptionError::KeyMaterialCorrupt)
}

pub(crate) fn export_admission_content_key_catalog(
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
            .map_err(|_| EncryptionError::KeyMaterialCorrupt)
        })
        .collect::<Result<Vec<_>, _>>()?;
    AdmissionContentKeyCatalogV1::new(
        material.state().current_content_key_id().as_str(),
        material.state().epoch().value(),
        entries,
    )
    .map_err(|_| EncryptionError::KeyMaterialCorrupt)
}
