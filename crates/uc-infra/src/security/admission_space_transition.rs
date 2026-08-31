use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use diesel::{connection::SimpleConnection, Connection, OptionalExtension, RunQueryDsl};
use sha2::{Digest, Sha256};
use uc_application::deps::{
    AdmissionSpaceTransitionError, AdmissionSpaceTransitionPort,
    AdmissionSpaceTransitionPreparationV2, AdmissionSpaceTransitionStepV2,
    CommitMembershipLedgerPort, CurrentSpaceIdentityError, DeviceManagementResetDataPort,
    InitialSpaceActivationPort, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipLedgerMutation, PeerReconciliationRecord,
};
use uc_core::blob::ports::BlobReaderPort;
use uc_core::crypto::aad;
use uc_core::crypto::domain::{Aad, ActiveSpace, Ciphertext};
use uc_core::ids::{BlobId, DeviceId, EntryId, EventId, RepresentationId, SpaceId};
use uc_core::membership::{
    ActiveSpaceGenerationManifestV2, AdmissionContentKeyCatalogV1,
    AdmissionSpaceTransitionResultV2, AdmissionSpaceTransitionV2, ContentKeyId,
    CrossSpaceTransitionPhaseV2, CrossSpaceTransitionResultV2, CrossSpaceTransitionV2,
    FreshSpaceTransitionPhaseV1, FreshSpaceTransitionV1, GroupEpoch, ProtectionGroupId,
    RevocationRepositoryPort, SameSpaceTransitionPhaseV1, SameSpaceTransitionV1, SpaceKeyMaterial,
    SpaceKeyState, CROSS_SPACE_TRANSITION_FORMAT_V2, FRESH_SPACE_TRANSITION_FORMAT_V1,
    SAME_SPACE_TRANSITION_FORMAT_V1,
};
use uc_core::ports::security::current_profile::CurrentProfilePort;
use uc_core::ports::security::BlobCipherPort;
use uc_core::ports::space::{DeriveSpaceSubkeyPort, SpaceAccessError};
use uc_core::ports::PeerAddressRecord;
use uc_core::search::RenderKey;
use uc_core::{MemberSyncPreferences, SpaceMember, TrustedPeer};

use crate::blob::{BlobStorePort, FilesystemBlobStore, SwitchableFilesystemBlobStore};
use crate::config_migration::db_snapshot;
use crate::db::executor::DieselSqliteExecutor;
use crate::db::pool::{init_db_pool, DbPool};
use crate::db::repositories::active_clipboard_register_cipher::{
    ActiveClipboardRegisterCipher, CONSUMABLE_HKDF_INFO,
};
use crate::db::repositories::directory_publish_log_cipher::DirectoryPublishLogCipher;
use crate::db::repositories::entry_file_set_cipher::EntryFileSetPathCipher;
use crate::db::repositories::receive_artifact_cipher::ReceiveArtifactCipher;
use crate::db::repositories::{DieselSpaceSecurityStore, EncryptedRelationshipStore};
use crate::file_transfer::persistence_cipher::TransferPersistenceCipher;
use crate::search::render_payload::RenderPayloadCodec;

use super::{
    active_space_generation_manifest_store::DeviceManagementResetJournalV1,
    ActiveSpaceGenerationManifestStore, AdmissionKeyManager, BlobCipherAdapter, EncryptedBlobStore,
};
use crate::space::{
    install_prepared_registration, DefaultSpaceAccessAdapter, InMemorySession,
    SqliteMembershipLedger,
};

struct TargetSessionSubkeyDeriver(InMemorySession);

#[async_trait]
impl DeriveSpaceSubkeyPort for TargetSessionSubkeyDeriver {
    async fn derive_subkey(&self, salt: &[u8], info: &[u8]) -> Result<[u8; 32], SpaceAccessError> {
        self.0
            .derive_stable_subkey(salt, info)
            .map_err(|error| SpaceAccessError::Internal(error.to_string()))
    }
}

pub fn space_generation_directory(
    generation_root: &Path,
    space_id: &str,
    generation: &[u8; 16],
) -> PathBuf {
    generation_directory(generation_root, space_id.as_bytes(), generation)
}

fn generation_directory(root: &Path, identity: &[u8], generation: &[u8; 16]) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/space-generation-directory/v1\0");
    hasher.update(identity);
    hasher.update(generation);
    let digest: [u8; 32] = hasher.finalize().into();
    root.join(short_hex(&digest))
}

pub struct DurableAdmissionSpaceTransition {
    generations: SqliteSpaceGenerationStore,
    source_pool: DbPool,
    blob_store: Arc<SwitchableFilesystemBlobStore>,
    generation_manifest_store: Arc<ActiveSpaceGenerationManifestStore>,
    space_access: Arc<DefaultSpaceAccessAdapter>,
    session: Arc<InMemorySession>,
    current_profile: Arc<dyn CurrentProfilePort>,
    admission_keys: Arc<AdmissionKeyManager>,
    device_reset_source: tokio::sync::Mutex<Option<DeviceResetSource>>,
}

struct DeviceResetSource {
    target_space_id: SpaceId,
    reset_id: [u8; 32],
    target_generation: [u8; 16],
    source_space_id: SpaceId,
    source_session: Arc<InMemorySession>,
    prepared: PreparedSpaceGeneration,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct TargetWorkspaceGenerationV1 {
    format_version: u16,
    attempt_id: [u8; 32],
    target_space_id: String,
    security_commitment: uc_core::membership::AdmissionSecurityCommitmentV1,
    membership_history: Vec<u8>,
    security_state: Vec<u8>,
    protection_group_id: String,
    content_key_catalog: Vec<u8>,
    local_device_id: DeviceId,
    relationships: Vec<uc_core::membership::AdmissionChangeFacts>,
    relayed_group_updates: Vec<uc_core::membership::PendingGroupUpdate>,
    target_admission_credentials: Vec<u8>,
}

#[derive(serde::Serialize)]
struct TargetPersistedContentKeyCatalogV2 {
    version: u8,
    entries: Vec<TargetPersistedContentKeyEntryV2>,
}

#[derive(serde::Serialize)]
struct TargetPersistedContentKeyEntryV2 {
    content_key_id: String,
    epoch: u64,
    key: Vec<u8>,
}

fn valid_relayed_group_updates(
    updates: &[uc_core::membership::PendingGroupUpdate],
    local_device_id: &DeviceId,
    relationships: &[uc_core::membership::AdmissionChangeFacts],
) -> bool {
    let relationship_devices = relationships
        .iter()
        .map(|facts| &facts.device_id)
        .collect::<std::collections::BTreeSet<_>>();
    let mut update_ids = std::collections::BTreeSet::new();
    let mut recipients = std::collections::BTreeSet::new();
    updates.iter().all(|update| {
        update.recipient() != local_device_id
            && relationship_devices.contains(update.recipient())
            && !update.update_id().is_empty()
            && !update.payload().is_empty()
            && update_ids.insert(update.update_id())
            && recipients.insert(update.recipient())
    })
}

impl DurableAdmissionSpaceTransition {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source_pool: DbPool,
        source_blob_root: PathBuf,
        generation_root: PathBuf,
        profile_salt: Vec<u8>,
        blob_store: Arc<SwitchableFilesystemBlobStore>,
        generation_manifest_store: Arc<ActiveSpaceGenerationManifestStore>,
        space_access: Arc<DefaultSpaceAccessAdapter>,
        session: Arc<InMemorySession>,
        current_profile: Arc<dyn CurrentProfilePort>,
        admission_keys: Arc<AdmissionKeyManager>,
    ) -> Self {
        Self {
            generations: SqliteSpaceGenerationStore::new(
                source_pool.clone(),
                source_blob_root,
                generation_root,
                profile_salt,
            ),
            source_pool,
            blob_store,
            generation_manifest_store,
            space_access,
            session,
            current_profile,
            admission_keys,
            device_reset_source: tokio::sync::Mutex::new(None),
        }
    }

