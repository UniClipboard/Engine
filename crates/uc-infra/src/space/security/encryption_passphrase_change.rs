use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::{
    ApplyEncryptionPassphraseChangePort, ApplyEncryptionPassphraseChangePortError,
};
use uc_core::crypto::domain::Passphrase;

use super::RuntimeSpaceAccessAdapter;
use crate::db::ports::DbExecutor;
use crate::security::{ActiveSpaceGenerationManifestStore, EncryptionPassphraseChangeJournal, Kek};
use crate::space::{prepare_registration, SqliteSpaceAdmissionCredentials};

pub struct EncryptionPassphraseChange<E> {
    access: Arc<RuntimeSpaceAccessAdapter>,
    credentials: Arc<SqliteSpaceAdmissionCredentials<E>>,
    manifests: Arc<ActiveSpaceGenerationManifestStore>,
    operation_lock: tokio::sync::Mutex<()>,
}

impl<E> EncryptionPassphraseChange<E> {
    pub fn new(
        access: Arc<RuntimeSpaceAccessAdapter>,
        credentials: Arc<SqliteSpaceAdmissionCredentials<E>>,
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
    ) -> Self {
        Self {
            access,
            credentials,
            manifests,
            operation_lock: tokio::sync::Mutex::new(()),
        }
    }
}

impl<E: DbExecutor> EncryptionPassphraseChange<E> {
    async fn complete(
        &self,
        journal: &EncryptionPassphraseChangeJournal,
    ) -> Result<(), ApplyEncryptionPassphraseChangePortError> {
        self.credentials
            .replace_registration_with_prepared(&journal.prepared_registration)
            .await
            .map_err(recovery)?;
        let kek = Kek::from_bytes(&journal.kek).map_err(recovery)?;
        self.access
            .install_encryption_passphrase_material(&journal.keyslot, &kek)
            .await
            .map_err(recovery)?;
        self.manifests
            .clear_encryption_passphrase_change_journal()
            .await
            .map_err(recovery)
    }

    async fn recover_locked(&self) -> Result<(), ApplyEncryptionPassphraseChangePortError> {
        let Some(journal) = self
            .manifests
            .load_encryption_passphrase_change_journal()
            .await
            .map_err(recovery)?
        else {
            return Ok(());
        };
        self.complete(&journal).await
    }

