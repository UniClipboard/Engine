use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::Binary;
use serde::{Deserialize, Serialize};
use uc_application::deps::{
    LoadMembershipLedgerPort, PrepareSpaceAdmissionCredentialsPort,
    SpaceAdmissionCredentialPreparationError,
};
use uc_core::crypto::domain::Passphrase;
use uc_core::membership::{
    ActiveSpaceGenerationManifestV2, AdmissionContinuationCredential, InvitationId,
    SpaceAdmissionId,
};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::db::pool::DbPool;
use crate::db::ports::DbExecutor;
use crate::network::iroh::{
    SpaceAdmissionChannelCredentialError, SpaceAdmissionChannelCredentialPort,
    SponsorOpaqueMaterial,
};
use crate::security::{
    ActiveRuntimeManifest, ActiveSpaceGenerationManifestStore, AdmissionKeyError,
    AdmissionKeyManager, SpaceAdmissionAuth,
};

use super::repository::{SpaceAdmissionStateStoreError, SqliteSpaceAdmissionState};

const CREDENTIAL_FORMAT_V1: u16 = 1;
const CREDENTIAL_FORMAT_V2: u16 = 2;
const CREDENTIAL_PURPOSE: &[u8] = b"space-admission-credentials-v1";
const PREPARED_CREDENTIAL_FORMAT_V1: u16 = 1;

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct PreparedCredentialsV1 {
    format_version: u16,
    server_setup: Vec<u8>,
    registration: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum SpaceAdmissionCredentialStoreError {
    #[error("space admission credentials are locked")]
    Locked {
        #[source]
        source: anyhow::Error,
    },
    #[error("space admission credentials require recovery")]
    RecoveryRequired {
        #[source]
        source: anyhow::Error,
    },
    #[error("space admission credential storage is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct PersistedCredentialsV1 {
    format_version: u16,
    profile_generation: [u8; 16],
    space_id: String,
    keyslot_generation: [u8; 16],
    database_generation: [u8; 16],
    security_generation: [u8; 16],
    server_setup: Vec<u8>,
    registration: Vec<u8>,
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct PersistedCredentialsV2 {
    format_version: u16,
    profile_generation: [u8; 16],
    space_id: String,
    keyslot_generation: [u8; 16],
    space_control_generation: [u8; 16],
    server_setup: Vec<u8>,
    registration: Vec<u8>,
}

#[derive(QueryableByName)]
struct EncryptedCredentialRow {
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
}

pub struct SqliteSpaceAdmissionCredentials<E> {
    executor: E,
    keys: Arc<AdmissionKeyManager>,
    manifests: Arc<ActiveSpaceGenerationManifestStore>,
    membership_ledger: Arc<dyn LoadMembershipLedgerPort>,
    admissions: Arc<SqliteSpaceAdmissionState<E>>,
}

#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
enum CredentialScope {
    Legacy {
        space_id: String,
        keyslot_generation: [u8; 16],
        database_generation: [u8; 16],
        security_generation: [u8; 16],
    },
    Control {
        space_id: String,
        keyslot_generation: [u8; 16],
        space_control_generation: [u8; 16],
    },
}

#[derive(Zeroize, ZeroizeOnDrop)]
struct DecodedCredentials {
    scope: CredentialScope,
    server_setup: Vec<u8>,
    registration: Vec<u8>,
}

pub(crate) fn prepare_registration(passphrase: &Passphrase) -> anyhow::Result<Vec<u8>> {
    let server_setup = SpaceAdmissionAuth::generate_server_setup();
    let registration =
        SpaceAdmissionAuth::register(&server_setup, passphrase).map_err(anyhow::Error::new)?;
    let setup = server_setup.encode_for_encryption();
    let registration = registration.encode_for_encryption();
    postcard::to_stdvec(&PreparedCredentialsV1 {
        format_version: PREPARED_CREDENTIAL_FORMAT_V1,
        server_setup: setup.as_bytes().to_vec(),
        registration: registration.as_bytes().to_vec(),
    })
    .map_err(anyhow::Error::new)
}

pub(crate) fn install_prepared_registration(
    pool: &DbPool,
    keys: &AdmissionKeyManager,
    manifest: &ActiveSpaceGenerationManifestV2,
    prepared: &[u8],
) -> anyhow::Result<()> {
    let scope = CredentialScope::from(manifest.clone());
    install_prepared_registration_for_scope(pool, keys, &scope, prepared)
}

/// 把一次 admission 已准备的 OPAQUE registration 直接安装到目标 V3
/// control-generation scope。调用方不能接触 credential DTO、purpose 或 SQL。
pub(crate) fn install_prepared_registration_for_control_generation(
    pool: &DbPool,
    keys: &AdmissionKeyManager,
    manifest: &crate::security::ActiveRuntimeManifestV3,
    prepared: &[u8],
) -> anyhow::Result<()> {
    let scope = CredentialScope::Control {
        space_id: manifest.layout().space_id().as_ref().to_owned(),
        keyslot_generation: *manifest.keyslot_generation(),
        space_control_generation: *manifest.layout().space_control_generation(),
    };
    install_prepared_registration_for_scope(pool, keys, &scope, prepared)
}

fn install_prepared_registration_for_scope(
    pool: &DbPool,
    keys: &AdmissionKeyManager,
    scope: &CredentialScope,
    prepared: &[u8],
) -> anyhow::Result<()> {
    let prepared = decode_prepared_registration(prepared)?;
    let encrypted = seal_credentials(
        keys,
        scope,
        prepared.server_setup.clone(),
        prepared.registration.clone(),
    )?;
    let mut conn = pool.get()?;
    conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
        sql_query(
            "INSERT INTO space_admission_credentials (singleton_id, encrypted_payload) \
             VALUES (1, ?) ON CONFLICT(singleton_id) DO UPDATE SET \
             encrypted_payload = excluded.encrypted_payload",
        )
        .bind::<Binary, _>(encrypted)
        .execute(conn)?;
        Ok(())
    })
}

