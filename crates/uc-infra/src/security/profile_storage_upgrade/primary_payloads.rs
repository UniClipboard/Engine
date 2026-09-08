//! Inline 与 UCBL 的一次性 V3 转换边界。
//!
//! `StoresSeparated` target 始终保持只读。转换先在唯一临时目录中构建完整
//! 数据库与 blob tree，可读内容使用正式 V3 reader 回读，认证失败的旧 blob
//! 校验原密文与 Lost 引用后再以目录 rename 发布；因此
//! 进程在任意 payload 之间终止都不会产生半转换数据库。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use diesel::connection::SimpleConnection as _;
use diesel::{Connection as _, RunQueryDsl as _};
use uc_core::blob::ports::BlobReaderPort;
use uc_core::crypto::aad;
use uc_core::crypto::domain::{Aad, Ciphertext};
use uc_core::ids::{EventId, RepresentationId};
use uc_core::ports::security::BlobCipherPort as _;
use uc_core::BlobId;

use crate::blob::{BlobStorePort, FilesystemBlobStore};
use crate::security::{
    BlobCipherAdapter, ContentProtection, EncryptedBlobStore, ProfileContentKeyVault,
    V3EncryptedBlobStore, V3InlinePayloadCipher,
};
use crate::space::InMemorySession;

use super::journal::UpgradeJournalV1;
use super::target::{file_digest, TargetGenerationStager};
use super::ProfileStorageUpgradeError;

const V3_BLOB_ALGORITHM: &str = "xchacha20poly1305-v3";
const PRESERVED_BLOB_ERROR: &str =
    "unreadable encrypted payload preserved during profile storage upgrade";

const OUTPUT_DATABASE: &str = "profile.sqlite";
const OUTPUT_BLOBS: &str = "blobs";
const BLOB_TREE_DIGEST_DOMAIN: &[u8] = b"uniclipboard/profile-upgrade-blob-tree/v1\0";

pub(super) struct PrimaryPayloadConverter {
    source_blob_root: PathBuf,
    source_session: Arc<InMemorySession>,
    content_protection: Arc<ContentProtection>,
}

pub(super) struct ConvertedPrimaryPayloads {
    pub(super) profile_database_digest: [u8; 32],
    pub(super) blob_tree_digest: [u8; 32],
    pub(super) inline_count: u64,
    pub(super) blob_count: u64,
}

impl PrimaryPayloadConverter {
    pub(super) fn new(
        source_blob_root: PathBuf,
        source_session: Arc<InMemorySession>,
        vault: Arc<ProfileContentKeyVault>,
    ) -> Self {
        Self {
            source_blob_root,
            content_protection: Arc::new(ContentProtection::for_content(
                Arc::clone(&source_session),
                vault,
            )),
            source_session,
        }
    }