    fn device_reset_id(target_space_id: &SpaceId) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard/device-management-reset/v1\0");
        hasher.update(target_space_id.as_ref().as_bytes());
        hasher.finalize().into()
    }

    fn generation(&self, attempt_id: &[u8; 32], purpose: &[u8]) -> [u8; 16] {
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard/admission-space-generation/v1\0");
        hasher.update(attempt_id);
        hasher.update((purpose.len() as u64).to_be_bytes());
        hasher.update(purpose);
        let value: [u8; 32] = hasher.finalize().into();
        value[..16].try_into().unwrap_or([0; 16])
    }

    fn prepared_source(&self, transition: &CrossSpaceTransitionV2) -> PreparedSpaceGeneration {
        let directory = self.generations.generation_directory(
            transition.attempt_id.as_bytes(),
            &transition.source_generation,
        );
        PreparedSpaceGeneration {
            backup_path: directory.join("source-backup.sqlite"),
            backup_digest: transition.source_backup_digest,
        }
    }

    fn final_source(
        &self,
        transition: &CrossSpaceTransitionV2,
    ) -> Result<FinalSourceGeneration, AdmissionSpaceTransitionError> {
        let digest = transition
            .final_manifest_digest
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
        let directory = self.generations.generation_directory(
            transition.target_space_id.as_bytes(),
            &transition.target_generation,
        );
        Ok(FinalSourceGeneration {
            database_path: directory.join("source-final.sqlite"),
            digest,
        })
    }

    fn target_directory(&self, transition: &CrossSpaceTransitionV2) -> PathBuf {
        self.target_generation_directory(&transition.target_space_id, &transition.target_generation)
    }

    fn target_generation_directory(&self, space_id: &str, generation: &[u8; 16]) -> PathBuf {
        self.generations
            .generation_directory(space_id.as_bytes(), generation)
    }

    fn target_workspace_path(&self, target_space_id: &str, generation: &[u8; 16]) -> PathBuf {
        self.generations
            .generation_directory(target_space_id.as_bytes(), generation)
            .join("workspace-state.bin")
    }

    fn target_workspace_aad(target_space_id: &str, generation: &[u8; 16]) -> Vec<u8> {
        let mut aad = Vec::with_capacity(80 + target_space_id.len());
        aad.extend_from_slice(b"uniclipboard/target-workspace-generation/v1\0");
        aad.extend_from_slice(&(target_space_id.len() as u64).to_be_bytes());
        aad.extend_from_slice(target_space_id.as_bytes());
        aad.extend_from_slice(generation);
        aad
    }

    fn stage_target_workspace(
        &self,
        input: &AdmissionSpaceTransitionPreparationV2,
        target_generation: &[u8; 16],
        target_session: &InMemorySession,
    ) -> Result<Vec<u8>, AdmissionSpaceTransitionError> {
        input
            .target_security_commitment
            .validate()
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        if input.target_security_commitment.attempt_id != *input.attempt_id.as_bytes()
            || input.target_security_commitment.lineage_id != input.target_space_id
            || input.target_membership_history.is_empty()
            || input.target_security_state.is_empty()
            || input.target_admission_credentials.is_empty()
            || input.target_protection_group_id.is_empty()
            || input.target_protection_group_id.len() > 128
            || !input.target_protection_group_id.is_ascii()
            || input.target_relationships.is_empty()
            || !input
                .target_relationships
                .iter()
                .any(|facts| facts.device_id == input.local_device_id)
            || !valid_relayed_group_updates(
                &input.relayed_group_updates,
                &input.local_device_id,
                &input.target_relationships,
            )
        {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        let content_key_catalog = AdmissionContentKeyCatalogV1::decode(&input.target_key_catalog)
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        if content_key_catalog.target_epoch != input.target_security_commitment.target_epoch
            || content_key_catalog.digest() != input.target_security_commitment.key_catalog_digest
        {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        let state = TargetWorkspaceGenerationV1 {
            format_version: 1,
            attempt_id: *input.attempt_id.as_bytes(),
            target_space_id: input.target_space_id.clone(),
            security_commitment: input.target_security_commitment.clone(),
            membership_history: input.target_membership_history.clone(),
            security_state: input.target_security_state.clone(),
            protection_group_id: input.target_protection_group_id.clone(),
            content_key_catalog: input.target_key_catalog.clone(),
            local_device_id: input.local_device_id.clone(),
            relationships: input.target_relationships.clone(),
            relayed_group_updates: input.relayed_group_updates.clone(),
            target_admission_credentials: input.target_admission_credentials.clone(),
        };
        let plaintext =
            postcard::to_stdvec(&state).map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        let encrypted = super::v1_aead::encrypt_blob_xchacha(
            &target_session
                .get_master_key()
                .map_err(|_| AdmissionSpaceTransitionError::Locked)?,
            &plaintext,
            &Self::target_workspace_aad(&input.target_space_id, target_generation),
        )
        .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        let bytes = serde_json::to_vec(&encrypted)
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        let path = self.target_workspace_path(&input.target_space_id, target_generation);
        write_new_file(&path, &bytes).map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        let reference = digest(&bytes).to_vec();
        self.open_target_workspace(
            &input.target_space_id,
            target_generation,
            &reference,
            target_session,
        )?;
        Ok(reference)
    }

    fn open_target_workspace(
        &self,
        target_space_id: &str,
        generation: &[u8; 16],
        expected_reference: &[u8],
        target_session: &InMemorySession,
    ) -> Result<TargetWorkspaceGenerationV1, AdmissionSpaceTransitionError> {
        let bytes = std::fs::read(self.target_workspace_path(target_space_id, generation))
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        if digest(&bytes).as_slice() != expected_reference {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        let encrypted: super::crypto_model::EncryptedBlob = serde_json::from_slice(&bytes)
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        let plaintext = super::v1_aead::decrypt_blob_xchacha(
            &target_session
                .get_master_key()
                .map_err(|_| AdmissionSpaceTransitionError::Locked)?,
            &encrypted.nonce,
            &encrypted.ciphertext,
            &Self::target_workspace_aad(target_space_id, generation),
        )
        .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        let state: TargetWorkspaceGenerationV1 = postcard::from_bytes(&plaintext)
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        if state.format_version != 1
            || state.target_space_id != target_space_id
            || state.membership_history.is_empty()
            || state.security_state.is_empty()
            || state.target_admission_credentials.is_empty()
            || state.protection_group_id.is_empty()
            || state.protection_group_id.len() > 128
            || !state.protection_group_id.is_ascii()
            || state.relationships.is_empty()
            || !state
                .relationships
                .iter()
                .any(|facts| facts.device_id == state.local_device_id)
            || !valid_relayed_group_updates(
                &state.relayed_group_updates,
                &state.local_device_id,
                &state.relationships,
            )
            || state.security_commitment.lineage_id != target_space_id
        {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        let content_key_catalog = AdmissionContentKeyCatalogV1::decode(&state.content_key_catalog)
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        if content_key_catalog.target_epoch != state.security_commitment.target_epoch
            || content_key_catalog.digest() != state.security_commitment.key_catalog_digest
        {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        state
            .security_commitment
            .validate()
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        Ok(state)
    }

    fn active_generation_manifest(
        &self,
        transition: &CrossSpaceTransitionV2,
    ) -> Result<ActiveSpaceGenerationManifestV2, AdmissionSpaceTransitionError> {
        self.active_generation_manifest_for(
            transition.attempt_id,
            &transition.target_space_id,
            transition.target_generation,
        )
    }

    fn active_generation_manifest_for(
        &self,
        attempt_id: uc_core::membership::SpaceAdmissionId,
        target_space_id: &str,
        target_generation: [u8; 16],
    ) -> Result<ActiveSpaceGenerationManifestV2, AdmissionSpaceTransitionError> {
        ActiveSpaceGenerationManifestV2::new(
            target_space_id.to_owned(),
            self.generation(attempt_id.as_bytes(), b"target-keyslot"),
            target_generation,
            self.generation(attempt_id.as_bytes(), b"target-security"),
        )
        .ok_or(AdmissionSpaceTransitionError::Inconsistent)
    }

    async fn advance_fresh(
        &self,
        transition: &FreshSpaceTransitionV1,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError> {
        if !transition.validate() {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        let advanced = |phase| {
            let mut next = transition.clone();
            next.phase = phase;
            transition
                .can_advance_to(&next)
                .then_some(AdmissionSpaceTransitionStepV2::Advanced(
                    AdmissionSpaceTransitionV2::Fresh(next),
                ))
                .ok_or(AdmissionSpaceTransitionError::Inconsistent)
        };
        match transition.phase {
            FreshSpaceTransitionPhaseV1::TargetStaged => {
                advanced(FreshSpaceTransitionPhaseV1::ActivationStarted)
            }
            FreshSpaceTransitionPhaseV1::ActivationStarted => {
                let target_session = self
                    .space_access
                    .prepared_target_session(
                        &SpaceId::from_str(&transition.target_space_id),
                        &transition.target_keyslot_ref,
                    )
                    .await
                    .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
                let target_workspace = self.open_target_workspace(
                    &transition.target_space_id,
                    &transition.target_generation,
                    &transition.target_workspace_ref,
                    target_session.as_ref(),
                )?;
                let directory = self.target_generation_directory(
                    &transition.target_space_id,
                    &transition.target_generation,
                );
                let target_database = directory.join("target.sqlite");
                init_db_pool(
                    target_database
                        .to_str()
                        .ok_or(AdmissionSpaceTransitionError::Storage)?,
                )
                .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
                self.install_and_reopen_target_security(
                    &target_database,
                    &target_workspace,
                    target_session.as_ref(),
                )
                .await?;
                self.install_and_reopen_target_relationships(
                    &target_database,
                    &target_workspace,
                    target_session.as_ref(),
                )
                .await?;
                let manifest = self.active_generation_manifest_for(
                    transition.attempt_id,
                    &transition.target_space_id,
                    transition.target_generation,
                )?;
                self.install_target_membership_ledger(
                    &target_database,
                    &target_workspace,
                    &manifest,
                )
                .await?;
                copy_profile_recovery_state(&self.source_pool, &target_database)?;
                self.generation_manifest_store
                    .promote(&manifest)
                    .await
                    .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
                self.source_pool
                    .replace_database(
                        target_database
                            .to_str()
                            .ok_or(AdmissionSpaceTransitionError::Storage)?,
                    )
                    .map_err(|_| AdmissionSpaceTransitionError::RecoveryRequired)?;
                self.blob_store.replace_root(directory.join("blobs"));
                advanced(FreshSpaceTransitionPhaseV1::TargetPromoted)
            }
            FreshSpaceTransitionPhaseV1::TargetPromoted => {
                let directory = self.target_generation_directory(
                    &transition.target_space_id,
                    &transition.target_generation,
                );
                let target_database = directory.join("target.sqlite");
                self.source_pool
                    .replace_database(
                        target_database
                            .to_str()
                            .ok_or(AdmissionSpaceTransitionError::Storage)?,
                    )
                    .map_err(|_| AdmissionSpaceTransitionError::RecoveryRequired)?;
                self.blob_store.replace_root(directory.join("blobs"));
                self.space_access
                    .activate_prepared_target_access(
                        &SpaceId::from_str(&transition.target_space_id),
                        &transition.target_keyslot_ref,
                    )
                    .await
                    .map_err(|_| AdmissionSpaceTransitionError::RecoveryRequired)?;
                self.reopen_active_target_security(&transition.target_space_id)
                    .await?;
                advanced(FreshSpaceTransitionPhaseV1::CleanupPending)
            }
            FreshSpaceTransitionPhaseV1::CleanupPending => Ok(
                AdmissionSpaceTransitionStepV2::Finished(AdmissionSpaceTransitionResultV2::Fresh {
                    target_space_id: transition.target_space_id.clone(),
                }),
            ),
        }
    }

    async fn advance_same_space(
        &self,
        transition: &SameSpaceTransitionV1,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError> {
        if !transition.validate()
            || self.session.current_space_id().ok().as_ref()
                != Some(&SpaceId::from_str(&transition.target_space_id))
        {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        let advanced = |phase| {
            let mut next = transition.clone();
            next.phase = phase;
            transition
                .can_advance_to(&next)
                .then_some(AdmissionSpaceTransitionStepV2::Advanced(
                    AdmissionSpaceTransitionV2::SameSpace(next),
                ))
                .ok_or(AdmissionSpaceTransitionError::Inconsistent)
        };
        match transition.phase {
            SameSpaceTransitionPhaseV1::TargetStaged => {
                advanced(SameSpaceTransitionPhaseV1::ActivationStarted)
            }
            SameSpaceTransitionPhaseV1::ActivationStarted => {
                let target_workspace = self.open_target_workspace(
                    &transition.target_space_id,
                    &transition.target_generation,
                    &transition.target_workspace_ref,
                    self.session.as_ref(),
                )?;
                let target_material = self.same_space_key_material(&target_workspace).await?;
                let directory = self.target_generation_directory(
                    &transition.target_space_id,
                    &transition.target_generation,
                );
                std::fs::create_dir_all(&directory)
                    .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
                let scratch = directory.join("same-space.snapshot.tmp");
                let source_bytes = db_snapshot::snapshot_to_bytes(&self.source_pool, &scratch)
                    .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
                let target_database = directory.join("target.sqlite");
                write_new_file(&target_database, &source_bytes)
                    .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
                self.install_and_reopen_security_material(
                    &target_database,
                    &target_material,
                    self.session.as_ref(),
                )
                .await?;
                self.install_and_reopen_target_relationships(
                    &target_database,
                    &target_workspace,
                    self.session.as_ref(),
                )
                .await?;
                copy_directory_contents(
                    &self.generations.source_blob_root,
                    &directory.join("blobs"),
                )?;
                self.generation_manifest_store
                    .promote(&self.active_generation_manifest_for(
                        transition.attempt_id,
                        &transition.target_space_id,
                        transition.target_generation,
                    )?)
                    .await
                    .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
                self.source_pool
                    .replace_database(
                        target_database
                            .to_str()
                            .ok_or(AdmissionSpaceTransitionError::Storage)?,
                    )
                    .map_err(|_| AdmissionSpaceTransitionError::RecoveryRequired)?;
                self.blob_store.replace_root(directory.join("blobs"));
                advanced(SameSpaceTransitionPhaseV1::TargetPromoted)
            }
            SameSpaceTransitionPhaseV1::TargetPromoted => {
                let directory = self.target_generation_directory(
                    &transition.target_space_id,
                    &transition.target_generation,
                );
                let target_database = directory.join("target.sqlite");
                self.source_pool
                    .replace_database(
                        target_database
                            .to_str()
                            .ok_or(AdmissionSpaceTransitionError::Storage)?,
                    )
                    .map_err(|_| AdmissionSpaceTransitionError::RecoveryRequired)?;
                self.blob_store.replace_root(directory.join("blobs"));
                self.reopen_active_target_security(&transition.target_space_id)
                    .await?;
                advanced(SameSpaceTransitionPhaseV1::CleanupPending)
            }
            SameSpaceTransitionPhaseV1::CleanupPending => {
                Ok(AdmissionSpaceTransitionStepV2::Finished(
                    AdmissionSpaceTransitionResultV2::SameSpace {
                        target_space_id: transition.target_space_id.clone(),
                    },
                ))
            }
        }
    }

    async fn install_and_reopen_target_security(
        &self,
        target_database: &Path,
        target_workspace: &TargetWorkspaceGenerationV1,
        target_session: &InMemorySession,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let material = Self::target_space_material(target_workspace)?;
        self.install_and_reopen_security_material(target_database, &material, target_session)
            .await
    }

    async fn install_target_membership_ledger(
        &self,
        target_database: &Path,
        target_workspace: &TargetWorkspaceGenerationV1,
        manifest: &ActiveSpaceGenerationManifestV2,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let local_member_instance = target_workspace
            .relationships
            .iter()
            .filter(|facts| facts.device_id == target_workspace.local_device_id)
            .map(|facts| facts.member_instance)
            .next()
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
        let peer_reconciliation = target_workspace
            .relationships
            .iter()
            .filter(|facts| facts.device_id != target_workspace.local_device_id)
            .map(|facts| {
                (
                    facts.device_id.clone(),
                    PeerReconciliationRecord {
                        peer_device_id: facts.device_id.clone(),
                        relationship:
                            uc_core::membership::MembershipHistoryRelationship::Consistent,
                        confirmed_position: None,
                        sync_state: Default::default(),
                        restricted_delivery: Vec::new(),
                        updated_at_ms: 0,
                    },
                )
            })
            .collect();
        let target_pool = init_db_pool(
            target_database
                .to_str()
                .ok_or(AdmissionSpaceTransitionError::Storage)?,
        )
        .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        install_prepared_registration(
            &target_pool,
            self.admission_keys.as_ref(),
            manifest,
            &target_workspace.target_admission_credentials,
        )
        .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        let ledger = SqliteMembershipLedger::new(
            Arc::new(DieselSqliteExecutor::new(target_pool)),
            Arc::clone(&self.admission_keys),
        );
        let current = ledger
            .load()
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        let expected_revision = current.revision;
        let expected_history_digest = current
            .membership_history
            .as_deref()
            .map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)));
        ledger
            .compare_and_commit(MembershipLedgerMutation {
                expected_revision,
                expected_history_digest,
                replacement: LoadedMembershipLedger {
                    revision: expected_revision
                        .checked_add(1)
                        .ok_or(AdmissionSpaceTransitionError::Inconsistent)?,
                    lineage_id: Some(target_workspace.target_space_id.clone()),
                    membership_history: Some(target_workspace.membership_history.clone()),
                    local_device_id: Some(target_workspace.local_device_id.clone()),
                    local_member_instance: Some(local_member_instance),
                    local_join_active: true,
                    peer_reconciliation,
                    history_sync_cursor: None,
                    inbound_transfers: Default::default(),
                    completed_inbound_transfers: Default::default(),
                    pending_effects: Default::default(),
                    membership_conflicts: Default::default(),
                    membership_branch_transitions: Default::default(),
                    consumed_membership_recovery_nonces: Default::default(),
                    membership_branch_recovery_sessions: Default::default(),
                },
            })
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        Ok(())
    }

    async fn install_and_reopen_security_material(
        &self,
        target_database: &Path,
        material: &SpaceKeyMaterial,
        target_session: &InMemorySession,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let target_pool = init_db_pool(
            target_database
                .to_str()
                .ok_or(AdmissionSpaceTransitionError::Storage)?,
        )
        .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        clear_source_space_runtime_state(&target_pool)?;
        let repository = DieselSpaceSecurityStore::new(
            Arc::new(DieselSqliteExecutor::new(target_pool)),
            target_session.clone(),
        );
        repository
            .save_space_material(material)
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        let reopened = repository
            .load_space_material(material.state().space_id())
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
        if &reopened != material {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        target_session
            .install_space_material(&reopened)
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)
    }

    async fn same_space_key_material(
        &self,
        target_workspace: &TargetWorkspaceGenerationV1,
    ) -> Result<SpaceKeyMaterial, AdmissionSpaceTransitionError> {
        let catalog = AdmissionContentKeyCatalogV1::decode(&target_workspace.content_key_catalog)
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        let legacy = self
            .session
            .legacy_content_key()
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        if catalog
            .entries
            .iter()
            .find(|entry| entry.content_key_id == ContentKeyId::legacy_v1().as_str())
            .is_none_or(|entry| entry.key.as_slice() != legacy.as_bytes())
        {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        let current_repository = DieselSpaceSecurityStore::new(
            Arc::new(DieselSqliteExecutor::new(self.source_pool.clone())),
            self.session.as_ref().clone(),
        );
        let incoming = Self::target_space_material(target_workspace)?;
        if let Some(previous) = current_repository
            .load_space_material(&SpaceId::from_str(&target_workspace.target_space_id))
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?
        {
            return self
                .session
                .merge_space_material_history(&previous, incoming)
                .map_err(|_| AdmissionSpaceTransitionError::Inconsistent);
        }
        Ok(incoming)
    }

    fn target_space_material(
        target_workspace: &TargetWorkspaceGenerationV1,
    ) -> Result<SpaceKeyMaterial, AdmissionSpaceTransitionError> {
        let catalog = AdmissionContentKeyCatalogV1::decode(&target_workspace.content_key_catalog)
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        if catalog.target_epoch != target_workspace.security_commitment.target_epoch
            || catalog.digest() != target_workspace.security_commitment.key_catalog_digest
        {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        let current_content_key_id = ContentKeyId::from_string(&catalog.current_content_key_id)
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        let protection_group_id =
            ProtectionGroupId::from_string(&target_workspace.protection_group_id)
                .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        let state = SpaceKeyState::ready_for_admission(
            SpaceId::from_str(&target_workspace.target_space_id),
            GroupEpoch::new(catalog.target_epoch),
            current_content_key_id,
            protection_group_id,
        )
        .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        let persisted_catalog = TargetPersistedContentKeyCatalogV2 {
            version: 2,
            entries: catalog
                .entries
                .into_iter()
                .map(|entry| TargetPersistedContentKeyEntryV2 {
                    content_key_id: entry.content_key_id,
                    epoch: entry.epoch,
                    key: entry.key,
                })
                .collect(),
        };
        let key_catalog = serde_json::to_vec(&persisted_catalog)
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        let mut material = SpaceKeyMaterial::new(
            state,
            target_workspace.security_state.clone(),
            key_catalog,
            0,
        );
        material.add_pending_group_updates(target_workspace.relayed_group_updates.clone(), 0);
        Ok(material)
    }

    fn install_target_security_in_session(
        target_workspace: &TargetWorkspaceGenerationV1,
        target_session: &InMemorySession,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        target_session
            .install_space_material(&Self::target_space_material(target_workspace)?)
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)
    }

    async fn install_and_reopen_target_relationships(
        &self,
        target_database: &Path,
        target_workspace: &TargetWorkspaceGenerationV1,
        target_session: &InMemorySession,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let target_pool = init_db_pool(
            target_database
                .to_str()
                .ok_or(AdmissionSpaceTransitionError::Storage)?,
        )
        .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        let store = EncryptedRelationshipStore::new(
            Arc::new(DieselSqliteExecutor::new(target_pool)),
            Arc::new(TargetSessionSubkeyDeriver(target_session.clone())),
            Arc::clone(&self.current_profile),
        );
        let timestamp = Utc
            .timestamp_millis_opt(0)
            .single()
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
        for facts in &target_workspace.relationships {
            store
                .save_member(&SpaceMember {
                    device_id: facts.device_id.clone(),
                    device_name: facts.device_name.clone(),
                    identity_fingerprint: facts.identity_fingerprint.clone(),
                    joined_at: timestamp,
                    sync_preferences: MemberSyncPreferences::default(),
                })
                .await
                .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
            if facts.device_id != target_workspace.local_device_id {
                store
                    .save_trusted_peer(&TrustedPeer {
                        local_device_id: target_workspace.local_device_id.clone(),
                        peer_device_id: facts.device_id.clone(),
                        peer_fingerprint: facts.identity_fingerprint.clone(),
                        trusted_at: timestamp,
                    })
                    .await
                    .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
                store
                    .save_peer_address(&PeerAddressRecord {
                        device_id: facts.device_id.clone(),
                        addr_blob: facts.transport_address_blob.clone(),
                        observed_at: timestamp,
                    })
                    .await
                    .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
            }
        }
        let expected_members = target_workspace
            .relationships
            .iter()
            .map(|facts| facts.device_id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let stored_members = store
            .list_members()
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?
            .into_iter()
            .map(|member| member.device_id)
            .collect::<std::collections::BTreeSet<_>>();
        let expected_peers = expected_members
            .iter()
            .filter(|device| **device != target_workspace.local_device_id)
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let stored_peers = store
            .list_trusted_peers()
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?
            .into_iter()
            .map(|peer| peer.peer_device_id)
            .collect::<std::collections::BTreeSet<_>>();
        let stored_addresses = store
            .list_peer_addresses()
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?
            .into_iter()
            .map(|address| address.device_id)
            .collect::<std::collections::BTreeSet<_>>();
        if stored_members != expected_members
            || stored_peers != expected_peers
            || stored_addresses != expected_peers
        {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        Ok(())
    }

    async fn reopen_active_target_security(
        &self,
        target_space_id: &str,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let target_space = SpaceId::from_str(target_space_id);
        let repository = DieselSpaceSecurityStore::new(
            Arc::new(DieselSqliteExecutor::new(self.source_pool.clone())),
            self.session.as_ref().clone(),
        );
        let material = repository
            .load_space_material(&target_space)
            .await
            .map_err(|_| AdmissionSpaceTransitionError::RecoveryRequired)?
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
        self.session
            .install_space_material(&material)
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)
    }

    async fn ensure_source_session(
        &self,
        transition: &CrossSpaceTransitionV2,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        self.space_access
            .resume_source_for_transition(&SpaceId::from_str(&transition.source_space_id))
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Locked)
    }

    async fn target_session(
        &self,
        transition: &CrossSpaceTransitionV2,
    ) -> Result<Arc<InMemorySession>, AdmissionSpaceTransitionError> {
        self.space_access
            .prepared_target_session(
                &SpaceId::from_str(&transition.target_space_id),
                &transition.target_keyslot_ref,
            )
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)
    }

    fn advanced(
        &self,
        transition: &CrossSpaceTransitionV2,
        phase: CrossSpaceTransitionPhaseV2,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError> {
        let mut next = transition.clone();
        next.phase = phase;
        transition
            .can_advance_to(&next)
            .then_some(AdmissionSpaceTransitionStepV2::Advanced(
                AdmissionSpaceTransitionV2::CrossSpace(next),
            ))
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)
    }
}

#[async_trait]
impl DeviceManagementResetDataPort for DurableAdmissionSpaceTransition {
    async fn prepare_device_management_reset(
        &self,
        target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let active_generation_manifest = self
            .generation_manifest_store
            .load()
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        if active_generation_manifest
            .as_ref()
            .is_some_and(|manifest| manifest.space_id == target_space_id.as_ref())
        {
            return Ok(());
        }
        let mut source = self.device_reset_source.lock().await;
        if source
            .as_ref()
            .is_some_and(|source| &source.target_space_id == target_space_id)
        {
            return Ok(());
        }
        std::fs::create_dir_all(&self.generations.generation_root)
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        let required_space =
            reset_required_free_bytes(&self.source_pool, &self.generations.source_blob_root)?;
        ensure_reset_capacity(
            available_space_bytes(&self.generations.generation_root)?,
            required_space,
        )?;
        let source_space_id = self
            .session
            .current_space_id()
            .map_err(|_| AdmissionSpaceTransitionError::Locked)?;
        let reset_id = Self::device_reset_id(target_space_id);
        let source_generation = self.generation(&reset_id, b"device-reset-source");
        let target_generation = self.generation(&reset_id, b"device-reset-target-database");
        let journal = match self
            .generation_manifest_store
            .load_device_reset_journal()
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?
        {
            Some(journal)
                if journal.target_space_id == target_space_id.as_ref()
                    && journal.target_generation == target_generation =>
            {
                journal
            }
            Some(_) => return Err(AdmissionSpaceTransitionError::Inconsistent),
            None => DeviceManagementResetJournalV1 {
                format_version: 1,
                target_space_id: target_space_id.as_ref().to_owned(),
                target_generation,
                source_space_id: active_generation_manifest
                    .as_ref()
                    .map(|manifest| manifest.space_id.clone()),
                source_generation: active_generation_manifest
                    .as_ref()
                    .map(|manifest| manifest.database_generation),
            },
        };
        self.generation_manifest_store
            .save_device_reset_journal(&journal)
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        let reset_source_directory = self
            .generations
            .generation_directory(&reset_id, &source_generation);
        remove_file_if_present(&reset_source_directory.join("source-backup.sqlite"))?;
        remove_file_if_present(&reset_source_directory.join("source-backup.snapshot.tmp"))?;
        let prepared = self
            .generations
            .prepare_source(reset_id, source_generation)
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        *source = Some(DeviceResetSource {
            target_space_id: target_space_id.clone(),
            reset_id,
            target_generation,
            source_space_id,
            source_session: self.session.detached_clone(),
            prepared,
        });
        Ok(())
    }

    async fn stage_device_management_reset_mutations(
        &self,
        target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        if self
            .generation_manifest_store
            .load()
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?
            .is_some_and(|manifest| manifest.space_id == target_space_id.as_ref())
        {
            return Ok(());
        }
        let source = self.device_reset_source.lock().await;
        let source = source
            .as_ref()
            .filter(|source| &source.target_space_id == target_space_id)
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
        verify_existing_snapshot(&source.prepared.backup_path, source.prepared.backup_digest)
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        let target_directory = self.generations.generation_directory(
            target_space_id.as_ref().as_bytes(),
            &source.target_generation,
        );
        std::fs::create_dir_all(&target_directory)
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        let working_database = target_directory.join("reset-working.sqlite");
        remove_sqlite_database_if_present(&working_database)?;
        let source_bytes = std::fs::read(&source.prepared.backup_path)
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        write_new_file(&working_database, &source_bytes)
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        self.source_pool
            .replace_database(
                working_database
                    .to_str()
                    .ok_or(AdmissionSpaceTransitionError::Storage)?,
            )
            .map_err(|_| AdmissionSpaceTransitionError::RecoveryRequired)
    }

    async fn promote_device_management_reset(
        &self,
        target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        if self
            .generation_manifest_store
            .load()
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?
            .is_some_and(|manifest| manifest.space_id == target_space_id.as_ref())
        {
            return Ok(());
        }
        let mut source = self.device_reset_source.lock().await;
        let prepared = source
            .as_ref()
            .filter(|source| &source.target_space_id == target_space_id)
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
        if self.session.current_space_id().ok().as_ref() != Some(target_space_id) {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        let final_source = self
            .generations
            .finalize_source(
                PreparedSpaceGeneration {
                    backup_path: prepared.prepared.backup_path.clone(),
                    backup_digest: prepared.prepared.backup_digest,
                },
                target_space_id,
                prepared.target_generation,
            )
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        let target_directory = self.generations.generation_directory(
            target_space_id.as_ref().as_bytes(),
            &prepared.target_generation,
        );
        remove_file_if_present(&target_directory.join("target.sqlite"))?;
        remove_directory_if_present(&target_directory.join("blobs"))?;
        let finalized = self
            .generations
            .rewrap_finalized_source(
                final_source,
                &prepared.source_space_id,
                Arc::clone(&prepared.source_session),
                target_space_id,
                Arc::clone(&self.session),
                prepared.target_generation,
                false,
            )
            .await
            .map_err(|_| AdmissionSpaceTransitionError::RecoveryRequired)?;
        let manifest = ActiveSpaceGenerationManifestV2::new(
            target_space_id.as_ref().to_owned(),
            self.generation(&prepared.reset_id, b"device-reset-keyslot"),
            prepared.target_generation,
            self.generation(&prepared.reset_id, b"device-reset-security"),
        )
        .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
        self.generation_manifest_store
            .promote(&manifest)
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        self.source_pool
            .replace_database(
                finalized
                    .database_path
                    .to_str()
                    .ok_or(AdmissionSpaceTransitionError::Storage)?,
            )
            .map_err(|_| AdmissionSpaceTransitionError::RecoveryRequired)?;
        self.blob_store.replace_root(target_directory.join("blobs"));
        *source = None;
        Ok(())
    }

    async fn finalize_device_management_reset(
        &self,
        target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let active_generation_manifest = self
            .generation_manifest_store
            .load()
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
        if active_generation_manifest.space_id != target_space_id.as_ref() {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        let Some(journal) = self
            .generation_manifest_store
            .load_device_reset_journal()
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?
        else {
            return Ok(());
        };
        if journal.target_space_id != target_space_id.as_ref()
            || journal.target_generation != active_generation_manifest.database_generation
        {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        if let (Some(source_space_id), Some(source_generation)) =
            (&journal.source_space_id, journal.source_generation)
        {
            let source_directory = self
                .generations
                .generation_directory(source_space_id.as_bytes(), &source_generation);
            let target_directory = self.generations.generation_directory(
                target_space_id.as_ref().as_bytes(),
                &journal.target_generation,
            );
            if source_directory != target_directory {
                remove_directory_if_present(&source_directory)?;
            }
        }
        let reset_id = Self::device_reset_id(target_space_id);
        let reset_source_generation = self.generation(&reset_id, b"device-reset-source");
        let reset_source_directory = self
            .generations
            .generation_directory(&reset_id, &reset_source_generation);
        remove_directory_if_present(&reset_source_directory)?;
        let target_directory = self.generations.generation_directory(
            target_space_id.as_ref().as_bytes(),
            &journal.target_generation,
        );
        remove_file_if_present(&target_directory.join("source-final.sqlite"))?;
        remove_sqlite_database_if_present(&target_directory.join("reset-working.sqlite"))?;
        self.generation_manifest_store
            .clear_device_reset_journal()
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)
    }
}

#[async_trait]
impl InitialSpaceActivationPort for DurableAdmissionSpaceTransition {
    async fn activate_initial_space(
        &self,
        space_id: &SpaceId,
    ) -> Result<(), CurrentSpaceIdentityError> {
        // 新安装也必须从第一天使用 generation 布局。这里复用同一个持久化
        // 提升事务，但不执行成员重置：初始化流程刚刚写入的本机成员、安全
        // 状态和 ledger 会整体迁移，active manifest 最后才对读者可见。
        self.prepare_device_management_reset(space_id)
            .await
            .map_err(map_initial_activation_error)?;
        self.stage_device_management_reset_mutations(space_id)
            .await
            .map_err(map_initial_activation_error)?;
        self.promote_device_management_reset(space_id)
            .await
            .map_err(map_initial_activation_error)?;
        self.finalize_device_management_reset(space_id)
            .await
            .map_err(map_initial_activation_error)
    }
}

fn map_initial_activation_error(error: AdmissionSpaceTransitionError) -> CurrentSpaceIdentityError {
    match error {
        AdmissionSpaceTransitionError::Inconsistent => CurrentSpaceIdentityError::Inconsistent,
        _ => CurrentSpaceIdentityError::Unavailable,
    }
}

#[async_trait]
impl AdmissionSpaceTransitionPort for DurableAdmissionSpaceTransition {
    async fn preflight_source_history(
        &self,
        preserve_unreadable_history: bool,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let Some(source_space) = self.session.current_space_id().ok() else {
            return Ok(());
        };
        preflight_source_inline_history(
            &self.source_pool,
            &source_space,
            Arc::clone(&self.session),
            preserve_unreadable_history,
        )
        .await
    }

    async fn prepare_if_needed(
        &self,
        input: &AdmissionSpaceTransitionPreparationV2,
    ) -> Result<AdmissionSpaceTransitionV2, AdmissionSpaceTransitionError> {
        let active_generation_manifest = self
            .generation_manifest_store
            .load()
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        let target_generation = self.generation(input.attempt_id.as_bytes(), b"target-database");
        let source_space = self.session.current_space_id().ok();
        if source_space
            .as_ref()
            .is_some_and(|space| space.as_ref() == input.target_space_id)
        {
            let target_workspace_ref =
                self.stage_target_workspace(input, &target_generation, self.session.as_ref())?;
            let source_generation = active_generation_manifest
                .as_ref()
                .filter(|manifest| manifest.space_id == input.target_space_id)
                .map(|manifest| manifest.database_generation)
                .unwrap_or_else(|| self.generation(input.attempt_id.as_bytes(), b"legacy-source"));
            return Ok(AdmissionSpaceTransitionV2::SameSpace(
                SameSpaceTransitionV1 {
                    transition_format_version: SAME_SPACE_TRANSITION_FORMAT_V1,
                    attempt_id: input.attempt_id,
                    target_space_id: input.target_space_id.clone(),
                    source_generation,
                    target_generation,
                    target_keyslot_ref: b"retained-active-keyslot-v1".to_vec(),
                    target_workspace_ref,
                    phase: SameSpaceTransitionPhaseV1::TargetStaged,
                },
            ));
        }
        let target_session = self
            .space_access
            .prepared_target_session(
                &SpaceId::from_str(&input.target_space_id),
                &input.target_access_state,
            )
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        let target_workspace_ref =
            self.stage_target_workspace(input, &target_generation, target_session.as_ref())?;
        let source_space = match source_space {
            Some(source_space) => source_space,
            None => {
                if active_generation_manifest.is_some() {
                    return Err(AdmissionSpaceTransitionError::Locked);
                }
                let directory =
                    self.target_generation_directory(&input.target_space_id, &target_generation);
                let target_database = directory.join("target.sqlite");
                init_db_pool(
                    target_database
                        .to_str()
                        .ok_or(AdmissionSpaceTransitionError::Storage)?,
                )
                .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
                return Ok(AdmissionSpaceTransitionV2::Fresh(FreshSpaceTransitionV1 {
                    transition_format_version: FRESH_SPACE_TRANSITION_FORMAT_V1,
                    attempt_id: input.attempt_id,
                    target_space_id: input.target_space_id.clone(),
                    target_generation,
                    target_keyslot_ref: input.target_access_state.clone(),
                    target_workspace_ref,
                    phase: FreshSpaceTransitionPhaseV1::TargetStaged,
                }));
            }
        };
        let source_generation = active_generation_manifest
            .filter(|manifest| manifest.space_id == source_space.as_ref())
            .map(|manifest| manifest.database_generation)
            .unwrap_or_else(|| self.generation(input.attempt_id.as_bytes(), b"legacy-source"));
        let prepared = self
            .generations
            .prepare_source(*input.attempt_id.as_bytes(), source_generation)
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        let source_revision_at_backup = self
            .source_pool
            .persistent_revision()
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        Ok(AdmissionSpaceTransitionV2::CrossSpace(
            CrossSpaceTransitionV2 {
                transition_format_version: CROSS_SPACE_TRANSITION_FORMAT_V2,
                attempt_id: input.attempt_id,
                source_space_id: source_space.as_ref().to_owned(),
                source_generation,
                source_backup_ref: b"source-backup-v1".to_vec(),
                source_backup_digest: prepared.backup_digest,
                source_revision_at_backup,
                target_space_id: input.target_space_id.clone(),
                target_generation,
                target_keyslot_ref: input.target_access_state.clone(),
                target_workspace_ref,
                phase: CrossSpaceTransitionPhaseV2::TargetStaged,
                final_source_revision: None,
                final_manifest_digest: None,
                migrated_records: 0,
                preserved_unreadable_records: 0,
                preserve_unreadable_history: input.preserve_unreadable_history,
            },
        ))
    }

    async fn advance(
        &self,
        transition: &AdmissionSpaceTransitionV2,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError> {
        let transition = match transition {
            AdmissionSpaceTransitionV2::Fresh(transition) => {
                return self.advance_fresh(transition).await;
            }
            AdmissionSpaceTransitionV2::SameSpace(transition) => {
                return self.advance_same_space(transition).await;
            }
            AdmissionSpaceTransitionV2::CrossSpace(transition) => transition,
        };
        if !transition.validate() {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        match transition.phase {
            CrossSpaceTransitionPhaseV2::SourcePrepared => {
                Err(AdmissionSpaceTransitionError::Inconsistent)
            }
            CrossSpaceTransitionPhaseV2::TargetStaged => {
                self.ensure_source_session(transition).await?;
                self.advanced(transition, CrossSpaceTransitionPhaseV2::ActivationStarted)
            }
            CrossSpaceTransitionPhaseV2::ActivationStarted => {
                self.ensure_source_session(transition).await?;
                let finalized = self
                    .generations
                    .finalize_source(
                        self.prepared_source(transition),
                        &SpaceId::from_str(&transition.target_space_id),
                        transition.target_generation,
                    )
                    .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
                let mut next = transition.clone();
                next.phase = CrossSpaceTransitionPhaseV2::SourceFinalized;
                next.final_source_revision = Some(
                    self.source_pool
                        .persistent_revision()
                        .map_err(|_| AdmissionSpaceTransitionError::Storage)?,
                );
                next.final_manifest_digest = Some(finalized.digest);
                transition
                    .can_advance_to(&next)
                    .then_some(AdmissionSpaceTransitionStepV2::Advanced(
                        AdmissionSpaceTransitionV2::CrossSpace(next),
                    ))
                    .ok_or(AdmissionSpaceTransitionError::Inconsistent)
            }
            CrossSpaceTransitionPhaseV2::SourceFinalized => {
                self.ensure_source_session(transition).await?;
                let target_session = self.target_session(transition).await?;
                let target_workspace = self.open_target_workspace(
                    &transition.target_space_id,
                    &transition.target_generation,
                    &transition.target_workspace_ref,
                    target_session.as_ref(),
                )?;
                Self::install_target_security_in_session(
                    &target_workspace,
                    target_session.as_ref(),
                )?;
                let finalized = self
                    .generations
                    .rewrap_finalized_source(
                        self.final_source(transition)?,
                        &SpaceId::from_str(&transition.source_space_id),
                        Arc::clone(&self.session),
                        &SpaceId::from_str(&transition.target_space_id),
                        target_session,
                        transition.target_generation,
                        transition.preserve_unreadable_history,
                    )
                    .await
                    .map_err(|_| AdmissionSpaceTransitionError::RecoveryRequired)?;
                if finalized.source_digest != transition.final_manifest_digest.unwrap_or([0; 32])
                    || finalized.database_path
                        != self.target_directory(transition).join("target.sqlite")
                    || finalized.target_digest == [0; 32]
                {
                    return Err(AdmissionSpaceTransitionError::Inconsistent);
                }
                let mut next = transition.clone();
                next.phase = CrossSpaceTransitionPhaseV2::DataRewrapped;
                next.migrated_records = finalized.migrated_records;
                next.preserved_unreadable_records = finalized.preserved_unreadable_records;
                transition
                    .can_advance_to(&next)
                    .then_some(AdmissionSpaceTransitionStepV2::Advanced(
                        AdmissionSpaceTransitionV2::CrossSpace(next),
                    ))
                    .ok_or(AdmissionSpaceTransitionError::Inconsistent)
            }
            CrossSpaceTransitionPhaseV2::DataRewrapped => {
                let target_session = self.target_session(transition).await?;
                let target_workspace = self.open_target_workspace(
                    &transition.target_space_id,
                    &transition.target_generation,
                    &transition.target_workspace_ref,
                    target_session.as_ref(),
                )?;
                if target_workspace.attempt_id != *transition.attempt_id.as_bytes() {
                    return Err(AdmissionSpaceTransitionError::Inconsistent);
                }
                let directory = self.target_directory(transition);
                let target_database = directory.join("target.sqlite");
                self.install_and_reopen_target_security(
                    &target_database,
                    &target_workspace,
                    target_session.as_ref(),
                )
                .await?;
                self.install_and_reopen_target_relationships(
                    &target_database,
                    &target_workspace,
                    target_session.as_ref(),
                )
                .await?;
                let manifest = self.active_generation_manifest(transition)?;
                self.install_target_membership_ledger(
                    &target_database,
                    &target_workspace,
                    &manifest,
                )
                .await?;
                copy_profile_recovery_state(&self.source_pool, &target_database)?;
                self.generation_manifest_store
                    .promote(&manifest)
                    .await
                    .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
                self.source_pool
                    .replace_database(
                        target_database
                            .to_str()
                            .ok_or(AdmissionSpaceTransitionError::Storage)?,
                    )
                    .map_err(|_| AdmissionSpaceTransitionError::RecoveryRequired)?;
                self.blob_store.replace_root(directory.join("blobs"));
                self.advanced(transition, CrossSpaceTransitionPhaseV2::TargetPromoted)
            }
            CrossSpaceTransitionPhaseV2::TargetPromoted => {
                let directory = self.target_directory(transition);
                let target_database = directory.join("target.sqlite");
                self.source_pool
                    .replace_database(
                        target_database
                            .to_str()
                            .ok_or(AdmissionSpaceTransitionError::Storage)?,
                    )
                    .map_err(|_| AdmissionSpaceTransitionError::RecoveryRequired)?;
                self.blob_store.replace_root(directory.join("blobs"));
                self.space_access
                    .activate_prepared_target_access(
                        &SpaceId::from_str(&transition.target_space_id),
                        &transition.target_keyslot_ref,
                    )
                    .await
                    .map_err(|_| AdmissionSpaceTransitionError::RecoveryRequired)?;
                self.reopen_active_target_security(&transition.target_space_id)
                    .await?;
                self.advanced(transition, CrossSpaceTransitionPhaseV2::CleanupPending)
            }
            CrossSpaceTransitionPhaseV2::CleanupPending => {
                let prepared = self.prepared_source(transition);
                remove_file_if_present(&prepared.backup_path)?;
                remove_file_if_present(&self.final_source(transition)?.database_path)?;
                let result = CrossSpaceTransitionResultV2::from_cleanup_pending(transition)
                    .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
                Ok(AdmissionSpaceTransitionStepV2::Finished(
                    AdmissionSpaceTransitionResultV2::CrossSpace(result),
                ))
            }
        }
    }

    async fn discard_pre_activation(
        &self,
        transition: &AdmissionSpaceTransitionV2,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        if let AdmissionSpaceTransitionV2::Fresh(fresh) = transition {
            if !fresh.validate()
                || fresh.phase.rank() >= FreshSpaceTransitionPhaseV1::ActivationStarted.rank()
            {
                return Err(AdmissionSpaceTransitionError::Inconsistent);
            }
            if self
                .generation_manifest_store
                .load()
                .await
                .map_err(|_| AdmissionSpaceTransitionError::Storage)?
                .is_some_and(|manifest| manifest.space_id == fresh.target_space_id)
            {
                return Err(AdmissionSpaceTransitionError::RecoveryRequired);
            }
            let target =
                self.target_generation_directory(&fresh.target_space_id, &fresh.target_generation);
            return match std::fs::remove_dir_all(target) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err(AdmissionSpaceTransitionError::Storage),
            };
        }
        if let AdmissionSpaceTransitionV2::SameSpace(same) = transition {
            if !same.validate()
                || same.phase.rank() >= SameSpaceTransitionPhaseV1::ActivationStarted.rank()
            {
                return Err(AdmissionSpaceTransitionError::Inconsistent);
            }
            if self
                .generation_manifest_store
                .load()
                .await
                .map_err(|_| AdmissionSpaceTransitionError::Storage)?
                .is_some_and(|manifest| {
                    manifest.space_id == same.target_space_id
                        && manifest.database_generation == same.target_generation
                })
            {
                return Err(AdmissionSpaceTransitionError::RecoveryRequired);
            }
            let target =
                self.target_generation_directory(&same.target_space_id, &same.target_generation);
            return match std::fs::remove_dir_all(target) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err(AdmissionSpaceTransitionError::Storage),
            };
        }
        let AdmissionSpaceTransitionV2::CrossSpace(transition) = transition else {
            return Err(AdmissionSpaceTransitionError::Unavailable);
        };
        if !transition.validate()
            || transition.phase.rank() >= CrossSpaceTransitionPhaseV2::ActivationStarted.rank()
        {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        if self
            .generation_manifest_store
            .load()
            .await
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?
            .is_some_and(|manifest| manifest.space_id == transition.target_space_id)
        {
            return Err(AdmissionSpaceTransitionError::RecoveryRequired);
        }
        let prepared = self.prepared_source(transition);
        remove_file_if_present(&prepared.backup_path)?;
        let target = self.target_directory(transition);
        match std::fs::remove_dir_all(target) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(AdmissionSpaceTransitionError::Storage),
        }
    }
}

#[derive(diesel::QueryableByName)]
struct ProfileRecoveryRow {
    #[diesel(sql_type = diesel::sql_types::Binary)]
    encrypted_payload: Vec<u8>,
}

fn copy_profile_recovery_state(
    source_pool: &DbPool,
    target_database: &Path,
) -> Result<(), AdmissionSpaceTransitionError> {
    let row = diesel::sql_query(
        "SELECT encrypted_payload FROM admission_repository_state WHERE singleton_id = 1",
    )
    .get_result::<ProfileRecoveryRow>(
        &mut source_pool
            .get()
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?,
    )
    .optional()
    .map_err(|_| AdmissionSpaceTransitionError::Storage)?
    .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
    let target_pool = init_db_pool(
        target_database
            .to_str()
            .ok_or(AdmissionSpaceTransitionError::Storage)?,
    )
    .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
    diesel::sql_query(
        "INSERT INTO admission_repository_state (singleton_id, encrypted_payload) VALUES (1, ?) \
         ON CONFLICT(singleton_id) DO UPDATE SET encrypted_payload = excluded.encrypted_payload",
    )
    .bind::<diesel::sql_types::Binary, _>(row.encrypted_payload)
    .execute(
        &mut target_pool
            .get()
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?,
    )
    .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
    Ok(())
}

fn clear_source_space_runtime_state(
    target_pool: &DbPool,
) -> Result<(), AdmissionSpaceTransitionError> {
    target_pool
        .get()
        .map_err(|_| AdmissionSpaceTransitionError::Storage)?
        .immediate_transaction::<_, diesel::result::Error, _>(|connection| {
            connection.batch_execute(
                "DELETE FROM encrypted_relationship;
                 DELETE FROM member_revocation_log;
                 DELETE FROM legacy_space_bootstrap_log;
                 DELETE FROM space_key_epoch_state;
                 DELETE FROM workspace_convergence_v3_active;
                 DELETE FROM workspace_convergence_v3_slots;
                 DELETE FROM workspace_convergence_v3_migrations;
                 DELETE FROM workspace_convergence_state;",
            )
        })
        .map_err(|_| AdmissionSpaceTransitionError::Storage)
}

fn remove_file_if_present(path: &Path) -> Result<(), AdmissionSpaceTransitionError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(AdmissionSpaceTransitionError::Storage),
    }
}

fn remove_sqlite_database_if_present(path: &Path) -> Result<(), AdmissionSpaceTransitionError> {
    remove_file_if_present(path)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(AdmissionSpaceTransitionError::Storage)?;
    for suffix in ["-wal", "-shm"] {
        remove_file_if_present(&path.with_file_name(format!("{file_name}{suffix}")))?;
    }
    Ok(())
}

fn remove_directory_if_present(path: &Path) -> Result<(), AdmissionSpaceTransitionError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(AdmissionSpaceTransitionError::Storage),
    }
}

fn ensure_reset_capacity(
    available_bytes: u64,
    required_bytes: u64,
) -> Result<(), AdmissionSpaceTransitionError> {
    (available_bytes >= required_bytes)
        .then_some(())
        .ok_or(AdmissionSpaceTransitionError::InsufficientStorage)
}

fn reset_required_free_bytes(
    source_pool: &DbPool,
    source_blob_root: &Path,
) -> Result<u64, AdmissionSpaceTransitionError> {
    #[derive(diesel::QueryableByName)]
    struct PageCountRow {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        page_count: i64,
    }
    #[derive(diesel::QueryableByName)]
    struct PageSizeRow {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        page_size: i64,
    }

    let mut connection = source_pool
        .get()
        .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
    let page_count = diesel::sql_query("PRAGMA page_count")
        .get_result::<PageCountRow>(&mut connection)
        .map_err(|_| AdmissionSpaceTransitionError::Storage)?
        .page_count;
    let page_size = diesel::sql_query("PRAGMA page_size")
        .get_result::<PageSizeRow>(&mut connection)
        .map_err(|_| AdmissionSpaceTransitionError::Storage)?
        .page_size;
    let database_bytes = u64::try_from(page_count)
        .ok()
        .and_then(|count| {
            u64::try_from(page_size)
                .ok()
                .map(|size| count.saturating_mul(size))
        })
        .ok_or(AdmissionSpaceTransitionError::Storage)?;
    let blob_bytes = directory_file_bytes(source_blob_root)?;

    Ok(database_bytes
        .saturating_mul(5)
        .saturating_add(blob_bytes.saturating_mul(2))
        .saturating_add(64 * 1024 * 1024))
}

fn directory_file_bytes(path: &Path) -> Result<u64, AdmissionSpaceTransitionError> {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(_) => return Err(AdmissionSpaceTransitionError::Storage),
    };
    let mut total = 0_u64;
    for entry in entries {
        let entry = entry.map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        let file_type = entry
            .file_type()
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        if file_type.is_dir() {
            total = total.saturating_add(directory_file_bytes(&entry.path())?);
        } else if file_type.is_file() {
            total = total.saturating_add(
                entry
                    .metadata()
                    .map_err(|_| AdmissionSpaceTransitionError::Storage)?
                    .len(),
            );
        } else {
            return Err(AdmissionSpaceTransitionError::Storage);
        }
    }
    Ok(total)
}

#[cfg(unix)]
fn available_space_bytes(path: &Path) -> Result<u64, AdmissionSpaceTransitionError> {
    use std::os::unix::ffi::OsStrExt;

    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let result = unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        return Err(AdmissionSpaceTransitionError::Storage);
    }
    let stats = unsafe { stats.assume_init() };
    Ok(u64::from(stats.f_bavail).saturating_mul(u64::from(stats.f_frsize)))
}

#[cfg(windows)]
fn available_space_bytes(path: &Path) -> Result<u64, AdmissionSpaceTransitionError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    wide.push(0);
    let mut available = 0_u64;
    let result = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (result != 0)
        .then_some(available)
        .ok_or(AdmissionSpaceTransitionError::Storage)
}