fn decode_prepared_registration(
    prepared: &[u8],
) -> anyhow::Result<Zeroizing<PreparedCredentialsV1>> {
    let prepared = Zeroizing::new(postcard::from_bytes::<PreparedCredentialsV1>(prepared)?);
    if prepared.format_version != PREPARED_CREDENTIAL_FORMAT_V1 {
        anyhow::bail!("prepared credential format is unsupported");
    }
    // 提升目标 generation 前先解码校验，避免把损坏的 OPAQUE 材料写入新空间。
    validate_material(&prepared.server_setup, &prepared.registration)?;
    Ok(prepared)
}

/// 以 credential owner 的正式 codec 回读并比对目标 control scope 与原始
/// prepared material，供不可变 control generation 在发布前完整验证。
pub(crate) fn verify_prepared_registration_for_control_generation(
    database: &Path,
    keys: &AdmissionKeyManager,
    manifest: &crate::security::ActiveRuntimeManifestV3,
    prepared: &[u8],
) -> anyhow::Result<()> {
    let expected = decode_prepared_registration(prepared)?;
    let expected_scope = CredentialScope::Control {
        space_id: manifest.layout().space_id().as_ref().to_owned(),
        keyslot_generation: *manifest.keyslot_generation(),
        space_control_generation: *manifest.layout().space_control_generation(),
    };
    let database = database
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("credential control database path is invalid"))?;
    let mut connection = SqliteConnection::establish(database)?;
    let row = load_encrypted_row(&mut connection)?
        .ok_or_else(|| anyhow::anyhow!("credential registration is missing"))?;
    let actual = open_credentials(keys, &row.encrypted_payload)?;
    if actual.scope != expected_scope
        || actual.server_setup != expected.server_setup
        || actual.registration != expected.registration
    {
        anyhow::bail!("credential registration verification failed");
    }
    validate_material(&actual.server_setup, &actual.registration)
}

fn load_encrypted_row(
    connection: &mut SqliteConnection,
) -> anyhow::Result<Option<EncryptedCredentialRow>> {
    sql_query("SELECT encrypted_payload FROM space_admission_credentials WHERE singleton_id = 1")
        .get_result::<EncryptedCredentialRow>(connection)
        .optional()
        .map_err(anyhow::Error::new)
}