    pub(super) async fn convert(
        &self,
        journal: &UpgradeJournalV1,
        target: &TargetGenerationStager,
    ) -> Result<ConvertedPrimaryPayloads, ProfileStorageUpgradeError> {
        target.verify_separated(journal)?;
        let paths = target.paths(journal);
        if paths.primary_output.is_dir() {
            let converted = self
                .inspect_output(&paths.profile_database, &paths.primary_output)
                .await?;
            target.verify_source_revision(journal)?;
            return Ok(converted);
        }
        if paths.primary_output.exists() {
            return Err(corrupt(anyhow::anyhow!(
                "profile upgrade primary output has an invalid type"
            )));
        }

        let parent = paths
            .primary_output
            .parent()
            .ok_or_else(|| storage(anyhow::anyhow!("primary output parent is missing")))?;
        std::fs::create_dir_all(parent).map_err(io_storage)?;
        let work = parent.join(format!(".v3-primary-{}.tmp", uuid::Uuid::new_v4()));
        let result = self
            .build_output(&paths.profile_database, &paths.primary_output, &work)
            .await;
        let converted = match result {
            Ok(converted) => converted,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&work);
                return Err(error);
            }
        };
        std::fs::rename(&work, &paths.primary_output).map_err(io_storage)?;
        sync_directory(parent).map_err(io_storage)?;
        target.verify_source_revision(journal)?;
        Ok(converted)
    }

    pub(super) async fn verify(
        &self,
        journal: &UpgradeJournalV1,
        target: &TargetGenerationStager,
    ) -> Result<(), ProfileStorageUpgradeError> {
        target.verify_separated(journal)?;
        let paths = target.paths(journal);
        let converted = self
            .inspect_output(&paths.profile_database, &paths.primary_output)
            .await?;
        let expected_database = journal.primary_profile_database_digest().ok_or_else(|| {
            corrupt(anyhow::anyhow!(
                "primary profile database digest is missing"
            ))
        })?;
        let expected_blobs = journal
            .primary_blob_tree_digest()
            .ok_or_else(|| corrupt(anyhow::anyhow!("primary blob tree digest is missing")))?;
        let expected_inline = journal
            .converted_inline_count()
            .ok_or_else(|| corrupt(anyhow::anyhow!("converted inline count is missing")))?;
        let expected_blob = journal
            .converted_blob_count()
            .ok_or_else(|| corrupt(anyhow::anyhow!("converted blob count is missing")))?;
        if converted.profile_database_digest != expected_database
            || converted.blob_tree_digest != expected_blobs
            || converted.inline_count != expected_inline
            || converted.blob_count != expected_blob
        {
            return Err(corrupt(anyhow::anyhow!(
                "profile upgrade primary output digest or count mismatch"
            )));
        }
        target.verify_source_revision(journal)
    }

    async fn build_output(
        &self,
        separated_database: &Path,
        final_output: &Path,
        work: &Path,
    ) -> Result<ConvertedPrimaryPayloads, ProfileStorageUpgradeError> {
        std::fs::create_dir(work).map_err(io_storage)?;
        let database = work.join(OUTPUT_DATABASE);
        std::fs::copy(separated_database, &database).map_err(io_storage)?;
        std::fs::File::open(&database)
            .and_then(|file| file.sync_all())
            .map_err(io_storage)?;
        let inline_count = self.convert_inline(&database).await?;
        let blob_count = self.convert_blobs(&database, work, final_output).await?;
        self.verify_payloads(&database, work).await?;
        compact_database(&database)?;
        sync_directory(work).map_err(io_storage)?;
        Ok(ConvertedPrimaryPayloads {
            profile_database_digest: file_digest(&database)?,
            blob_tree_digest: blob_tree_digest(&work.join(OUTPUT_BLOBS))?,
            inline_count,
            blob_count,
        })
    }

    async fn inspect_output(
        &self,
        separated_database: &Path,
        output: &Path,
    ) -> Result<ConvertedPrimaryPayloads, ProfileStorageUpgradeError> {
        let database = output.join(OUTPUT_DATABASE);
        verify_row_identity(separated_database, &database)?;
        let (inline_count, blob_count) = self.verify_payloads(&database, output).await?;
        Ok(ConvertedPrimaryPayloads {
            profile_database_digest: file_digest(&database)?,
            blob_tree_digest: blob_tree_digest(&output.join(OUTPUT_BLOBS))?,
            inline_count,
            blob_count,
        })
    }

    async fn convert_inline(&self, database: &Path) -> Result<u64, ProfileStorageUpgradeError> {
        let rows = load_inline_rows(&mut open_connection(database)?)?;
        let legacy = BlobCipherAdapter::new(Arc::clone(&self.source_session));
        let v3 = V3InlinePayloadCipher::new(Arc::clone(&self.content_protection));
        let mut converted = Vec::with_capacity(rows.len());
        for row in rows {
            let aad = inline_aad(&row);
            let plaintext = legacy
                .decrypt(&Ciphertext::new(row.inline_data), &aad)
                .await
                .map_err(|source| {
                    corrupt(anyhow::Error::new(source).context("open legacy inline payload"))
                })?;
            let ciphertext = v3.encrypt(&plaintext, &aad).await.map_err(|source| {
                security(anyhow::Error::new(source).context("seal V3 inline payload"))
            })?;
            converted.push((row.id, ciphertext.into_bytes()));
        }
        let count = u64::try_from(converted.len())
            .map_err(|source| corrupt(anyhow::Error::new(source).context("count inline rows")))?;
        let mut connection = open_connection(database)?;
        connection
            .transaction::<_, diesel::result::Error, _>(|connection| {
                for (id, ciphertext) in &converted {
                    diesel::sql_query(
                        "UPDATE clipboard_snapshot_representation SET inline_data = ? WHERE id = ?",
                    )
                    .bind::<diesel::sql_types::Binary, _>(ciphertext)
                    .bind::<diesel::sql_types::Text, _>(id)
                    .execute(connection)?;
                }
                Ok(())
            })
            .map_err(database_storage)?;
        Ok(count)
    }

    async fn convert_blobs(
        &self,
        database: &Path,
        work: &Path,
        final_output: &Path,
    ) -> Result<u64, ProfileStorageUpgradeError> {
        let rows = load_blob_rows(&mut open_connection(database)?)?;
        let source = EncryptedBlobStore::new(
            Arc::new(FilesystemBlobStore::new(self.source_blob_root.clone())),
            Arc::clone(&self.source_session),
        );
        let work_blob_root = work.join(OUTPUT_BLOBS);
        std::fs::create_dir_all(&work_blob_root).map_err(io_storage)?;
        let target = V3EncryptedBlobStore::new(
            Arc::new(FilesystemBlobStore::new(work_blob_root.clone())),
            Arc::clone(&self.content_protection),
        );
        let mut converted = Vec::with_capacity(rows.len());
        for row in rows {
            let blob_id = BlobId::from(row.blob_id.as_str());
            let source_bytes =
                std::fs::read(self.source_blob_root.join(blob_id.as_str())).map_err(io_storage)?;
            let plaintext = match source.open_bytes(&blob_id, &source_bytes) {
                Ok(plaintext) => plaintext,
                Err(source) if is_unreadable_ciphertext(&source) => {
                    // 只保存原密文；不可用状态与 blob 行在同一候选数据库事务中提交。
                    // 格式、会话、缺钥和介质失败仍向上传递，不能把它们猜成历史损坏。
                    let preserved = work_blob_root.join(blob_id.as_str());
                    std::fs::write(&preserved, &source_bytes).map_err(io_storage)?;
                    std::fs::File::open(&preserved)
                        .and_then(|file| file.sync_all())
                        .map_err(io_storage)?;
                    converted.push((
                        row.blob_id,
                        final_output.join(OUTPUT_BLOBS).join(blob_id.as_str()),
                        None,
                    ));
                    continue;
                }
                Err(source) => return Err(corrupt(source.context("open legacy UCBL payload"))),
            };
            drop(source_bytes);
            let (_, compressed_size) = target
                .put(&blob_id, &plaintext)
                .await
                .map_err(|source| storage(source.context("persist V3 UCBL payload")))?;
            let reopened = BlobReaderPort::get(&target, &blob_id)
                .await
                .map_err(|source| corrupt(source.context("reopen V3 UCBL payload")))?;
            if reopened != plaintext {
                return Err(corrupt(anyhow::anyhow!("V3 blob verification mismatch")));
            }
            let storage_path = final_output.join(OUTPUT_BLOBS).join(blob_id.as_str());
            converted.push((row.blob_id, storage_path, Some(compressed_size)));
        }
        sync_directory(&work_blob_root).map_err(io_storage)?;
        let count = u64::try_from(converted.len())
            .map_err(|source| corrupt(anyhow::Error::new(source).context("count blob rows")))?;
        let mut connection = open_connection(database)?;
        connection
            .transaction::<_, diesel::result::Error, _>(|connection| {
                for (blob_id, storage_path, compressed_size) in &converted {
                    if let Some(compressed_size) = compressed_size {
                        diesel::sql_query(
                            "UPDATE blob SET storage_path = ?, compressed_size = ?, \
                             encryption_algo = ? WHERE blob_id = ?",
                        )
                        .bind::<diesel::sql_types::Text, _>(storage_path.to_string_lossy().as_ref())
                        .bind::<diesel::sql_types::Nullable<diesel::sql_types::BigInt>, _>(
                            compressed_size,
                        )
                        .bind::<diesel::sql_types::Text, _>(V3_BLOB_ALGORITHM)
                        .bind::<diesel::sql_types::Text, _>(blob_id)
                        .execute(connection)?;
                    } else {
                        diesel::sql_query("UPDATE blob SET storage_path = ? WHERE blob_id = ?")
                            .bind::<diesel::sql_types::Text, _>(
                                storage_path.to_string_lossy().as_ref(),
                            )
                            .bind::<diesel::sql_types::Text, _>(blob_id)
                            .execute(connection)?;
                        diesel::sql_query(
                            "UPDATE clipboard_snapshot_representation \
                             SET payload_state = 'Lost', last_error = ? WHERE blob_id = ?",
                        )
                        .bind::<diesel::sql_types::Text, _>(PRESERVED_BLOB_ERROR)
                        .bind::<diesel::sql_types::Text, _>(blob_id)
                        .execute(connection)?;
                    }
                }
                Ok(())
            })
            .map_err(database_storage)?;
        Ok(count)
    }

    fn verify_preserved_blob(
        &self,
        database: &Path,
        output: &Path,
        row: &BlobRow,
    ) -> Result<(), ProfileStorageUpgradeError> {
        let blob_id = BlobId::from(row.blob_id.as_str());
        let original =
            std::fs::read(self.source_blob_root.join(blob_id.as_str())).map_err(io_storage)?;
        let preserved =
            std::fs::read(output.join(OUTPUT_BLOBS).join(blob_id.as_str())).map_err(io_storage)?;
        if original != preserved {
            return Err(corrupt(anyhow::anyhow!(
                "preserved legacy ciphertext changed"
            )));
        }
        let legacy = EncryptedBlobStore::new(
            Arc::new(FilesystemBlobStore::new(self.source_blob_root.clone())),
            Arc::clone(&self.source_session),
        );
        match legacy.open_bytes(&blob_id, &preserved) {
            Err(source) if is_unreadable_ciphertext(&source) => {}
            Err(source) => {
                return Err(corrupt(
                    source.context("verify preserved legacy ciphertext"),
                ))
            }
            Ok(_) => {
                return Err(corrupt(anyhow::anyhow!(
                    "readable legacy ciphertext was not converted"
                )))
            }
        }
        use crate::db::schema::clipboard_snapshot_representation::dsl as representation;
        use diesel::prelude::*;
        let states = representation::clipboard_snapshot_representation
            .filter(representation::blob_id.eq(blob_id.as_str()))
            .select((representation::payload_state, representation::last_error))
            .load::<(String, Option<String>)>(&mut open_connection(database)?)
            .map_err(database_storage)?;
        if states
            .iter()
            .any(|(state, error)| state != "Lost" || error.as_deref() != Some(PRESERVED_BLOB_ERROR))
        {
            return Err(corrupt(anyhow::anyhow!(
                "preserved legacy ciphertext is not marked unavailable"
            )));
        }
        Ok(())
    }

    async fn verify_payloads(
        &self,
        database: &Path,
        output: &Path,
    ) -> Result<(u64, u64), ProfileStorageUpgradeError> {
        let v3_inline = V3InlinePayloadCipher::new(Arc::clone(&self.content_protection));
        let inline_rows = load_inline_rows(&mut open_connection(database)?)?;
        for row in &inline_rows {
            v3_inline
                .decrypt(&Ciphertext::new(row.inline_data.clone()), &inline_aad(row))
                .await
                .map_err(|source| {
                    corrupt(anyhow::Error::new(source).context("verify V3 inline payload"))
                })?;
        }
        let v3_blobs = V3EncryptedBlobStore::new(
            Arc::new(FilesystemBlobStore::new(output.join(OUTPUT_BLOBS))),
            Arc::clone(&self.content_protection),
        );
        let blob_rows = load_blob_rows(&mut open_connection(database)?)?;
        for row in &blob_rows {
            if row.encryption_algo.as_deref() != Some(V3_BLOB_ALGORITHM) {
                self.verify_preserved_blob(database, output, row)?;
                continue;
            }
            BlobReaderPort::get(&v3_blobs, &BlobId::from(row.blob_id.as_str()))
                .await
                .map_err(|source| corrupt(source.context("verify V3 UCBL payload")))?;
        }
        Ok((
            u64::try_from(inline_rows.len()).map_err(|source| {
                corrupt(anyhow::Error::new(source).context("count verified inline rows"))
            })?,
            u64::try_from(blob_rows.len()).map_err(|source| {
                corrupt(anyhow::Error::new(source).context("count verified blob rows"))
            })?,
        ))
    }
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
struct BlobRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    blob_id: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    encryption_algo: Option<String>,
}