fn copy_directory_contents(
    source: &Path,
    destination: &Path,
) -> Result<(), AdmissionSpaceTransitionError> {
    if source == destination {
        return Ok(());
    }
    std::fs::create_dir_all(destination).map_err(|_| AdmissionSpaceTransitionError::Storage)?;
    let entries = match std::fs::read_dir(source) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(AdmissionSpaceTransitionError::Storage),
    };
    for entry in entries {
        let entry = entry.map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        let file_type = entry
            .file_type()
            .map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_directory_contents(&entry.path(), &target)?;
        } else if file_type.is_file() {
            let bytes =
                std::fs::read(entry.path()).map_err(|_| AdmissionSpaceTransitionError::Storage)?;
            write_new_file(&target, &bytes).map_err(|_| AdmissionSpaceTransitionError::Storage)?;
        } else {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
    }
    Ok(())
}

pub(crate) struct PreparedSpaceGeneration {
    backup_path: PathBuf,
    backup_digest: [u8; 32],
}

pub(crate) struct FinalizedSpaceGeneration {
    pub(crate) database_path: PathBuf,
    pub(crate) source_digest: [u8; 32],
    pub(crate) target_digest: [u8; 32],
    pub(crate) migrated_records: u64,
    pub(crate) preserved_unreadable_records: u64,
}

