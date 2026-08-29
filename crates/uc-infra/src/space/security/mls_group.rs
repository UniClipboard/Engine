use openmls::{
    group::MlsGroup,
    prelude::{tls_codec::*, *},
    treesync::LeafNodeParameters,
};
use openmls_basic_credential::SignatureKeyPair;
use openmls_memory_storage::MemoryStorage;
use openmls_rust_crypto::RustCrypto;
use openmls_traits::{
    OpenMlsProvider, crypto::OpenMlsCrypto, signatures::Signer, storage::StorageProvider,
    types::SignatureScheme,
};
use sha2::{Digest, Sha256};
use uc_core::membership::{
    ADMISSION_SECURITY_COMMITMENT_FORMAT_V1, AdmissionSecurityCommitmentV1,
    BaseMembershipHistoryPosition, ED25519_SIGNATURE_ALGORITHM_V1, MembershipCredential,
};

use crate::security::MasterKey;

const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
const STATE_VERSION: u8 = 1;
const EXPORT_LABEL: &str = "uniclipboard-key-catalog-wrap-v1";

#[derive(Debug, thiserror::Error)]
pub(crate) enum MlsGroupError {
    #[error("invalid MLS state")]
    InvalidState,
    #[error("invalid MLS message")]
    InvalidMessage,
    #[error("MLS credential identity mismatch")]
    IdentityMismatch,
    #[error("MLS protocol operation failed")]
    Protocol,
}

#[derive(Debug)]
struct SnapshotProvider {
    crypto: RustCrypto,
    storage: MemoryStorage,
}

impl Default for SnapshotProvider {
    fn default() -> Self {
        Self {
            crypto: RustCrypto::default(),
            storage: MemoryStorage::default(),
        }
    }
}

impl OpenMlsProvider for SnapshotProvider {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = MemoryStorage;

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }

    fn crypto(&self) -> &Self::CryptoProvider {
        &self.crypto
    }

    fn rand(&self) -> &Self::RandProvider {
        &self.crypto
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredClientState {
    version: u8,
    serialized_storage: Vec<u8>,
    signer_public: Vec<u8>,
    group_id: Option<Vec<u8>>,
}

pub(crate) struct MlsClientState {
    bytes: Vec<u8>,
}

impl MlsClientState {
    pub(crate) fn from_bytes(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl std::fmt::Debug for MlsClientState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MlsClientState")
            .field("bytes_len", &self.bytes.len())
            .finish()
    }
}

pub(crate) struct PendingMlsJoin {
    pub(crate) key_package: Vec<u8>,
    pub(crate) client_state: MlsClientState,
    /// Member instance derived from this admission's fresh credential,
    /// when the credential was generated during preparation.
    pub(crate) member_instance: Option<uc_core::membership::MemberInstanceId>,
}

impl PendingMlsJoin {
    pub(crate) fn new(key_package: Vec<u8>, client_state: MlsClientState) -> Self {
        Self {
            key_package,
            client_state,
            member_instance: None,
        }
    }

    pub(crate) fn with_member_instance(
        mut self,
        instance: uc_core::membership::MemberInstanceId,
    ) -> Self {
        self.member_instance = Some(instance);
        self
    }
}

impl std::fmt::Debug for PendingMlsJoin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingMlsJoin")
            .field("key_package_len", &self.key_package.len())
            .field("client_state", &self.client_state)
            .field(
                "member_instance",
                &self
                    .member_instance
                    .map_or_else(|| "none".to_owned(), |own| own.to_string()),
            )
            .finish()
    }
}

pub(crate) struct MlsAdmission {
    pub(crate) sponsor_state: MlsClientState,
    pub(crate) commit: Vec<u8>,
    pub(crate) welcome: Vec<u8>,
    pub(crate) epoch: u64,
    pub(crate) wrapping_key: MasterKey,
}

impl std::fmt::Debug for MlsAdmission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MlsAdmission")
            .field("sponsor_state", &self.sponsor_state)
            .field("welcome_len", &self.welcome.len())
            .field("commit_len", &self.commit.len())
            .field("epoch", &self.epoch)
            .field("wrapping_key", &"[REDACTED]")
            .finish()
    }
}

pub(crate) struct CompletedMlsJoin {
    pub(crate) client_state: MlsClientState,
    pub(crate) epoch: u64,
    pub(crate) wrapping_key: MasterKey,
}

pub(crate) struct MlsRemoval {
    pub(crate) sponsor_state: MlsClientState,
    pub(crate) commit: Vec<u8>,
    pub(crate) epoch: u64,
    pub(crate) wrapping_key: MasterKey,
}

impl std::fmt::Debug for MlsRemoval {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MlsRemoval")
            .field("sponsor_state", &self.sponsor_state)
            .field("commit_len", &self.commit.len())
            .field("epoch", &self.epoch)
            .field("wrapping_key", &"[REDACTED]")
            .finish()
    }
}