fn seal_credentials(
    keys: &AdmissionKeyManager,
    scope: &CredentialScope,
    server_setup: Vec<u8>,
    registration: Vec<u8>,
) -> anyhow::Result<Vec<u8>> {
    let plaintext = match scope {
        CredentialScope::Legacy {
            space_id,
            keyslot_generation,
            database_generation,
            security_generation,
        } => Zeroizing::new(postcard::to_stdvec(&PersistedCredentialsV1 {
            format_version: CREDENTIAL_FORMAT_V1,
            profile_generation: keys.profile_generation(),
            space_id: space_id.clone(),
            keyslot_generation: *keyslot_generation,
            database_generation: *database_generation,
            security_generation: *security_generation,
            server_setup,
            registration,
        })?),
        CredentialScope::Control {
            space_id,
            keyslot_generation,
            space_control_generation,
        } => Zeroizing::new(postcard::to_stdvec(&PersistedCredentialsV2 {
            format_version: CREDENTIAL_FORMAT_V2,
            profile_generation: keys.profile_generation(),
            space_id: space_id.clone(),
            keyslot_generation: *keyslot_generation,
            space_control_generation: *space_control_generation,
            server_setup,
            registration,
        })?),
    };
    keys.seal_profile_payload(CREDENTIAL_PURPOSE, &plaintext)
        .map_err(anyhow::Error::new)
}

fn open_credentials(
    keys: &AdmissionKeyManager,
    encrypted: &[u8],
) -> anyhow::Result<DecodedCredentials> {
    let plaintext = Zeroizing::new(
        keys.open_profile_payload(CREDENTIAL_PURPOSE, encrypted)
            .map_err(anyhow::Error::new)?,
    );
    let (format_version, _) = postcard::take_from_bytes::<u16>(&plaintext)?;
    match format_version {
        CREDENTIAL_FORMAT_V1 => {
            let persisted =
                Zeroizing::new(postcard::from_bytes::<PersistedCredentialsV1>(&plaintext)?);
            if persisted.format_version != CREDENTIAL_FORMAT_V1
                || persisted.profile_generation != keys.profile_generation()
            {
                anyhow::bail!("credential generation is inconsistent");
            }
            Ok(DecodedCredentials {
                scope: CredentialScope::Legacy {
                    space_id: persisted.space_id.clone(),
                    keyslot_generation: persisted.keyslot_generation,
                    database_generation: persisted.database_generation,
                    security_generation: persisted.security_generation,
                },
                server_setup: persisted.server_setup.clone(),
                registration: persisted.registration.clone(),
            })
        }
        CREDENTIAL_FORMAT_V2 => {
            let persisted =
                Zeroizing::new(postcard::from_bytes::<PersistedCredentialsV2>(&plaintext)?);
            if persisted.format_version != CREDENTIAL_FORMAT_V2
                || persisted.profile_generation != keys.profile_generation()
                || persisted.space_control_generation == [0; 16]
            {
                anyhow::bail!("credential generation is inconsistent");
            }
            Ok(DecodedCredentials {
                scope: CredentialScope::Control {
                    space_id: persisted.space_id.clone(),
                    keyslot_generation: persisted.keyslot_generation,
                    space_control_generation: persisted.space_control_generation,
                },
                server_setup: persisted.server_setup.clone(),
                registration: persisted.registration.clone(),
            })
        }
        _ => anyhow::bail!("credential format is unsupported"),
    }
}

fn validate_material(server_setup: &[u8], registration: &[u8]) -> anyhow::Result<()> {
    SpaceAdmissionAuth::decode_server_setup_after_decryption(server_setup)
        .map_err(anyhow::Error::new)?;
    SpaceAdmissionAuth::decode_registration_after_decryption(registration)
        .map_err(anyhow::Error::new)?;
    Ok(())
}

