mod diagnostics;
mod inventory;
mod record;
mod security_materials;
mod security_stream;
mod store;

pub(crate) use record::RECORD_KEY as PROFILE_UPGRADE_BACKUP_RECORD_KEY;
pub use store::ProfileUpgradeBackupStore;

#[derive(Debug, thiserror::Error)]
#[error("profile upgrade backup record key is missing")]
pub struct ProfileUpgradeBackupRecordKeyMissing;

#[cfg(test)]
mod tests;