impl std::fmt::Debug for CompletedMlsJoin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CompletedMlsJoin")
            .field("client_state", &self.client_state)
            .field("epoch", &self.epoch)
            .field("wrapping_key", &"[REDACTED]")
            .finish()
    }
}

pub(crate) struct MlsGroupEngine;

impl MlsGroupEngine {
    pub(crate) fn validate_state(
        client_state: &MlsClientState,
        expected_space_id: &[u8],
    ) -> Result<(), MlsGroupError> {
        let (provider, stored) = restore(client_state)?;
        let group_id = stored.group_id.ok_or(MlsGroupError::InvalidState)?;
        if group_id != expected_space_id {
            return Err(MlsGroupError::IdentityMismatch);
        }
        let group = MlsGroup::load(provider.storage(), &GroupId::from_slice(&group_id))
            .map_err(|_| MlsGroupError::Protocol)?
            .ok_or(MlsGroupError::InvalidState)?;
        if !group.is_active() {
            return Err(MlsGroupError::InvalidState);
        }
        Ok(())
    }

    pub(crate) fn create_sponsor(
        space_id: &[u8],
        device_identity: &[u8],
    ) -> Result<MlsClientState, MlsGroupError> {
        let provider = SnapshotProvider::default();
        let (credential, signer) = credential(device_identity, &provider)?;
        let config = group_config();
        let mut group = MlsGroup::new_with_group_id(
            &provider,
            &signer,
            &config,
            GroupId::from_slice(space_id),
            credential,
        )
        .map_err(|_| MlsGroupError::Protocol)?;
        group
            .self_update(&provider, &signer, LeafNodeParameters::default())
            .map_err(|_| MlsGroupError::Protocol)?;
        group
            .merge_pending_commit(&provider)
            .map_err(|_| MlsGroupError::Protocol)?;
        snapshot(&provider, &signer, Some(group.group_id().as_slice()))
    }

    pub(crate) fn prepare_join(device_identity: &[u8]) -> Result<PendingMlsJoin, MlsGroupError> {
        let provider = SnapshotProvider::default();
        let (credential, signer) = credential(device_identity, &provider)?;
        let bundle = KeyPackage::builder()
            .build(CIPHERSUITE, &provider, &signer, credential)
            .map_err(|_| MlsGroupError::Protocol)?;
        let key_package = bundle
            .key_package()
            .tls_serialize_detached()
            .map_err(|_| MlsGroupError::Protocol)?;
        let client_state = snapshot(&provider, &signer, None)?;
        let member_instance = uc_core::membership::MemberInstanceId::derive(
            std::str::from_utf8(device_identity).unwrap_or_default(),
            &signer.to_public_vec(),
        );
        Ok(PendingMlsJoin::new(key_package, client_state).with_member_instance(member_instance))
    }

    pub(crate) fn admit_member(
        sponsor_state: &MlsClientState,
        expected_device_identity: &[u8],
        key_package: &[u8],
    ) -> Result<MlsAdmission, MlsGroupError> {
        let (provider, stored) = restore(sponsor_state)?;
        let signer = restore_signer(&provider, &stored)?;
        let group_id = stored.group_id.ok_or(MlsGroupError::InvalidState)?;
        let mut group = MlsGroup::load(provider.storage(), &GroupId::from_slice(&group_id))
            .map_err(|_| MlsGroupError::Protocol)?
            .ok_or(MlsGroupError::InvalidState)?;
        let key_package = KeyPackageIn::tls_deserialize_exact(key_package.to_vec())
            .map_err(|_| MlsGroupError::InvalidMessage)?
            .validate(provider.crypto(), ProtocolVersion::Mls10)
            .map_err(|_| MlsGroupError::InvalidMessage)?;
        let credential = BasicCredential::try_from(key_package.leaf_node().credential().clone())
            .map_err(|_| MlsGroupError::InvalidMessage)?;
        if credential.identity() != expected_device_identity {
            return Err(MlsGroupError::IdentityMismatch);
        }
        let (commit, welcome, _) = group
            .add_members(&provider, &signer, &[key_package])
            .map_err(|_| MlsGroupError::Protocol)?;
        group
            .merge_pending_commit(&provider)
            .map_err(|_| MlsGroupError::Protocol)?;
        let wrapping_key = export_wrapping_key(&group, &provider)?;
        let epoch = group.epoch().as_u64();
        let welcome = welcome
            .tls_serialize_detached()
            .map_err(|_| MlsGroupError::Protocol)?;
        let commit = commit
            .tls_serialize_detached()
            .map_err(|_| MlsGroupError::Protocol)?;
        let sponsor_state = snapshot(&provider, &signer, Some(group.group_id().as_slice()))?;
        Ok(MlsAdmission {
            sponsor_state,
            commit,
            welcome,
            epoch,
            wrapping_key,
        })
    }

