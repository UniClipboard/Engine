mod content_protection;
mod decrypting_clipboard_event_repo;
mod decrypting_representation_repo;
mod encrypted_blob_store;
mod encrypting_clipboard_event_writer;
mod encrypting_inbound_receive_commit;
mod profile_backup_archive;
mod profile_lifecycle;
mod profile_payload_adapters;
mod profile_reset;
mod profile_runtime_layout;
mod profile_startup_storage;
mod profile_storage_upgrade;
mod profile_upgrade_backup;
mod space_control_generation;
mod space_transition_activation;
mod v3_admission_space_transition;
mod v3_device_management_reset;
mod v3_initial_space_activation;
mod v3_membership_branch_transition;

pub use content_protection::V3EncryptedBlobStore;
pub use decrypting_clipboard_event_repo::DecryptingClipboardEventRepository;
pub use decrypting_representation_repo::DecryptingClipboardRepresentationRepository;
pub use encrypted_blob_store::EncryptedBlobStore;
pub use encrypting_clipboard_event_writer::EncryptingClipboardEventWriter;
pub use encrypting_inbound_receive_commit::EncryptingInboundReceiveCommit;
pub use profile_backup_archive::{
    ProfileArchiveReceipt, ProfileBackupArchive, ProfileBackupArchiveError, ProfileBackupSource,
};
pub use profile_lifecycle::ProfileLifecycleRepository;
pub use profile_payload_adapters::ProfilePayloadAdapters;
pub use profile_reset::{ProfileKeyWiper, ProfileStateCleaner};
pub use profile_runtime_layout::ProfileRuntimeLayout;
pub use profile_startup_storage::ProfileStartupStorage;
pub use profile_storage_upgrade::{
    ProfileStorageUpgrade, ProfileStorageUpgradeError, ProfileStorageUpgradeOutcome,
    StorageUpgradeFailure, StorageUpgradeObserver, StorageUpgradeProgressOutcome,
    StorageUpgradeSnapshot, StorageUpgradeStep, StorageUpgradeStepProgress, StorageUpgradeUnit,
};
pub use profile_upgrade_backup::{ProfileUpgradeBackupRecordKeyMissing, ProfileUpgradeBackupStore};
pub use space_control_generation::{
    AdmissionInputInconsistency, AdmissionInputIssue, PreparedSpaceControlGeneration,
    SpaceControlGeneration, SpaceControlGenerationError,
};
pub use space_transition_activation::{
    SpaceTransitionActivation, SpaceTransitionActivationError, SpaceTransitionActivationOutcome,
};
pub use uc_infra_crypto::crypto_model::{
    EncryptedBlob, KdfParams, KdfParamsV1, KeyScope, KeySlot, KeySlotConvertError, KeySlotFile,
    WrappedMasterKey,
};
pub use uc_infra_crypto::hashing::Blake3Hasher;
pub use uc_infra_crypto::identity_fingerprint::{
    FingerprintDerivationError, Sha256IdentityFingerprintFactory,
};
pub(crate) use uc_infra_crypto::secrets::{Kek, MasterKey};
pub use uc_infra_crypto::space_admission_auth::{
    SpaceAdmissionAuth, SpaceAdmissionAuthContext, SpaceAdmissionAuthError,
    SpaceAdmissionClientState, SpaceAdmissionContinuationCredential, SpaceAdmissionKe1,
    SpaceAdmissionKe2, SpaceAdmissionKe3, SpaceAdmissionPasswordEquivalent,
    SpaceAdmissionRegistration, SpaceAdmissionRegistrationEncoding, SpaceAdmissionServerSetup,
    SpaceAdmissionServerSetupEncoding, SpaceAdmissionServerState,
};
pub use uc_infra_security::{
    AdmissionKeyError, AdmissionKeyManager, BlobCipherAdapter, ContentProtection,
    ContentProtectionError, DefaultCurrentProfile, DefaultKeyMigrationAdapter,
    InstalledProfileCatalog, ProfileContentKeyVault, ProfileContentKeyVaultError,
    ResolvedProfileContentKey, V3InlinePayloadCipher, WrappedSpaceAdmissionDataKey,
};
pub(crate) use uc_infra_storage::EncryptionPassphraseChangeJournal;
pub use uc_infra_storage::{
    ActiveRuntimeManifest, ActiveRuntimeManifestV3, ActiveSpaceGenerationManifestStore,
    ActiveSpaceGenerationManifestStoreError,
};
pub use v3_admission_space_transition::V3AdmissionSpaceTransition;
pub use v3_device_management_reset::V3DeviceManagementReset;
pub use v3_initial_space_activation::V3InitialSpaceActivation;
pub use v3_membership_branch_transition::V3MembershipBranchTransition;

mod profile_key_recovery;
pub use profile_key_recovery::{
    ProfileKeyRecoveryError, ProfileKeyRecoveryStore, ProfilePassphraseRecoveryPort,
    ProfileRecoveryLosses, ProfileRecoveryOutcome, ProfileRecoveryPreparation,
    PROFILE_SECRET_FILE_NAME,
};