/// 将 control target 中的既有 V2 credential 完整转换为 V3 control-generation scope。
///
/// 调用方只提供已认证 source 与目标 control generation；credential 格式、OPAQUE
/// 校验、AEAD purpose、SQL 单例和幂等恢复均由 owner 保持私有。
pub(crate) fn upgrade_registration_to_control_generation(
    database: &Path,
    keys: &AdmissionKeyManager,
    source: &ActiveSpaceGenerationManifestV2,
    target_control_generation: [u8; 16],
) -> anyhow::Result<()> {
    if !source.validate() || target_control_generation == [0; 16] {
        anyhow::bail!("credential scope upgrade target is invalid");
    }
    let database = database
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("credential control database path is invalid"))?;
    let mut connection = SqliteConnection::establish(database)?;
    connection.immediate_transaction::<_, anyhow::Error, _>(|connection| {
        let row = load_encrypted_row(connection)?;
        let Some(row) = row else {
            return Ok(());
        };
        let source_scope = CredentialScope::from(source.clone());
        let target_scope = CredentialScope::Control {
            space_id: source.space_id.clone(),
            keyslot_generation: source.keyslot_generation,
            space_control_generation: target_control_generation,
        };
        let decoded = open_credentials(keys, &row.encrypted_payload)?;
        if decoded.scope == target_scope {
            return Ok(());
        }
        if decoded.scope != source_scope {
            anyhow::bail!("credential source scope is inconsistent");
        }
        validate_material(&decoded.server_setup, &decoded.registration)?;
        let encrypted = seal_credentials(
            keys,
            &target_scope,
            decoded.server_setup.clone(),
            decoded.registration.clone(),
        )?;
        sql_query(
            "UPDATE space_admission_credentials SET encrypted_payload = ? WHERE singleton_id = 1",
        )
        .bind::<Binary, _>(encrypted)
        .execute(connection)?;
        let persisted = load_encrypted_row(connection)?
            .ok_or_else(|| anyhow::anyhow!("credential scope upgrade was not durable"))?;
        let verified = open_credentials(keys, &persisted.encrypted_payload)?;
        if verified.scope != target_scope {
            anyhow::bail!("credential scope upgrade verification failed");
        }
        validate_material(&verified.server_setup, &verified.registration)
    })
}

/// 在两个 V3 control generation 之间重绑定既有 OPAQUE registration。
///
/// Reset 与 membership branch 只提供已认证 source/target manifest；credential
/// DTO、profile AEAD、SQL 单例、OPAQUE 校验和幂等恢复仍由本 owner 独占。
pub(crate) fn rebind_registration_to_control_generation(
    database: &Path,
    keys: &AdmissionKeyManager,
    source: &crate::security::ActiveRuntimeManifestV3,
    target: &crate::security::ActiveRuntimeManifestV3,
) -> anyhow::Result<()> {
    if source.layout().profile_data_generation() != target.layout().profile_data_generation()
        || source.layout().space_control_generation() == target.layout().space_control_generation()
    {
        anyhow::bail!("credential control scope transition is invalid");
    }
    let source_scope = CredentialScope::Control {
        space_id: source.layout().space_id().as_ref().to_owned(),
        keyslot_generation: *source.keyslot_generation(),
        space_control_generation: *source.layout().space_control_generation(),
    };
    let target_scope = CredentialScope::Control {
        space_id: target.layout().space_id().as_ref().to_owned(),
        keyslot_generation: *target.keyslot_generation(),
        space_control_generation: *target.layout().space_control_generation(),
    };
    let database = database
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("credential control database path is invalid"))?;
    let mut connection = SqliteConnection::establish(database)?;
    connection.immediate_transaction::<_, anyhow::Error, _>(|connection| {
        let Some(row) = load_encrypted_row(connection)? else {
            return Ok(());
        };
        let decoded = open_credentials(keys, &row.encrypted_payload)?;
        if decoded.scope == target_scope {
            return Ok(());
        }
        if decoded.scope != source_scope {
            anyhow::bail!("credential source control scope is inconsistent");
        }
        validate_material(&decoded.server_setup, &decoded.registration)?;
        let encrypted = seal_credentials(
            keys,
            &target_scope,
            decoded.server_setup.clone(),
            decoded.registration.clone(),
        )?;
        sql_query(
            "UPDATE space_admission_credentials SET encrypted_payload = ? WHERE singleton_id = 1",
        )
        .bind::<Binary, _>(encrypted)
        .execute(connection)?;
        let persisted = load_encrypted_row(connection)?
            .ok_or_else(|| anyhow::anyhow!("credential control scope was not durable"))?;
        let verified = open_credentials(keys, &persisted.encrypted_payload)?;
        if verified.scope != target_scope {
            anyhow::bail!("credential control scope verification failed");
        }
        validate_material(&verified.server_setup, &verified.registration)
    })
}