    pub(crate) fn complete_join(
        pending: PendingMlsJoin,
        expected_space_id: &[u8],
        welcome: &[u8],
    ) -> Result<CompletedMlsJoin, MlsGroupError> {
        let message = MlsMessageIn::tls_deserialize_exact(welcome.to_vec()).map_err(|_| {
            tracing::warn!(failure = "welcome_decode_failed", "MLS welcome rejected");
            MlsGroupError::InvalidMessage
        })?;
        let welcome = match message.extract() {
            MlsMessageBodyIn::Welcome(welcome) => welcome,
            _ => {
                tracing::warn!(
                    failure = "welcome_message_type_invalid",
                    "MLS welcome rejected"
                );
                return Err(MlsGroupError::InvalidMessage);
            }
        };
        Self::complete_join_from_welcome(pending, expected_space_id, welcome)
    }

    fn complete_join_from_welcome(
        pending: PendingMlsJoin,
        expected_space_id: &[u8],
        welcome: Welcome,
    ) -> Result<CompletedMlsJoin, MlsGroupError> {
        let (provider, stored) = restore(&pending.client_state)?;
        let signer = restore_signer(&provider, &stored)?;
        let staged =
            StagedWelcome::new_from_welcome(&provider, group_config().join_config(), welcome, None)
                .map_err(|_| {
                    tracing::warn!(failure = "welcome_staging_failed", "MLS welcome rejected");
                    MlsGroupError::Protocol
                })?;
        let group = staged.into_group(&provider).map_err(|_| {
            tracing::warn!(failure = "welcome_install_failed", "MLS welcome rejected");
            MlsGroupError::Protocol
        })?;
        if group.group_id().as_slice() != expected_space_id {
            tracing::warn!(failure = "welcome_space_mismatch", "MLS welcome rejected");
            return Err(MlsGroupError::IdentityMismatch);
        }
        let wrapping_key = export_wrapping_key(&group, &provider)?;
        let epoch = group.epoch().as_u64();
        let client_state = snapshot(&provider, &signer, Some(group.group_id().as_slice()))?;
        Ok(CompletedMlsJoin {
            client_state,
            epoch,
            wrapping_key,
        })
    }

    pub(crate) fn remove_member(
        sponsor_state: &MlsClientState,
        target_device_identity: &[u8],
    ) -> Result<MlsRemoval, MlsGroupError> {
        let (provider, stored) = restore(sponsor_state)?;
        let signer = restore_signer(&provider, &stored)?;
        let group_id = stored.group_id.ok_or(MlsGroupError::InvalidState)?;
        let mut group = MlsGroup::load(provider.storage(), &GroupId::from_slice(&group_id))
            .map_err(|_| MlsGroupError::Protocol)?
            .ok_or(MlsGroupError::InvalidState)?;
        let target = group
            .members()
            .find_map(|member| {
                let credential = BasicCredential::try_from(member.credential).ok()?;
                (credential.identity() == target_device_identity).then_some(member.index)
            })
            .ok_or(MlsGroupError::IdentityMismatch)?;
        if target == group.own_leaf_index() {
            return Err(MlsGroupError::IdentityMismatch);
        }
        let (commit, _, _) = group
            .remove_members(&provider, &signer, &[target])
            .map_err(|_| MlsGroupError::Protocol)?;
        group
            .merge_pending_commit(&provider)
            .map_err(|_| MlsGroupError::Protocol)?;
        let wrapping_key = export_wrapping_key(&group, &provider)?;
        let epoch = group.epoch().as_u64();
        let commit = commit
            .tls_serialize_detached()
            .map_err(|_| MlsGroupError::Protocol)?;
        let sponsor_state = snapshot(&provider, &signer, Some(group.group_id().as_slice()))?;
        Ok(MlsRemoval {
            sponsor_state,
            commit,
            epoch,
            wrapping_key,
        })
    }

    pub(crate) fn contains_active_member(
        client_state: &MlsClientState,
        expected_device_identity: &[u8],
    ) -> Result<bool, MlsGroupError> {
        let (provider, stored) = restore(client_state)?;
        let group_id = stored.group_id.ok_or(MlsGroupError::InvalidState)?;
        let group = MlsGroup::load(provider.storage(), &GroupId::from_slice(&group_id))
            .map_err(|_| MlsGroupError::Protocol)?
            .ok_or(MlsGroupError::InvalidState)?;
        if !group.is_active() {
            return Ok(false);
        }
        let contains_member = group.members().any(|member| {
            BasicCredential::try_from(member.credential)
                .is_ok_and(|credential| credential.identity() == expected_device_identity)
        });
        Ok(contains_member)
    }