fn is_unreadable_ciphertext(source: &anyhow::Error) -> bool {
    matches!(
        source.downcast_ref::<crate::security::v1_aead::AeadError>(),
        Some(crate::security::v1_aead::AeadError::DecryptFailed)
    )
}

fn load_inline_rows(
    connection: &mut diesel::sqlite::SqliteConnection,
) -> Result<Vec<InlineRow>, ProfileStorageUpgradeError> {
    diesel::sql_query(
        "SELECT id, event_id, inline_data FROM clipboard_snapshot_representation \
         WHERE inline_data IS NOT NULL ORDER BY id",
    )
    .load::<InlineRow>(connection)
    .map_err(database_storage)
}

fn load_blob_rows(
    connection: &mut diesel::sqlite::SqliteConnection,
) -> Result<Vec<BlobRow>, ProfileStorageUpgradeError> {
    diesel::sql_query("SELECT blob_id, encryption_algo FROM blob ORDER BY blob_id")
        .load::<BlobRow>(connection)
        .map_err(database_storage)
}

fn inline_aad(row: &InlineRow) -> Aad {
    Aad::from(aad::for_inline(
        &EventId::from_string(row.event_id.clone()),
        &RepresentationId::from(row.id.clone()),
    ))
}

fn verify_row_identity(
    separated_database: &Path,
    converted_database: &Path,
) -> Result<(), ProfileStorageUpgradeError> {
    let separated_inline = load_inline_rows(&mut open_connection(separated_database)?)?
        .into_iter()
        .map(|row| (row.id, row.event_id))
        .collect::<Vec<_>>();
    let converted_inline = load_inline_rows(&mut open_connection(converted_database)?)?
        .into_iter()
        .map(|row| (row.id, row.event_id))
        .collect::<Vec<_>>();
    let separated_blobs = load_blob_rows(&mut open_connection(separated_database)?)?
        .into_iter()
        .map(|row| row.blob_id)
        .collect::<Vec<_>>();
    let converted_blobs = load_blob_rows(&mut open_connection(converted_database)?)?
        .into_iter()
        .map(|row| row.blob_id)
        .collect::<Vec<_>>();
    if separated_inline != converted_inline || separated_blobs != converted_blobs {
        return Err(corrupt(anyhow::anyhow!(
            "profile upgrade primary row identity mismatch"
        )));
    }
    Ok(())
}