    pub async fn recover_pending(&self) -> Result<(), ApplyEncryptionPassphraseChangePortError> {
        let _guard = self.operation_lock.lock().await;
        self.recover_locked().await
    }
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> ApplyEncryptionPassphraseChangePort
    for EncryptionPassphraseChange<E>
{
    async fn ensure_encryption_passphrase_change_ready(
        &self,
    ) -> Result<(), ApplyEncryptionPassphraseChangePortError> {
        self.recover_pending().await
    }

    async fn apply_encryption_passphrase_change(
        &self,
        passphrase: &Passphrase,
    ) -> Result<(), ApplyEncryptionPassphraseChangePortError> {
        let _guard = self.operation_lock.lock().await;
        self.recover_locked().await?;
        let (keyslot, kek) = self
            .access
            .prepare_encryption_passphrase_material(passphrase)
            .await
            .map_err(unavailable)?;
        let prepared_registration = prepare_registration(passphrase).map_err(unavailable)?;
        let journal = EncryptionPassphraseChangeJournal {
            format_version: 1,
            keyslot,
            kek: kek.as_bytes().to_vec(),
            prepared_registration,
        };
        self.manifests
            .save_encryption_passphrase_change_journal(&journal)
            .await
            .map_err(unavailable)?;
        self.complete(&journal).await
    }
}

fn recovery(error: impl Into<anyhow::Error>) -> ApplyEncryptionPassphraseChangePortError {
    ApplyEncryptionPassphraseChangePortError::RecoveryRequired {
        source: error.into(),
    }
}

fn unavailable(error: impl Into<anyhow::Error>) -> ApplyEncryptionPassphraseChangePortError {
    ApplyEncryptionPassphraseChangePortError::Unavailable {
        source: error.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use uc_application::deps::{LoadedMembershipLedger, MembershipLedgerError};
    use uc_core::ids::SpaceId;
    use uc_core::membership::{
        ActiveSpaceGenerationManifestV2, AdmissionChannelPeerId, InvitationId,
        LegacyBootstrapRepositoryPort, RevocationRepositoryPort, SpaceAdmissionId,
        SpaceAdmissionProtocolVersion,
    };
    use uc_core::ports::space::{SpaceAccessError, SpaceAccessStore};
    use uc_core::ports::{SecureStorageError, SecureStoragePort};

    use super::*;
    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::pool::init_db_pool;
    use crate::db::repositories::DieselSpaceSecurityStore;
    use crate::fs::key_slot_store::JsonKeySlotStore;
    use crate::network::iroh::SpaceAdmissionChannelCredentialPort;
    use crate::security::{
        AdmissionKeyManager, DefaultCurrentProfile, ProfileContentKeyVault, SpaceAdmissionAuth,
        SpaceAdmissionAuthContext,
    };
    use crate::space::{InMemorySession, KeyMaterialStore, SqliteSpaceAdmissionState};

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

    #[async_trait]
    impl uc_application::deps::LoadMembershipLedgerPort for EmptyLedger {
        async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
            Ok(LoadedMembershipLedger::no_current_space())
        }
    }

    type Executor = Arc<DieselSqliteExecutor>;

    struct Fixture {
        _directory: tempfile::TempDir,
        root: std::path::PathBuf,
        secure_storage: Arc<dyn SecureStoragePort>,
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
        credentials: Arc<SqliteSpaceAdmissionCredentials<Executor>>,
        access: Arc<RuntimeSpaceAccessAdapter>,
        session: Arc<InMemorySession>,
        space_id: SpaceId,
    }

    impl Fixture {
        async fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().join("profile");
            std::fs::create_dir_all(&root).unwrap();
            let secure_storage: Arc<dyn SecureStoragePort> =
                Arc::new(MemorySecureStorage::default());
            let keys = Arc::new(AdmissionKeyManager::new(
                Arc::clone(&secure_storage),
                [0x31; 16],
            ));
            let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
                root.join("vault"),
                Arc::clone(&keys),
            ));
            manifests
                .promote(
                    &ActiveSpaceGenerationManifestV2::new(
                        "space-a".to_owned(),
                        [0x32; 16],
                        [0x33; 16],
                        [0x34; 16],
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            let executor = Arc::new(DieselSqliteExecutor::new(
                init_db_pool(root.join("control.sqlite").to_str().unwrap()).unwrap(),
            ));
            let session = Arc::new(InMemorySession::new());
            let security_store = Arc::new(DieselSpaceSecurityStore::new(
                Arc::clone(&executor),
                session.as_ref().clone(),
            ));
            let access = Arc::new(RuntimeSpaceAccessAdapter::new(
                Arc::new(KeyMaterialStore::new(
                    Arc::clone(&secure_storage),
                    Arc::new(JsonKeySlotStore::new(root.join("vault"))),
                )),
                Arc::new(DefaultCurrentProfile::new()),
                Arc::clone(&session),
                security_store.clone() as Arc<dyn RevocationRepositoryPort>,
                security_store as Arc<dyn LegacyBootstrapRepositoryPort>,
                Arc::new(ProfileContentKeyVault::new(
                    root.join("content-keys"),
                    Arc::clone(&secure_storage),
                    [0x35; 16],
                )),
            ));
            let admissions = Arc::new(SqliteSpaceAdmissionState::new(
                Arc::clone(&executor),
                Arc::clone(&keys),
                Arc::clone(&manifests),
                Arc::new(EmptyLedger),
            ));
            let credentials = Arc::new(SqliteSpaceAdmissionCredentials::new(
                executor,
                keys,
                Arc::clone(&manifests),
                Arc::new(EmptyLedger),
                admissions,
            ));
            let space_id = SpaceId::from_string("space-a".to_owned());
            access
                .initialize(&space_id, &Passphrase::new("old-passphrase"))
                .await
                .unwrap();
            credentials
                .ensure_registration(&Passphrase::new("old-passphrase"))
                .await
                .unwrap();
            Self {
                _directory: directory,
                root,
                secure_storage,
                manifests,
                credentials,
                access,
                session,
                space_id,
            }
        }

        fn restarted_access(&self) -> Arc<RuntimeSpaceAccessAdapter> {
            let session = Arc::new(InMemorySession::new());
            let executor = Arc::new(DieselSqliteExecutor::new(
                init_db_pool(self.root.join("control.sqlite").to_str().unwrap()).unwrap(),
            ));
            let security_store = Arc::new(DieselSpaceSecurityStore::new(
                executor,
                session.as_ref().clone(),
            ));
            Arc::new(RuntimeSpaceAccessAdapter::new(
                Arc::new(KeyMaterialStore::new(
                    Arc::clone(&self.secure_storage),
                    Arc::new(JsonKeySlotStore::new(self.root.join("vault"))),
                )),
                Arc::new(DefaultCurrentProfile::new()),
                session,
                security_store.clone() as Arc<dyn RevocationRepositoryPort>,
                security_store as Arc<dyn LegacyBootstrapRepositoryPort>,
                Arc::new(ProfileContentKeyVault::new(
                    self.root.join("content-keys"),
                    Arc::clone(&self.secure_storage),
                    [0x35; 16],
                )),
            ))
        }
    }