    pub(crate) fn sign_member_payload(
        client_state: &MlsClientState,
        payload: &[u8],
    ) -> Result<Vec<u8>, MlsGroupError> {
        let (provider, stored) = restore(client_state)?;
        let signer = restore_signer(&provider, &stored)?;
        let group_id = stored.group_id.ok_or(MlsGroupError::InvalidState)?;
        let group = MlsGroup::load(provider.storage(), &GroupId::from_slice(&group_id))
            .map_err(|_| MlsGroupError::Protocol)?
            .ok_or(MlsGroupError::InvalidState)?;
        if !group.is_active() {
            return Err(MlsGroupError::InvalidState);
        }
        signer.sign(payload).map_err(|_| MlsGroupError::Protocol)
    }

    pub(crate) fn signing_public_key(
        client_state: &MlsClientState,
    ) -> Result<Vec<u8>, MlsGroupError> {
        let (_, stored) = restore(client_state)?;
        if stored.signer_public.is_empty() {
            return Err(MlsGroupError::InvalidState);
        }
        Ok(stored.signer_public)
    }

    pub(crate) fn sign_pending_member_payload(
        client_state: &MlsClientState,
        payload: &[u8],
    ) -> Result<Vec<u8>, MlsGroupError> {
        let (provider, stored) = restore(client_state)?;
        if stored.group_id.is_some() {
            return Err(MlsGroupError::InvalidState);
        }
        let signer = restore_signer(&provider, &stored)?;
        signer.sign(payload).map_err(|_| MlsGroupError::Protocol)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn derive_public_admission_commitment(
        client_state: &MlsClientState,
        attempt_id: [u8; 32],
        base_history_position: BaseMembershipHistoryPosition,
        candidate_core_digest: [u8; 32],
        commit: &[u8],
        key_catalog_digest: [u8; 32],
        admission_bundle_digest: [u8; 32],
    ) -> Result<AdmissionSecurityCommitmentV1, MlsGroupError> {
        let local_signer_public = Self::signing_public_key(client_state)?;
        let (provider, stored) = restore(client_state)?;
        let group_id = stored.group_id.ok_or(MlsGroupError::InvalidState)?;
        let group = MlsGroup::load(provider.storage(), &GroupId::from_slice(&group_id))
            .map_err(|_| MlsGroupError::Protocol)?
            .ok_or(MlsGroupError::InvalidState)?;
        if !group.is_active() {
            return Err(MlsGroupError::InvalidState);
        }

        let mut members = Vec::new();
        let mut contains_local_signer = false;
        for member in group.members() {
            let identity = BasicCredential::try_from(member.credential)
                .map_err(|_| MlsGroupError::InvalidState)?
                .identity()
                .to_vec();
            let device_id =
                std::str::from_utf8(&identity).map_err(|_| MlsGroupError::IdentityMismatch)?;
            let public_key = member.signature_key.as_slice();
            contains_local_signer |= public_key == local_signer_public;
            let credential =
                MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, public_key.to_vec());
            members.push((
                credential.member_instance_id(&uc_core::ids::DeviceId::new(device_id)),
                credential.credential_id,
            ));
        }
        if !contains_local_signer {
            return Err(MlsGroupError::InvalidState);
        }
        members.sort_unstable();
        let mut member_bytes = Vec::with_capacity(members.len() * 64 + 8);
        member_bytes.extend_from_slice(&(members.len() as u64).to_be_bytes());
        for (member, credential) in members {
            member_bytes.extend_from_slice(member.as_bytes());
            member_bytes.extend_from_slice(credential.as_bytes());
        }
        let group_context: GroupContext = provider
            .storage()
            .group_context(&GroupId::from_slice(&group_id))
            .map_err(|_| MlsGroupError::Protocol)?
            .ok_or(MlsGroupError::InvalidState)?;
        let group_context = group_context
            .tls_serialize_detached()
            .map_err(|_| MlsGroupError::Protocol)?;
        let target_epoch = group.epoch().as_u64();
        let base_epoch = target_epoch
            .checked_sub(1)
            .ok_or(MlsGroupError::InvalidState)?;

        AdmissionSecurityCommitmentV1::new(
            ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
            String::from_utf8(group_id.clone()).map_err(|_| MlsGroupError::IdentityMismatch)?,
            group_id,
            attempt_id,
            base_history_position,
            candidate_core_digest,
            u16::from(group.ciphersuite()),
            base_epoch,
            target_epoch,
            domain_digest(b"uniclipboard/mls-commit/v1\0", commit),
            domain_digest(b"uniclipboard/mls-group-context/v1\0", &group_context),
            domain_digest(b"uniclipboard/mls-member-credentials/v1\0", &member_bytes),
            key_catalog_digest,
            admission_bundle_digest,
        )
        .map_err(|_| MlsGroupError::InvalidState)
    }