impl<E> SqliteSpaceAdmissionCredentials<E> {
    pub fn new(
        executor: E,
        keys: Arc<AdmissionKeyManager>,
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
        membership_ledger: Arc<dyn LoadMembershipLedgerPort>,
        admissions: Arc<SqliteSpaceAdmissionState<E>>,
    ) -> Self {
        Self {
            executor,
            keys,
            manifests,
            membership_ledger,
            admissions,
        }
    }
}

impl<E: DbExecutor> SqliteSpaceAdmissionCredentials<E> {
    pub async fn ensure_registration(
        &self,
        passphrase: &Passphrase,
    ) -> Result<(), SpaceAdmissionCredentialStoreError> {
        let scope = self.active_scope().await.map_err(map_store_error)?;
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    if self.load_on(conn, &scope)?.is_some() {
                        return Ok(());
                    }
                    let server_setup = SpaceAdmissionAuth::generate_server_setup();
                    let registration = SpaceAdmissionAuth::register(&server_setup, passphrase)
                        .map_err(anyhow::Error::new)?;
                    let setup = server_setup.encode_for_encryption();
                    let registration = registration.encode_for_encryption();
                    let encrypted = seal_credentials(
                        &self.keys,
                        &scope,
                        setup.as_bytes().to_vec(),
                        registration.as_bytes().to_vec(),
                    )?;
                    sql_query(
                        "INSERT INTO space_admission_credentials (singleton_id, encrypted_payload) \
                     VALUES (1, ?) ON CONFLICT(singleton_id) DO UPDATE SET \
                     encrypted_payload = excluded.encrypted_payload",
                    )
                    .bind::<Binary, _>(encrypted)
                    .execute(conn)?;
                    self.load_on(conn, &scope)?
                        .ok_or_else(|| anyhow::anyhow!("credential write was not durable"))?;
                    Ok(())
                })
            })
            .map_err(map_store_error)
    }

    fn load_on(
        &self,
        conn: &mut SqliteConnection,
        scope: &CredentialScope,
    ) -> anyhow::Result<Option<SponsorOpaqueMaterial>> {
        let row = load_encrypted_row(conn)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let persisted = open_credentials(&self.keys, &row.encrypted_payload)?;
        if persisted.scope != *scope {
            return Ok(None);
        }
        let server_setup =
            SpaceAdmissionAuth::decode_server_setup_after_decryption(&persisted.server_setup)
                .map_err(anyhow::Error::new)?;
        let registration =
            SpaceAdmissionAuth::decode_registration_after_decryption(&persisted.registration)
                .map_err(anyhow::Error::new)?;
        Ok(Some(SponsorOpaqueMaterial::new(server_setup, registration)))
    }

    async fn load_initial(
        &self,
    ) -> Result<SponsorOpaqueMaterial, SpaceAdmissionCredentialStoreError> {
        let scope = self.active_scope().await.map_err(map_store_error)?;
        self.executor
            .run(|conn| {
                self.load_on(conn, &scope)?
                    .ok_or_else(|| anyhow::anyhow!("space admission registration is missing"))
            })
            .map_err(map_store_error)
    }

    async fn active_scope(&self) -> anyhow::Result<CredentialScope> {
        if let Some(manifest) = self
            .manifests
            .load_runtime()
            .await
            .map_err(anyhow::Error::new)?
        {
            return Ok(CredentialScope::from(manifest));
        }
        let ledger = self
            .membership_ledger
            .load()
            .await
            .map_err(anyhow::Error::new)?;
        let space_id = ledger
            .lineage_id
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("current legacy Space identity is missing"))?;
        Ok(CredentialScope::Legacy {
            space_id,
            keyslot_generation: [0; 16],
            database_generation: [0; 16],
            security_generation: [0; 16],
        })
    }
}

impl From<ActiveSpaceGenerationManifestV2> for CredentialScope {
    fn from(manifest: ActiveSpaceGenerationManifestV2) -> Self {
        Self::Legacy {
            space_id: manifest.space_id,
            keyslot_generation: manifest.keyslot_generation,
            database_generation: manifest.database_generation,
            security_generation: manifest.security_generation,
        }
    }
}