fn open_connection(
    path: &Path,
) -> Result<diesel::sqlite::SqliteConnection, ProfileStorageUpgradeError> {
    let database = path
        .to_str()
        .ok_or_else(|| storage(anyhow::anyhow!("primary database path is invalid")))?;
    let mut connection =
        diesel::sqlite::SqliteConnection::establish(database).map_err(|source| {
            storage(anyhow::Error::new(source).context("open primary conversion database"))
        })?;
    connection
        .batch_execute("PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;")
        .map_err(database_storage)?;
    Ok(connection)
}

pub(super) fn compact_database(path: &Path) -> Result<(), ProfileStorageUpgradeError> {
    let database = path
        .to_str()
        .ok_or_else(|| storage(anyhow::anyhow!("primary database path is invalid")))?;
    let mut connection =
        diesel::sqlite::SqliteConnection::establish(database).map_err(|source| {
            storage(anyhow::Error::new(source).context("open primary database for compaction"))
        })?;
    connection
        .batch_execute("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode = DELETE; VACUUM;")
        .map_err(database_storage)?;
    std::fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(io_storage)
}

pub(super) fn blob_tree_digest(root: &Path) -> Result<[u8; 32], ProfileStorageUpgradeError> {
    let mut names = Vec::new();
    if root.is_dir() {
        for entry in std::fs::read_dir(root).map_err(io_storage)? {
            let entry = entry.map_err(io_storage)?;
            if !entry.file_type().map_err(io_storage)?.is_file() {
                return Err(corrupt(anyhow::anyhow!(
                    "profile upgrade blob tree contains a non-file entry"
                )));
            }
            names.push(entry.file_name());
        }
    }
    names.sort();
    let mut hasher = blake3::Hasher::new();
    hasher.update(BLOB_TREE_DIGEST_DOMAIN);
    for name in names {
        let name = name
            .to_str()
            .ok_or_else(|| corrupt(anyhow::anyhow!("blob tree name is invalid")))?;
        let bytes = std::fs::read(root.join(name)).map_err(io_storage)?;
        hasher.update(&(name.len() as u64).to_be_bytes());
        hasher.update(name.as_bytes());
        hasher.update(&(bytes.len() as u64).to_be_bytes());
        hasher.update(&bytes);
    }
    Ok(*hasher.finalize().as_bytes())
}

