use std::sync::Arc;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    CandidateStatus, DeviceAnnouncement, PendingMembershipBatch, RelationshipStateResetError,
    RelationshipStateResetPort, RelayedSecurityUpdate, SpaceMembershipCandidate,
    VerifiedPeerPromotionError, VerifiedPeerPromotionPort,
};
use uc_core::ports::security::current_profile::CurrentProfilePort;
use uc_core::ports::space::DeriveSpaceSubkeyPort;
use uc_core::ports::PeerAddressRecord;
use uc_core::security::IdentityFingerprint;
use uc_core::{MemberSyncPreferences, SpaceMember, TrustedPeer};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::db::models::{EncryptedRelationshipRow, NewEncryptedRelationshipRow};
use crate::db::ports::DbExecutor;
use crate::db::schema::{encrypted_relationship, relationship_privacy_maintenance};
use crate::security::v1_aead::{decrypt_xchacha_raw, encrypt_xchacha_raw};

const RELATIONSHIP_KEY_INFO: &[u8] = b"uniclipboard-relationship/v1";
const RELATIONSHIP_MAGIC: [u8; 4] = *b"UCRL";
const RELATIONSHIP_FORMAT_VERSION: u8 = 1;
const NONCE_LEN: usize = 24;
const HEADER_LEN: usize = 4 + 1 + NONCE_LEN;

#[derive(Debug, thiserror::Error)]
pub enum RelationshipStoreError {
    #[error("relationship store is locked")]
    Locked,
    #[error("relationship ciphertext is invalid")]
    InvalidCiphertext,
    #[error("relationship storage failed: {0}")]
    Storage(String),
}

#[derive(Clone, Copy)]
enum RelationshipKind {
    Member,
    TrustedPeer,
    PeerAddress,
    Candidate,
    MembershipAnnouncement,
    MembershipOutbox,
    MembershipAppliedSecurityUpdate,
}

impl RelationshipKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::TrustedPeer => "trusted_peer",
            Self::PeerAddress => "peer_address",
            Self::Candidate => "candidate",
            Self::MembershipAnnouncement => "membership_announcement",
            Self::MembershipOutbox => "membership_outbox",
            Self::MembershipAppliedSecurityUpdate => "membership_applied_security_update",
        }
    }
}

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
struct RelationshipCipher {
    key: [u8; 32],
}

impl RelationshipCipher {
    fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    fn lookup_key(&self, kind: RelationshipKind, identity: &str) -> Vec<u8> {
        let mut input = Vec::with_capacity(kind.as_str().len() + identity.len() + 1);
        input.extend_from_slice(kind.as_str().as_bytes());
        input.push(0);
        input.extend_from_slice(identity.as_bytes());
        blake3::keyed_hash(&self.key, &input).as_bytes().to_vec()
    }

    fn seal(
        &self,
        kind: RelationshipKind,
        lookup_key: &[u8],
        plaintext: &[u8],
    ) -> Result<Vec<u8>, RelationshipStoreError> {
        let aad = relationship_aad(kind, lookup_key);
        let (nonce, ciphertext) = encrypt_xchacha_raw(&self.key, plaintext, &aad)
            .map_err(|error| RelationshipStoreError::Storage(error.to_string()))?;
        let mut envelope = Vec::with_capacity(HEADER_LEN + ciphertext.len());
        envelope.extend_from_slice(&RELATIONSHIP_MAGIC);
        envelope.push(RELATIONSHIP_FORMAT_VERSION);
        envelope.extend_from_slice(&nonce);
        envelope.extend_from_slice(&ciphertext);
        Ok(envelope)
    }

    fn open(
        &self,
        kind: RelationshipKind,
        lookup_key: &[u8],
        envelope: &[u8],
    ) -> Result<Vec<u8>, RelationshipStoreError> {
        if envelope.len() < HEADER_LEN
            || envelope[..4] != RELATIONSHIP_MAGIC
            || envelope[4] != RELATIONSHIP_FORMAT_VERSION
        {
            return Err(RelationshipStoreError::InvalidCiphertext);
        }
        let aad = relationship_aad(kind, lookup_key);
        decrypt_xchacha_raw(
            &self.key,
            &envelope[5..HEADER_LEN],
            &envelope[HEADER_LEN..],
            &aad,
        )
        .map_err(|_| RelationshipStoreError::InvalidCiphertext)
    }
}

fn relationship_aad(kind: RelationshipKind, lookup_key: &[u8]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(
        RELATIONSHIP_KEY_INFO.len() + kind.as_str().len() + lookup_key.len() + 2,
    );
    aad.extend_from_slice(RELATIONSHIP_KEY_INFO);
    aad.push(0);
    aad.extend_from_slice(kind.as_str().as_bytes());
    aad.push(0);
    aad.extend_from_slice(lookup_key);
    aad
}

#[derive(Serialize, Deserialize)]
struct MemberPayloadV1 {
    version: u8,
    member: SpaceMember,
}

#[derive(Serialize, Deserialize)]
struct TrustedPeerPayloadV1 {
    version: u8,
    peer: TrustedPeer,
}

#[derive(Serialize, Deserialize)]
struct PeerAddressPayloadV1 {
    version: u8,
    device_id: String,
    addr_blob: Vec<u8>,
    observed_at: i64,
}

#[derive(Serialize, Deserialize)]
struct CandidatePayloadV1 {
    version: u8,
    candidate: SpaceMembershipCandidate,
}

#[derive(Serialize, Deserialize)]
struct MembershipAnnouncementPayloadV1 {
    version: u8,
    announcement: DeviceAnnouncement,
}

#[derive(Serialize, Deserialize)]
struct MembershipOutboxPayloadV1 {
    version: u8,
    pending: PendingMembershipBatch,
}

#[derive(Serialize, Deserialize)]
struct MembershipAppliedSecurityUpdatePayloadV1 {
    version: u8,
    space_id: String,
    update: RelayedSecurityUpdate,
}

#[derive(QueryableByName)]
struct LegacyMemberRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    device_id: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    device_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    identity_fingerprint: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    joined_at: i64,
    #[diesel(sql_type = diesel::sql_types::Text)]
    sync_preferences: String,
}

#[derive(QueryableByName)]
struct LegacyTrustedPeerRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    peer_device_id: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    local_device_id: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    peer_fingerprint: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    trusted_at: i64,
}

#[derive(QueryableByName)]
struct LegacyPeerAddressRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    device_id: String,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    addr_blob: Vec<u8>,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    observed_at: i64,
}

pub struct EncryptedRelationshipStore<E> {
    executor: E,
    derive_subkey: Arc<dyn DeriveSpaceSubkeyPort>,
    current_profile: Arc<dyn CurrentProfilePort>,
    migration_lock: Mutex<()>,
}

