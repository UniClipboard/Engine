mod access;
mod active_space_security_session;
pub mod admission_key_manager;
mod admission_proof;
mod blob_cipher_adapter;
mod content_protection;
mod default_current_profile;
mod group_update_error;
pub use group_update_error::group_update_failure_detail;
mod key_material;
pub mod key_migration_adapter;
pub mod key_slot_store;
mod membership_update;
pub mod profile_content_key_vault;
pub mod profile_passphrase_recovery;
mod scope_identifier;
mod secure_storage_access;
mod session;
mod session_rebind;

pub use access::{MigrationSpaceAccessAdapter, RuntimeSpaceAccessAdapter};
pub use admission_key_manager::{
    AdmissionKeyError, AdmissionKeyManager, WrappedSpaceAdmissionDataKey,
};
pub use admission_proof::HmacProofAdapter;
pub use blob_cipher_adapter::BlobCipherAdapter;
pub use content_protection::{ContentProtection, ContentProtectionError, V3InlinePayloadCipher};
pub use default_current_profile::DefaultCurrentProfile;
pub use key_material::KeyMaterialStore;
pub use key_migration_adapter::DefaultKeyMigrationAdapter;
pub use membership_update::DefaultMembershipSecurityUpdateAdapter;
pub use profile_content_key_vault::ProfileKeyReadLease;
pub use profile_content_key_vault::{
    InstalledProfileCatalog, ProfileContentKeyVault, ProfileContentKeyVaultError,
    ResolvedProfileContentKey,
};
pub use profile_passphrase_recovery::{ProfileKeyRecoveryError, ProfilePassphraseRecoveryPort};
pub(crate) use secure_storage_access::SecureStorageAccess;
pub use session::InMemorySession;
pub use session_rebind::SpaceSessionRebindAdapter;

pub use uc_infra_crypto::secrets::{Kek, MasterKey};