pub(crate) struct FinalSourceGeneration {
    database_path: PathBuf,
    digest: [u8; 32],
}

pub(crate) struct SqliteSpaceGenerationStore {
    source_pool: DbPool,
    source_blob_root: PathBuf,
    generation_root: PathBuf,
    profile_salt: Vec<u8>,
}

impl SqliteSpaceGenerationStore {
    pub(crate) fn new(
        source_pool: DbPool,
        source_blob_root: PathBuf,
        generation_root: PathBuf,
        profile_salt: Vec<u8>,
    ) -> Self {
        Self {
            source_pool,
            source_blob_root,
            generation_root,
            profile_salt,
        }
    }

    pub(crate) fn prepare_source(
        &self,
        attempt_id: [u8; 32],
        source_generation: [u8; 16],
    ) -> Result<PreparedSpaceGeneration, String> {
        let directory = self.generation_directory(&attempt_id, &source_generation);
        std::fs::create_dir_all(&directory)
            .map_err(|_| "create generation directory".to_owned())?;
        let backup_path = directory.join("source-backup.sqlite");
        if let Ok(bytes) = std::fs::read(&backup_path) {
            return Ok(PreparedSpaceGeneration {
                backup_path,
                backup_digest: digest(&bytes),
            });
        }
        let scratch = directory.join("source-backup.snapshot.tmp");
        let bytes = db_snapshot::snapshot_to_bytes(&self.source_pool, &scratch)
            .map_err(|_| "snapshot source generation".to_owned())?;
        write_new_file(&backup_path, &bytes)?;
        Ok(PreparedSpaceGeneration {
            backup_path,
            backup_digest: digest(&bytes),
        })
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    pub(crate) async fn finalize_and_rewrap(
        &self,
        prepared: PreparedSpaceGeneration,
        source_space: &SpaceId,
        source_session: Arc<InMemorySession>,
        target_space: &SpaceId,
        target_session: Arc<InMemorySession>,
        target_generation: [u8; 16],
    ) -> Result<FinalizedSpaceGeneration, String> {
        let finalized_source = self.finalize_source(prepared, target_space, target_generation)?;
        self.rewrap_finalized_source(
            finalized_source,
            source_space,
            source_session,
            target_space,
            target_session,
            target_generation,
            false,
        )
        .await
    }

    pub(crate) fn finalize_source(
        &self,
        prepared: PreparedSpaceGeneration,
        target_space: &SpaceId,
        target_generation: [u8; 16],
    ) -> Result<FinalSourceGeneration, String> {
        verify_existing_snapshot(&prepared.backup_path, prepared.backup_digest)?;
        let directory =
            self.generation_directory(target_space.as_ref().as_bytes(), &target_generation);
        std::fs::create_dir_all(&directory).map_err(|_| "create target generation".to_owned())?;
        let scratch = directory.join("source-final.snapshot.tmp");
        let source_bytes = db_snapshot::snapshot_to_bytes(&self.source_pool, &scratch)
            .map_err(|_| "snapshot final source generation".to_owned())?;
        let database_path = directory.join("source-final.sqlite");
        remove_file_if_present(&database_path)
            .map_err(|_| "remove stale final source".to_owned())?;
        write_new_file(&database_path, &source_bytes)?;
        Ok(FinalSourceGeneration {
            database_path,
            digest: digest(&source_bytes),
        })
    }

    pub(crate) async fn rewrap_finalized_source(
        &self,
        finalized_source: FinalSourceGeneration,
        source_space: &SpaceId,
        source_session: Arc<InMemorySession>,
        target_space: &SpaceId,
        target_session: Arc<InMemorySession>,
        target_generation: [u8; 16],
        preserve_unreadable_history: bool,
    ) -> Result<FinalizedSpaceGeneration, String> {
        let source_bytes = std::fs::read(&finalized_source.database_path)
            .map_err(|_| "read final source generation".to_owned())?;
        if digest(&source_bytes) != finalized_source.digest {
            return Err("final source generation digest mismatch".to_owned());
        }
        let directory =
            self.generation_directory(target_space.as_ref().as_bytes(), &target_generation);
        let target_path = directory.join("target.sqlite");
        write_new_file(&target_path, &source_bytes)?;

        let target_pool = init_db_pool(
            target_path
                .to_str()
                .ok_or_else(|| "target generation path is invalid".to_owned())?,
        )
        .map_err(|_| "open target generation".to_owned())?;
        let rows = load_inline_rows(&target_pool)?;
        let source_cipher = BlobCipherAdapter::new(Arc::clone(&source_session));
        let target_cipher = BlobCipherAdapter::new(Arc::clone(&target_session));
        let mut rewrapped = Vec::with_capacity(rows.len());
        let mut preserved = Vec::new();
        for row in rows {
            let event_id = EventId::from_string(row.event_id.clone());
            let representation_id = RepresentationId::from(row.id.clone());
            let associated_data = Aad::from(aad::for_inline(&event_id, &representation_id));
            let plaintext = match source_cipher
                .decrypt(
                    &ActiveSpace::new(source_space.clone()),
                    &Ciphertext::new(row.inline_data),
                    &associated_data,
                )
                .await
            {
                Ok(plaintext) => plaintext,
                Err(uc_core::ports::security::BlobCipherError::InvalidCiphertext)
                    if preserve_unreadable_history =>
                {
                    preserved.push(row.id);
                    continue;
                }
                Err(_) => return Err("decrypt source representation".to_owned()),
            };
            let ciphertext = target_cipher
                .encrypt(
                    &ActiveSpace::new(target_space.clone()),
                    &plaintext,
                    &associated_data,
                )
                .await
                .map_err(|_| "encrypt target representation".to_owned())?;
            rewrapped.push((row.id, ciphertext.into_bytes()));
        }
        save_rewrapped_inline_rows(&target_pool, &rewrapped)?;
        mark_preserved_inline_rows(&target_pool, &preserved)?;
        let derived_records = rewrap_derived_payloads(
            &target_pool,
            &self.profile_salt,
            Arc::clone(&source_session),
            Arc::clone(&target_session),
        )?;
        let blob_records = self
            .rewrap_blobs(
                &target_pool,
                &target_path,
                source_session,
                Arc::clone(&target_session),
            )
            .await?;
        verify_rewrapped_inline_rows(&target_pool, target_space, target_session, &preserved)
            .await?;
        drop(target_pool);
        let target_bytes = std::fs::read(&target_path)
            .map_err(|_| "read verified target generation".to_owned())?;
        Ok(FinalizedSpaceGeneration {
            database_path: target_path,
            source_digest: finalized_source.digest,
            target_digest: digest(&target_bytes),
            migrated_records: rewrapped.len() as u64 + derived_records + blob_records,
            preserved_unreadable_records: preserved.len() as u64,
        })
    }

    fn generation_directory(&self, identity: &[u8], generation: &[u8; 16]) -> PathBuf {
        generation_directory(&self.generation_root, identity, generation)
    }

    async fn rewrap_blobs(
        &self,
        target_pool: &DbPool,
        target_database_path: &Path,
        source_session: Arc<InMemorySession>,
        target_session: Arc<InMemorySession>,
    ) -> Result<u64, String> {
        let source = EncryptedBlobStore::new(
            Arc::new(FilesystemBlobStore::new(self.source_blob_root.clone())),
            source_session,
        );
        let target_root = target_database_path
            .parent()
            .ok_or_else(|| "target database directory is missing".to_owned())?
            .join("blobs");
        let target = EncryptedBlobStore::new(
            Arc::new(FilesystemBlobStore::new(target_root)),
            target_session,
        );
        let mut connection = target_pool
            .get()
            .map_err(|_| "open target database for blobs".to_owned())?;
        let rows = diesel::sql_query("SELECT blob_id FROM blob ORDER BY blob_id")
            .load::<BlobGenerationRow>(&mut connection)
            .map_err(|_| "load target blob records".to_owned())?;
        let mut rewritten = Vec::with_capacity(rows.len());
        for row in rows {
            let blob_id = BlobId::from(row.blob_id.as_str());
            let plaintext = BlobReaderPort::get(&source, &blob_id)
                .await
                .map_err(|_| "read source blob".to_owned())?;
            let (storage_path, compressed_size) = target
                .put(&blob_id, &plaintext)
                .await
                .map_err(|_| "write target blob".to_owned())?;
            let verified = BlobReaderPort::get(&target, &blob_id)
                .await
                .map_err(|_| "reopen target blob".to_owned())?;
            if verified != plaintext {
                return Err("target blob verification mismatch".to_owned());
            }
            rewritten.push((
                row.blob_id,
                storage_path.to_string_lossy().into_owned(),
                compressed_size,
            ));
        }
        connection
            .transaction::<_, diesel::result::Error, _>(|connection| {
                for (blob_id, storage_path, compressed_size) in &rewritten {
                    diesel::sql_query(
                        "UPDATE blob SET storage_path = ?, compressed_size = ?, \
                         encryption_algo = 'xchacha20poly1305' WHERE blob_id = ?",
                    )
                    .bind::<diesel::sql_types::Text, _>(storage_path)
                    .bind::<diesel::sql_types::Nullable<diesel::sql_types::BigInt>, _>(
                        compressed_size,
                    )
                    .bind::<diesel::sql_types::Text, _>(blob_id)
                    .execute(connection)?;
                }
                Ok(())
            })
            .map_err(|_| "save target blob records".to_owned())?;
        Ok(rewritten.len() as u64)
    }
}

#[derive(diesel::QueryableByName)]
struct BlobGenerationRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    blob_id: String,
}

const FILE_SET_KEY_INFO: &[u8] = b"uniclipboard-file-set/v1";
const TRANSFER_METADATA_KEY_INFO: &[u8] = b"uniclipboard-file-transfer-metadata/v1";
const TRANSFER_EVENT_KEY_INFO: &[u8] = b"uniclipboard-file-transfer-events/v1";
const SEARCH_RENDER_KEY_INFO: &[u8] = b"uniclipboard-search-render/v1";
const DIRECTORY_PUBLISH_KEY_INFO: &[u8] = b"uniclipboard-directory-publish-log/v1";
const RECEIVE_ARTIFACT_KEY_INFO: &[u8] = b"uniclipboard-receive-artifact-log/v1";

#[derive(diesel::QueryableByName)]
struct FileSetGenerationRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    entry_id: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    line_index: i64,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Binary>)]
    original_text_ct: Option<Vec<u8>>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Binary>)]
    relative_path_ct: Option<Vec<u8>>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Binary>)]
    root_name_ct: Option<Vec<u8>>,
}

#[derive(diesel::QueryableByName)]
struct TransferGenerationRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    transfer_id: String,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    metadata_ciphertext: Vec<u8>,
}

#[derive(diesel::QueryableByName)]
struct SearchGenerationRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    entry_id: String,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    render_payload: Vec<u8>,
}

#[derive(diesel::QueryableByName)]
struct TransferEventGenerationRow {
    #[diesel(sql_type = diesel::sql_types::Integer)]
    id: i32,
    #[diesel(sql_type = diesel::sql_types::Text)]
    transfer_id: String,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    sequence: i32,
    #[diesel(sql_type = diesel::sql_types::Text)]
    event_type: String,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    payload_ciphertext: Vec<u8>,
}

#[derive(diesel::QueryableByName)]
struct ActiveRegisterGenerationRow {
    #[diesel(sql_type = diesel::sql_types::Integer)]
    id: i32,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    consumable_ref_ciphertext: Vec<u8>,
}

#[derive(diesel::QueryableByName)]
struct DirectoryPublishGenerationRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    entry_id: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    attempt_id: String,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    root_map_ciphertext: Vec<u8>,
}

#[derive(diesel::QueryableByName)]
struct ReceiveArtifactGenerationRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    entry_id: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    attempt_id: String,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    artifact_ciphertext: Vec<u8>,
}

struct RewrappedFileSetRow {
    entry_id: String,
    line_index: i64,
    original_text_ct: Option<Vec<u8>>,
    relative_path_ct: Option<Vec<u8>>,
    root_name_ct: Option<Vec<u8>>,
}