#[cfg(not(windows))]
pub(super) fn sync_directory(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
pub(super) fn sync_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn database_storage(source: diesel::result::Error) -> ProfileStorageUpgradeError {
    storage(anyhow::Error::new(source).context("update primary conversion database"))
}

fn io_storage(source: std::io::Error) -> ProfileStorageUpgradeError {
    storage(anyhow::Error::new(source).context("persist primary conversion output"))
}

fn storage(source: anyhow::Error) -> ProfileStorageUpgradeError {
    ProfileStorageUpgradeError::Storage { source }
}

fn security(source: anyhow::Error) -> ProfileStorageUpgradeError {
    ProfileStorageUpgradeError::Security { source }
}

fn corrupt(source: anyhow::Error) -> ProfileStorageUpgradeError {
    ProfileStorageUpgradeError::Corrupt { source }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use diesel::prelude::*;
    use uc_core::crypto::domain::Plaintext;
    use uc_core::ids::SpaceId;
    use uc_core::membership::ActiveSpaceGenerationManifestV2;
    use uc_core::ports::{SecureStorageError, SecureStoragePort};

    use super::*;
    use crate::db::pool::init_db_pool;
    use crate::security::MasterKey;

    #[derive(Default)]
    struct MemorySecureStorage(Mutex<BTreeMap<String, Vec<u8>>>);

    impl SecureStoragePort for MemorySecureStorage {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
            Ok(self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(key)
                .cloned())
        }

        fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(key.to_owned(), value.to_vec());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(key);
            Ok(())
        }
    }

    #[tokio::test]
    async fn primary_output_is_atomic_preserves_unreadable_blobs_and_is_digest_bound() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("profile");
        std::fs::create_dir_all(&root).unwrap();
        let source_database = root.join("source.sqlite");
        let source_pool = init_db_pool(source_database.to_str().unwrap()).unwrap();
        let source_blob_root = root.join("source-blobs");
        let session = Arc::new(InMemorySession::new());
        let space_id = SpaceId::from_str("source-space");
        session.set_master_key_for_space(
            space_id.clone(),
            MasterKey::from_bytes(&[0xB2; 32]).unwrap(),
        );
        let material = session
            .create_migrated_space_material(&space_id, 1)
            .unwrap();
        session.install_space_material(&material).unwrap();
        let secure_storage: Arc<dyn SecureStoragePort> = Arc::new(MemorySecureStorage::default());
        let vault = Arc::new(ProfileContentKeyVault::new(
            root.clone(),
            Arc::clone(&secure_storage),
            [0xB1; 16],
        ));
        vault
            .install_verified_space_material(&material)
            .await
            .unwrap();

        let event_id = EventId::from_string("event-primary".to_owned());
        let representation_id = RepresentationId::from("representation-primary");
        let inline_aad = Aad::from(aad::for_inline(&event_id, &representation_id));
        let inline_ciphertext = BlobCipherAdapter::new(Arc::clone(&session))
            .encrypt(
                &Plaintext::new(b"private inline payload".to_vec()),
                &inline_aad,
            )
            .await
            .unwrap();
        let blob_id = BlobId::from("blob-primary");
        let source_blob_store = EncryptedBlobStore::new(
            Arc::new(FilesystemBlobStore::new(source_blob_root.clone())),
            Arc::clone(&session),
        );
        let (source_blob_path, compressed_size) = source_blob_store
            .put(&blob_id, b"private blob payload")
            .await
            .unwrap();
        let unreadable_blob_id = BlobId::from("blob-unreadable");
        let unreadable_source_path = source_blob_root.join(unreadable_blob_id.as_str());
        // 与现场相同：V1 文件使用早期 MasterKey，当前 session 无法认证。
        let compressed = zstd::bulk::compress(b"unreadable private legacy payload", 3).unwrap();
        let encrypted = crate::security::v1_aead::encrypt_blob_xchacha(
            &MasterKey::from_bytes(&[0xA0; 32]).unwrap(),
            &compressed,
            &aad::for_blob_v2(&unreadable_blob_id),
        )
        .unwrap();
        let mut unreadable_source_bytes = b"UCBL\x01".to_vec();
        unreadable_source_bytes.extend_from_slice(&encrypted.nonce);
        unreadable_source_bytes.extend_from_slice(&encrypted.ciphertext);
        std::fs::write(&unreadable_source_path, &unreadable_source_bytes).unwrap();
        let mut connection = source_pool.get().unwrap();
        diesel::sql_query(
            "INSERT INTO clipboard_event \
             (event_id, captured_at_ms, source_device, snapshot_hash) \
             VALUES ('event-primary', 1, 'device-a', 'snapshot-primary')",
        )
        .execute(&mut connection)
        .unwrap();
        diesel::sql_query(
            "INSERT INTO blob \
             (blob_id, storage_path, storage_backend, size_bytes, content_hash, encryption_algo, \
              created_at_ms, compressed_size) \
             VALUES (?, ?, 'local_fs', 20, 'hash-unreadable', 'xchacha20poly1305', 1, ?)",
        )
        .bind::<diesel::sql_types::Text, _>(unreadable_blob_id.as_ref())
        .bind::<diesel::sql_types::Text, _>(unreadable_source_path.to_string_lossy().as_ref())
        .bind::<diesel::sql_types::Nullable<diesel::sql_types::BigInt>, _>(compressed_size)
        .execute(&mut connection)
        .unwrap();
        diesel::sql_query(
            "INSERT INTO blob \
             (blob_id, storage_path, storage_backend, size_bytes, content_hash, encryption_algo, \
              created_at_ms, compressed_size) \
             VALUES (?, ?, 'local_fs', 20, 'hash-primary', 'xchacha20poly1305', 1, ?)",
        )
        .bind::<diesel::sql_types::Text, _>(blob_id.as_ref())
        .bind::<diesel::sql_types::Text, _>(source_blob_path.to_string_lossy().as_ref())
        .bind::<diesel::sql_types::Nullable<diesel::sql_types::BigInt>, _>(compressed_size)
        .execute(&mut connection)
        .unwrap();
        diesel::sql_query(
            "INSERT INTO clipboard_snapshot_representation \
             (id, event_id, format_id, mime_type, size_bytes, inline_data, blob_id, payload_state) \
             VALUES ('representation-unreadable', 'event-primary', 'image', 'image/png', 20, \
                     NULL, ?, 'BlobReady')",
        )
        .bind::<diesel::sql_types::Text, _>(unreadable_blob_id.as_ref())
        .execute(&mut connection)
        .unwrap();
        diesel::sql_query(
            "INSERT INTO clipboard_snapshot_representation \
             (id, event_id, format_id, mime_type, size_bytes, inline_data, blob_id) \
             VALUES ('representation-primary', 'event-primary', 'text', 'text/plain', 22, ?, NULL)",
        )
        .bind::<diesel::sql_types::Binary, _>(inline_ciphertext.as_bytes())
        .execute(&mut connection)
        .unwrap();
        drop(connection);

        let source_manifest = ActiveSpaceGenerationManifestV2::new(
            "source-space".to_owned(),
            [0xB3; 16],
            [0xB4; 16],
            [0xB5; 16],
        )
        .unwrap();
        let mut journal = UpgradeJournalV1::detected(Some(&source_manifest));
        let keys = Arc::new(crate::security::AdmissionKeyManager::new(
            Arc::clone(&secure_storage),
            [0xB6; 16],
        ));
        let target = TargetGenerationStager::new(root, source_pool, keys);
        let staged = target.stage(&journal).unwrap();
        journal
            .mark_target_staged(
                staged.source_snapshot_digest,
                staged.source_database_revision,
            )
            .unwrap();
        let separated = target.separate(&journal, Some(&source_manifest)).unwrap();
        journal
            .mark_stores_separated(
                separated.profile_database_digest,
                separated.control_database_digest,
            )
            .unwrap();
        let converter = PrimaryPayloadConverter::new(
            source_blob_root,
            Arc::clone(&session),
            Arc::clone(&vault),
        );

        // 介质缺失必须保留 source chain，且不能发布半转换结果。
        std::fs::remove_file(&unreadable_source_path).unwrap();
        let missing = converter.convert(&journal, &target).await.err().unwrap();
        match missing {
            ProfileStorageUpgradeError::Storage { source } => {
                assert_eq!(
                    source.downcast_ref::<std::io::Error>().unwrap().kind(),
                    std::io::ErrorKind::NotFound
                );
            }
            other => panic!("unexpected missing-file result: {other:?}"),
        }
        assert!(!target.paths(&journal).primary_output.exists());
        std::fs::write(&unreadable_source_path, b"unrecognized format").unwrap();
        assert!(matches!(
            converter.convert(&journal, &target).await,
            Err(ProfileStorageUpgradeError::Corrupt { .. })
        ));
        assert!(!target.paths(&journal).primary_output.exists());
        std::fs::write(&unreadable_source_path, &unreadable_source_bytes).unwrap();

        let converted = converter.convert(&journal, &target).await.unwrap();
        assert_eq!(converted.inline_count, 1);
        assert_eq!(converted.blob_count, 2);
        let recovered = converter.convert(&journal, &target).await.unwrap();
        assert_eq!(
            recovered.profile_database_digest,
            converted.profile_database_digest
        );
        assert_eq!(recovered.blob_tree_digest, converted.blob_tree_digest);

        let output = target.paths(&journal).primary_output;
        let inline = load_inline_rows(&mut open_connection(&output.join(OUTPUT_DATABASE)).unwrap())
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(&inline.inline_data[..4], b"UCP3");
        let v3_inline = V3InlinePayloadCipher::new(Arc::new(ContentProtection::for_content(
            Arc::clone(&session),
            Arc::clone(&vault),
        )));
        assert_eq!(
            v3_inline
                .decrypt(&Ciphertext::new(inline.inline_data), &inline_aad)
                .await
                .unwrap()
                .as_bytes(),
            b"private inline payload"
        );
        let v3_blobs = V3EncryptedBlobStore::new(
            Arc::new(FilesystemBlobStore::new(output.join(OUTPUT_BLOBS))),
            Arc::new(ContentProtection::for_content(session, vault)),
        );
        assert_eq!(
            BlobReaderPort::get(&v3_blobs, &blob_id).await.unwrap(),
            b"private blob payload"
        );
        assert_eq!(
            std::fs::read(output.join(OUTPUT_BLOBS).join(unreadable_blob_id.as_str())).unwrap(),
            unreadable_source_bytes
        );
        let mut output_connection = open_connection(&output.join(OUTPUT_DATABASE)).unwrap();
        let (payload_state, last_error) =
            crate::db::schema::clipboard_snapshot_representation::table
                .filter(
                    crate::db::schema::clipboard_snapshot_representation::id
                        .eq("representation-unreadable"),
                )
                .select((
                    crate::db::schema::clipboard_snapshot_representation::payload_state,
                    crate::db::schema::clipboard_snapshot_representation::last_error,
                ))
                .first::<(String, Option<String>)>(&mut output_connection)
                .unwrap();
        assert_eq!(payload_state, "Lost");
        assert_eq!(
            last_error.as_deref(),
            Some("unreadable encrypted payload preserved during profile storage upgrade")
        );

        // 未写 journal 的发布窗口也必须检查保留副本与 Lost 状态。
        let preserved_path = output.join(OUTPUT_BLOBS).join(unreadable_blob_id.as_str());
        std::fs::write(&preserved_path, b"tampered ciphertext").unwrap();
        assert!(matches!(
            converter.convert(&journal, &target).await,
            Err(ProfileStorageUpgradeError::Corrupt { .. })
        ));
        std::fs::write(&preserved_path, &unreadable_source_bytes).unwrap();
        diesel::sql_query("UPDATE clipboard_snapshot_representation SET payload_state = 'BlobReady' WHERE id = 'representation-unreadable'")
            .execute(&mut output_connection).unwrap();
        assert!(matches!(
            converter.convert(&journal, &target).await,
            Err(ProfileStorageUpgradeError::Corrupt { .. })
        ));
        diesel::sql_query("UPDATE clipboard_snapshot_representation SET payload_state = 'Lost' WHERE id = 'representation-unreadable'")
            .execute(&mut output_connection).unwrap();
        // 恢复测试修改后重新取得摘要，正式 journal 必须覆盖完整候选库。
        let converted = converter.convert(&journal, &target).await.unwrap();
        assert_eq!(
            std::fs::read(&unreadable_source_path).unwrap(),
            unreadable_source_bytes
        );
        assert!(BlobReaderPort::get(&v3_blobs, &unreadable_blob_id)
            .await
            .is_err());

        journal
            .mark_primary_payloads_converted(
                converted.profile_database_digest,
                converted.blob_tree_digest,
                converted.inline_count,
                converted.blob_count,
            )
            .unwrap();
        converter.verify(&journal, &target).await.unwrap();
        std::fs::write(&preserved_path, b"tampered ciphertext").unwrap();
        assert!(matches!(
            converter.verify(&journal, &target).await,
            Err(ProfileStorageUpgradeError::Corrupt { .. })
        ));
        std::fs::write(&preserved_path, &unreadable_source_bytes).unwrap();
        converter.verify(&journal, &target).await.unwrap();
        std::fs::write(
            output.join(OUTPUT_BLOBS).join(blob_id.as_str()),
            b"tampered",
        )
        .unwrap();
        assert!(matches!(
            converter.verify(&journal, &target).await,
            Err(ProfileStorageUpgradeError::Corrupt { .. })
        ));
    }
}
