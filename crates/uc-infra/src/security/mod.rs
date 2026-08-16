mod admission_key_manager;
mod admission_proof;
mod admission_security_transition;
mod blob_cipher_adapter;
pub mod crypto_model;
mod decrypting_clipboard_event_repo;
mod decrypting_representation_repo;
mod default_current_profile;
mod encrypted_blob_store;
mod encrypting_clipboard_event_writer;
mod encrypting_inbound_receive_commit;
mod hashing;
mod historical_signature_adapter;
mod identity_fingerprint;
pub(crate) mod key_epoch_aad;
mod key_material;
mod key_migration_adapter;
pub(crate) mod legacy_upgrade;
mod membership_security_update_adapter;
pub(crate) mod mls_group;
mod peer_admission_adapter;
mod profile_lifecycle;
mod scope_identifier;
mod secrets;
mod session;
mod space_access_adapter;
pub(crate) mod v1_aead;

use std::sync::Arc;

pub use admission_key_manager::{
    AdmissionKeyError, AdmissionKeyManager, WrappedAdmissionAttemptDataKey,
};
pub use admission_proof::HmacProofAdapter;
pub use admission_security_transition::AdmissionSecurityTransitionAdapter;
pub use blob_cipher_adapter::BlobCipherAdapter;
pub use crypto_model::{
    EncryptedBlob, KdfParams, KdfParamsV1, KeyScope, KeySlot, KeySlotConvertError, KeySlotFile,
    WrappedMasterKey,
};
pub use decrypting_clipboard_event_repo::DecryptingClipboardEventRepository;
pub use decrypting_representation_repo::DecryptingClipboardRepresentationRepository;
pub use default_current_profile::DefaultCurrentProfile;
pub use encrypted_blob_store::EncryptedBlobStore;
pub use encrypting_clipboard_event_writer::EncryptingClipboardEventWriter;
pub use encrypting_inbound_receive_commit::EncryptingInboundReceiveCommit;
pub use hashing::{hash_pin, verify_pin, Argon2PinHasher, Blake3Hasher};
pub use historical_signature_adapter::OpenMlsHistoricalSignatureVerifier;
pub use identity_fingerprint::{
    FingerprintDerivationError, Sha256IdentityFingerprintFactory, Sha256ShortCodeGenerator,
    ShortCodeGenerator,
};
pub use key_material::KeyMaterialStore;
pub use key_migration_adapter::DefaultKeyMigrationAdapter;
pub use legacy_upgrade::DefaultLegacyProtection;
pub use membership_security_update_adapter::DefaultMembershipSecurityUpdateAdapter;
pub use peer_admission_adapter::MlsPeerAdmissionAdapter;
pub use profile_lifecycle::{
    FactoryResetPhaseV1, ProfileLifecycleError, ProfileLifecycleManager, ProfileLifecycleMarkerV1,
};
pub(crate) use secrets::MasterKey;
pub use session::InMemorySession;
pub use space_access_adapter::DefaultSpaceAccessAdapter;

pub fn default_legacy_protection(
    space_access: Arc<DefaultSpaceAccessAdapter>,
    attempt_store: Arc<
        crate::db::repositories::DieselSpaceSecurityStore<
            Arc<crate::db::executor::DieselSqliteExecutor>,
        >,
    >,
) -> DefaultLegacyProtection {
    DefaultLegacyProtection::new(space_access, attempt_store)
}