impl From<ActiveRuntimeManifest> for CredentialScope {
    fn from(manifest: ActiveRuntimeManifest) -> Self {
        match manifest {
            ActiveRuntimeManifest::V2(manifest) => Self::from(manifest),
            ActiveRuntimeManifest::V3(manifest) => Self::Control {
                space_id: manifest.layout().space_id().as_ref().to_owned(),
                keyslot_generation: *manifest.keyslot_generation(),
                space_control_generation: *manifest.layout().space_control_generation(),
            },
        }
    }
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> SpaceAdmissionChannelCredentialPort
    for SqliteSpaceAdmissionCredentials<E>
{
    async fn resolve_initial(
        &self,
        _invitation_id: InvitationId,
        _admission_id: SpaceAdmissionId,
    ) -> Result<SponsorOpaqueMaterial, SpaceAdmissionChannelCredentialError> {
        self.load_initial().await.map_err(map_channel_error)
    }

    async fn load_continuation(
        &self,
        admission_id: SpaceAdmissionId,
    ) -> Result<AdmissionContinuationCredential, SpaceAdmissionChannelCredentialError> {
        self.admissions
            .load_continuation_credential(admission_id)
            .map_err(map_admission_state_error)
    }
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> PrepareSpaceAdmissionCredentialsPort
    for SqliteSpaceAdmissionCredentials<E>
{
    async fn ensure_for_unlocked_space(
        &self,
        passphrase: &Passphrase,
    ) -> Result<(), SpaceAdmissionCredentialPreparationError> {
        self.ensure_registration(passphrase)
            .await
            .map_err(|error| match error {
                SpaceAdmissionCredentialStoreError::Locked { source } => {
                    SpaceAdmissionCredentialPreparationError::Locked { source }
                }
                SpaceAdmissionCredentialStoreError::RecoveryRequired { source } => {
                    SpaceAdmissionCredentialPreparationError::RecoveryRequired { source }
                }
                SpaceAdmissionCredentialStoreError::Unavailable { source } => {
                    SpaceAdmissionCredentialPreparationError::Unavailable { source }
                }
            })
    }
}

fn map_store_error(error: anyhow::Error) -> SpaceAdmissionCredentialStoreError {
    if matches!(
        error.downcast_ref::<AdmissionKeyError>(),
        Some(AdmissionKeyError::SecureStorage)
    ) {
        SpaceAdmissionCredentialStoreError::Locked { source: error }
    } else if error.downcast_ref::<AdmissionKeyError>().is_some() {
        SpaceAdmissionCredentialStoreError::RecoveryRequired { source: error }
    } else {
        SpaceAdmissionCredentialStoreError::Unavailable { source: error }
    }
}

fn map_channel_error(
    error: SpaceAdmissionCredentialStoreError,
) -> SpaceAdmissionChannelCredentialError {
    match error {
        SpaceAdmissionCredentialStoreError::Locked { source }
        | SpaceAdmissionCredentialStoreError::Unavailable { source } => {
            SpaceAdmissionChannelCredentialError::Unavailable { source }
        }
        SpaceAdmissionCredentialStoreError::RecoveryRequired { source } => {
            SpaceAdmissionChannelCredentialError::Rejected { source }
        }
    }
}

fn map_admission_state_error(
    error: SpaceAdmissionStateStoreError,
) -> SpaceAdmissionChannelCredentialError {
    match error {
        SpaceAdmissionStateStoreError::Locked | SpaceAdmissionStateStoreError::Unavailable => {
            SpaceAdmissionChannelCredentialError::Unavailable {
                source: anyhow::Error::new(error),
            }
        }
        SpaceAdmissionStateStoreError::Conflict | SpaceAdmissionStateStoreError::Corrupt => {
            SpaceAdmissionChannelCredentialError::Rejected {
                source: anyhow::Error::new(error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use diesel::sql_query;
    use diesel::sql_types::Binary;
    use diesel::{QueryableByName, RunQueryDsl};
    use uc_application::deps::{
        LoadMembershipLedgerPort, LoadedMembershipLedger, MembershipLedgerError,
    };
    use uc_core::ids::SpaceId;
    use uc_core::membership::{
        ActiveRuntimeLayout, AdmissionChannelPeerId, SpaceAdmissionProtocolVersion,
    };
    use uc_core::ports::{SecureStorageError, SecureStoragePort};

    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::pool::init_db_pool;
    use crate::db::ports::DbExecutor;
    use crate::security::{
        ActiveRuntimeManifestV3, ActiveSpaceGenerationManifestStore, SpaceAdmissionAuthContext,
    };

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

    struct EmptyLedger;

    #[async_trait::async_trait]
    impl LoadMembershipLedgerPort for EmptyLedger {
        async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
            Ok(LoadedMembershipLedger::no_current_space())
        }
    }

    struct LegacyLedger;

    #[async_trait::async_trait]
    impl LoadMembershipLedgerPort for LegacyLedger {
        async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
            let mut ledger = LoadedMembershipLedger::no_current_space();
            ledger.lineage_id = Some("legacy-space".to_owned());
            Ok(ledger)
        }
    }

    #[tokio::test]
    async fn legacy_layout_binds_registration_to_membership_lineage() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("credentials.sqlite");
        let secure_storage = Arc::new(MemorySecureStorage::default());
        let keys = Arc::new(AdmissionKeyManager::new(secure_storage, [0x71; 16]));
        let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
            temp.path().join("vault"),
            Arc::clone(&keys),
        ));
        let executor = Arc::new(DieselSqliteExecutor::new(
            init_db_pool(db_path.to_str().unwrap()).unwrap(),
        ));
        let admissions = Arc::new(SqliteSpaceAdmissionState::new(
            Arc::clone(&executor),
            Arc::clone(&keys),
            Arc::clone(&manifests),
            Arc::new(LegacyLedger),
        ));
        let credentials = SqliteSpaceAdmissionCredentials::new(
            executor,
            keys,
            manifests,
            Arc::new(LegacyLedger),
            admissions,
        );

        credentials
            .ensure_registration(&Passphrase::new("legacy passphrase"))
            .await
            .unwrap();
        credentials
            .resolve_initial(
                InvitationId::from_bytes([0x72; 32]).unwrap(),
                SpaceAdmissionId::from_bytes([0x73; 32]).unwrap(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn encrypted_registration_reopens_and_authenticates_the_space_passphrase() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("credentials.sqlite");
        let secure_storage = Arc::new(MemorySecureStorage::default());
        let manifest_keys = Arc::new(AdmissionKeyManager::new(secure_storage.clone(), [0x81; 16]));
        let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
            temp.path().join("vault"),
            manifest_keys,
        ));
        manifests
            .promote(
                &ActiveSpaceGenerationManifestV2::new(
                    "space-a".to_owned(),
                    [0x91; 16],
                    [0x92; 16],
                    [0x93; 16],
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let open = || {
            let executor = Arc::new(DieselSqliteExecutor::new(
                init_db_pool(db_path.to_str().unwrap()).unwrap(),
            ));
            let keys = Arc::new(AdmissionKeyManager::new(secure_storage.clone(), [0x81; 16]));
            let admissions = Arc::new(SqliteSpaceAdmissionState::new(
                executor.clone(),
                keys.clone(),
                manifests.clone(),
                Arc::new(EmptyLedger),
            ));
            SqliteSpaceAdmissionCredentials::new(
                executor,
                keys,
                manifests.clone(),
                Arc::new(EmptyLedger),
                admissions,
            )
        };
        let passphrase = Passphrase::new("correct horse battery staple");
        open().ensure_registration(&passphrase).await.unwrap();

        let invitation_id = InvitationId::from_bytes([0x82; 32]).unwrap();
        let admission_id = SpaceAdmissionId::from_bytes([0x83; 32]).unwrap();
        let material = open()
            .resolve_initial(invitation_id, admission_id)
            .await
            .unwrap();
        let (server_setup, registration) = material.into_parts();
        let context = SpaceAdmissionAuthContext::new(
            SpaceAdmissionProtocolVersion::V1,
            admission_id,
            invitation_id,
            AdmissionChannelPeerId::from_bytes([0x84; 32]).unwrap(),
            AdmissionChannelPeerId::from_bytes([0x85; 32]).unwrap(),
        );
        let (client, ke1) = SpaceAdmissionAuth::start_client(&passphrase, &context).unwrap();
        let (server, ke2) =
            SpaceAdmissionAuth::start_server(&server_setup, &registration, &context, ke1).unwrap();
        let (client_credential, ke3) = client.finish(&context, ke2).unwrap();
        let server_credential = server.finish(&context, ke3).unwrap();
        assert!(client_credential == server_credential);

        #[derive(QueryableByName)]
        struct Row {
            #[diesel(sql_type = Binary)]
            encrypted_payload: Vec<u8>,
        }
        let executor = DieselSqliteExecutor::new(init_db_pool(db_path.to_str().unwrap()).unwrap());
        let encrypted = executor
            .run(|conn| {
                Ok(sql_query(
                    "SELECT encrypted_payload FROM space_admission_credentials WHERE singleton_id = 1",
                )
                .get_result::<Row>(conn)?
                .encrypted_payload)
            })
            .unwrap();
        assert!(!encrypted
            .windows(passphrase.expose().len())
            .any(|window| window == passphrase.expose().as_bytes()));

        manifests
            .promote(
                &ActiveSpaceGenerationManifestV2::new(
                    "space-b".to_owned(),
                    [0xa1; 16],
                    [0xa2; 16],
                    [0xa3; 16],
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(open()
            .resolve_initial(invitation_id, admission_id)
            .await
            .is_err());
        open().ensure_registration(&passphrase).await.unwrap();
        let replacement = executor
            .run(|conn| {
                Ok(sql_query(
                    "SELECT encrypted_payload FROM space_admission_credentials WHERE singleton_id = 1",
                )
                .get_result::<Row>(conn)?
                .encrypted_payload)
            })
            .unwrap();
        assert_ne!(replacement, encrypted);

        let replacement_material = open()
            .resolve_initial(invitation_id, admission_id)
            .await
            .unwrap();
        let (server_setup, registration) = replacement_material.into_parts();
        let (client, ke1) = SpaceAdmissionAuth::start_client(&passphrase, &context).unwrap();
        let (server, ke2) =
            SpaceAdmissionAuth::start_server(&server_setup, &registration, &context, ke1).unwrap();
        let (client_credential, ke3) = client.finish(&context, ke2).unwrap();
        let server_credential = server.finish(&context, ke3).unwrap();
        assert!(client_credential == server_credential);
    }

    #[tokio::test]
    async fn v3_registration_binds_to_the_active_control_generation() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("credentials.sqlite");
        let secure_storage = Arc::new(MemorySecureStorage::default());
        let keys = Arc::new(AdmissionKeyManager::new(secure_storage, [0xb1; 16]));
        let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
            temp.path().join("vault"),
            Arc::clone(&keys),
        ));
        let source = ActiveSpaceGenerationManifestV2::new(
            "space-a".to_owned(),
            [0xb2; 16],
            [0xb3; 16],
            [0xb4; 16],
        )
        .unwrap();
        manifests.promote(&source).await.unwrap();
        let target = ActiveRuntimeManifestV3::new(
            ActiveRuntimeLayout::new(
                SpaceId::from_string("space-a".to_owned()),
                [0xb5; 16],
                [0xb6; 16],
            )
            .unwrap(),
            source.keyslot_generation,
        )
        .unwrap();
        let _ = manifests
            .promote_v3_from_v2(&source, &target)
            .await
            .unwrap();

        let executor = Arc::new(DieselSqliteExecutor::new(
            init_db_pool(db_path.to_str().unwrap()).unwrap(),
        ));
        let admissions = Arc::new(SqliteSpaceAdmissionState::new(
            Arc::clone(&executor),
            Arc::clone(&keys),
            Arc::clone(&manifests),
            Arc::new(EmptyLedger),
        ));
        let credentials = SqliteSpaceAdmissionCredentials::new(
            executor,
            keys,
            manifests,
            Arc::new(EmptyLedger),
            admissions,
        );

        credentials
            .ensure_registration(&Passphrase::new("v3 passphrase"))
            .await
            .unwrap();
        credentials
            .resolve_initial(
                InvitationId::from_bytes([0xb7; 32]).unwrap(),
                SpaceAdmissionId::from_bytes([0xb8; 32]).unwrap(),
            )
            .await
            .unwrap();
    }
}