fn rewrap_derived_payloads(
    pool: &DbPool,
    profile_salt: &[u8],
    source_session: Arc<InMemorySession>,
    target_session: Arc<InMemorySession>,
) -> Result<u64, String> {
    let source_file_set = EntryFileSetPathCipher::new(
        source_session
            .derive_stable_subkey(profile_salt, FILE_SET_KEY_INFO)
            .map_err(|_| "derive source file-set key".to_owned())?,
    );
    let target_file_set = EntryFileSetPathCipher::new(
        target_session
            .derive_stable_subkey(profile_salt, FILE_SET_KEY_INFO)
            .map_err(|_| "derive target file-set key".to_owned())?,
    );
    let source_transfer = TransferPersistenceCipher::new(
        source_session
            .derive_stable_subkey(profile_salt, TRANSFER_METADATA_KEY_INFO)
            .map_err(|_| "derive source transfer metadata key".to_owned())?,
        source_session
            .derive_stable_subkey(profile_salt, TRANSFER_EVENT_KEY_INFO)
            .map_err(|_| "derive source transfer event key".to_owned())?,
    );
    let target_transfer = TransferPersistenceCipher::new(
        target_session
            .derive_stable_subkey(profile_salt, TRANSFER_METADATA_KEY_INFO)
            .map_err(|_| "derive target transfer metadata key".to_owned())?,
        target_session
            .derive_stable_subkey(profile_salt, TRANSFER_EVENT_KEY_INFO)
            .map_err(|_| "derive target transfer event key".to_owned())?,
    );
    let source_render = RenderPayloadCodec::new(
        RenderKey::from_bytes(
            &source_session
                .derive_stable_subkey(profile_salt, SEARCH_RENDER_KEY_INFO)
                .map_err(|_| "derive source search render key".to_owned())?,
        )
        .map_err(|_| "construct source search render key".to_owned())?,
    );
    let target_render = RenderPayloadCodec::new(
        RenderKey::from_bytes(
            &target_session
                .derive_stable_subkey(profile_salt, SEARCH_RENDER_KEY_INFO)
                .map_err(|_| "derive target search render key".to_owned())?,
        )
        .map_err(|_| "construct target search render key".to_owned())?,
    );
    let source_active = ActiveClipboardRegisterCipher::new(
        source_session
            .derive_stable_subkey(profile_salt, CONSUMABLE_HKDF_INFO)
            .map_err(|_| "derive source active register key".to_owned())?,
    );
    let target_active = ActiveClipboardRegisterCipher::new(
        target_session
            .derive_stable_subkey(profile_salt, CONSUMABLE_HKDF_INFO)
            .map_err(|_| "derive target active register key".to_owned())?,
    );
    let source_publish = DirectoryPublishLogCipher::new(
        source_session
            .derive_stable_subkey(profile_salt, DIRECTORY_PUBLISH_KEY_INFO)
            .map_err(|_| "derive source directory publish key".to_owned())?,
    );
    let target_publish = DirectoryPublishLogCipher::new(
        target_session
            .derive_stable_subkey(profile_salt, DIRECTORY_PUBLISH_KEY_INFO)
            .map_err(|_| "derive target directory publish key".to_owned())?,
    );
    let source_artifact = ReceiveArtifactCipher::new(
        source_session
            .derive_stable_subkey(profile_salt, RECEIVE_ARTIFACT_KEY_INFO)
            .map_err(|_| "derive source receive artifact key".to_owned())?,
    );
    let target_artifact = ReceiveArtifactCipher::new(
        target_session
            .derive_stable_subkey(profile_salt, RECEIVE_ARTIFACT_KEY_INFO)
            .map_err(|_| "derive target receive artifact key".to_owned())?,
    );

    let mut connection = pool
        .get()
        .map_err(|_| "open target database for derived payloads".to_owned())?;
    let file_rows = diesel::sql_query(
        "SELECT entry_id, line_index, original_text_ct, relative_path_ct, root_name_ct \
         FROM entry_file_set ORDER BY entry_id, line_index",
    )
    .load::<FileSetGenerationRow>(&mut connection)
    .map_err(|_| "load target file-set paths".to_owned())?;
    let transfer_rows = diesel::sql_query(
        "SELECT transfer_id, metadata_ciphertext FROM file_transfer ORDER BY transfer_id",
    )
    .load::<TransferGenerationRow>(&mut connection)
    .map_err(|_| "load target transfer metadata".to_owned())?;
    let search_rows = diesel::sql_query(
        "SELECT entry_id, render_payload FROM search_document \
         WHERE render_payload IS NOT NULL ORDER BY profile_id, entry_id",
    )
    .load::<SearchGenerationRow>(&mut connection)
    .map_err(|_| "load target search render payloads".to_owned())?;
    let transfer_event_rows = diesel::sql_query(
        "SELECT id, transfer_id, sequence, event_type, payload_ciphertext \
         FROM file_transfer_events ORDER BY id",
    )
    .load::<TransferEventGenerationRow>(&mut connection)
    .map_err(|_| "load target transfer events".to_owned())?;
    let active_rows = diesel::sql_query(
        "SELECT id, consumable_ref_ciphertext FROM active_clipboard_register \
         WHERE consumable_ref_ciphertext IS NOT NULL ORDER BY id",
    )
    .load::<ActiveRegisterGenerationRow>(&mut connection)
    .map_err(|_| "load target active register".to_owned())?;
    let publish_rows = diesel::sql_query(
        "SELECT entry_id, attempt_id, root_map_ciphertext FROM directory_publish_log \
         WHERE root_map_ciphertext IS NOT NULL ORDER BY entry_id, attempt_id",
    )
    .load::<DirectoryPublishGenerationRow>(&mut connection)
    .map_err(|_| "load target directory publish records".to_owned())?;
    let artifact_rows = diesel::sql_query(
        "SELECT entry_id, attempt_id, artifact_ciphertext FROM receive_artifact_log \
         ORDER BY entry_id, attempt_id",
    )
    .load::<ReceiveArtifactGenerationRow>(&mut connection)
    .map_err(|_| "load target receive artifact records".to_owned())?;

    let mut rewrapped_file_rows = Vec::with_capacity(file_rows.len());
    for row in file_rows {
        let entry_id = EntryId::from(row.entry_id.as_str());
        let original_text_ct = row
            .original_text_ct
            .map(|ciphertext| {
                source_file_set
                    .open_original_text(&entry_id, row.line_index, &ciphertext)
                    .map_err(|_| "open source file-set original path".to_owned())
                    .and_then(|plaintext| {
                        target_file_set
                            .seal_original_text(&entry_id, row.line_index, &plaintext)
                            .map_err(|_| "seal target file-set original path".to_owned())
                    })
            })
            .transpose()?;
        let relative_path_ct = row
            .relative_path_ct
            .map(|ciphertext| {
                source_file_set
                    .open_relative_path(&entry_id, row.line_index, &ciphertext)
                    .map_err(|_| "open source file-set relative path".to_owned())
                    .and_then(|plaintext| {
                        target_file_set
                            .seal_relative_path(&entry_id, row.line_index, &plaintext)
                            .map_err(|_| "seal target file-set relative path".to_owned())
                    })
            })
            .transpose()?;
        let root_name_ct = row
            .root_name_ct
            .map(|ciphertext| {
                source_file_set
                    .open_root_name(&entry_id, row.line_index, &ciphertext)
                    .map_err(|_| "open source file-set root name".to_owned())
                    .and_then(|plaintext| {
                        target_file_set
                            .seal_root_name(&entry_id, row.line_index, &plaintext)
                            .map_err(|_| "seal target file-set root name".to_owned())
                    })
            })
            .transpose()?;
        rewrapped_file_rows.push(RewrappedFileSetRow {
            entry_id: row.entry_id,
            line_index: row.line_index,
            original_text_ct,
            relative_path_ct,
            root_name_ct,
        });
    }
    let rewrapped_transfers = transfer_rows
        .into_iter()
        .map(|row| {
            let metadata = source_transfer
                .open_metadata(&row.transfer_id, &row.metadata_ciphertext)
                .map_err(|_| "open source transfer metadata".to_owned())?;
            let ciphertext = target_transfer
                .seal_metadata(&row.transfer_id, &metadata)
                .map_err(|_| "seal target transfer metadata".to_owned())?;
            Ok((row.transfer_id, ciphertext))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let rewrapped_search = search_rows
        .into_iter()
        .map(|row| {
            let entry_id = EntryId::from(row.entry_id.as_str());
            let fields = source_render
                .decrypt(&entry_id, &row.render_payload)
                .map_err(|_| "open source search render payload".to_owned())?;
            let ciphertext = target_render
                .encrypt(&entry_id, &fields)
                .map_err(|_| "seal target search render payload".to_owned())?;
            Ok((row.entry_id, ciphertext))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let rewrapped_transfer_events = transfer_event_rows
        .into_iter()
        .map(|row| {
            let event = source_transfer
                .open_event(
                    &row.transfer_id,
                    row.sequence,
                    &row.event_type,
                    &row.payload_ciphertext,
                )
                .map_err(|_| "open source transfer event".to_owned())?;
            let ciphertext = target_transfer
                .seal_event(&row.transfer_id, row.sequence, &row.event_type, &event)
                .map_err(|_| "seal target transfer event".to_owned())?;
            Ok((row.id, ciphertext))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let rewrapped_active = active_rows
        .into_iter()
        .map(|row| {
            let reference = source_active
                .open(&row.consumable_ref_ciphertext)
                .map_err(|_| "open source active register".to_owned())?;
            let ciphertext = target_active
                .seal(&reference)
                .map_err(|_| "seal target active register".to_owned())?;
            Ok((row.id, ciphertext))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let rewrapped_publish = publish_rows
        .into_iter()
        .map(|row| {
            let entry_id = EntryId::from(row.entry_id.as_str());
            let roots = source_publish
                .open(&entry_id, &row.attempt_id, &row.root_map_ciphertext)
                .map_err(|_| "open source directory publish record".to_owned())?;
            let ciphertext = target_publish
                .seal(&entry_id, &row.attempt_id, &roots)
                .map_err(|_| "seal target directory publish record".to_owned())?;
            Ok((row.entry_id, row.attempt_id, ciphertext))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let rewrapped_artifacts = artifact_rows
        .into_iter()
        .map(|row| {
            let artifacts = source_artifact
                .open(&row.entry_id, &row.attempt_id, &row.artifact_ciphertext)
                .map_err(|_| "open source receive artifact record".to_owned())?;
            let ciphertext = target_artifact
                .seal(&row.entry_id, &row.attempt_id, &artifacts)
                .map_err(|_| "seal target receive artifact record".to_owned())?;
            Ok((row.entry_id, row.attempt_id, ciphertext))
        })
        .collect::<Result<Vec<_>, String>>()?;

    let count = rewrapped_file_rows.len()
        + rewrapped_transfers.len()
        + rewrapped_search.len()
        + rewrapped_transfer_events.len()
        + rewrapped_active.len()
        + rewrapped_publish.len()
        + rewrapped_artifacts.len();
    connection
        .transaction::<_, diesel::result::Error, _>(|connection| {
            for row in &rewrapped_file_rows {
                diesel::sql_query(
                    "UPDATE entry_file_set SET original_text_ct = ?, relative_path_ct = ?, \
                     root_name_ct = ? WHERE entry_id = ? AND line_index = ?",
                )
                .bind::<diesel::sql_types::Nullable<diesel::sql_types::Binary>, _>(
                    row.original_text_ct.as_deref(),
                )
                .bind::<diesel::sql_types::Nullable<diesel::sql_types::Binary>, _>(
                    row.relative_path_ct.as_deref(),
                )
                .bind::<diesel::sql_types::Nullable<diesel::sql_types::Binary>, _>(
                    row.root_name_ct.as_deref(),
                )
                .bind::<diesel::sql_types::Text, _>(&row.entry_id)
                .bind::<diesel::sql_types::BigInt, _>(row.line_index)
                .execute(connection)?;
            }
            for (transfer_id, ciphertext) in &rewrapped_transfers {
                diesel::sql_query(
                    "UPDATE file_transfer SET metadata_ciphertext = ? WHERE transfer_id = ?",
                )
                .bind::<diesel::sql_types::Binary, _>(ciphertext)
                .bind::<diesel::sql_types::Text, _>(transfer_id)
                .execute(connection)?;
            }
            for (entry_id, ciphertext) in &rewrapped_search {
                diesel::sql_query(
                    "UPDATE search_document SET render_payload = ? WHERE entry_id = ?",
                )
                .bind::<diesel::sql_types::Binary, _>(ciphertext)
                .bind::<diesel::sql_types::Text, _>(entry_id)
                .execute(connection)?;
            }
            for (id, ciphertext) in &rewrapped_transfer_events {
                diesel::sql_query(
                    "UPDATE file_transfer_events SET payload_ciphertext = ? WHERE id = ?",
                )
                .bind::<diesel::sql_types::Binary, _>(ciphertext)
                .bind::<diesel::sql_types::Integer, _>(id)
                .execute(connection)?;
            }
            for (id, ciphertext) in &rewrapped_active {
                diesel::sql_query(
                    "UPDATE active_clipboard_register SET consumable_ref_ciphertext = ? WHERE id = ?",
                )
                .bind::<diesel::sql_types::Binary, _>(ciphertext)
                .bind::<diesel::sql_types::Integer, _>(id)
                .execute(connection)?;
            }
            for (entry_id, attempt_id, ciphertext) in &rewrapped_publish {
                diesel::sql_query(
                    "UPDATE directory_publish_log SET root_map_ciphertext = ? \
                     WHERE entry_id = ? AND attempt_id = ?",
                )
                .bind::<diesel::sql_types::Binary, _>(ciphertext)
                .bind::<diesel::sql_types::Text, _>(entry_id)
                .bind::<diesel::sql_types::Text, _>(attempt_id)
                .execute(connection)?;
            }
            for (entry_id, attempt_id, ciphertext) in &rewrapped_artifacts {
                diesel::sql_query(
                    "UPDATE receive_artifact_log SET artifact_ciphertext = ? \
                     WHERE entry_id = ? AND attempt_id = ?",
                )
                .bind::<diesel::sql_types::Binary, _>(ciphertext)
                .bind::<diesel::sql_types::Text, _>(entry_id)
                .bind::<diesel::sql_types::Text, _>(attempt_id)
                .execute(connection)?;
            }
            diesel::sql_query("DELETE FROM search_posting").execute(connection)?;
            diesel::sql_query("DELETE FROM search_entry_tag").execute(connection)?;
            diesel::sql_query(
                "UPDATE search_index_meta SET search_blocked = 1, \
                 last_rebuild_started_at_ms = NULL, last_rebuild_completed_at_ms = NULL",
            )
            .execute(connection)?;
            Ok(())
        })
        .map_err(|_| "save target derived payloads".to_owned())?;
    Ok(count as u64)
}

#[derive(diesel::QueryableByName)]
struct InlineGenerationRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    id: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    event_id: String,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    inline_data: Vec<u8>,
}

fn load_inline_rows(pool: &DbPool) -> Result<Vec<InlineGenerationRow>, String> {
    diesel::sql_query(
        "SELECT id, event_id, inline_data FROM clipboard_snapshot_representation \
         WHERE inline_data IS NOT NULL ORDER BY id",
    )
    .load::<InlineGenerationRow>(&mut pool.get().map_err(|_| "open target database".to_owned())?)
    .map_err(|_| "load target representations".to_owned())
}

async fn preflight_source_inline_history(
    pool: &DbPool,
    source_space: &SpaceId,
    source_session: Arc<InMemorySession>,
    preserve_unreadable_history: bool,
) -> Result<(), AdmissionSpaceTransitionError> {
    let rows = load_inline_rows(pool).map_err(|_| AdmissionSpaceTransitionError::Storage)?;
    let source_cipher = BlobCipherAdapter::new(source_session);
    for row in rows {
        let event_id = EventId::from_string(row.event_id);
        let representation_id = RepresentationId::from(row.id);
        let associated_data = Aad::from(aad::for_inline(&event_id, &representation_id));
        match source_cipher
            .decrypt(
                &ActiveSpace::new(source_space.clone()),
                &Ciphertext::new(row.inline_data),
                &associated_data,
            )
            .await
        {
            Ok(_) => {}
            Err(uc_core::ports::security::BlobCipherError::InvalidCiphertext)
                if !preserve_unreadable_history =>
            {
                return Err(AdmissionSpaceTransitionError::UnreadableHistoryRequiresConfirmation);
            }
            Err(uc_core::ports::security::BlobCipherError::InvalidCiphertext) => {}
            Err(_) => return Err(AdmissionSpaceTransitionError::Storage),
        }
    }
    Ok(())
}

fn save_rewrapped_inline_rows(pool: &DbPool, rows: &[(String, Vec<u8>)]) -> Result<(), String> {
    let mut connection = pool
        .get()
        .map_err(|_| "open target database for update".to_owned())?;
    connection
        .transaction::<_, diesel::result::Error, _>(|connection| {
            for (id, ciphertext) in rows {
                diesel::sql_query(
                    "UPDATE clipboard_snapshot_representation SET inline_data = ? WHERE id = ?",
                )
                .bind::<diesel::sql_types::Binary, _>(ciphertext)
                .bind::<diesel::sql_types::Text, _>(id)
                .execute(connection)?;
            }
            Ok(())
        })
        .map_err(|_| "save target representations".to_owned())
}

fn mark_preserved_inline_rows(pool: &DbPool, ids: &[String]) -> Result<(), String> {
    let mut connection = pool
        .get()
        .map_err(|_| "open target database for preservation".to_owned())?;
    connection
        .transaction::<_, diesel::result::Error, _>(|connection| {
            for id in ids {
                diesel::sql_query(
                    "UPDATE clipboard_snapshot_representation \
                     SET payload_state = 'Lost', \
                         last_error = 'unreadable encrypted payload preserved during space switch' \
                     WHERE id = ?",
                )
                .bind::<diesel::sql_types::Text, _>(id)
                .execute(connection)?;
            }
            Ok(())
        })
        .map_err(|_| "mark preserved target representations".to_owned())
}

async fn verify_rewrapped_inline_rows(
    pool: &DbPool,
    target_space: &SpaceId,
    target_session: Arc<InMemorySession>,
    preserved_ids: &[String],
) -> Result<(), String> {
    let cipher = BlobCipherAdapter::new(target_session);
    for row in load_inline_rows(pool)? {
        if preserved_ids.contains(&row.id) {
            continue;
        }
        let event_id = EventId::from_string(row.event_id);
        let representation_id = RepresentationId::from(row.id);
        cipher
            .decrypt(
                &ActiveSpace::new(target_space.clone()),
                &Ciphertext::new(row.inline_data),
                &Aad::from(aad::for_inline(&event_id, &representation_id)),
            )
            .await
            .map_err(|_| "reopen target representation".to_owned())?;
    }
    Ok(())
}

fn verify_existing_snapshot(path: &Path, expected_digest: [u8; 32]) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|_| "read source backup".to_owned())?;
    (digest(&bytes) == expected_digest)
        .then_some(())
        .ok_or_else(|| "source backup digest mismatch".to_owned())
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if path.exists() {
        let existing = std::fs::read(path).map_err(|_| "read existing generation".to_owned())?;
        if existing == bytes {
            return Ok(());
        }
    }
    let parent = path
        .parent()
        .ok_or_else(|| "generation parent is missing".to_owned())?;
    std::fs::create_dir_all(parent).map_err(|_| "create generation parent".to_owned())?;
    let temporary = path.with_extension("write.tmp");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|_| "create temporary generation".to_owned())?;
    std::io::Write::write_all(&mut file, bytes)
        .map_err(|_| "write temporary generation".to_owned())?;
    file.sync_all()
        .map_err(|_| "sync temporary generation".to_owned())?;
    drop(file);
    replace_file_atomically(&temporary, path)
        .map_err(|_| "replace existing generation".to_owned())?;
    let directory = std::fs::File::open(parent).map_err(|_| "open generation parent".to_owned())?;
    directory
        .sync_all()
        .map_err(|_| "sync generation parent".to_owned())
}

#[cfg(not(windows))]
fn replace_file_atomically(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file_atomically(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let wide = |path: &Path| {
        let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
        value.push(0);
        value
    };
    let source = wide(source);
    let destination = wide(destination);
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn short_hex(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(32);
    for byte in &bytes[..16] {
        value.push(HEX[(byte >> 4) as usize] as char);
        value.push(HEX[(byte & 0x0f) as usize] as char);
    }
    value
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use diesel::{Connection, RunQueryDsl, SqliteConnection};
    use tempfile::tempdir;
    use uc_application::deps::AdmissionSpaceTransitionPort;
    use uc_application::deps::{
        AdmissionSpaceTransitionError, AdmissionSpaceTransitionPreparationV2,
        AdmissionSpaceTransitionStepV2, DeviceManagementResetDataPort,
    };
    use uc_core::blob::ports::BlobReaderPort;
    use uc_core::clipboard::MobileConsumableRef;
    use uc_core::crypto::aad;
    use uc_core::crypto::domain::{Aad, ActiveSpace, Ciphertext, Passphrase, Plaintext};
    use uc_core::file_transfer::FileTransferEvent;
    use uc_core::ids::{BlobId, DeviceId, EntryId, EventId, RepresentationId, SpaceId};
    use uc_core::membership::{
        AdmissionChangeFacts, AdmissionContentKeyCatalogV1, AdmissionContentKeyEntryV1,
        AdmissionSecurityCommitmentV1, AdmissionSpaceTransitionResultV2,
        AdmissionSpaceTransitionV2, BaseMembershipHistoryPosition, ContentKeyPurpose,
        CrossSpaceTransitionPhaseV2, MembershipCredential, PendingGroupUpdate,
        RevocationRepositoryPort, SpaceAdmissionId, ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
        ED25519_SIGNATURE_ALGORITHM_V1,
    };
    use uc_core::ports::security::current_profile::CurrentProfilePort;
    use uc_core::ports::security::BlobCipherPort;
    use uc_core::ports::space::{
        DeriveSpaceSubkeyPort, PrepareAdmissionTargetAccessPort, SpaceAccessStore,
    };
    use uc_core::ports::{
        ReceiveArtifact, ReceiveArtifactOwnership, SecureStorageError, SecureStoragePort,
    };
    use uc_core::search::RenderKey;

    use crate::blob::{BlobStorePort, FilesystemBlobStore, SwitchableFilesystemBlobStore};
    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::pool::init_db_pool;
    use crate::db::repositories::active_clipboard_register_cipher::{
        ActiveClipboardRegisterCipher, CONSUMABLE_HKDF_INFO,
    };
    use crate::db::repositories::directory_publish_log_cipher::DirectoryPublishLogCipher;
    use crate::db::repositories::entry_file_set_cipher::EntryFileSetPathCipher;
    use crate::db::repositories::receive_artifact_cipher::ReceiveArtifactCipher;
    use crate::db::repositories::{DieselSpaceSecurityStore, EncryptedRelationshipStore};
    use crate::file_transfer::persistence_cipher::{TransferMetadata, TransferPersistenceCipher};
    use crate::fs::key_slot_store::JsonKeySlotStore;
    use crate::search::render_payload::{RenderFields, RenderPayloadCodec};
    use crate::security::{
        ActiveSpaceGenerationManifestStore, AdmissionKeyManager, BlobCipherAdapter,
        DefaultCurrentProfile, EncryptedBlobStore, MasterKey,
    };
    use crate::space::{
        prepare_registration, DefaultSpaceAccessAdapter, InMemorySession, KeyMaterialStore,
    };

    use super::{
        ensure_reset_capacity, DurableAdmissionSpaceTransition, SqliteSpaceGenerationStore,
    };

    #[derive(diesel::QueryableByName)]
    struct CountRow {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        count: i64,
    }

    #[test]
    fn reset_capacity_fails_closed_before_writes_when_space_is_insufficient() {
        assert_eq!(
            ensure_reset_capacity(99, 100),
            Err(AdmissionSpaceTransitionError::InsufficientStorage)
        );
        assert_eq!(ensure_reset_capacity(100, 100), Ok(()));
    }

    #[derive(Default)]
    struct MemorySecureStorage(Mutex<HashMap<String, Vec<u8>>>);

    impl SecureStoragePort for MemorySecureStorage {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }

        fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
            self.0
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.to_vec());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }

    fn session(space: &SpaceId, key: u8) -> Arc<InMemorySession> {
        let session = Arc::new(InMemorySession::new());
        session.set_master_key_for_space(space.clone(), MasterKey::from_bytes(&[key; 32]).unwrap());
        session
    }

    fn test_security_commitment(
        target_space: &SpaceId,
        attempt_id: [u8; 32],
    ) -> AdmissionSecurityCommitmentV1 {
        let catalog = test_content_key_catalog();
        test_security_commitment_with_catalog(target_space, attempt_id, &catalog)
    }

    fn test_security_commitment_with_catalog(
        target_space: &SpaceId,
        attempt_id: [u8; 32],
        catalog: &AdmissionContentKeyCatalogV1,
    ) -> AdmissionSecurityCommitmentV1 {
        AdmissionSecurityCommitmentV1::new(
            ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
            target_space.as_ref().to_owned(),
            b"target-mls-group".to_vec(),
            attempt_id,
            BaseMembershipHistoryPosition {
                event_id: None,
                depth: 0,
                history_digest: [0x74; 32],
            },
            [0x75; 32],
            1,
            0,
            1,
            [0x76; 32],
            [0x77; 32],
            [0x78; 32],
            catalog.digest(),
            [0x7a; 32],
        )
        .unwrap()
    }

    fn test_content_key_catalog() -> AdmissionContentKeyCatalogV1 {
        AdmissionContentKeyCatalogV1::new(
            "target-content-key",
            1,
            vec![
                AdmissionContentKeyEntryV1::new("legacy-v1", 0, vec![0x7b; 32]).unwrap(),
                AdmissionContentKeyEntryV1::new("target-content-key", 1, vec![0x7c; 32]).unwrap(),
            ],
        )
        .unwrap()
    }

    fn test_relationships() -> Vec<AdmissionChangeFacts> {
        [
            ("target-local", "target local", 0x81),
            ("target-peer", "target peer", 0x82),
        ]
        .into_iter()
        .map(|(device, name, key)| {
            let device_id = DeviceId::new(device);
            let credential =
                MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![key; 32]);
            AdmissionChangeFacts {
                member_instance: credential.member_instance_id(&device_id),
                device_id,
                device_name: name.to_owned(),
                identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                    "ABCD-EFGH-IJKL-MNOP",
                )
                .unwrap(),
                transport_public_key: vec![key],
                transport_address_blob: vec![key, key],
                identity_signature: vec![key, key, key],
            }
        })
        .collect()
    }

    async fn encrypted_inline(
        session: Arc<InMemorySession>,
        space: &SpaceId,
        event_id: &EventId,
        representation_id: &RepresentationId,
        plaintext: &[u8],
    ) -> Vec<u8> {
        BlobCipherAdapter::new(session)
            .encrypt(
                &ActiveSpace::new(space.clone()),
                &Plaintext::new(plaintext.to_vec()),
                &Aad::from(aad::for_inline(event_id, representation_id)),
            )
            .await
            .unwrap()
            .into_bytes()
    }

    fn insert_inline(
        connection: &mut SqliteConnection,
        event_id: &EventId,
        representation_id: &RepresentationId,
        ciphertext: &[u8],
    ) {
        diesel::sql_query(
            "INSERT OR IGNORE INTO clipboard_event \
             (event_id, captured_at_ms, source_device, snapshot_hash) \
             VALUES (?, 1, 'device-a', ?)",
        )
        .bind::<diesel::sql_types::Text, _>(event_id.as_ref())
        .bind::<diesel::sql_types::Text, _>(format!("snapshot-{event_id}"))
        .execute(connection)
        .unwrap();
        diesel::sql_query(
            "INSERT INTO clipboard_snapshot_representation \
             (id, event_id, format_id, mime_type, size_bytes, inline_data, blob_id, payload_state, last_error) \
             VALUES (?, ?, 'text/plain', 'text/plain', ?, ?, NULL, 'Inline', NULL)",
        )
        .bind::<diesel::sql_types::Text, _>(representation_id.as_ref())
        .bind::<diesel::sql_types::Text, _>(event_id.as_ref())
        .bind::<diesel::sql_types::BigInt, _>(ciphertext.len() as i64)
        .bind::<diesel::sql_types::Binary, _>(ciphertext)
        .execute(connection)
        .unwrap();
    }

    #[derive(diesel::QueryableByName)]
    struct EventIdRow {
        #[diesel(sql_type = diesel::sql_types::Text)]
        event_id: String,
    }

    #[tokio::test]
    async fn device_management_reset_stages_mutations_without_touching_active_database() {
        let directory = tempdir().unwrap();
        let source_path = directory.path().join("source.sqlite");
        let source_blob_root = directory.path().join("source-blobs");
        let generation_root = directory.path().join("space-generations");
        let vault = directory.path().join("vault");
        let pool = init_db_pool(source_path.to_str().unwrap()).unwrap();
        diesel::sql_query(
            "INSERT INTO clipboard_event \
             (event_id, captured_at_ms, source_device, snapshot_hash) \
             VALUES ('active-event', 1, 'old-peer', 'active-snapshot')",
        )
        .execute(&mut pool.get().unwrap())
        .unwrap();

        let secure_storage: Arc<dyn SecureStoragePort> = Arc::new(MemorySecureStorage::default());
        let session = Arc::new(InMemorySession::new());
        let key_material = Arc::new(KeyMaterialStore::new(
            Arc::clone(&secure_storage),
            Arc::new(JsonKeySlotStore::new(vault.clone())),
        ));
        let key_epoch_repository: Arc<dyn RevocationRepositoryPort> =
            Arc::new(DieselSpaceSecurityStore::new(
                Arc::new(DieselSqliteExecutor::new(pool.clone())),
                session.as_ref().clone(),
            ));
        let access = Arc::new(DefaultSpaceAccessAdapter::new_with_key_epoch_repository(
            key_material,
            Arc::new(DefaultCurrentProfile::new()),
            Arc::clone(&session),
            key_epoch_repository,
        ));
        SpaceAccessStore::initialize(
            access.as_ref(),
            &SpaceId::from_str("old-space"),
            &Passphrase::new("source passphrase"),
        )
        .await
        .unwrap();
        let generation_manifest_store = Arc::new(ActiveSpaceGenerationManifestStore::new(
            vault,
            Arc::new(AdmissionKeyManager::new(
                Arc::clone(&secure_storage),
                [0x41; 16],
            )),
        ));
        let transitioner = DurableAdmissionSpaceTransition::new(
            pool.clone(),
            source_blob_root.clone(),
            generation_root.clone(),
            b"default".to_vec(),
            Arc::new(SwitchableFilesystemBlobStore::new(source_blob_root.clone())),
            Arc::clone(&generation_manifest_store),
            Arc::clone(&access),
            Arc::clone(&session),
            Arc::new(DefaultCurrentProfile::new()),
            Arc::new(AdmissionKeyManager::new(
                Arc::clone(&secure_storage),
                [0x41; 16],
            )),
        );
        let target = SpaceId::from_str("reset-target");

        transitioner
            .prepare_device_management_reset(&target)
            .await
            .unwrap();
        transitioner
            .stage_device_management_reset_mutations(&target)
            .await
            .unwrap();
        diesel::sql_query("DELETE FROM clipboard_event WHERE event_id = 'active-event'")
            .execute(&mut pool.get().unwrap())
            .unwrap();

        let reopened = init_db_pool(source_path.to_str().unwrap()).unwrap();
        let event = diesel::sql_query(
            "SELECT event_id FROM clipboard_event WHERE event_id = 'active-event'",
        )
        .get_result::<EventIdRow>(&mut reopened.get().unwrap())
        .unwrap();
        assert_eq!(event.event_id, "active-event");

        drop(transitioner);
        diesel::sql_query(
            "INSERT INTO clipboard_event \
             (event_id, captured_at_ms, source_device, snapshot_hash) \
             VALUES ('post-failure-event', 2, 'old-peer', 'post-failure-snapshot')",
        )
        .execute(&mut reopened.get().unwrap())
        .unwrap();
        let retried = DurableAdmissionSpaceTransition::new(
            reopened.clone(),
            source_blob_root.clone(),
            generation_root,
            b"default".to_vec(),
            Arc::new(SwitchableFilesystemBlobStore::new(source_blob_root)),
            generation_manifest_store,
            access,
            session,
            Arc::new(DefaultCurrentProfile::new()),
            Arc::new(AdmissionKeyManager::new(secure_storage, [0x41; 16])),
        );
        retried
            .prepare_device_management_reset(&target)
            .await
            .unwrap();
        retried
            .stage_device_management_reset_mutations(&target)
            .await
            .unwrap();
        let new_event = diesel::sql_query(
            "SELECT event_id FROM clipboard_event WHERE event_id = 'post-failure-event'",
        )
        .get_result::<EventIdRow>(&mut reopened.get().unwrap())
        .unwrap();
        assert_eq!(new_event.event_id, "post-failure-event");
    }

    #[tokio::test]
    async fn durable_transition_promotes_database_blobs_manifest_and_target_access_together() {
        let directory = tempdir().unwrap();
        let source_path = directory.path().join("source.sqlite");
        let source_blob_root = directory.path().join("source-blobs");
        let generation_root = directory.path().join("space-generations");
        let vault = directory.path().join("vault");
        let pool = init_db_pool(source_path.to_str().unwrap()).unwrap();
        diesel::sql_query(
            "INSERT INTO admission_repository_state (singleton_id, encrypted_payload) \
             VALUES (1, X'010203')",
        )
        .execute(&mut pool.get().unwrap())
        .unwrap();

        let secure_storage: Arc<dyn SecureStoragePort> = Arc::new(MemorySecureStorage::default());
        let session = Arc::new(InMemorySession::new());
        let key_material = Arc::new(KeyMaterialStore::new(
            Arc::clone(&secure_storage),
            Arc::new(JsonKeySlotStore::new(vault.clone())),
        ));
        let key_epoch_repository: Arc<dyn RevocationRepositoryPort> =
            Arc::new(DieselSpaceSecurityStore::new(
                Arc::new(DieselSqliteExecutor::new(pool.clone())),
                session.as_ref().clone(),
            ));
        let access = Arc::new(DefaultSpaceAccessAdapter::new_with_key_epoch_repository(
            key_material,
            Arc::new(DefaultCurrentProfile::new()),
            Arc::clone(&session),
            key_epoch_repository,
        ));
        let source_space = SpaceId::from_str("source-space");
        let target_space = SpaceId::from_str("target-space");
        SpaceAccessStore::initialize(
            access.as_ref(),
            &source_space,
            &Passphrase::new("source passphrase"),
        )
        .await
        .unwrap();
        let target_access = PrepareAdmissionTargetAccessPort::prepare_target_access(
            access.as_ref(),
            &target_space,
            &Passphrase::new("target passphrase"),
        )
        .await
        .unwrap();
        let event_id = EventId::from_string("event-transition".to_owned());
        let representation_id = RepresentationId::from("representation-transition");
        let ciphertext = encrypted_inline(
            Arc::clone(&session),
            &source_space,
            &event_id,
            &representation_id,
            b"survives transition",
        )
        .await;
        insert_inline(
            &mut pool.get().unwrap(),
            &event_id,
            &representation_id,
            &ciphertext,
        );

        let blob_store = Arc::new(SwitchableFilesystemBlobStore::new(source_blob_root.clone()));
        let generation_manifest_store = Arc::new(ActiveSpaceGenerationManifestStore::new(
            vault,
            Arc::new(AdmissionKeyManager::new(
                Arc::clone(&secure_storage),
                [0x71; 16],
            )),
        ));
        let transitioner = DurableAdmissionSpaceTransition::new(
            pool.clone(),
            source_blob_root,
            generation_root,
            b"default".to_vec(),
            Arc::clone(&blob_store),
            Arc::clone(&generation_manifest_store),
            Arc::clone(&access),
            Arc::clone(&session),
            Arc::new(DefaultCurrentProfile::new()),
            Arc::new(AdmissionKeyManager::new(
                Arc::clone(&secure_storage),
                [0x71; 16],
            )),
        );
        let relayed_update = PendingGroupUpdate::persistent(
            DeviceId::new("target-peer"),
            b"sealed-existing-member-update".to_vec(),
        );
        let input = AdmissionSpaceTransitionPreparationV2 {
            attempt_id: SpaceAdmissionId::from_bytes([0x72; 32]).expect("valid admission id"),
            target_space_id: target_space.as_ref().to_owned(),
            target_security_commitment: test_security_commitment(&target_space, [0x72; 32]),
            target_membership_history: b"verified history".to_vec(),
            target_security_state: b"verified security state".to_vec(),
            target_protection_group_id: "target-protection-group".to_owned(),
            target_key_catalog: test_content_key_catalog().encode().unwrap(),
            local_device_id: DeviceId::new("target-local"),
            target_relationships: test_relationships(),
            relayed_group_updates: vec![relayed_update.clone()],
            target_access_state: target_access.into_bytes(),
            target_admission_credentials: prepare_registration(&Passphrase::new(
                "fresh target passphrase",
            ))
            .unwrap(),
            preserve_unreadable_history: false,
        };
        let mut transition = transitioner.prepare_if_needed(&input).await.unwrap();
        let AdmissionSpaceTransitionV2::CrossSpace(cross_space) = &transition else {
            panic!("expected a cross-space transition");
        };
        assert_eq!(cross_space.phase, CrossSpaceTransitionPhaseV2::TargetStaged);
        let workspace_bytes = std::fs::read(
            transitioner
                .target_directory(cross_space)
                .join("workspace-state.bin"),
        )
        .unwrap();
        assert!(!workspace_bytes
            .windows(b"verified history".len())
            .any(|window| window == b"verified history"));
        assert!(transitioner
            .open_target_workspace(
                &cross_space.target_space_id,
                &cross_space.target_generation,
                &cross_space.target_workspace_ref,
                session.as_ref(),
            )
            .is_err());

        let result = loop {
            session.clear();
            let first = transitioner.advance(&transition).await.unwrap();
            session.clear();
            let replay = transitioner.advance(&transition).await.unwrap();
            assert_eq!(first, replay);
            match first {
                AdmissionSpaceTransitionStepV2::Advanced(next) => transition = next,
                AdmissionSpaceTransitionStepV2::Finished(result) => break result,
            }
        };

        let AdmissionSpaceTransitionResultV2::CrossSpace(result) = result else {
            panic!("expected a cross-space result");
        };
        assert_eq!(result.target_space_id, target_space.as_ref());
        SpaceAccessStore::try_resume_session(transitioner.space_access.as_ref(), &target_space)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(session.current_space_id().unwrap(), target_space);
        let security_repository = DieselSpaceSecurityStore::new(
            Arc::new(DieselSqliteExecutor::new(pool.clone())),
            session.as_ref().clone(),
        );
        let material = security_repository
            .load_space_material(&target_space)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(material.state().epoch().value(), 1);
        assert_eq!(material.pending_group_updates(), &[relayed_update]);
        assert_eq!(
            material.state().current_content_key_id().as_str(),
            "target-content-key"
        );
        assert_eq!(
            session
                .current_content_key(&target_space, ContentKeyPurpose::Content)
                .unwrap()
                .content_key_id()
                .as_str(),
            "target-content-key"
        );
        let relationship_store = EncryptedRelationshipStore::new(
            Arc::new(DieselSqliteExecutor::new(pool.clone())),
            Arc::clone(&transitioner.space_access) as Arc<dyn DeriveSpaceSubkeyPort>,
            Arc::new(DefaultCurrentProfile::new()) as Arc<dyn CurrentProfilePort>,
        );
        let member_ids = relationship_store
            .list_members()
            .await
            .unwrap()
            .into_iter()
            .map(|member| member.device_id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            member_ids,
            [DeviceId::new("target-local"), DeviceId::new("target-peer")]
                .into_iter()
                .collect()
        );
        assert_eq!(
            relationship_store
                .list_trusted_peers()
                .await
                .unwrap()
                .into_iter()
                .map(|peer| peer.peer_device_id)
                .collect::<Vec<_>>(),
            vec![DeviceId::new("target-peer")]
        );
        assert_eq!(
            relationship_store
                .list_peer_addresses()
                .await
                .unwrap()
                .into_iter()
                .map(|address| address.device_id)
                .collect::<Vec<_>>(),
            vec![DeviceId::new("target-peer")]
        );
        let manifest = generation_manifest_store.load().await.unwrap().unwrap();
        assert_eq!(manifest.space_id, target_space.as_ref());
        let credential_count = diesel::sql_query(
            "SELECT COUNT(*) AS count FROM space_admission_credentials WHERE singleton_id = 1",
        )
        .get_result::<CountRow>(&mut pool.get().unwrap())
        .unwrap()
        .count;
        assert_eq!(credential_count, 1);
        assert_eq!(
            blob_store.current_root(),
            super::space_generation_directory(
                directory.path().join("space-generations").as_path(),
                target_space.as_ref(),
                &manifest.database_generation,
            )
            .join("blobs")
        );
        let row = diesel::sql_query(
            "SELECT id, event_id, inline_data FROM clipboard_snapshot_representation \
             WHERE id = ?",
        )
        .bind::<diesel::sql_types::Text, _>(representation_id.as_ref())
        .get_result::<InlineRow>(&mut pool.get().unwrap())
        .unwrap();
        let plaintext = BlobCipherAdapter::new(session)
            .decrypt(
                &ActiveSpace::new(target_space),
                &uc_core::crypto::domain::Ciphertext::new(row.inline_data),
                &Aad::from(aad::for_inline(&event_id, &representation_id)),
            )
            .await
            .unwrap();
        assert_eq!(plaintext.as_bytes(), b"survives transition");
    }

    #[tokio::test]
    async fn fresh_transition_creates_first_generation_and_reopens_target_state() {
        let directory = tempdir().unwrap();
        let profile_path = directory.path().join("profile.sqlite");
        let profile_blob_root = directory.path().join("profile-blobs");
        let generation_root = directory.path().join("space-generations");
        let vault = directory.path().join("vault");
        let pool = init_db_pool(profile_path.to_str().unwrap()).unwrap();
        diesel::sql_query(
            "INSERT INTO admission_repository_state (singleton_id, encrypted_payload) \
             VALUES (1, X'010203')",
        )
        .execute(&mut pool.get().unwrap())
        .unwrap();

        let secure_storage: Arc<dyn SecureStoragePort> = Arc::new(MemorySecureStorage::default());
        let session = Arc::new(InMemorySession::new());
        let current_profile: Arc<dyn CurrentProfilePort> = Arc::new(
            DefaultCurrentProfile::for_profile(uc_core::ids::ProfileId::from("upgrade-profile")),
        );
        let key_material = Arc::new(KeyMaterialStore::new(
            Arc::clone(&secure_storage),
            Arc::new(JsonKeySlotStore::new(vault.clone())),
        ));
        let key_epoch_repository: Arc<dyn RevocationRepositoryPort> =
            Arc::new(DieselSpaceSecurityStore::new(
                Arc::new(DieselSqliteExecutor::new(pool.clone())),
                session.as_ref().clone(),
            ));
        let access = Arc::new(DefaultSpaceAccessAdapter::new_with_key_epoch_repository(
            key_material,
            Arc::clone(&current_profile),
            Arc::clone(&session),
            key_epoch_repository,
        ));
        let target_space = SpaceId::from_str("fresh-target-space");
        let target_access = PrepareAdmissionTargetAccessPort::prepare_target_access(
            access.as_ref(),
            &target_space,
            &Passphrase::new("fresh target passphrase"),
        )
        .await
        .unwrap();
        let blob_store = Arc::new(SwitchableFilesystemBlobStore::new(
            profile_blob_root.clone(),
        ));
        let generation_manifest_store = Arc::new(ActiveSpaceGenerationManifestStore::new(
            vault,
            Arc::new(AdmissionKeyManager::new(
                Arc::clone(&secure_storage),
                [0x91; 16],
            )),
        ));
        let transitioner = DurableAdmissionSpaceTransition::new(
            pool.clone(),
            profile_blob_root,
            generation_root,
            b"default".to_vec(),
            Arc::clone(&blob_store),
            Arc::clone(&generation_manifest_store),
            Arc::clone(&access),
            Arc::clone(&session),
            Arc::clone(&current_profile),
            Arc::new(AdmissionKeyManager::new(
                Arc::clone(&secure_storage),
                [0x91; 16],
            )),
        );
        let input = AdmissionSpaceTransitionPreparationV2 {
            attempt_id: SpaceAdmissionId::from_bytes([0x92; 32]).expect("valid admission id"),
            target_space_id: target_space.as_ref().to_owned(),
            target_security_commitment: test_security_commitment(&target_space, [0x92; 32]),
            target_membership_history: b"verified fresh history".to_vec(),
            target_security_state: b"verified fresh security state".to_vec(),
            target_protection_group_id: "fresh-protection-group".to_owned(),
            target_key_catalog: test_content_key_catalog().encode().unwrap(),
            local_device_id: DeviceId::new("target-local"),
            target_relationships: test_relationships(),
            relayed_group_updates: Vec::new(),
            target_access_state: target_access.into_bytes(),
            target_admission_credentials: prepare_registration(&Passphrase::new(
                "fresh target passphrase",
            ))
            .unwrap(),
            preserve_unreadable_history: false,
        };

        let mut transition = transitioner.prepare_if_needed(&input).await.unwrap();
        assert!(matches!(transition, AdmissionSpaceTransitionV2::Fresh(_)));
        let result = loop {
            session.clear();
            let first = transitioner.advance(&transition).await.unwrap();
            session.clear();
            let replay = transitioner.advance(&transition).await.unwrap();
            assert_eq!(first, replay);
            match first {
                AdmissionSpaceTransitionStepV2::Advanced(next) => transition = next,
                AdmissionSpaceTransitionStepV2::Finished(result) => break result,
            }
        };
        assert_eq!(
            result,
            AdmissionSpaceTransitionResultV2::Fresh {
                target_space_id: target_space.as_ref().to_owned(),
            }
        );

        session.clear();
        SpaceAccessStore::try_resume_session(access.as_ref(), &target_space)
            .await
            .unwrap()
            .unwrap();
        let manifest = generation_manifest_store.load().await.unwrap().unwrap();
        assert_eq!(manifest.space_id, target_space.as_ref());
        assert_eq!(session.current_space_id().unwrap(), target_space);
        let security_repository = DieselSpaceSecurityStore::new(
            Arc::new(DieselSqliteExecutor::new(pool.clone())),
            session.as_ref().clone(),
        );
        let material = security_repository
            .load_space_material(&target_space)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(material.state().epoch().value(), 1);
        assert_eq!(
            material.state().current_content_key_id().as_str(),
            "target-content-key"
        );
        let relationship_store = EncryptedRelationshipStore::new(
            Arc::new(DieselSqliteExecutor::new(pool)),
            Arc::clone(&transitioner.space_access) as Arc<dyn DeriveSpaceSubkeyPort>,
            current_profile,
        );
        assert_eq!(relationship_store.list_members().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn same_space_transition_preserves_content_and_promotes_verified_state() {
        let directory = tempdir().unwrap();
        let source_path = directory.path().join("same-space.sqlite");
        let source_blob_root = directory.path().join("same-space-blobs");
        let generation_root = directory.path().join("space-generations");
        let vault = directory.path().join("vault");
        let pool = init_db_pool(source_path.to_str().unwrap()).unwrap();
        diesel::sql_query(
            "INSERT INTO admission_repository_state (singleton_id, encrypted_payload) \
             VALUES (1, X'010203')",
        )
        .execute(&mut pool.get().unwrap())
        .unwrap();

        let secure_storage: Arc<dyn SecureStoragePort> = Arc::new(MemorySecureStorage::default());
        let session = Arc::new(InMemorySession::new());
        let key_material = Arc::new(KeyMaterialStore::new(
            Arc::clone(&secure_storage),
            Arc::new(JsonKeySlotStore::new(vault.clone())),
        ));
        let key_epoch_repository: Arc<dyn RevocationRepositoryPort> =
            Arc::new(DieselSpaceSecurityStore::new(
                Arc::new(DieselSqliteExecutor::new(pool.clone())),
                session.as_ref().clone(),
            ));
        let access = Arc::new(DefaultSpaceAccessAdapter::new_with_key_epoch_repository(
            key_material,
            Arc::new(DefaultCurrentProfile::new()),
            Arc::clone(&session),
            key_epoch_repository,
        ));
        let target_space = SpaceId::from_str("same-space");
        SpaceAccessStore::initialize(
            access.as_ref(),
            &target_space,
            &Passphrase::new("same space passphrase"),
        )
        .await
        .unwrap();
        let target_access = PrepareAdmissionTargetAccessPort::prepare_target_access(
            access.as_ref(),
            &target_space,
            &Passphrase::new("same space passphrase"),
        )
        .await
        .unwrap();
        let same_space_catalog = AdmissionContentKeyCatalogV1::new(
            "target-content-key",
            1,
            vec![
                AdmissionContentKeyEntryV1::new(
                    "legacy-v1",
                    0,
                    session.legacy_content_key().unwrap().as_bytes().to_vec(),
                )
                .unwrap(),
                AdmissionContentKeyEntryV1::new("target-content-key", 1, vec![0x7c; 32]).unwrap(),
            ],
        )
        .unwrap();
        let event_id = EventId::from_string("same-space-event".to_owned());
        let representation_id = RepresentationId::from("same-space-representation");
        let ciphertext = encrypted_inline(
            Arc::clone(&session),
            &target_space,
            &event_id,
            &representation_id,
            b"same-space content remains readable",
        )
        .await;
        insert_inline(
            &mut pool.get().unwrap(),
            &event_id,
            &representation_id,
            &ciphertext,
        );

        let blob_store = Arc::new(SwitchableFilesystemBlobStore::new(source_blob_root.clone()));
        let generation_manifest_store = Arc::new(ActiveSpaceGenerationManifestStore::new(
            vault,
            Arc::new(AdmissionKeyManager::new(
                Arc::clone(&secure_storage),
                [0xa1; 16],
            )),
        ));
        let transitioner = DurableAdmissionSpaceTransition::new(
            pool.clone(),
            source_blob_root,
            generation_root,
            b"default".to_vec(),
            Arc::clone(&blob_store),
            Arc::clone(&generation_manifest_store),
            Arc::clone(&access),
            Arc::clone(&session),
            Arc::new(DefaultCurrentProfile::new()),
            Arc::new(AdmissionKeyManager::new(
                Arc::clone(&secure_storage),
                [0xa1; 16],
            )),
        );
        let input = AdmissionSpaceTransitionPreparationV2 {
            attempt_id: SpaceAdmissionId::from_bytes([0xa2; 32]).expect("valid admission id"),
            target_space_id: target_space.as_ref().to_owned(),
            target_security_commitment: test_security_commitment_with_catalog(
                &target_space,
                [0xa2; 32],
                &same_space_catalog,
            ),
            target_membership_history: b"verified same-space history".to_vec(),
            target_security_state: b"verified same-space security state".to_vec(),
            target_protection_group_id: "same-space-protection-group".to_owned(),
            target_key_catalog: same_space_catalog.encode().unwrap(),
            local_device_id: DeviceId::new("target-local"),
            target_relationships: test_relationships(),
            relayed_group_updates: Vec::new(),
            target_access_state: target_access.into_bytes(),
            target_admission_credentials: prepare_registration(&Passphrase::new(
                "target passphrase",
            ))
            .unwrap(),
            preserve_unreadable_history: false,
        };

        let mut transition = transitioner.prepare_if_needed(&input).await.unwrap();
        assert!(matches!(
            transition,
            AdmissionSpaceTransitionV2::SameSpace(_)
        ));
        let result = loop {
            session.clear();
            SpaceAccessStore::try_resume_session(access.as_ref(), &target_space)
                .await
                .unwrap()
                .unwrap();
            let first = transitioner.advance(&transition).await.unwrap();
            session.clear();
            SpaceAccessStore::try_resume_session(access.as_ref(), &target_space)
                .await
                .unwrap()
                .unwrap();
            let replay = transitioner.advance(&transition).await.unwrap();
            assert_eq!(first, replay);
            match first {
                AdmissionSpaceTransitionStepV2::Advanced(next) => transition = next,
                AdmissionSpaceTransitionStepV2::Finished(result) => break result,
            }
        };
        assert_eq!(
            result,
            AdmissionSpaceTransitionResultV2::SameSpace {
                target_space_id: target_space.as_ref().to_owned(),
            }
        );
        assert_eq!(session.current_space_id().unwrap(), target_space);
        let row = diesel::sql_query(
            "SELECT id, event_id, inline_data FROM clipboard_snapshot_representation WHERE id = ?",
        )
        .bind::<diesel::sql_types::Text, _>(representation_id.as_ref())
        .get_result::<InlineRow>(&mut pool.get().unwrap())
        .unwrap();
        let plaintext = BlobCipherAdapter::new(session)
            .decrypt(
                &ActiveSpace::new(target_space.clone()),
                &Ciphertext::new(row.inline_data),
                &Aad::from(aad::for_inline(&event_id, &representation_id)),
            )
            .await
            .unwrap();
        assert_eq!(plaintext.as_bytes(), b"same-space content remains readable");
        assert_eq!(
            generation_manifest_store
                .load()
                .await
                .unwrap()
                .unwrap()
                .space_id,
            "same-space"
        );
    }

    #[tokio::test]
    async fn unreadable_history_preflight_requires_confirmation_before_transition_preparation() {
        let directory = tempdir().unwrap();
        let source_path = directory.path().join("source.sqlite");
        let pool = init_db_pool(source_path.to_str().unwrap()).unwrap();
        let source_space = SpaceId::from_str("source-space");
        let source_session = session(&source_space, 0x50);
        let event_id = EventId::from_string("unreadable-preflight-event".to_owned());
        let representation_id = RepresentationId::from("unreadable-preflight-representation");
        insert_inline(
            &mut pool.get().unwrap(),
            &event_id,
            &representation_id,
            b"invalid-source-ciphertext",
        );

        assert!(matches!(
            super::preflight_source_inline_history(
                &pool,
                &source_space,
                Arc::clone(&source_session),
                false,
            )
            .await,
            Err(
                uc_application::deps::AdmissionSpaceTransitionError::UnreadableHistoryRequiresConfirmation
            )
        ));
        super::preflight_source_inline_history(&pool, &source_space, source_session, true)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn confirmed_unreadable_history_is_preserved_and_counted_during_rewrap() {
        #[derive(diesel::QueryableByName)]
        struct PreservedRow {
            #[diesel(sql_type = diesel::sql_types::Binary)]
            inline_data: Vec<u8>,
            #[diesel(sql_type = diesel::sql_types::Text)]
            payload_state: String,
            #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
            last_error: Option<String>,
        }

        let directory = tempdir().unwrap();
        let source_path = directory.path().join("source.sqlite");
        let pool = init_db_pool(source_path.to_str().unwrap()).unwrap();
        let source_space = SpaceId::from_str("source-space");
        let target_space = SpaceId::from_str("target-space");
        let source_session = session(&source_space, 0x51);
        let target_session = session(&target_space, 0x52);
        let event_id = EventId::from_string("unreadable-event".to_owned());
        let representation_id = RepresentationId::from("unreadable-representation");
        let unreadable_ciphertext = b"preserved-unreadable-ciphertext".to_vec();
        insert_inline(
            &mut pool.get().unwrap(),
            &event_id,
            &representation_id,
            &unreadable_ciphertext,
        );
        let store = SqliteSpaceGenerationStore::new(
            pool,
            directory.path().join("source-blobs"),
            directory.path().join("generations"),
            b"default".to_vec(),
        );
        let prepared = store.prepare_source([0x53; 32], [0x54; 16]).unwrap();
        let finalized_source = store
            .finalize_source(prepared, &target_space, [0x55; 16])
            .unwrap();

        let finalized = store
            .rewrap_finalized_source(
                finalized_source,
                &source_space,
                source_session,
                &target_space,
                target_session,
                [0x55; 16],
                true,
            )
            .await
            .unwrap();

        assert_eq!(finalized.migrated_records, 0);
        assert_eq!(finalized.preserved_unreadable_records, 1);
        let row = diesel::sql_query(
            "SELECT inline_data, payload_state, last_error \
             FROM clipboard_snapshot_representation WHERE id = ?",
        )
        .bind::<diesel::sql_types::Text, _>(representation_id.as_ref())
        .get_result::<PreservedRow>(
            &mut SqliteConnection::establish(finalized.database_path.to_str().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(row.inline_data, unreadable_ciphertext);
        assert_eq!(row.payload_state, "Lost");
        assert_eq!(
            row.last_error.as_deref(),
            Some("unreadable encrypted payload preserved during space switch")
        );
    }

    #[tokio::test]
    async fn final_generation_includes_post_backup_additions_and_deletions_under_target_key() {
        let directory = tempdir().unwrap();
        let source_path = directory.path().join("source.sqlite");
        let target_root = directory.path().join("generations");
        let pool = init_db_pool(source_path.to_str().unwrap()).unwrap();
        let source_space = SpaceId::from_str("source-space");
        let target_space = SpaceId::from_str("target-space");
        let source_session = session(&source_space, 0x11);
        let target_session = session(&target_space, 0x22);
        let old_event = EventId::from_string("event-before-backup".to_owned());
        let old_representation = RepresentationId::from("representation-before-backup");
        let old_ciphertext = encrypted_inline(
            Arc::clone(&source_session),
            &source_space,
            &old_event,
            &old_representation,
            b"deleted after J1",
        )
        .await;
        insert_inline(
            &mut pool.get().unwrap(),
            &old_event,
            &old_representation,
            &old_ciphertext,
        );

        let store = SqliteSpaceGenerationStore::new(
            pool.clone(),
            directory.path().join("source-blobs"),
            target_root,
            b"default".to_vec(),
        );
        let prepared = store.prepare_source([0x31; 32], [0x32; 16]).unwrap();

        diesel::sql_query("DELETE FROM clipboard_snapshot_representation WHERE id = ?")
            .bind::<diesel::sql_types::Text, _>(old_representation.as_ref())
            .execute(&mut pool.get().unwrap())
            .unwrap();
        let new_event = EventId::from_string("event-after-backup".to_owned());
        let new_representation = RepresentationId::from("representation-after-backup");
        let new_ciphertext = encrypted_inline(
            Arc::clone(&source_session),
            &source_space,
            &new_event,
            &new_representation,
            b"created after J1",
        )
        .await;
        insert_inline(
            &mut pool.get().unwrap(),
            &new_event,
            &new_representation,
            &new_ciphertext,
        );

        let finalized = store
            .finalize_and_rewrap(
                prepared,
                &source_space,
                Arc::clone(&source_session),
                &target_space,
                Arc::clone(&target_session),
                [0x33; 16],
            )
            .await
            .unwrap();

        let mut target =
            SqliteConnection::establish(finalized.database_path.to_str().unwrap()).unwrap();
        let rows = diesel::sql_query(
            "SELECT id, event_id, inline_data FROM clipboard_snapshot_representation ORDER BY id",
        )
        .load::<InlineRow>(&mut target)
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, new_representation.as_ref());
        assert_eq!(rows[0].event_id, new_event.as_ref());

        let target_cipher = uc_core::crypto::domain::Ciphertext::new(rows[0].inline_data.clone());
        let target_aad = Aad::from(aad::for_inline(&new_event, &new_representation));
        let plaintext = BlobCipherAdapter::new(target_session)
            .decrypt(&ActiveSpace::new(target_space), &target_cipher, &target_aad)
            .await
            .unwrap();
        assert_eq!(plaintext.as_bytes(), b"created after J1");
        assert!(BlobCipherAdapter::new(source_session)
            .decrypt(&ActiveSpace::new(source_space), &target_cipher, &target_aad,)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn final_generation_rewraps_file_paths_transfer_state_and_search_render_data() {
        let directory = tempdir().unwrap();
        let source_path = directory.path().join("source.sqlite");
        let pool = init_db_pool(source_path.to_str().unwrap()).unwrap();
        let source_space = SpaceId::from_str("source-space");
        let target_space = SpaceId::from_str("target-space");
        let source_session = session(&source_space, 0x41);
        let target_session = session(&target_space, 0x42);
        let entry_id = EntryId::from("entry-private-data");

        let source_file_set = EntryFileSetPathCipher::new(
            source_session
                .derive_stable_subkey(b"default", b"uniclipboard-file-set/v1")
                .unwrap(),
        );
        let original_text = source_file_set
            .seal_original_text(&entry_id, 0, "/private/source/report.txt")
            .unwrap();
        let relative_path = source_file_set
            .seal_relative_path(&entry_id, 0, "folder/report.txt")
            .unwrap();
        let root_name = source_file_set
            .seal_root_name(&entry_id, 0, "private-root")
            .unwrap();

        let source_transfer = TransferPersistenceCipher::new(
            source_session
                .derive_stable_subkey(b"default", b"uniclipboard-file-transfer-metadata/v1")
                .unwrap(),
            source_session
                .derive_stable_subkey(b"default", b"uniclipboard-file-transfer-events/v1")
                .unwrap(),
        );
        let transfer_metadata = TransferMetadata {
            filename: "report.txt".to_owned(),
            cached_path: Some("/private/cache/report.txt".to_owned()),
            failure_detail: Some("resume after restart".to_owned()),
        };
        let transfer_ciphertext = source_transfer
            .seal_metadata("transfer-private-data", &transfer_metadata)
            .unwrap();
        let transfer_event =
            FileTransferEvent::started("transfer-private-data", "device-a", "report.txt", Some(1));
        let transfer_event_ciphertext = source_transfer
            .seal_event("transfer-private-data", 1, "started", &transfer_event)
            .unwrap();

        let active_reference = MobileConsumableRef::new("private-snapshot-hash", entry_id.clone());
        let source_active = ActiveClipboardRegisterCipher::new(
            source_session
                .derive_stable_subkey(b"default", CONSUMABLE_HKDF_INFO)
                .unwrap(),
        );
        let active_ciphertext = source_active.seal(&active_reference).unwrap();
        let root_map = vec![(
            PathBuf::from("/private/staging/report.txt"),
            PathBuf::from("/private/final/report.txt"),
        )];
        let source_publish = DirectoryPublishLogCipher::new(
            source_session
                .derive_stable_subkey(b"default", b"uniclipboard-directory-publish-log/v1")
                .unwrap(),
        );
        let publish_ciphertext = source_publish
            .seal(&entry_id, "receive-attempt", &root_map)
            .unwrap();
        let artifacts = vec![ReceiveArtifact {
            item_id: "item-private".to_owned(),
            staged_path: PathBuf::from("/private/staging/report.txt"),
            final_path: PathBuf::from("/private/final/report.txt"),
            ownership: ReceiveArtifactOwnership::ManagedStaging,
        }];
        let source_artifact = ReceiveArtifactCipher::new(
            source_session
                .derive_stable_subkey(b"default", b"uniclipboard-receive-artifact-log/v1")
                .unwrap(),
        );
        let artifact_ciphertext = source_artifact
            .seal(entry_id.as_ref(), "receive-attempt", &artifacts)
            .unwrap();

        let source_render = RenderPayloadCodec::new(
            RenderKey::from_bytes(
                &source_session
                    .derive_stable_subkey(b"default", b"uniclipboard-search-render/v1")
                    .unwrap(),
            )
            .unwrap(),
        );
        let render_fields = RenderFields::new(
            Some("private preview".to_owned()),
            vec!["report.txt".to_owned()],
            Vec::new(),
            vec!["/private/source/report.txt".to_owned()],
            Some(15),
        );
        let render_payload = source_render.encrypt(&entry_id, &render_fields).unwrap();
        let blob_id = BlobId::from("blob-private-data");
        let source_blob_root = directory.path().join("source-blobs");
        let source_blob_store = EncryptedBlobStore::new(
            Arc::new(FilesystemBlobStore::new(source_blob_root.clone())),
            Arc::clone(&source_session),
        );
        let (source_blob_path, compressed_size) = source_blob_store
            .put(&blob_id, b"private blob content")
            .await
            .unwrap();

        let mut connection = pool.get().unwrap();
        diesel::sql_query(
            "INSERT INTO clipboard_event (event_id, captured_at_ms, source_device, snapshot_hash) \
             VALUES ('event-private-data', 1, 'device-a', 'snapshot-private-data')",
        )
        .execute(&mut connection)
        .unwrap();
        diesel::sql_query(
            "INSERT INTO blob \
             (blob_id, storage_path, storage_backend, size_bytes, content_hash, encryption_algo, \
              created_at_ms, compressed_size) \
             VALUES (?, ?, 'local_fs', 20, 'private-content-hash', 'xchacha20poly1305', 1, ?)",
        )
        .bind::<diesel::sql_types::Text, _>(blob_id.as_ref())
        .bind::<diesel::sql_types::Text, _>(source_blob_path.to_string_lossy().as_ref())
        .bind::<diesel::sql_types::Nullable<diesel::sql_types::BigInt>, _>(compressed_size)
        .execute(&mut connection)
        .unwrap();
        diesel::sql_query(
            "INSERT INTO file_transfer_events \
             (transfer_id, sequence, event_type, payload_ciphertext, occurred_at_ms) \
             VALUES ('transfer-private-data', 1, 'started', ?, 1)",
        )
        .bind::<diesel::sql_types::Binary, _>(transfer_event_ciphertext)
        .execute(&mut connection)
        .unwrap();
        diesel::sql_query(
            "INSERT INTO active_clipboard_register \
             (id, snapshot_hash, entry_id, activated_at_ms, activated_by, consumable_ref_ciphertext) \
             VALUES (1, 'private-snapshot-hash', ?, 1, 'device-a', ?)",
        )
        .bind::<diesel::sql_types::Text, _>(entry_id.as_ref())
        .bind::<diesel::sql_types::Binary, _>(active_ciphertext)
        .execute(&mut connection)
        .unwrap();
        diesel::sql_query(
            "INSERT INTO directory_publish_log \
             (entry_id, attempt_id, phase, root_map_ciphertext, partial_publication, \
              partial_root_count, landed, updated_at_ms) \
             VALUES (?, 'receive-attempt', 'publishing', ?, 0, 0, 0, 1)",
        )
        .bind::<diesel::sql_types::Text, _>(entry_id.as_ref())
        .bind::<diesel::sql_types::Binary, _>(publish_ciphertext)
        .execute(&mut connection)
        .unwrap();
        diesel::sql_query(
            "INSERT INTO receive_artifact_log \
             (entry_id, attempt_id, phase, resolution, artifact_ciphertext, updated_at_ms) \
             VALUES (?, 'receive-attempt', 'publishing', 'pending', ?, 1)",
        )
        .bind::<diesel::sql_types::Text, _>(entry_id.as_ref())
        .bind::<diesel::sql_types::Binary, _>(artifact_ciphertext)
        .execute(&mut connection)
        .unwrap();
        diesel::sql_query(
            "INSERT INTO clipboard_entry \
             (entry_id, event_id, created_at_ms, active_time_ms, total_size, pinned, deleted_at_ms, \
              delivery_tracked, is_favorited, content_category) \
             VALUES (?, 'event-private-data', 1, 1, 1, 0, NULL, 0, 0, 'files')",
        )
        .bind::<diesel::sql_types::Text, _>(entry_id.as_ref())
        .execute(&mut connection)
        .unwrap();
        diesel::sql_query(
            "INSERT INTO entry_file_set \
             (entry_id, line_index, kind, original_text_ct, root_index, relative_path_ct, kind_tag, root_name_ct) \
             VALUES (?, 0, 'file', ?, 0, ?, 'file', ?)",
        )
        .bind::<diesel::sql_types::Text, _>(entry_id.as_ref())
        .bind::<diesel::sql_types::Binary, _>(original_text)
        .bind::<diesel::sql_types::Binary, _>(relative_path)
        .bind::<diesel::sql_types::Binary, _>(root_name)
        .execute(&mut connection)
        .unwrap();
        diesel::sql_query(
            "INSERT INTO file_transfer \
             (transfer_id, entry_id, file_size, attempt_id, binding_state, receive_item_id, item_role, \
              content_hash, status, source_device, failure_code, metadata_ciphertext, created_at_ms, updated_at_ms) \
             VALUES ('transfer-private-data', ?, 1, NULL, 'legacy', NULL, NULL, NULL, 'pending', \
                     'device-a', NULL, ?, 1, 1)",
        )
        .bind::<diesel::sql_types::Text, _>(entry_id.as_ref())
        .bind::<diesel::sql_types::Binary, _>(transfer_ciphertext)
        .execute(&mut connection)
        .unwrap();
        diesel::sql_query(
            "INSERT INTO search_document \
             (profile_id, entry_id, event_id, active_time_ms, captured_at_ms, file_type, file_extensions, \
              mime_type, indexed_at_ms, index_version, source_device, payload_state, render_payload) \
             VALUES ('default', ?, 'event-private-data', 1, 1, '', '', '', 1, 'v1', 'device-a', 'ready', ?)",
        )
        .bind::<diesel::sql_types::Text, _>(entry_id.as_ref())
        .bind::<diesel::sql_types::Binary, _>(render_payload)
        .execute(&mut connection)
        .unwrap();
        diesel::sql_query(
            "INSERT INTO search_posting (profile_id, term_tag, entry_id, field_mask, term_freq) \
             VALUES ('default', zeroblob(32), ?, 1, 1)",
        )
        .bind::<diesel::sql_types::Text, _>(entry_id.as_ref())
        .execute(&mut connection)
        .unwrap();
        diesel::sql_query(
            "INSERT INTO search_entry_tag (profile_id, entry_id, tag_id) \
             VALUES ('default', ?, 'old-key-tag')",
        )
        .bind::<diesel::sql_types::Text, _>(entry_id.as_ref())
        .execute(&mut connection)
        .unwrap();
        diesel::sql_query(
            "INSERT INTO search_index_meta \
             (profile_id, index_version, search_blocked, last_rebuild_started_at_ms, \
              last_rebuild_completed_at_ms, plaintext_purge_done_ms) \
             VALUES ('default', 'v1', 0, 1, 1, 1)",
        )
        .execute(&mut connection)
        .unwrap();
        drop(connection);

        let store = SqliteSpaceGenerationStore::new(
            pool,
            source_blob_root,
            directory.path().join("generations"),
            b"default".to_vec(),
        );
        let prepared = store.prepare_source([0x51; 32], [0x52; 16]).unwrap();
        let finalized = store
            .finalize_and_rewrap(
                prepared,
                &source_space,
                source_session,
                &target_space,
                Arc::clone(&target_session),
                [0x53; 16],
            )
            .await
            .unwrap();

        let target_file_set = EntryFileSetPathCipher::new(
            target_session
                .derive_stable_subkey(b"default", b"uniclipboard-file-set/v1")
                .unwrap(),
        );
        let target_transfer = TransferPersistenceCipher::new(
            target_session
                .derive_stable_subkey(b"default", b"uniclipboard-file-transfer-metadata/v1")
                .unwrap(),
            target_session
                .derive_stable_subkey(b"default", b"uniclipboard-file-transfer-events/v1")
                .unwrap(),
        );
        let target_render = RenderPayloadCodec::new(
            RenderKey::from_bytes(
                &target_session
                    .derive_stable_subkey(b"default", b"uniclipboard-search-render/v1")
                    .unwrap(),
            )
            .unwrap(),
        );
        let target_active = ActiveClipboardRegisterCipher::new(
            target_session
                .derive_stable_subkey(b"default", CONSUMABLE_HKDF_INFO)
                .unwrap(),
        );
        let target_publish = DirectoryPublishLogCipher::new(
            target_session
                .derive_stable_subkey(b"default", b"uniclipboard-directory-publish-log/v1")
                .unwrap(),
        );
        let target_artifact = ReceiveArtifactCipher::new(
            target_session
                .derive_stable_subkey(b"default", b"uniclipboard-receive-artifact-log/v1")
                .unwrap(),
        );
        let mut target =
            SqliteConnection::establish(finalized.database_path.to_str().unwrap()).unwrap();
        let file_set = diesel::sql_query(
            "SELECT original_text_ct, relative_path_ct, root_name_ct FROM entry_file_set \
             WHERE entry_id = ? AND line_index = 0",
        )
        .bind::<diesel::sql_types::Text, _>(entry_id.as_ref())
        .get_result::<PrivateFileSetRow>(&mut target)
        .unwrap();
        assert_eq!(
            target_file_set
                .open_original_text(&entry_id, 0, &file_set.original_text_ct)
                .unwrap(),
            "/private/source/report.txt"
        );
        assert_eq!(
            target_file_set
                .open_relative_path(&entry_id, 0, &file_set.relative_path_ct)
                .unwrap(),
            "folder/report.txt"
        );
        assert_eq!(
            target_file_set
                .open_root_name(&entry_id, 0, &file_set.root_name_ct)
                .unwrap(),
            "private-root"
        );
        let transfer = diesel::sql_query(
            "SELECT metadata_ciphertext FROM file_transfer WHERE transfer_id = 'transfer-private-data'",
        )
        .get_result::<PrivateTransferRow>(&mut target)
        .unwrap();
        assert_eq!(
            target_transfer
                .open_metadata("transfer-private-data", &transfer.metadata_ciphertext)
                .unwrap(),
            transfer_metadata
        );
        let search = diesel::sql_query(
            "SELECT render_payload FROM search_document WHERE profile_id = 'default' AND entry_id = ?",
        )
        .bind::<diesel::sql_types::Text, _>(entry_id.as_ref())
        .get_result::<PrivateSearchRow>(&mut target)
        .unwrap();
        assert_eq!(
            target_render
                .decrypt(&entry_id, &search.render_payload)
                .unwrap(),
            render_fields
        );
        let derived_search = diesel::sql_query(
            "SELECT \
             (SELECT COUNT(*) FROM search_posting) AS posting_count, \
             (SELECT COUNT(*) FROM search_entry_tag) AS tag_count, \
             (SELECT search_blocked FROM search_index_meta WHERE profile_id = 'default') AS search_blocked",
        )
        .get_result::<DerivedSearchStateRow>(&mut target)
        .unwrap();
        assert_eq!(derived_search.posting_count, 0);
        assert_eq!(derived_search.tag_count, 0);
        assert!(derived_search.search_blocked);
        let event = diesel::sql_query(
            "SELECT payload_ciphertext FROM file_transfer_events \
             WHERE transfer_id = 'transfer-private-data' AND sequence = 1",
        )
        .get_result::<PrivateTransferEventRow>(&mut target)
        .unwrap();
        assert_eq!(
            target_transfer
                .open_event(
                    "transfer-private-data",
                    1,
                    "started",
                    &event.payload_ciphertext,
                )
                .unwrap(),
            transfer_event
        );
        let active = diesel::sql_query(
            "SELECT consumable_ref_ciphertext FROM active_clipboard_register WHERE id = 1",
        )
        .get_result::<PrivateActiveRow>(&mut target)
        .unwrap();
        assert_eq!(
            target_active
                .open(&active.consumable_ref_ciphertext)
                .unwrap(),
            active_reference
        );
        let publish = diesel::sql_query(
            "SELECT root_map_ciphertext FROM directory_publish_log \
             WHERE entry_id = ? AND attempt_id = 'receive-attempt'",
        )
        .bind::<diesel::sql_types::Text, _>(entry_id.as_ref())
        .get_result::<PrivatePublishRow>(&mut target)
        .unwrap();
        assert_eq!(
            target_publish
                .open(&entry_id, "receive-attempt", &publish.root_map_ciphertext)
                .unwrap(),
            root_map
        );
        let artifact = diesel::sql_query(
            "SELECT artifact_ciphertext FROM receive_artifact_log \
             WHERE entry_id = ? AND attempt_id = 'receive-attempt'",
        )
        .bind::<diesel::sql_types::Text, _>(entry_id.as_ref())
        .get_result::<PrivateArtifactRow>(&mut target)
        .unwrap();
        assert_eq!(
            target_artifact
                .open(
                    entry_id.as_ref(),
                    "receive-attempt",
                    &artifact.artifact_ciphertext,
                )
                .unwrap(),
            artifacts
        );
        let target_blob_store = EncryptedBlobStore::new(
            Arc::new(FilesystemBlobStore::new(
                finalized.database_path.parent().unwrap().join("blobs"),
            )),
            target_session,
        );
        assert_eq!(
            BlobReaderPort::get(&target_blob_store, &blob_id)
                .await
                .unwrap(),
            b"private blob content"
        );
    }

    #[derive(diesel::QueryableByName)]
    struct InlineRow {
        #[diesel(sql_type = diesel::sql_types::Text)]
        id: String,
        #[diesel(sql_type = diesel::sql_types::Text)]
        event_id: String,
        #[diesel(sql_type = diesel::sql_types::Binary)]
        inline_data: Vec<u8>,
    }

    #[derive(diesel::QueryableByName)]
    struct PrivateFileSetRow {
        #[diesel(sql_type = diesel::sql_types::Binary)]
        original_text_ct: Vec<u8>,
        #[diesel(sql_type = diesel::sql_types::Binary)]
        relative_path_ct: Vec<u8>,
        #[diesel(sql_type = diesel::sql_types::Binary)]
        root_name_ct: Vec<u8>,
    }

    #[derive(diesel::QueryableByName)]
    struct PrivateTransferRow {
        #[diesel(sql_type = diesel::sql_types::Binary)]
        metadata_ciphertext: Vec<u8>,
    }

    #[derive(diesel::QueryableByName)]
    struct PrivateSearchRow {
        #[diesel(sql_type = diesel::sql_types::Binary)]
        render_payload: Vec<u8>,
    }

    #[derive(diesel::QueryableByName)]
    struct PrivateTransferEventRow {
        #[diesel(sql_type = diesel::sql_types::Binary)]
        payload_ciphertext: Vec<u8>,
    }

    #[derive(diesel::QueryableByName)]
    struct PrivateActiveRow {
        #[diesel(sql_type = diesel::sql_types::Binary)]
        consumable_ref_ciphertext: Vec<u8>,
    }

    #[derive(diesel::QueryableByName)]
    struct PrivatePublishRow {
        #[diesel(sql_type = diesel::sql_types::Binary)]
        root_map_ciphertext: Vec<u8>,
    }

    #[derive(diesel::QueryableByName)]
    struct PrivateArtifactRow {
        #[diesel(sql_type = diesel::sql_types::Binary)]
        artifact_ciphertext: Vec<u8>,
    }

    #[derive(diesel::QueryableByName)]
    struct DerivedSearchStateRow {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        posting_count: i64,
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        tag_count: i64,
        #[diesel(sql_type = diesel::sql_types::Bool)]
        search_blocked: bool,
    }
}