    pub(crate) fn current_epoch(client_state: &MlsClientState) -> Result<u64, MlsGroupError> {
        let (provider, stored) = restore(client_state)?;
        let group_id = stored.group_id.ok_or(MlsGroupError::InvalidState)?;
        let group = MlsGroup::load(provider.storage(), &GroupId::from_slice(&group_id))
            .map_err(|_| MlsGroupError::Protocol)?
            .ok_or(MlsGroupError::InvalidState)?;
        if !group.is_active() {
            return Err(MlsGroupError::InvalidState);
        }
        Ok(group.epoch().as_u64())
    }

    pub(crate) fn current_member_instance(
        client_state: &MlsClientState,
        device_identity: &[u8],
    ) -> Result<uc_core::membership::MemberInstanceId, MlsGroupError> {
        let (provider, stored) = restore(client_state)?;
        let group_id = stored
            .group_id
            .as_ref()
            .ok_or(MlsGroupError::InvalidState)?;
        let group = MlsGroup::load(provider.storage(), &GroupId::from_slice(group_id))
            .map_err(|_| MlsGroupError::Protocol)?
            .ok_or(MlsGroupError::InvalidState)?;
        if !group.is_active() {
            return Err(MlsGroupError::InvalidState);
        }
        Ok(uc_core::membership::MemberInstanceId::derive(
            std::str::from_utf8(device_identity).map_err(|_| MlsGroupError::IdentityMismatch)?,
            &stored.signer_public,
        ))
    }

