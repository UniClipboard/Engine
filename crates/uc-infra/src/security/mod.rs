mod active_space_generation_manifest_store;
mod admission_key_manager;
mod admission_proof;
mod admission_space_transition;
mod blob_cipher_adapter;
pub mod crypto_model;
mod decrypting_clipboard_event_repo;
mod decrypting_representation_repo;
mod default_current_profile;
mod encrypted_blob_store;
mod encrypting_clipboard_event_writer;
mod encrypting_inbound_receive_commit;
mod fail_closed_admission_space_transition;
mod hashing;
mod identity_fingerprint;
pub(crate) mod key_epoch_aad;
mod key_migration_adapter;
mod profile_lifecycle;
mod profile_reset;
mod secrets;
mod space_admission_auth;
pub(crate) mod v1_aead;

pub use active_space_generation_manifest_store::{
    ActiveSpaceGenerationManifestStore, ActiveSpaceGenerationManifestStoreError,
};
pub use admission_key_manager::{
    AdmissionKeyError, AdmissionKeyManager, WrappedSpaceAdmissionDataKey,
};
pub use admission_proof::HmacProofAdapter;
pub use admission_space_transition::{space_generation_directory, DurableAdmissionSpaceTransition};
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
pub use fail_closed_admission_space_transition::FailClosedAdmissionSpaceTransition;
pub use hashing::{hash_pin, verify_pin, Argon2PinHasher, Blake3Hasher};
pub use identity_fingerprint::{
    FingerprintDerivationError, Sha256IdentityFingerprintFactory, Sha256ShortCodeGenerator,
    ShortCodeGenerator,
};
pub use key_migration_adapter::DefaultKeyMigrationAdapter;
pub use profile_lifecycle::ProfileLifecycleRepository;
pub use profile_reset::{ProfileKeyWiper, ProfileStateCleaner};
pub(crate) use secrets::{Kek, MasterKey};
pub use space_admission_auth::{
    SpaceAdmissionAuth, SpaceAdmissionAuthContext, SpaceAdmissionAuthError,
    SpaceAdmissionClientState, SpaceAdmissionContinuationCredential, SpaceAdmissionKe1,
    SpaceAdmissionKe2, SpaceAdmissionKe3, SpaceAdmissionRegistration, SpaceAdmissionServerSetup,
    SpaceAdmissionServerState,
};