impl<E> EncryptedRelationshipStore<E>
where
    E: DbExecutor,
{
    pub fn new(
        executor: E,
        derive_subkey: Arc<dyn DeriveSpaceSubkeyPort>,
        current_profile: Arc<dyn CurrentProfilePort>,
    ) -> Self {
        Self {
            executor,
            derive_subkey,
            current_profile,
            migration_lock: Mutex::new(()),
        }
    }

    async fn cipher(&self) -> Result<RelationshipCipher, RelationshipStoreError> {
        let profile = self
            .current_profile
            .current_profile()
            .await
            .map_err(|error| RelationshipStoreError::Storage(error.to_string()))?;
        let key = self
            .derive_subkey
            .derive_subkey(profile.as_ref().as_bytes(), RELATIONSHIP_KEY_INFO)
            .await
            .map_err(|error| match error {
                uc_core::ports::space::SpaceAccessError::NotUnlocked => {
                    RelationshipStoreError::Locked
                }
                other => RelationshipStoreError::Storage(other.to_string()),
            })?;
        Ok(RelationshipCipher::new(key))
    }

    async fn ready_cipher(&self) -> Result<RelationshipCipher, RelationshipStoreError> {
        let cipher = self.cipher().await?;
        let _guard = self.migration_lock.lock().await;
        self.migrate_if_needed(&cipher)?;
        Ok(cipher)
    }

    fn migration_state(&self) -> Result<String, RelationshipStoreError> {
        self.executor
            .run(|conn| {
                relationship_privacy_maintenance::table
                    .find(1)
                    .select(relationship_privacy_maintenance::state)
                    .first::<String>(conn)
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
            })
            .map_err(|error| RelationshipStoreError::Storage(error.to_string()))
    }

    fn migrate_if_needed(&self, cipher: &RelationshipCipher) -> Result<(), RelationshipStoreError> {
        let state = self.migration_state()?;
        if state == "completed" {
            return Ok(());
        }
        if state == "pending_rows" {
            self.migrate_legacy_members(cipher)?;
            self.migrate_legacy_trusted_peers(cipher)?;
            self.migrate_legacy_peer_addresses(cipher)?;
            self.executor
                .run(|conn| {
                    conn.transaction::<_, anyhow::Error, _>(|conn| {
                        conn.batch_execute(
                            "DROP TABLE relationship_legacy_space_member;\
                             DROP TABLE relationship_legacy_trusted_peer;\
                             DROP TABLE relationship_legacy_peer_address;",
                        )?;
                        diesel::update(relationship_privacy_maintenance::table.find(1))
                            .set(
                                relationship_privacy_maintenance::state
                                    .eq("pending_physical_purge"),
                            )
                            .execute(conn)?;
                        Ok(())
                    })
                })
                .map_err(|error| RelationshipStoreError::Storage(error.to_string()))?;
        }
        if self.migration_state()? == "pending_physical_purge" {
            self.executor
                .run(|conn| {
                    conn.batch_execute("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
                    diesel::update(relationship_privacy_maintenance::table.find(1))
                        .set(relationship_privacy_maintenance::state.eq("completed"))
                        .execute(conn)?;
                    Ok(())
                })
                .map_err(|error| RelationshipStoreError::Storage(error.to_string()))?;
        }
        Ok(())
    }

    fn migrate_legacy_members(
        &self,
        cipher: &RelationshipCipher,
    ) -> Result<(), RelationshipStoreError> {
        loop {
            let row = self
                .executor
                .run(|conn| {
                    diesel::sql_query(
                        "SELECT device_id, device_name, identity_fingerprint, joined_at, \
                         sync_preferences FROM relationship_legacy_space_member LIMIT 1",
                    )
                    .get_result::<LegacyMemberRow>(conn)
                    .optional()
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
                })
                .map_err(|error| RelationshipStoreError::Storage(error.to_string()))?;
            let Some(row) = row else { break };
            let identity = row.device_id.clone();
            let member = legacy_member_to_domain(row)?;
            let payload = encode_member(&member)?;
            self.upsert_verified(cipher, RelationshipKind::Member, &identity, &payload)?;
            self.delete_legacy_row(
                "DELETE FROM relationship_legacy_space_member WHERE device_id = ?",
                &identity,
            )?;
        }
        Ok(())
    }

    fn migrate_legacy_trusted_peers(
        &self,
        cipher: &RelationshipCipher,
    ) -> Result<(), RelationshipStoreError> {
        loop {
            let row = self
                .executor
                .run(|conn| {
                    diesel::sql_query(
                        "SELECT peer_device_id, local_device_id, peer_fingerprint, trusted_at \
                         FROM relationship_legacy_trusted_peer LIMIT 1",
                    )
                    .get_result::<LegacyTrustedPeerRow>(conn)
                    .optional()
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
                })
                .map_err(|error| RelationshipStoreError::Storage(error.to_string()))?;
            let Some(row) = row else { break };
            let identity = row.peer_device_id.clone();
            let peer = legacy_trusted_peer_to_domain(row)?;
            let payload = encode_trusted_peer(&peer)?;
            self.upsert_verified(cipher, RelationshipKind::TrustedPeer, &identity, &payload)?;
            self.delete_legacy_row(
                "DELETE FROM relationship_legacy_trusted_peer WHERE peer_device_id = ?",
                &identity,
            )?;
        }
        Ok(())
    }

    fn migrate_legacy_peer_addresses(
        &self,
        cipher: &RelationshipCipher,
    ) -> Result<(), RelationshipStoreError> {
        loop {
            let row = self
                .executor
                .run(|conn| {
                    diesel::sql_query(
                        "SELECT device_id, addr_blob, observed_at \
                         FROM relationship_legacy_peer_address LIMIT 1",
                    )
                    .get_result::<LegacyPeerAddressRow>(conn)
                    .optional()
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
                })
                .map_err(|error| RelationshipStoreError::Storage(error.to_string()))?;
            let Some(row) = row else { break };
            let identity = row.device_id.clone();
            let record = legacy_peer_address_to_domain(row)?;
            let payload = encode_peer_address(&record)?;
            self.upsert_verified(cipher, RelationshipKind::PeerAddress, &identity, &payload)?;
            self.delete_legacy_row(
                "DELETE FROM relationship_legacy_peer_address WHERE device_id = ?",
                &identity,
            )?;
        }
        Ok(())
    }

    fn delete_legacy_row(
        &self,
        statement: &'static str,
        identity: &str,
    ) -> Result<(), RelationshipStoreError> {
        let identity = identity.to_owned();
        self.executor
            .run(move |conn| {
                diesel::sql_query(statement)
                    .bind::<diesel::sql_types::Text, _>(identity)
                    .execute(conn)?;
                Ok(())
            })
            .map_err(|error| RelationshipStoreError::Storage(error.to_string()))
    }

    fn upsert_verified(
        &self,
        cipher: &RelationshipCipher,
        kind: RelationshipKind,
        identity: &str,
        payload: &[u8],
    ) -> Result<(), RelationshipStoreError> {
        let lookup_key = cipher.lookup_key(kind, identity);
        let payload_ciphertext = cipher.seal(kind, &lookup_key, payload)?;
        let row = NewEncryptedRelationshipRow {
            kind: kind.as_str().to_owned(),
            lookup_key: lookup_key.clone(),
            payload_ciphertext,
        };
        self.executor
            .run(move |conn| {
                diesel::insert_into(encrypted_relationship::table)
                    .values(&row)
                    .on_conflict((
                        encrypted_relationship::kind,
                        encrypted_relationship::lookup_key,
                    ))
                    .do_update()
                    .set(
                        encrypted_relationship::payload_ciphertext
                            .eq(row.payload_ciphertext.clone()),
                    )
                    .execute(conn)?;
                Ok(())
            })
            .map_err(|error| RelationshipStoreError::Storage(error.to_string()))?;
        let stored = self.load_envelope(kind, &lookup_key)?.ok_or_else(|| {
            RelationshipStoreError::Storage("relationship write was not observable".to_string())
        })?;
        let verified = cipher.open(kind, &lookup_key, &stored)?;
        if verified != payload {
            return Err(RelationshipStoreError::InvalidCiphertext);
        }
        Ok(())
    }

    fn load_envelope(
        &self,
        kind: RelationshipKind,
        lookup_key: &[u8],
    ) -> Result<Option<Vec<u8>>, RelationshipStoreError> {
        let kind_value = kind.as_str().to_owned();
        let lookup_value = lookup_key.to_vec();
        self.executor
            .run(move |conn| {
                encrypted_relationship::table
                    .filter(encrypted_relationship::kind.eq(kind_value))
                    .filter(encrypted_relationship::lookup_key.eq(lookup_value))
                    .select(encrypted_relationship::payload_ciphertext)
                    .first::<Vec<u8>>(conn)
                    .optional()
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
            })
            .map_err(|error| RelationshipStoreError::Storage(error.to_string()))
    }

    async fn get_payload(
        &self,
        kind: RelationshipKind,
        identity: &str,
    ) -> Result<Option<Vec<u8>>, RelationshipStoreError> {
        let cipher = self.ready_cipher().await?;
        let lookup_key = cipher.lookup_key(kind, identity);
        self.load_envelope(kind, &lookup_key)?
            .map(|envelope| cipher.open(kind, &lookup_key, &envelope))
            .transpose()
    }

    async fn list_payloads(
        &self,
        kind: RelationshipKind,
    ) -> Result<Vec<Vec<u8>>, RelationshipStoreError> {
        let cipher = self.ready_cipher().await?;
        let kind_value = kind.as_str().to_owned();
        let rows = self
            .executor
            .run(move |conn| {
                encrypted_relationship::table
                    .filter(encrypted_relationship::kind.eq(kind_value))
                    .select(EncryptedRelationshipRow::as_select())
                    .load::<EncryptedRelationshipRow>(conn)
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
            })
            .map_err(|error| RelationshipStoreError::Storage(error.to_string()))?;
        rows.into_iter()
            .map(|row| {
                if row.kind != kind.as_str() {
                    return Err(RelationshipStoreError::InvalidCiphertext);
                }
                cipher.open(kind, &row.lookup_key, &row.payload_ciphertext)
            })
            .collect()
    }

    async fn save_payload(
        &self,
        kind: RelationshipKind,
        identity: &str,
        payload: &[u8],
    ) -> Result<(), RelationshipStoreError> {
        let cipher = self.ready_cipher().await?;
        self.upsert_verified(&cipher, kind, identity, payload)
    }

    async fn remove_payload(
        &self,
        kind: RelationshipKind,
        identity: &str,
    ) -> Result<bool, RelationshipStoreError> {
        let cipher = self.ready_cipher().await?;
        let kind_value = kind.as_str().to_owned();
        let lookup_value = cipher.lookup_key(kind, identity);
        let affected = self
            .executor
            .run(move |conn| {
                diesel::delete(
                    encrypted_relationship::table
                        .filter(encrypted_relationship::kind.eq(kind_value))
                        .filter(encrypted_relationship::lookup_key.eq(lookup_value)),
                )
                .execute(conn)
                .map_err(|error| anyhow::anyhow!(error.to_string()))
            })
            .map_err(|error| RelationshipStoreError::Storage(error.to_string()))?;
        Ok(affected > 0)
    }

    pub async fn get_member(
        &self,
        device_id: &DeviceId,
    ) -> Result<Option<SpaceMember>, RelationshipStoreError> {
        self.get_payload(RelationshipKind::Member, device_id.as_str())
            .await?
            .map(|payload| decode_member(&payload))
            .transpose()
    }

    pub async fn list_members(&self) -> Result<Vec<SpaceMember>, RelationshipStoreError> {
        self.list_payloads(RelationshipKind::Member)
            .await?
            .into_iter()
            .map(|payload| decode_member(&payload))
            .collect()
    }

    pub async fn save_member(&self, member: &SpaceMember) -> Result<(), RelationshipStoreError> {
        let payload = encode_member(member)?;
        self.save_payload(
            RelationshipKind::Member,
            member.device_id.as_str(),
            &payload,
        )
        .await
    }

    pub async fn remove_member(
        &self,
        device_id: &DeviceId,
    ) -> Result<bool, RelationshipStoreError> {
        self.remove_payload(RelationshipKind::Member, device_id.as_str())
            .await
    }

    pub async fn get_trusted_peer(
        &self,
        device_id: &DeviceId,
    ) -> Result<Option<TrustedPeer>, RelationshipStoreError> {
        self.get_payload(RelationshipKind::TrustedPeer, device_id.as_str())
            .await?
            .map(|payload| decode_trusted_peer(&payload))
            .transpose()
    }

    pub async fn list_trusted_peers(&self) -> Result<Vec<TrustedPeer>, RelationshipStoreError> {
        self.list_payloads(RelationshipKind::TrustedPeer)
            .await?
            .into_iter()
            .map(|payload| decode_trusted_peer(&payload))
            .collect()
    }

    pub async fn save_trusted_peer(
        &self,
        peer: &TrustedPeer,
    ) -> Result<(), RelationshipStoreError> {
        let payload = encode_trusted_peer(peer)?;
        self.save_payload(
            RelationshipKind::TrustedPeer,
            peer.peer_device_id.as_str(),
            &payload,
        )
        .await
    }

    pub async fn remove_trusted_peer(
        &self,
        device_id: &DeviceId,
    ) -> Result<bool, RelationshipStoreError> {
        self.remove_payload(RelationshipKind::TrustedPeer, device_id.as_str())
            .await
    }

    pub async fn get_peer_address(
        &self,
        device_id: &DeviceId,
    ) -> Result<Option<PeerAddressRecord>, RelationshipStoreError> {
        self.get_payload(RelationshipKind::PeerAddress, device_id.as_str())
            .await?
            .map(|payload| decode_peer_address(&payload))
            .transpose()
    }

    pub async fn list_peer_addresses(
        &self,
    ) -> Result<Vec<PeerAddressRecord>, RelationshipStoreError> {
        self.list_payloads(RelationshipKind::PeerAddress)
            .await?
            .into_iter()
            .map(|payload| decode_peer_address(&payload))
            .collect()
    }

    pub async fn save_peer_address(
        &self,
        record: &PeerAddressRecord,
    ) -> Result<(), RelationshipStoreError> {
        let payload = encode_peer_address(record)?;
        self.save_payload(
            RelationshipKind::PeerAddress,
            record.device_id.as_str(),
            &payload,
        )
        .await
    }

    pub async fn remove_peer_address(
        &self,
        device_id: &DeviceId,
    ) -> Result<bool, RelationshipStoreError> {
        self.remove_payload(RelationshipKind::PeerAddress, device_id.as_str())
            .await
    }

    pub async fn get_candidate(
        &self,
        space_id: &uc_core::ids::SpaceId,
        device_id: &DeviceId,
    ) -> Result<Option<SpaceMembershipCandidate>, RelationshipStoreError> {
        let identity = candidate_identity(space_id, device_id);
        self.get_payload(RelationshipKind::Candidate, &identity)
            .await?
            .map(|payload| decode_candidate(&payload))
            .transpose()
    }

    pub async fn list_candidates(
        &self,
        space_id: &uc_core::ids::SpaceId,
    ) -> Result<Vec<SpaceMembershipCandidate>, RelationshipStoreError> {
        self.list_payloads(RelationshipKind::Candidate)
            .await?
            .into_iter()
            .map(|payload| decode_candidate(&payload))
            .filter_map(|candidate| match candidate {
                Ok(candidate) if candidate.space_id() == space_id => Some(Ok(candidate)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    pub async fn save_candidate(
        &self,
        candidate: &SpaceMembershipCandidate,
    ) -> Result<(), RelationshipStoreError> {
        let identity = candidate_identity(candidate.space_id(), candidate.device_id());
        let payload = encode_candidate(candidate)?;
        self.save_payload(RelationshipKind::Candidate, &identity, &payload)
            .await
    }

    pub async fn promote_verified_peer(
        &self,
        member: &SpaceMember,
        trusted_peer: &TrustedPeer,
        peer_address: &PeerAddressRecord,
        ready_candidate: &SpaceMembershipCandidate,
    ) -> Result<(), RelationshipStoreError> {
        if member.device_id != trusted_peer.peer_device_id
            || member.device_id != peer_address.device_id
            || &member.device_id != ready_candidate.device_id()
            || member.identity_fingerprint != trusted_peer.peer_fingerprint
            || ready_candidate.status() != CandidateStatus::Ready
        {
            return Err(RelationshipStoreError::Storage(
                "verified peer relationship set is inconsistent".to_string(),
            ));
        }

        let cipher = self.ready_cipher().await?;
        let candidate_identity =
            candidate_identity(ready_candidate.space_id(), ready_candidate.device_id());
        let payloads = [
            (
                RelationshipKind::PeerAddress,
                member.device_id.as_str().to_owned(),
                encode_peer_address(peer_address)?,
            ),
            (
                RelationshipKind::TrustedPeer,
                member.device_id.as_str().to_owned(),
                encode_trusted_peer(trusted_peer)?,
            ),
            (
                RelationshipKind::Member,
                member.device_id.as_str().to_owned(),
                encode_member(member)?,
            ),
            (
                RelationshipKind::Candidate,
                candidate_identity,
                encode_candidate(ready_candidate)?,
            ),
        ];
        let sealed = payloads
            .into_iter()
            .map(|(kind, identity, plaintext)| {
                let lookup_key = cipher.lookup_key(kind, &identity);
                let payload_ciphertext = cipher.seal(kind, &lookup_key, &plaintext)?;
                Ok((
                    kind,
                    plaintext,
                    NewEncryptedRelationshipRow {
                        kind: kind.as_str().to_owned(),
                        lookup_key,
                        payload_ciphertext,
                    },
                ))
            })
            .collect::<Result<Vec<_>, RelationshipStoreError>>()?;

        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    for (_, _, row) in &sealed {
                        diesel::insert_into(encrypted_relationship::table)
                            .values(row)
                            .on_conflict((
                                encrypted_relationship::kind,
                                encrypted_relationship::lookup_key,
                            ))
                            .do_update()
                            .set(
                                encrypted_relationship::payload_ciphertext
                                    .eq(&row.payload_ciphertext),
                            )
                            .execute(conn)?;
                    }
                    for (kind, plaintext, row) in &sealed {
                        let stored = encrypted_relationship::table
                            .filter(encrypted_relationship::kind.eq(&row.kind))
                            .filter(encrypted_relationship::lookup_key.eq(&row.lookup_key))
                            .select(encrypted_relationship::payload_ciphertext)
                            .first::<Vec<u8>>(conn)?;
                        let verified = cipher
                            .open(*kind, &row.lookup_key, &stored)
                            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                        if &verified != plaintext {
                            return Err(anyhow::anyhow!(
                                "verified peer relationship write was not observable"
                            ));
                        }
                    }
                    Ok(())
                })
            })
            .map_err(|error| RelationshipStoreError::Storage(error.to_string()))
    }

    pub async fn remove_candidate(
        &self,
        space_id: &uc_core::ids::SpaceId,
        device_id: &DeviceId,
    ) -> Result<bool, RelationshipStoreError> {
        let identity = candidate_identity(space_id, device_id);
        self.remove_payload(RelationshipKind::Candidate, &identity)
            .await
    }

    pub async fn get_membership_announcement(
        &self,
        space_id: &uc_core::ids::SpaceId,
        device_id: &DeviceId,
    ) -> Result<Option<DeviceAnnouncement>, RelationshipStoreError> {
        let identity = candidate_identity(space_id, device_id);
        self.get_payload(RelationshipKind::MembershipAnnouncement, &identity)
            .await?
            .map(|payload| decode_membership_announcement(&payload))
            .transpose()
    }

    pub async fn list_membership_announcements(
        &self,
        space_id: &uc_core::ids::SpaceId,
    ) -> Result<Vec<DeviceAnnouncement>, RelationshipStoreError> {
        self.list_payloads(RelationshipKind::MembershipAnnouncement)
            .await?
            .into_iter()
            .map(|payload| decode_membership_announcement(&payload))
            .filter_map(|announcement| match announcement {
                Ok(announcement) if &announcement.space_id == space_id => Some(Ok(announcement)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    pub async fn save_membership_announcement(
        &self,
        announcement: &DeviceAnnouncement,
    ) -> Result<(), RelationshipStoreError> {
        let identity = candidate_identity(&announcement.space_id, &announcement.device_id);
        let payload = encode_membership_announcement(announcement)?;
        self.save_payload(
            RelationshipKind::MembershipAnnouncement,
            &identity,
            &payload,
        )
        .await
    }

    pub async fn remove_membership_announcement(
        &self,
        space_id: &uc_core::ids::SpaceId,
        device_id: &DeviceId,
    ) -> Result<bool, RelationshipStoreError> {
        let identity = candidate_identity(space_id, device_id);
        self.remove_payload(RelationshipKind::MembershipAnnouncement, &identity)
            .await
    }

    pub async fn get_membership_outbox(
        &self,
        space_id: &uc_core::ids::SpaceId,
        recipient_device_id: &DeviceId,
        batch_id: &[u8; 32],
    ) -> Result<Option<PendingMembershipBatch>, RelationshipStoreError> {
        let identity = membership_outbox_identity(space_id, recipient_device_id, batch_id);
        self.get_payload(RelationshipKind::MembershipOutbox, &identity)
            .await?
            .map(|payload| decode_membership_outbox(&payload))
            .transpose()
    }

    pub async fn list_membership_outbox(
        &self,
        space_id: &uc_core::ids::SpaceId,
    ) -> Result<Vec<PendingMembershipBatch>, RelationshipStoreError> {
        self.list_payloads(RelationshipKind::MembershipOutbox)
            .await?
            .into_iter()
            .map(|payload| decode_membership_outbox(&payload))
            .filter_map(|pending| match pending {
                Ok(pending) if &pending.batch().space_id == space_id => Some(Ok(pending)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    pub async fn save_membership_outbox(
        &self,
        pending: &PendingMembershipBatch,
    ) -> Result<(), RelationshipStoreError> {
        let identity = membership_outbox_identity(
            &pending.batch().space_id,
            pending.recipient_device_id(),
            &pending.batch().batch_id,
        );
        let payload = encode_membership_outbox(pending)?;
        self.save_payload(RelationshipKind::MembershipOutbox, &identity, &payload)
            .await
    }

    pub async fn remove_membership_outbox(
        &self,
        space_id: &uc_core::ids::SpaceId,
        recipient_device_id: &DeviceId,
        batch_id: &[u8; 32],
    ) -> Result<bool, RelationshipStoreError> {
        let identity = membership_outbox_identity(space_id, recipient_device_id, batch_id);
        self.remove_payload(RelationshipKind::MembershipOutbox, &identity)
            .await
    }

    pub async fn get_membership_applied_security_update(
        &self,
        space_id: &uc_core::ids::SpaceId,
        next_epoch: u64,
    ) -> Result<Option<RelayedSecurityUpdate>, RelationshipStoreError> {
        let identity = membership_applied_security_update_identity(space_id, next_epoch);
        self.get_payload(RelationshipKind::MembershipAppliedSecurityUpdate, &identity)
            .await?
            .map(|payload| decode_membership_applied_security_update(&payload))
            .transpose()
            .map(|decoded| decoded.map(|decoded| decoded.update))
    }

    pub async fn list_membership_applied_security_updates(
        &self,
        space_id: &uc_core::ids::SpaceId,
    ) -> Result<Vec<RelayedSecurityUpdate>, RelationshipStoreError> {
        self.list_payloads(RelationshipKind::MembershipAppliedSecurityUpdate)
            .await?
            .into_iter()
            .map(|payload| decode_membership_applied_security_update(&payload))
            .filter_map(|update| match update {
                Ok(update) if &update.space_id == space_id.as_ref() => Some(Ok(update.update)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    pub async fn save_membership_applied_security_update(
        &self,
        space_id: &uc_core::ids::SpaceId,
        update: &RelayedSecurityUpdate,
    ) -> Result<(), RelationshipStoreError> {
        let identity = membership_applied_security_update_identity(space_id, update.next_epoch);
        let payload = encode_membership_applied_security_update(space_id, update)?;
        self.save_payload(
            RelationshipKind::MembershipAppliedSecurityUpdate,
            &identity,
            &payload,
        )
        .await
    }
}

#[async_trait]
impl<E> RelationshipStateResetPort for EncryptedRelationshipStore<E>
where
    E: DbExecutor,
{
    async fn clear_all_relationships(&self) -> Result<(), RelationshipStateResetError> {
        self.executor
            .run(|conn| {
                diesel::delete(encrypted_relationship::table)
                    .execute(conn)
                    .map(|_| ())
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
            })
            .map_err(|error| RelationshipStateResetError::Repository(error.to_string()))
    }
}

#[async_trait]
impl<E> VerifiedPeerPromotionPort for EncryptedRelationshipStore<E>
where
    E: DbExecutor,
{
    async fn promote_verified_peer(
        &self,
        member: &SpaceMember,
        trusted_peer: &TrustedPeer,
        peer_address: &PeerAddressRecord,
        ready_candidate: &SpaceMembershipCandidate,
    ) -> Result<(), VerifiedPeerPromotionError> {
        EncryptedRelationshipStore::promote_verified_peer(
            self,
            member,
            trusted_peer,
            peer_address,
            ready_candidate,
        )
        .await
        .map_err(|error| match error {
            RelationshipStoreError::Locked => VerifiedPeerPromotionError::Locked,
            RelationshipStoreError::InvalidCiphertext => VerifiedPeerPromotionError::Corrupt,
            RelationshipStoreError::Storage(message) => {
                VerifiedPeerPromotionError::Repository(message)
            }
        })
    }
}

fn candidate_identity(space_id: &uc_core::ids::SpaceId, device_id: &DeviceId) -> String {
    format!(
        "{}:{}{}",
        space_id.as_ref().len(),
        space_id.as_ref(),
        device_id.as_str()
    )
}

fn membership_outbox_identity(
    space_id: &uc_core::ids::SpaceId,
    recipient_device_id: &DeviceId,
    batch_id: &[u8; 32],
) -> String {
    let batch_id = batch_id
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!(
        "{}:{}:{}:{}",
        space_id.as_ref().len(),
        space_id.as_ref(),
        recipient_device_id.as_str(),
        batch_id
    )
}

fn membership_applied_security_update_identity(
    space_id: &uc_core::ids::SpaceId,
    next_epoch: u64,
) -> String {
    format!(
        "{}:{}:{}",
        space_id.as_ref().len(),
        space_id.as_ref(),
        next_epoch
    )
}

fn encode_member(member: &SpaceMember) -> Result<Vec<u8>, RelationshipStoreError> {
    serde_json::to_vec(&MemberPayloadV1 {
        version: 1,
        member: member.clone(),
    })
    .map_err(|error| RelationshipStoreError::Storage(error.to_string()))
}

fn decode_member(payload: &[u8]) -> Result<SpaceMember, RelationshipStoreError> {
    let decoded: MemberPayloadV1 =
        serde_json::from_slice(payload).map_err(|_| RelationshipStoreError::InvalidCiphertext)?;
    if decoded.version != 1 {
        return Err(RelationshipStoreError::InvalidCiphertext);
    }
    Ok(decoded.member)
}

fn encode_trusted_peer(peer: &TrustedPeer) -> Result<Vec<u8>, RelationshipStoreError> {
    serde_json::to_vec(&TrustedPeerPayloadV1 {
        version: 1,
        peer: peer.clone(),
    })
    .map_err(|error| RelationshipStoreError::Storage(error.to_string()))
}

fn decode_trusted_peer(payload: &[u8]) -> Result<TrustedPeer, RelationshipStoreError> {
    let decoded: TrustedPeerPayloadV1 =
        serde_json::from_slice(payload).map_err(|_| RelationshipStoreError::InvalidCiphertext)?;
    if decoded.version != 1 {
        return Err(RelationshipStoreError::InvalidCiphertext);
    }
    Ok(decoded.peer)
}

fn encode_peer_address(record: &PeerAddressRecord) -> Result<Vec<u8>, RelationshipStoreError> {
    serde_json::to_vec(&PeerAddressPayloadV1 {
        version: 1,
        device_id: record.device_id.as_str().to_owned(),
        addr_blob: record.addr_blob.clone(),
        observed_at: record.observed_at.timestamp(),
    })
    .map_err(|error| RelationshipStoreError::Storage(error.to_string()))
}

fn decode_peer_address(payload: &[u8]) -> Result<PeerAddressRecord, RelationshipStoreError> {
    let decoded: PeerAddressPayloadV1 =
        serde_json::from_slice(payload).map_err(|_| RelationshipStoreError::InvalidCiphertext)?;
    if decoded.version != 1 {
        return Err(RelationshipStoreError::InvalidCiphertext);
    }
    let observed_at = Utc
        .timestamp_opt(decoded.observed_at, 0)
        .single()
        .ok_or(RelationshipStoreError::InvalidCiphertext)?;
    Ok(PeerAddressRecord {
        device_id: DeviceId::new(decoded.device_id),
        addr_blob: decoded.addr_blob,
        observed_at,
    })
}

fn encode_candidate(
    candidate: &SpaceMembershipCandidate,
) -> Result<Vec<u8>, RelationshipStoreError> {
    serde_json::to_vec(&CandidatePayloadV1 {
        version: 1,
        candidate: candidate.clone(),
    })
    .map_err(|error| RelationshipStoreError::Storage(error.to_string()))
}

fn decode_candidate(payload: &[u8]) -> Result<SpaceMembershipCandidate, RelationshipStoreError> {
    let decoded: CandidatePayloadV1 =
        serde_json::from_slice(payload).map_err(|_| RelationshipStoreError::InvalidCiphertext)?;
    if decoded.version != 1 {
        return Err(RelationshipStoreError::InvalidCiphertext);
    }
    Ok(decoded.candidate)
}

fn encode_membership_announcement(
    announcement: &DeviceAnnouncement,
) -> Result<Vec<u8>, RelationshipStoreError> {
    serde_json::to_vec(&MembershipAnnouncementPayloadV1 {
        version: 1,
        announcement: announcement.clone(),
    })
    .map_err(|error| RelationshipStoreError::Storage(error.to_string()))
}

fn decode_membership_announcement(
    payload: &[u8],
) -> Result<DeviceAnnouncement, RelationshipStoreError> {
    let decoded: MembershipAnnouncementPayloadV1 =
        serde_json::from_slice(payload).map_err(|_| RelationshipStoreError::InvalidCiphertext)?;
    if decoded.version != 1 {
        return Err(RelationshipStoreError::InvalidCiphertext);
    }
    Ok(decoded.announcement)
}

fn encode_membership_outbox(
    pending: &PendingMembershipBatch,
) -> Result<Vec<u8>, RelationshipStoreError> {
    serde_json::to_vec(&MembershipOutboxPayloadV1 {
        version: 1,
        pending: pending.clone(),
    })
    .map_err(|error| RelationshipStoreError::Storage(error.to_string()))
}

fn decode_membership_outbox(
    payload: &[u8],
) -> Result<PendingMembershipBatch, RelationshipStoreError> {
    let decoded: MembershipOutboxPayloadV1 =
        serde_json::from_slice(payload).map_err(|_| RelationshipStoreError::InvalidCiphertext)?;
    if decoded.version != 1 {
        return Err(RelationshipStoreError::InvalidCiphertext);
    }
    Ok(decoded.pending)
}

fn encode_membership_applied_security_update(
    space_id: &uc_core::ids::SpaceId,
    update: &RelayedSecurityUpdate,
) -> Result<Vec<u8>, RelationshipStoreError> {
    serde_json::to_vec(&MembershipAppliedSecurityUpdatePayloadV1 {
        version: 1,
        space_id: space_id.as_ref().to_owned(),
        update: update.clone(),
    })
    .map_err(|error| RelationshipStoreError::Storage(error.to_string()))
}

fn decode_membership_applied_security_update(
    payload: &[u8],
) -> Result<MembershipAppliedSecurityUpdatePayloadV1, RelationshipStoreError> {
    let decoded: MembershipAppliedSecurityUpdatePayloadV1 =
        serde_json::from_slice(payload).map_err(|_| RelationshipStoreError::InvalidCiphertext)?;
    if decoded.version != 1 {
        return Err(RelationshipStoreError::InvalidCiphertext);
    }
    Ok(decoded)
}

fn legacy_member_to_domain(row: LegacyMemberRow) -> Result<SpaceMember, RelationshipStoreError> {
    let joined_at = Utc
        .timestamp_opt(row.joined_at, 0)
        .single()
        .ok_or(RelationshipStoreError::InvalidCiphertext)?;
    let sync_preferences = serde_json::from_str::<MemberSyncPreferences>(&row.sync_preferences)
        .map_err(|_| RelationshipStoreError::InvalidCiphertext)?;
    let identity_fingerprint = IdentityFingerprint::from_display_string(row.identity_fingerprint)
        .map_err(|_| RelationshipStoreError::InvalidCiphertext)?;
    Ok(SpaceMember {
        device_id: DeviceId::new(row.device_id),
        device_name: row.device_name,
        identity_fingerprint,
        joined_at,
        sync_preferences,
    })
}

fn legacy_trusted_peer_to_domain(
    row: LegacyTrustedPeerRow,
) -> Result<TrustedPeer, RelationshipStoreError> {
    let trusted_at = Utc
        .timestamp_opt(row.trusted_at, 0)
        .single()
        .ok_or(RelationshipStoreError::InvalidCiphertext)?;
    let peer_fingerprint = IdentityFingerprint::from_display_string(row.peer_fingerprint)
        .map_err(|_| RelationshipStoreError::InvalidCiphertext)?;
    Ok(TrustedPeer {
        local_device_id: DeviceId::new(row.local_device_id),
        peer_device_id: DeviceId::new(row.peer_device_id),
        peer_fingerprint,
        trusted_at,
    })
}

fn legacy_peer_address_to_domain(
    row: LegacyPeerAddressRow,
) -> Result<PeerAddressRecord, RelationshipStoreError> {
    let observed_at = Utc
        .timestamp_opt(row.observed_at, 0)
        .single()
        .ok_or(RelationshipStoreError::InvalidCiphertext)?;
    Ok(PeerAddressRecord {
        device_id: DeviceId::new(row.device_id),
        addr_blob: row.addr_blob,
        observed_at,
    })
}

#[cfg(test)]
pub(crate) fn test_relationship_store(
    pool: crate::db::pool::DbPool,
) -> Arc<EncryptedRelationshipStore<Arc<crate::db::executor::DieselSqliteExecutor>>> {
    struct TestSubkey;
    struct TestProfile;

    #[async_trait::async_trait]
    impl DeriveSpaceSubkeyPort for TestSubkey {
        async fn derive_subkey(
            &self,
            _salt: &[u8],
            _info: &[u8],
        ) -> Result<[u8; 32], uc_core::ports::space::SpaceAccessError> {
            Ok([0x61; 32])
        }
    }

    #[async_trait::async_trait]
    impl CurrentProfilePort for TestProfile {
        async fn current_profile(
            &self,
        ) -> Result<
            uc_core::ids::ProfileId,
            uc_core::ports::security::current_profile::CurrentProfileError,
        > {
            Ok(uc_core::ids::ProfileId::from("relationship-unit-test"))
        }
    }

    Arc::new(EncryptedRelationshipStore::new(
        Arc::new(crate::db::executor::DieselSqliteExecutor::new(pool)),
        Arc::new(TestSubkey),
        Arc::new(TestProfile),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use diesel::prelude::*;
    use tempfile::{tempdir, TempDir};
    use uc_core::ids::ProfileId;
    use uc_core::membership::{
        CandidateStatus, DeviceAnnouncement, MembershipAnnouncementRepositoryPort,
        MembershipAppliedSecurityUpdateRepositoryPort, MembershipCandidateRepositoryPort,
        MembershipEventBatch, MembershipGossipEvent, MembershipOutboxRepositoryPort,
        PendingMembershipBatch, RelationshipStateResetPort, RelayedSecurityUpdate,
        SpaceMembershipCandidate, SponsorCandidateSeed,
    };
    use uc_core::ports::security::current_profile::{CurrentProfileError, CurrentProfilePort};
    use uc_core::ports::space::{DeriveSpaceSubkeyPort, SpaceAccessError};
    use uc_core::ports::{PeerAddressRecord, PeerAddressRepositoryPort};
    use uc_core::security::IdentityFingerprint;
    use uc_core::{
        DeviceId, MemberRepositoryPort, MemberSyncPreferences, SpaceMember, TrustedPeer,
        TrustedPeerRepositoryPort,
    };

    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::pool::init_db_pool;
    use crate::db::ports::DbExecutor;
    use crate::db::repositories::{
        DieselPeerAddressRepository, DieselSpaceMemberRepository, DieselTrustedPeerRepository,
        EncryptedMembershipAnnouncementRepository,
        EncryptedMembershipAppliedSecurityUpdateRepository, EncryptedMembershipCandidateRepository,
        EncryptedMembershipOutboxRepository,
    };

    use super::EncryptedRelationshipStore;

    struct FixedSubkey;

    #[async_trait]
    impl DeriveSpaceSubkeyPort for FixedSubkey {
        async fn derive_subkey(
            &self,
            _salt: &[u8],
            _info: &[u8],
        ) -> Result<[u8; 32], SpaceAccessError> {
            Ok([0x5a; 32])
        }
    }

    struct LockedSubkey;

    #[async_trait]
    impl DeriveSpaceSubkeyPort for LockedSubkey {
        async fn derive_subkey(
            &self,
            _salt: &[u8],
            _info: &[u8],
        ) -> Result<[u8; 32], SpaceAccessError> {
            Err(SpaceAccessError::NotUnlocked)
        }
    }

    struct TestProfile;

    #[async_trait]
    impl CurrentProfilePort for TestProfile {
        async fn current_profile(&self) -> Result<ProfileId, CurrentProfileError> {
            Ok(ProfileId::from("relationship-test-profile"))
        }
    }

    type TestStore = EncryptedRelationshipStore<Arc<DieselSqliteExecutor>>;

    fn store(
        derive_subkey: Arc<dyn DeriveSpaceSubkeyPort>,
    ) -> (Arc<TestStore>, Arc<DieselSqliteExecutor>, TempDir) {
        let tempdir = tempdir().unwrap();
        let db_path = tempdir.path().join("relationships.sqlite");
        let pool = init_db_pool(db_path.to_str().unwrap()).unwrap();
        let executor = Arc::new(DieselSqliteExecutor::new(pool));
        let store = Arc::new(EncryptedRelationshipStore::new(
            executor.clone(),
            derive_subkey,
            Arc::new(TestProfile),
        ));
        (store, executor, tempdir)
    }

    fn fingerprint(raw: &str) -> IdentityFingerprint {
        IdentityFingerprint::from_raw_string(raw).unwrap()
    }

    #[tokio::test]
    async fn migrates_all_legacy_relationships_and_removes_plaintext_tables() {
        let (store, executor, tempdir) = store(Arc::new(FixedSubkey));
        let preferences = serde_json::to_string(&MemberSyncPreferences::default()).unwrap();
        executor
            .run(move |conn| {
                diesel::sql_query(
                    "INSERT INTO relationship_legacy_space_member \
                     (device_id, device_name, identity_fingerprint, joined_at, sync_preferences) \
                     VALUES ('legacy-member-id', 'legacy-member-name-probe-93ab', \
                     'LEGACYMEMBERFP01', 1700000000, ?)",
                )
                .bind::<diesel::sql_types::Text, _>(preferences)
                .execute(conn)?;
                diesel::sql_query(
                    "INSERT INTO relationship_legacy_trusted_peer \
                     (peer_device_id, local_device_id, peer_fingerprint, trusted_at) \
                     VALUES ('legacy-peer-id', 'legacy-local-id', 'LEGACYPEERFP0001', 1700000001)",
                )
                .execute(conn)?;
                diesel::sql_query(
                    "INSERT INTO relationship_legacy_peer_address \
                     (device_id, addr_blob, observed_at) \
                     VALUES ('legacy-address-id', X'6c65676163792d616464726573732d70726f62652d37316334', 1700000002)",
                )
                .execute(conn)?;
                Ok(())
            })
            .unwrap();

        let member_repo = DieselSpaceMemberRepository::new(store.clone());
        let trusted_repo = DieselTrustedPeerRepository::new(store.clone());
        let address_repo = DieselPeerAddressRepository::new(store);

        let members = member_repo.list().await.unwrap();
        let peers = trusted_repo.list().await.unwrap();
        let addresses = address_repo.list().await.unwrap();
        assert_eq!(members[0].device_name, "legacy-member-name-probe-93ab");
        assert_eq!(peers[0].peer_device_id.as_str(), "legacy-peer-id");
        assert_eq!(addresses[0].addr_blob, b"legacy-address-probe-71c4");

        let table_names: Vec<String> = executor
            .run(|conn| {
                #[derive(QueryableByName)]
                struct NameRow {
                    #[diesel(sql_type = diesel::sql_types::Text)]
                    name: String,
                }
                Ok(diesel::sql_query(
                    "SELECT name FROM sqlite_master WHERE type = 'table' \
                     AND name LIKE 'relationship_legacy_%'",
                )
                .load::<NameRow>(conn)?
                .into_iter()
                .map(|row| row.name)
                .collect())
            })
            .unwrap();
        assert!(table_names.is_empty());

        for entry in std::fs::read_dir(tempdir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                let bytes = std::fs::read(&path).unwrap();
                for marker in [
                    b"legacy-member-name-probe-93ab".as_slice(),
                    b"LEGACYPEERFP0001".as_slice(),
                    b"legacy-address-probe-71c4".as_slice(),
                ] {
                    assert!(
                        !bytes.windows(marker.len()).any(|window| window == marker),
                        "legacy relationship plaintext remained in {}",
                        path.display()
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn locked_store_cannot_read_or_migrate_relationships() {
        let (store, executor, _tempdir) = store(Arc::new(LockedSubkey));
        executor
            .run(|conn| {
                diesel::sql_query(
                    "INSERT INTO relationship_legacy_space_member \
                     (device_id, device_name, identity_fingerprint, joined_at, sync_preferences) \
                     VALUES ('locked-member', 'locked-name', 'LOCKEDMEMBERFP01', 1700000000, \
                     '{\"send_enabled\":true,\"receive_enabled\":true,\
                     \"send_content_types\":{\"text\":true,\"image\":true,\"link\":true,\
                     \"file\":true,\"code_snippet\":true,\"rich_text\":true},\
                     \"receive_content_types\":{\"text\":true,\"image\":true,\"link\":true,\
                     \"file\":true,\"code_snippet\":true,\"rich_text\":true}}')",
                )
                .execute(conn)?;
                Ok(())
            })
            .unwrap();
        let repo = DieselSpaceMemberRepository::new(store);

        assert!(repo.list().await.is_err());

        let count: i64 = executor
            .run(|conn| {
                #[derive(QueryableByName)]
                struct CountRow {
                    #[diesel(sql_type = diesel::sql_types::BigInt)]
                    count: i64,
                }
                Ok(diesel::sql_query(
                    "SELECT COUNT(*) AS count FROM relationship_legacy_space_member",
                )
                .get_result::<CountRow>(conn)?
                .count)
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn encrypted_repositories_roundtrip_all_relationship_types() {
        let (store, _executor, _tempdir) = store(Arc::new(FixedSubkey));
        let member_repo = DieselSpaceMemberRepository::new(store.clone());
        let trusted_repo = DieselTrustedPeerRepository::new(store.clone());
        let address_repo = DieselPeerAddressRepository::new(store);
        let member = SpaceMember {
            device_id: DeviceId::new("member-a"),
            device_name: "member-name".to_string(),
            identity_fingerprint: fingerprint("MEMBERFP00000001"),
            joined_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            sync_preferences: MemberSyncPreferences::default(),
        };
        let peer = TrustedPeer {
            local_device_id: DeviceId::new("local-a"),
            peer_device_id: DeviceId::new("peer-a"),
            peer_fingerprint: fingerprint("PEERFP0000000001"),
            trusted_at: Utc.timestamp_opt(1_700_000_001, 0).unwrap(),
        };
        let address = PeerAddressRecord {
            device_id: DeviceId::new("peer-a"),
            addr_blob: b"opaque-address".to_vec(),
            observed_at: Utc.timestamp_opt(1_700_000_002, 0).unwrap(),
        };

        member_repo.save(&member).await.unwrap();
        trusted_repo.save(&peer).await.unwrap();
        address_repo.upsert(&address).await.unwrap();

        assert_eq!(
            member_repo.get(&member.device_id).await.unwrap(),
            Some(member)
        );
        assert_eq!(
            trusted_repo.get(&peer.peer_device_id).await.unwrap(),
            Some(peer)
        );
        assert_eq!(
            address_repo.get(&address.device_id).await.unwrap(),
            Some(address)
        );
    }

    #[tokio::test]
    async fn applied_security_updates_roundtrip_encrypted_and_scoped_per_space() {
        let (store, _executor, _tempdir) = store(Arc::new(FixedSubkey));
        let repo = EncryptedMembershipAppliedSecurityUpdateRepository::new(store.clone());
        let space_a = uc_core::ids::SpaceId::from("space-a");
        let space_b = uc_core::ids::SpaceId::from("space-b");
        let update = RelayedSecurityUpdate {
            previous_epoch: 4,
            next_epoch: 5,
            payload: b"epoch-4-to-5".to_vec(),
            digest: [7; 32],
        };

        repo.save(&space_a, &update).await.unwrap();
        repo.save(&space_a, &update).await.unwrap();

        let listed = repo.list(&space_a).await.unwrap();
        assert_eq!(listed, vec![update.clone()]);
        assert!(repo.list(&space_b).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn verified_peer_promotion_rolls_back_every_relationship_when_one_write_fails() {
        let (store, executor, _tempdir) = store(Arc::new(FixedSubkey));
        let member = SpaceMember {
            device_id: DeviceId::new("promoted-peer"),
            device_name: "promoted peer".to_string(),
            identity_fingerprint: fingerprint("PROMOTEDPEERFP01"),
            joined_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            sync_preferences: MemberSyncPreferences::default(),
        };
        let peer = TrustedPeer {
            local_device_id: DeviceId::new("local-peer"),
            peer_device_id: member.device_id,
            peer_fingerprint: member.identity_fingerprint.clone(),
            trusted_at: member.joined_at,
        };
        let address = PeerAddressRecord {
            device_id: member.device_id,
            addr_blob: b"promoted-address".to_vec(),
            observed_at: member.joined_at,
        };
        let candidate = SpaceMembershipCandidate::from_sponsor_seed(
            SponsorCandidateSeed {
                space_id: uc_core::ids::SpaceId::from("promotion-space"),
                device_id: member.device_id,
                device_name_hint: member.device_name.clone(),
                identity_fingerprint_hint: member.identity_fingerprint.clone(),
                transport_address_blob: address.addr_blob.clone(),
                address_observed_at_ms: member.joined_at.timestamp_millis(),
                source_device_id: DeviceId::new("sponsor-peer"),
                security_updates: Vec::new(),
                expires_at_ms: member.joined_at.timestamp_millis() + 60_000,
            },
            member.joined_at.timestamp_millis(),
        )
        .unwrap();
        store.save_candidate(&candidate).await.unwrap();
        let mut ready_candidate = candidate.clone();
        ready_candidate
            .apply(
                uc_core::membership::CandidateEvent::Admitted,
                member.joined_at.timestamp_millis(),
            )
            .unwrap();
        executor
            .run(|conn| {
                diesel::sql_query(
                    "CREATE TRIGGER fail_verified_member_promotion \
                     BEFORE INSERT ON encrypted_relationship \
                     WHEN NEW.kind = 'member' \
                     BEGIN SELECT RAISE(FAIL, 'forced promotion failure'); END",
                )
                .execute(conn)?;
                Ok(())
            })
            .unwrap();

        let result = store
            .promote_verified_peer(&member, &peer, &address, &ready_candidate)
            .await;

        assert!(result.is_err());
        assert!(store.get_member(&member.device_id).await.unwrap().is_none());
        assert!(store
            .get_trusted_peer(&member.device_id)
            .await
            .unwrap()
            .is_none());
        assert!(store
            .get_peer_address(&member.device_id)
            .await
            .unwrap()
            .is_none());
        let stored_candidate = store
            .get_candidate(candidate.space_id(), candidate.device_id())
            .await
            .unwrap()
            .unwrap();
        assert_ne!(stored_candidate.status(), CandidateStatus::Ready);
    }

    #[tokio::test]
    async fn relationship_reset_removes_members_trust_addresses_and_candidates() {
        let (store, _executor, _tempdir) = store(Arc::new(FixedSubkey));
        let member_repo = DieselSpaceMemberRepository::new(store.clone());
        let trusted_repo = DieselTrustedPeerRepository::new(store.clone());
        let address_repo = DieselPeerAddressRepository::new(store.clone());
        let candidate_repo = EncryptedMembershipCandidateRepository::new(store.clone());
        let member = SpaceMember {
            device_id: DeviceId::new("old-member"),
            device_name: "old member".to_string(),
            identity_fingerprint: fingerprint("OLDMEMBERFP00001"),
            joined_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            sync_preferences: MemberSyncPreferences::default(),
        };
        let peer = TrustedPeer {
            local_device_id: DeviceId::new("local"),
            peer_device_id: DeviceId::new("old-peer"),
            peer_fingerprint: fingerprint("OLDPEERFP0000001"),
            trusted_at: Utc.timestamp_opt(1_700_000_001, 0).unwrap(),
        };
        let address = PeerAddressRecord {
            device_id: DeviceId::new("old-peer"),
            addr_blob: b"old-address".to_vec(),
            observed_at: Utc.timestamp_opt(1_700_000_002, 0).unwrap(),
        };
        let candidate = SpaceMembershipCandidate::from_sponsor_seed(
            SponsorCandidateSeed {
                space_id: uc_core::ids::SpaceId::from("old-space"),
                device_id: DeviceId::new("old-candidate"),
                device_name_hint: "old candidate".to_string(),
                identity_fingerprint_hint: fingerprint("OLDCANDIDATEFP01"),
                transport_address_blob: b"candidate-address".to_vec(),
                address_observed_at_ms: 1_000,
                source_device_id: DeviceId::new("old-sponsor"),
                security_updates: Vec::new(),
                expires_at_ms: 50_000,
            },
            2_000,
        )
        .unwrap();

        member_repo.save(&member).await.unwrap();
        trusted_repo.save(&peer).await.unwrap();
        address_repo.upsert(&address).await.unwrap();
        candidate_repo.save(&candidate).await.unwrap();

        store.clear_all_relationships().await.unwrap();

        assert!(member_repo.list().await.unwrap().is_empty());
        assert!(trusted_repo.list().await.unwrap().is_empty());
        assert!(address_repo.list().await.unwrap().is_empty());
        assert!(candidate_repo
            .list(&uc_core::ids::SpaceId::from("old-space"))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn encrypted_candidate_survives_repository_restart_without_plaintext() {
        let (store, executor, tempdir) = store(Arc::new(FixedSubkey));
        let repo = EncryptedMembershipCandidateRepository::new(store);
        let candidate = SpaceMembershipCandidate::from_sponsor_seed(
            SponsorCandidateSeed {
                space_id: uc_core::ids::SpaceId::from("candidate-space"),
                device_id: DeviceId::new("candidate-device"),
                device_name_hint: "candidate-name-plaintext-probe-2c81".to_string(),
                identity_fingerprint_hint: fingerprint("CANDIDATEFP00001"),
                transport_address_blob: b"candidate-address-plaintext-probe-6a24".to_vec(),
                address_observed_at_ms: 1_000,
                source_device_id: DeviceId::new("candidate-sponsor"),
                security_updates: vec![RelayedSecurityUpdate {
                    previous_epoch: 4,
                    next_epoch: 5,
                    payload: b"candidate-update-plaintext-probe-9e13".to_vec(),
                    digest: [9; 32],
                }],
                expires_at_ms: 50_000,
            },
            2_000,
        )
        .unwrap();
        repo.save(&candidate).await.unwrap();
        drop(repo);

        let reopened_store = Arc::new(EncryptedRelationshipStore::new(
            executor,
            Arc::new(FixedSubkey),
            Arc::new(TestProfile),
        ));
        let reopened = EncryptedMembershipCandidateRepository::new(reopened_store);
        let loaded = reopened
            .get(candidate.space_id(), candidate.device_id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded, candidate);
        assert_eq!(loaded.status(), CandidateStatus::Pending);

        for entry in std::fs::read_dir(tempdir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                let bytes = std::fs::read(&path).unwrap();
                for marker in [
                    b"candidate-name-plaintext-probe-2c81".as_slice(),
                    b"candidate-address-plaintext-probe-6a24".as_slice(),
                    b"candidate-update-plaintext-probe-9e13".as_slice(),
                ] {
                    assert!(
                        !bytes.windows(marker.len()).any(|window| window == marker),
                        "candidate plaintext leaked into {}",
                        path.display()
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn encrypted_membership_outbox_survives_restart_without_plaintext() {
        let (store, executor, tempdir) = store(Arc::new(FixedSubkey));
        let repo = EncryptedMembershipOutboxRepository::new(store);
        let pending = PendingMembershipBatch::new(
            DeviceId::new("outbox-recipient"),
            MembershipEventBatch {
                space_id: uc_core::ids::SpaceId::from("outbox-space"),
                batch_id: [7; 32],
                events: vec![MembershipGossipEvent::SponsorSeed(SponsorCandidateSeed {
                    space_id: uc_core::ids::SpaceId::from("outbox-space"),
                    device_id: DeviceId::new("outbox-device"),
                    device_name_hint: "outbox-name-plaintext-probe-81f3".to_string(),
                    identity_fingerprint_hint: fingerprint("OUTBOXMEMBERFP01"),
                    transport_address_blob: b"outbox-address-plaintext-probe-43ac".to_vec(),
                    address_observed_at_ms: 1_000,
                    source_device_id: DeviceId::new("outbox-sponsor"),
                    security_updates: vec![RelayedSecurityUpdate {
                        previous_epoch: 4,
                        next_epoch: 5,
                        payload: b"outbox-update-plaintext-probe-620d".to_vec(),
                        digest: [6; 32],
                    }],
                    expires_at_ms: 50_000,
                })],
            },
            2_000,
        )
        .unwrap();
        repo.save(&pending).await.unwrap();
        drop(repo);

        let reopened_store = Arc::new(EncryptedRelationshipStore::new(
            executor,
            Arc::new(FixedSubkey),
            Arc::new(TestProfile),
        ));
        let reopened = EncryptedMembershipOutboxRepository::new(reopened_store);
        let loaded = reopened
            .get(
                &pending.batch().space_id,
                pending.recipient_device_id(),
                &pending.batch().batch_id,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded, pending);

        for entry in std::fs::read_dir(tempdir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                let bytes = std::fs::read(&path).unwrap();
                for marker in [
                    b"outbox-name-plaintext-probe-81f3".as_slice(),
                    b"outbox-address-plaintext-probe-43ac".as_slice(),
                    b"outbox-update-plaintext-probe-620d".as_slice(),
                ] {
                    assert!(
                        !bytes.windows(marker.len()).any(|window| window == marker),
                        "membership outbox plaintext leaked into {}",
                        path.display()
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn encrypted_membership_announcement_survives_restart_without_plaintext() {
        let (store, executor, tempdir) = store(Arc::new(FixedSubkey));
        let repo = EncryptedMembershipAnnouncementRepository::new(store);
        let announcement = DeviceAnnouncement {
            space_id: uc_core::ids::SpaceId::from("announcement-space"),
            device_id: DeviceId::new("announcement-device"),
            device_name: "announcement-name-plaintext-probe-5b21".to_owned(),
            identity_fingerprint: fingerprint("ANNOUNCEMENTFP01"),
            transport_public_key: b"announcement-key-plaintext-probe-f901".to_vec(),
            transport_address_blob: b"announcement-address-plaintext-probe-29ab".to_vec(),
            sequence: 7,
            group_epoch: 5,
            expires_at_ms: 50_000,
            content_digest: [8; 32],
            signature: b"announcement-signature-plaintext-probe-7c12".to_vec(),
        };
        repo.save(&announcement).await.unwrap();
        drop(repo);

        let reopened_store = Arc::new(EncryptedRelationshipStore::new(
            executor,
            Arc::new(FixedSubkey),
            Arc::new(TestProfile),
        ));
        let reopened = EncryptedMembershipAnnouncementRepository::new(reopened_store);
        let loaded = reopened
            .get(&announcement.space_id, &announcement.device_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded, announcement);

        for entry in std::fs::read_dir(tempdir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                let bytes = std::fs::read(&path).unwrap();
                for marker in [
                    b"announcement-name-plaintext-probe-5b21".as_slice(),
                    b"announcement-address-plaintext-probe-29ab".as_slice(),
                    b"announcement-signature-plaintext-probe-7c12".as_slice(),
                ] {
                    assert!(
                        !bytes.windows(marker.len()).any(|window| window == marker),
                        "membership announcement plaintext leaked into {}",
                        path.display()
                    );
                }
            }
        }
    }
}