    pub(crate) fn verify_member_payload(
        client_state: &MlsClientState,
        expected_device_identity: &[u8],
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, MlsGroupError> {
        let (provider, stored) = restore(client_state)?;
        let group_id = stored.group_id.ok_or(MlsGroupError::InvalidState)?;
        let group = MlsGroup::load(provider.storage(), &GroupId::from_slice(&group_id))
            .map_err(|_| MlsGroupError::Protocol)?
            .ok_or(MlsGroupError::InvalidState)?;
        if !group.is_active() {
            return Ok(false);
        }
        let signature_key = group.members().find_map(|member| {
            let credential = BasicCredential::try_from(member.credential).ok()?;
            (credential.identity() == expected_device_identity).then_some(member.signature_key)
        });
        let Some(signature_key) = signature_key else {
            return Ok(false);
        };
        Ok(provider
            .crypto()
            .verify_signature(
                CIPHERSUITE.signature_algorithm(),
                payload,
                &signature_key,
                signature,
            )
            .is_ok())
    }

    pub(crate) fn verify_member_instance_payload(
        client_state: &MlsClientState,
        expected_device_identity: &[u8],
        expected_member_instance: uc_core::membership::MemberInstanceId,
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, MlsGroupError> {
        let (provider, stored) = restore(client_state)?;
        let group_id = stored.group_id.ok_or(MlsGroupError::InvalidState)?;
        let group = MlsGroup::load(provider.storage(), &GroupId::from_slice(&group_id))
            .map_err(|_| MlsGroupError::Protocol)?
            .ok_or(MlsGroupError::InvalidState)?;
        if !group.is_active() {
            return Ok(false);
        }
        let device_id = std::str::from_utf8(expected_device_identity)
            .map_err(|_| MlsGroupError::IdentityMismatch)?;
        let signature_key = group.members().find_map(|member| {
            let credential = BasicCredential::try_from(member.credential).ok()?;
            let instance = uc_core::membership::MemberInstanceId::derive(
                device_id,
                member.signature_key.as_slice(),
            );
            (credential.identity() == expected_device_identity
                && instance == expected_member_instance)
                .then_some(member.signature_key)
        });
        let Some(signature_key) = signature_key else {
            return Ok(false);
        };
        Ok(provider
            .crypto()
            .verify_signature(
                CIPHERSUITE.signature_algorithm(),
                payload,
                &signature_key,
                signature,
            )
            .is_ok())
    }

    pub(crate) fn apply_commit(
        client_state: &MlsClientState,
        expected_space_id: &[u8],
        commit: &[u8],
    ) -> Result<CompletedMlsJoin, MlsGroupError> {
        let (provider, stored) = restore(client_state)?;
        let signer = restore_signer(&provider, &stored)?;
        let group_id = stored.group_id.ok_or(MlsGroupError::InvalidState)?;
        if group_id != expected_space_id {
            return Err(MlsGroupError::IdentityMismatch);
        }
        let mut group = MlsGroup::load(provider.storage(), &GroupId::from_slice(&group_id))
            .map_err(|_| MlsGroupError::Protocol)?
            .ok_or(MlsGroupError::InvalidState)?;
        let message = MlsMessageIn::tls_deserialize_exact(commit.to_vec())
            .map_err(|_| MlsGroupError::InvalidMessage)?
            .try_into_protocol_message()
            .map_err(|_| MlsGroupError::InvalidMessage)?;
        let processed = group
            .process_message(&provider, message)
            .map_err(|_| MlsGroupError::Protocol)?;
        let ProcessedMessageContent::StagedCommitMessage(staged) = processed.into_content() else {
            return Err(MlsGroupError::InvalidMessage);
        };
        group
            .merge_staged_commit(&provider, *staged)
            .map_err(|_| MlsGroupError::Protocol)?;
        if !group.is_active() {
            return Err(MlsGroupError::IdentityMismatch);
        }
        let wrapping_key = export_wrapping_key(&group, &provider)?;
        let epoch = group.epoch().as_u64();
        let client_state = snapshot(&provider, &signer, Some(group.group_id().as_slice()))?;
        Ok(CompletedMlsJoin {
            client_state,
            epoch,
            wrapping_key,
        })
    }
}

fn domain_digest(domain: &[u8], value: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
    hasher.finalize().into()
}

fn group_config() -> MlsGroupCreateConfig {
    MlsGroupCreateConfig::builder()
        .ciphersuite(CIPHERSUITE)
        .use_ratchet_tree_extension(true)
        .build()
}

fn credential(
    identity: &[u8],
    provider: &impl OpenMlsProvider,
) -> Result<(CredentialWithKey, SignatureKeyPair), MlsGroupError> {
    let signer = SignatureKeyPair::new(CIPHERSUITE.signature_algorithm())
        .map_err(|_| MlsGroupError::Protocol)?;
    signer
        .store(provider.storage())
        .map_err(|_| MlsGroupError::Protocol)?;
    Ok((
        CredentialWithKey {
            credential: BasicCredential::new(identity.to_vec()).into(),
            signature_key: signer.to_public_vec().into(),
        },
        signer,
    ))
}

fn snapshot(
    provider: &SnapshotProvider,
    signer: &SignatureKeyPair,
    group_id: Option<&[u8]>,
) -> Result<MlsClientState, MlsGroupError> {
    let mut serialized_storage = Vec::new();
    provider
        .storage
        .serialize(&mut serialized_storage)
        .map_err(|_| MlsGroupError::InvalidState)?;
    let stored = StoredClientState {
        version: STATE_VERSION,
        serialized_storage,
        signer_public: signer.to_public_vec(),
        group_id: group_id.map(ToOwned::to_owned),
    };
    let bytes = serde_json::to_vec(&stored).map_err(|_| MlsGroupError::InvalidState)?;
    Ok(MlsClientState { bytes })
}

fn restore(state: &MlsClientState) -> Result<(SnapshotProvider, StoredClientState), MlsGroupError> {
    let stored: StoredClientState =
        serde_json::from_slice(state.as_bytes()).map_err(|_| MlsGroupError::InvalidState)?;
    if stored.version != STATE_VERSION {
        return Err(MlsGroupError::InvalidState);
    }
    let storage = MemoryStorage::deserialize(&mut stored.serialized_storage.as_slice())
        .map_err(|_| MlsGroupError::InvalidState)?;
    let provider = SnapshotProvider {
        crypto: RustCrypto::default(),
        storage,
    };
    Ok((provider, stored))
}

fn restore_signer(
    provider: &SnapshotProvider,
    stored: &StoredClientState,
) -> Result<SignatureKeyPair, MlsGroupError> {
    SignatureKeyPair::read(
        provider.storage(),
        &stored.signer_public,
        SignatureScheme::ED25519,
    )
    .ok_or(MlsGroupError::InvalidState)
}

fn export_wrapping_key(
    group: &MlsGroup,
    provider: &SnapshotProvider,
) -> Result<MasterKey, MlsGroupError> {
    let bytes = group
        .export_secret(provider.crypto(), EXPORT_LABEL, b"", 32)
        .map_err(|_| MlsGroupError::Protocol)?;
    MasterKey::from_bytes(&bytes).map_err(|_| MlsGroupError::Protocol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::OpenMlsHistoricalSignatureVerifier;
    use uc_core::membership::{
        BaseMembershipHistoryPosition, ED25519_SIGNATURE_ALGORITHM_V1,
        HistoricalMembershipSignatureVerifier, MembershipEventId,
    };

    #[test]
    fn sponsor_and_joiner_export_the_same_epoch_wrapping_key_after_cold_restore() {
        let sponsor = MlsGroupEngine::create_sponsor(b"space-a", b"alice").unwrap();
        let pending = MlsGroupEngine::prepare_join(b"bob").unwrap();
        let admission =
            MlsGroupEngine::admit_member(&sponsor, b"bob", &pending.key_package).unwrap();
        let joined =
            MlsGroupEngine::complete_join(pending, b"space-a", &admission.welcome).unwrap();

        assert_eq!(admission.epoch, 2);
        assert_eq!(joined.epoch, admission.epoch);
        assert_eq!(joined.wrapping_key, admission.wrapping_key);

        let charlie = MlsGroupEngine::prepare_join(b"charlie").unwrap();
        let next = MlsGroupEngine::admit_member(
            &MlsClientState::from_bytes(admission.sponsor_state.into_bytes()),
            b"charlie",
            &charlie.key_package,
        )
        .unwrap();
        assert_eq!(next.epoch, 3);
        assert_ne!(next.wrapping_key, joined.wrapping_key);
    }

    #[test]
    fn key_package_identity_must_match_the_pairing_request() {
        let sponsor = MlsGroupEngine::create_sponsor(b"space-a", b"alice").unwrap();
        let pending = MlsGroupEngine::prepare_join(b"bob").unwrap();

        assert!(matches!(
            MlsGroupEngine::admit_member(&sponsor, b"mallory", &pending.key_package),
            Err(MlsGroupError::IdentityMismatch)
        ));
    }

    #[test]
    fn welcome_cannot_be_opened_by_a_different_pending_join() {
        let sponsor = MlsGroupEngine::create_sponsor(b"space-a", b"alice").unwrap();
        let bob = MlsGroupEngine::prepare_join(b"bob").unwrap();
        let admission = MlsGroupEngine::admit_member(&sponsor, b"bob", &bob.key_package).unwrap();
        let mallory = MlsGroupEngine::prepare_join(b"mallory").unwrap();

        assert!(MlsGroupEngine::complete_join(mallory, b"space-a", &admission.welcome).is_err());
    }

    #[test]
    fn retained_members_converge_after_admission_and_removal_commits() {
        let alice = MlsGroupEngine::create_sponsor(b"space-a", b"alice").unwrap();
        let bob_pending = MlsGroupEngine::prepare_join(b"bob").unwrap();
        let bob_admission =
            MlsGroupEngine::admit_member(&alice, b"bob", &bob_pending.key_package).unwrap();
        let bob =
            MlsGroupEngine::complete_join(bob_pending, b"space-a", &bob_admission.welcome).unwrap();

        let charlie_pending = MlsGroupEngine::prepare_join(b"charlie").unwrap();
        let charlie_admission = MlsGroupEngine::admit_member(
            &bob_admission.sponsor_state,
            b"charlie",
            &charlie_pending.key_package,
        )
        .unwrap();
        let bob_after_admission =
            MlsGroupEngine::apply_commit(&bob.client_state, b"space-a", &charlie_admission.commit)
                .unwrap();
        let charlie =
            MlsGroupEngine::complete_join(charlie_pending, b"space-a", &charlie_admission.welcome)
                .unwrap();
        assert_eq!(bob_after_admission.epoch, charlie_admission.epoch);
        assert_eq!(bob_after_admission.wrapping_key, charlie.wrapping_key);

        let removal =
            MlsGroupEngine::remove_member(&charlie_admission.sponsor_state, b"charlie").unwrap();
        let bob_after_removal = MlsGroupEngine::apply_commit(
            &bob_after_admission.client_state,
            b"space-a",
            &removal.commit,
        )
        .unwrap();

        assert_eq!(removal.epoch, charlie_admission.epoch + 1);
        assert_eq!(bob_after_removal.epoch, removal.epoch);
        assert_eq!(bob_after_removal.wrapping_key, removal.wrapping_key);
        assert_ne!(charlie.wrapping_key, removal.wrapping_key);
        assert!(matches!(
            MlsGroupEngine::apply_commit(&charlie.client_state, b"space-a", &removal.commit,),
            Err(MlsGroupError::IdentityMismatch)
        ));
    }

    #[test]
    fn member_signature_verifies_from_another_current_member_tree() {
        let alice = MlsGroupEngine::create_sponsor(b"space-a", b"alice").unwrap();
        let bob_pending = MlsGroupEngine::prepare_join(b"bob").unwrap();
        let admission =
            MlsGroupEngine::admit_member(&alice, b"bob", &bob_pending.key_package).unwrap();
        let bob =
            MlsGroupEngine::complete_join(bob_pending, b"space-a", &admission.welcome).unwrap();
        let payload = b"member-attestation-transcript";

        let signature =
            MlsGroupEngine::sign_member_payload(&admission.sponsor_state, payload).unwrap();

        assert!(MlsGroupEngine::verify_member_payload(
            &bob.client_state,
            b"alice",
            payload,
            &signature,
        )
        .unwrap());
    }

    #[test]
    fn member_signature_rejects_changed_payload_and_wrong_identity() {
        let alice = MlsGroupEngine::create_sponsor(b"space-a", b"alice").unwrap();
        let bob_pending = MlsGroupEngine::prepare_join(b"bob").unwrap();
        let admission =
            MlsGroupEngine::admit_member(&alice, b"bob", &bob_pending.key_package).unwrap();
        let bob =
            MlsGroupEngine::complete_join(bob_pending, b"space-a", &admission.welcome).unwrap();
        let signature = MlsGroupEngine::sign_member_payload(
            &admission.sponsor_state,
            b"member-attestation-transcript",
        )
        .unwrap();

        assert!(
            !MlsGroupEngine::verify_member_payload(
                &bob.client_state,
                b"alice",
                b"changed-transcript",
                &signature,
            )
            .unwrap()
        );
        assert!(
            !MlsGroupEngine::verify_member_payload(
                &bob.client_state,
                b"bob",
                b"member-attestation-transcript",
                &signature,
            )
            .unwrap()
        );
    }

    #[test]
    fn removed_member_signature_is_rejected_by_current_member_tree() {
        let alice = MlsGroupEngine::create_sponsor(b"space-a", b"alice").unwrap();
        let bob_pending = MlsGroupEngine::prepare_join(b"bob").unwrap();
        let admission =
            MlsGroupEngine::admit_member(&alice, b"bob", &bob_pending.key_package).unwrap();
        let bob =
            MlsGroupEngine::complete_join(bob_pending, b"space-a", &admission.welcome).unwrap();
        let payload = b"member-attestation-transcript";
        let signature = MlsGroupEngine::sign_member_payload(&bob.client_state, payload).unwrap();
        let removal = MlsGroupEngine::remove_member(&admission.sponsor_state, b"bob").unwrap();

        assert!(
            !MlsGroupEngine::verify_member_payload(
                &removal.sponsor_state,
                b"bob",
                payload,
                &signature,
            )
            .unwrap()
        );
    }

    #[test]
    fn historical_signature_verification_uses_only_the_explicit_public_key() {
        let state = MlsGroupEngine::create_sponsor(b"space-a", b"alice").unwrap();
        let payload = b"historical-membership-event";
        let signature = MlsGroupEngine::sign_member_payload(&state, payload).unwrap();
        let public_key = MlsGroupEngine::signing_public_key(&state).unwrap();
        let verifier = OpenMlsHistoricalSignatureVerifier;

        assert_eq!(
            verifier.verify(
                ED25519_SIGNATURE_ALGORITHM_V1,
                &public_key,
                payload,
                &signature,
            ),
            Ok(true)
        );
        assert_eq!(
            verifier.verify(
                ED25519_SIGNATURE_ALGORITHM_V1,
                &public_key,
                b"altered",
                &signature,
            ),
            Ok(false)
        );
    }

    #[test]
    fn prepared_join_signer_proves_facts_without_activating_a_group() {
        let pending = MlsGroupEngine::prepare_join(b"bob").unwrap();
        let public_key = MlsGroupEngine::signing_public_key(&pending.client_state).unwrap();
        let payload = b"pre-admission-member-facts";
        let signature =
            MlsGroupEngine::sign_pending_member_payload(&pending.client_state, payload).unwrap();

        assert!(MlsGroupEngine::sign_member_payload(&pending.client_state, payload).is_err());
        assert_eq!(
            OpenMlsHistoricalSignatureVerifier.verify(
                ED25519_SIGNATURE_ALGORITHM_V1,
                &public_key,
                payload,
                &signature,
            ),
            Ok(true)
        );
    }

    #[test]
    fn sponsor_and_joiner_derive_the_same_public_admission_commitment() {
        let sponsor = MlsGroupEngine::create_sponsor(b"space-a", b"alice").unwrap();
        let pending = MlsGroupEngine::prepare_join(b"bob").unwrap();
        let admission =
            MlsGroupEngine::admit_member(&sponsor, b"bob", &pending.key_package).unwrap();
        let joined =
            MlsGroupEngine::complete_join(pending, b"space-a", &admission.welcome).unwrap();
        assert_ne!(
            admission.sponsor_state.as_bytes(),
            joined.client_state.as_bytes()
        );
        let base = BaseMembershipHistoryPosition {
            event_id: Some(
                MembershipEventId::from_hex(&"11".repeat(32)).expect("test event id is valid"),
            ),
            depth: 4,
            history_digest: [2; 32],
        };

        let sponsor_commitment = MlsGroupEngine::derive_public_admission_commitment(
            &admission.sponsor_state,
            [3; 32],
            base.clone(),
            [4; 32],
            &admission.commit,
            [5; 32],
            [6; 32],
        )
        .unwrap();
        let joiner_commitment = MlsGroupEngine::derive_public_admission_commitment(
            &joined.client_state,
            [3; 32],
            base,
            [4; 32],
            &admission.commit,
            [5; 32],
            [6; 32],
        )
        .unwrap();

        assert_eq!(sponsor_commitment, joiner_commitment);
    }
}