    async fn authenticates(
        credentials: &SqliteSpaceAdmissionCredentials<Executor>,
        passphrase: &Passphrase,
    ) -> bool {
        let invitation_id = InvitationId::from_bytes([0x41; 32]).unwrap();
        let admission_id = SpaceAdmissionId::from_bytes([0x42; 32]).unwrap();
        let material = credentials
            .resolve_initial(invitation_id, admission_id)
            .await
            .unwrap();
        let (server_setup, registration) = material.into_parts();
        let context = SpaceAdmissionAuthContext::new(
            SpaceAdmissionProtocolVersion::V1,
            admission_id,
            invitation_id,
            AdmissionChannelPeerId::from_bytes([0x43; 32]).unwrap(),
            AdmissionChannelPeerId::from_bytes([0x44; 32]).unwrap(),
        );
        let Ok((client, ke1)) = SpaceAdmissionAuth::start_client(passphrase, &context) else {
            return false;
        };
        let Ok((server, ke2)) =
            SpaceAdmissionAuth::start_server(&server_setup, &registration, &context, ke1)
        else {
            return false;
        };
        let Ok((client_credential, ke3)) = client.finish(&context, ke2) else {
            return false;
        };
        server
            .finish(&context, ke3)
            .is_ok_and(|server_credential| client_credential == server_credential)
    }

    #[tokio::test]
    async fn change_keeps_master_key_and_replaces_unlock_and_pairing_passphrase() {
        let fixture = Fixture::new().await;
        let original_master_key = fixture.session.get_master_key().unwrap();
        let change = EncryptionPassphraseChange::new(
            Arc::clone(&fixture.access),
            Arc::clone(&fixture.credentials),
            Arc::clone(&fixture.manifests),
        );

        change
            .apply_encryption_passphrase_change(&Passphrase::new("new-passphrase"))
            .await
            .unwrap();

        assert_eq!(
            fixture.session.get_master_key().unwrap(),
            original_master_key
        );
        assert!(!authenticates(&fixture.credentials, &Passphrase::new("old-passphrase")).await);
        assert!(authenticates(&fixture.credentials, &Passphrase::new("new-passphrase")).await);
        fixture.access.lock(&fixture.space_id).await.unwrap();
        assert!(matches!(
            fixture
                .access
                .unlock(&fixture.space_id, &Passphrase::new("old-passphrase"))
                .await,
            Err(SpaceAccessError::WrongPassphrase)
        ));
        fixture
            .access
            .unlock(&fixture.space_id, &Passphrase::new("new-passphrase"))
            .await
            .unwrap();
        let restarted = fixture.restarted_access();
        restarted
            .unlock(&fixture.space_id, &Passphrase::new("new-passphrase"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn protected_journal_completes_after_restart_without_plaintext_passphrase() {
        let fixture = Fixture::new().await;
        let (keyslot, kek) = fixture
            .access
            .prepare_encryption_passphrase_material(&Passphrase::new("recovered-passphrase"))
            .await
            .unwrap();
        let journal = EncryptionPassphraseChangeJournal {
            format_version: 1,
            keyslot,
            kek: kek.as_bytes().to_vec(),
            prepared_registration: prepare_registration(&Passphrase::new("recovered-passphrase"))
                .unwrap(),
        };
        let encoded = serde_json::to_vec(&journal).unwrap();
        let decoded: EncryptionPassphraseChangeJournal = serde_json::from_slice(&encoded).unwrap();
        assert!(decoded.validate());
        fixture
            .manifests
            .save_encryption_passphrase_change_journal(&journal)
            .await
            .unwrap();
        assert!(fixture
            .manifests
            .load_encryption_passphrase_change_journal()
            .await
            .unwrap()
            .is_some());
        let restarted = fixture.restarted_access();
        let recovery = EncryptionPassphraseChange::new(
            Arc::clone(&restarted),
            Arc::clone(&fixture.credentials),
            Arc::clone(&fixture.manifests),
        );

        recovery.recover_pending().await.unwrap();

        restarted
            .unlock(&fixture.space_id, &Passphrase::new("recovered-passphrase"))
            .await
            .unwrap();
        assert!(fixture
            .manifests
            .load_encryption_passphrase_change_journal()
            .await
            .unwrap()
            .is_none());
        assert!(
            authenticates(
                &fixture.credentials,
                &Passphrase::new("recovered-passphrase")
            )
            .await
        );
    }
}
