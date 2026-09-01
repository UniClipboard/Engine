mod material;
mod persistence;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use uc_application::deps::{
    AdmissionSpaceTransitionPreparationV2, CommitMembershipLedgerPort, LoadMembershipLedgerPort,
    MembershipLedgerMutation,
};
use uc_core::membership::RevocationRepositoryPort;
use uc_core::ports::atomic_publish::AtomicPublishPort;
use uc_core::ports::security::current_profile::CurrentProfilePort;

use self::material::PreparedAdmissionControl;
use self::persistence::{
    acquire_lease, compact_database, database_digest, open_existing_pool, open_pool,
    remove_directory_if_present, sync_directory, verify_sqlite, TargetSessionSubkeyDeriver,
};
use super::{ActiveRuntimeManifestV3, AdmissionKeyManager, ProfileRuntimeLayout};
use crate::db::executor::DieselSqliteExecutor;
use crate::db::repositories::{DieselSpaceSecurityStore, EncryptedRelationshipStore};
use crate::fs::FsAtomicPublisher;
use crate::space::{
    install_prepared_registration_for_control_generation,
    verify_prepared_registration_for_control_generation, DefaultSpaceAccessAdapter,
    InMemorySession, SqliteMembershipLedger,
};

/// 已完整写入、由 production repository 回读且原子发布的控制世代证明。
///
/// 构造器保持私有，后续 activation 只能消费本模块产生的证明，不能用一条
/// manifest 引用冒充已经准备好的 control generation。
#[derive(Clone, PartialEq, Eq)]
pub struct PreparedSpaceControlGeneration {
    manifest: ActiveRuntimeManifestV3,
    database_digest: [u8; 32],
}

impl PreparedSpaceControlGeneration {
    pub const fn manifest(&self) -> &ActiveRuntimeManifestV3 {
        &self.manifest
    }

    pub const fn database_digest(&self) -> &[u8; 32] {
        &self.database_digest
    }
}

impl std::fmt::Debug for PreparedSpaceControlGeneration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedSpaceControlGeneration")
            .field("identifiers", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SpaceControlGenerationError {
    #[error("space control generation preparation is busy")]
    Busy {
        #[source]
        source: anyhow::Error,
    },
    #[error("space control generation input is inconsistent")]
    Inconsistent {
        #[source]
        source: anyhow::Error,
    },
    #[error("space control generation storage is unavailable")]
    Storage {
        #[source]
        source: anyhow::Error,
    },
}

/// 一个目标 Space 控制面的完整 generation owner。
///
/// 调用方只提交已经验证的 admission material 与目标 manifest。成员账本、
/// 关系、OPAQUE credential、MLS/security state、SQLite 恢复、回读验证和
/// 原子发布都留在 implementation 内部。
pub struct SpaceControlGeneration {
    profile_root: PathBuf,
    space_access: Arc<DefaultSpaceAccessAdapter>,
    current_profile: Arc<dyn CurrentProfilePort>,
    admission_keys: Arc<AdmissionKeyManager>,
    prepare_lock: tokio::sync::Mutex<()>,
}

impl SpaceControlGeneration {
    pub fn new(
        profile_root: PathBuf,
        space_access: Arc<DefaultSpaceAccessAdapter>,
        current_profile: Arc<dyn CurrentProfilePort>,
        admission_keys: Arc<AdmissionKeyManager>,
    ) -> Self {
        Self {
            profile_root,
            space_access,
            current_profile,
            admission_keys,
            prepare_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub async fn prepare_admission(
        &self,
        input: &AdmissionSpaceTransitionPreparationV2,
        manifest: &ActiveRuntimeManifestV3,
    ) -> Result<PreparedSpaceControlGeneration, SpaceControlGenerationError> {
        let _guard = self.prepare_lock.lock().await;
        let prepared = PreparedAdmissionControl::try_from_input(input, manifest)?;
        let target_session = self
            .space_access
            .prepared_target_session(prepared.space_id(), &input.target_access_state)
            .await
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        target_session
            .install_space_material(prepared.security_material())
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;

        let layout = ProfileRuntimeLayout::v3(&self.profile_root, manifest);
        let final_database = layout.control_database();
        let final_directory = final_database
            .parent()
            .ok_or_else(|| storage(anyhow::anyhow!("control generation directory is missing")))?;
        let generation_parent = final_directory
            .parent()
            .ok_or_else(|| storage(anyhow::anyhow!("control generation parent is missing")))?;
        std::fs::create_dir_all(generation_parent)
            .map_err(|source| storage(anyhow::Error::new(source)))?;
        let _lease = acquire_lease(generation_parent)?;

        if final_database.is_file() {
            self.verify_database(final_database, &prepared, manifest, target_session.as_ref())
                .await?;
            return Ok(PreparedSpaceControlGeneration {
                manifest: manifest.clone(),
                database_digest: database_digest(final_database)?,
            });
        }
        if final_database.exists() {
            return Err(inconsistent(anyhow::anyhow!(
                "control generation destination is not a database"
            )));
        }

        let work_directory = generation_parent.join(format!(
            ".space-control-generation-{}.tmp",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&work_directory)
            .map_err(|source| storage(anyhow::Error::new(source)))?;
        let work_database = work_directory.join("control.sqlite");

        let result = async {
            self.build_database(&work_database, &prepared, manifest, target_session.as_ref())
                .await?;
            compact_database(&work_database)?;
            self.verify_database(&work_database, &prepared, manifest, target_session.as_ref())
                .await?;
            compact_database(&work_database)?;
            let staged_digest = database_digest(&work_database)?;
            FsAtomicPublisher
                .publish_into_free_name(&work_directory, final_directory)
                .await
                .map_err(|source| storage(anyhow::Error::new(source)))?;
            sync_directory(generation_parent)?;
            Ok(PreparedSpaceControlGeneration {
                manifest: manifest.clone(),
                database_digest: staged_digest,
            })
        }
        .await;
        if result.is_err() {
            let _ = remove_directory_if_present(&work_directory);
        }
        result
    }

    async fn build_database(
        &self,
        database: &Path,
        prepared: &PreparedAdmissionControl,
        manifest: &ActiveRuntimeManifestV3,
        target_session: &InMemorySession,
    ) -> Result<(), SpaceControlGenerationError> {
        let pool = open_pool(database)?;
        let executor = Arc::new(DieselSqliteExecutor::new(pool.clone()));
        let security = DieselSpaceSecurityStore::new(Arc::clone(&executor), target_session.clone());
        security
            .save_space_material(prepared.security_material())
            .await
            .map_err(|source| storage(anyhow::Error::new(source)))?;

        let relationships = EncryptedRelationshipStore::new(
            Arc::clone(&executor),
            Arc::new(TargetSessionSubkeyDeriver::new(target_session.clone())),
            Arc::clone(&self.current_profile),
        );
        for member in prepared.members() {
            relationships
                .save_member(member)
                .await
                .map_err(|source| storage(anyhow::Error::new(source)))?;
        }
        for peer in prepared.trusted_peers() {
            relationships
                .save_trusted_peer(peer)
                .await
                .map_err(|source| storage(anyhow::Error::new(source)))?;
        }
        for address in prepared.peer_addresses() {
            relationships
                .save_peer_address(address)
                .await
                .map_err(|source| storage(anyhow::Error::new(source)))?;
        }

        install_prepared_registration_for_control_generation(
            &pool,
            self.admission_keys.as_ref(),
            manifest,
            prepared.credentials(),
        )
        .map_err(storage)?;
        let ledger = SqliteMembershipLedger::new(executor, Arc::clone(&self.admission_keys));
        let current = ledger
            .load()
            .await
            .map_err(|source| storage(anyhow::Error::new(source)))?;
        ledger
            .compare_and_commit(MembershipLedgerMutation {
                expected_revision: current.revision,
                expected_history_digest: current.membership_history.as_deref().map(|history| {
                    use sha2::Digest as _;
                    sha2::Sha256::digest(history).into()
                }),
                replacement: prepared.ledger(current.revision)?,
            })
            .await
            .map_err(|source| storage(anyhow::Error::new(source)))?;
        Ok(())
    }

    async fn verify_database(
        &self,
        database: &Path,
        prepared: &PreparedAdmissionControl,
        manifest: &ActiveRuntimeManifestV3,
        target_session: &InMemorySession,
    ) -> Result<(), SpaceControlGenerationError> {
        verify_sqlite(database)?;
        let pool = open_existing_pool(database)?;
        let executor = Arc::new(DieselSqliteExecutor::new(pool));
        let security = DieselSpaceSecurityStore::new(Arc::clone(&executor), target_session.clone());
        let reopened = security
            .load_space_material(prepared.space_id())
            .await
            .map_err(|source| storage(anyhow::Error::new(source)))?
            .ok_or_else(|| inconsistent(anyhow::anyhow!("control security material is missing")))?;
        if &reopened != prepared.security_material() {
            return Err(inconsistent(anyhow::anyhow!(
                "control security material does not match"
            )));
        }

        let relationships = EncryptedRelationshipStore::new(
            Arc::clone(&executor),
            Arc::new(TargetSessionSubkeyDeriver::new(target_session.clone())),
            Arc::clone(&self.current_profile),
        );
        let mut members = relationships
            .list_members()
            .await
            .map_err(|source| storage(anyhow::Error::new(source)))?;
        let mut trusted_peers = relationships
            .list_trusted_peers()
            .await
            .map_err(|source| storage(anyhow::Error::new(source)))?;
        let mut peer_addresses = relationships
            .list_peer_addresses()
            .await
            .map_err(|source| storage(anyhow::Error::new(source)))?;
        members.sort_by(|left, right| left.device_id.cmp(&right.device_id));
        trusted_peers.sort_by(|left, right| left.peer_device_id.cmp(&right.peer_device_id));
        peer_addresses.sort_by(|left, right| left.device_id.cmp(&right.device_id));
        if members != prepared.members()
            || trusted_peers != prepared.trusted_peers()
            || peer_addresses != prepared.peer_addresses()
        {
            return Err(inconsistent(anyhow::anyhow!(
                "control relationships do not match"
            )));
        }

        let ledger = SqliteMembershipLedger::new(executor, Arc::clone(&self.admission_keys));
        let actual_ledger = ledger
            .load()
            .await
            .map_err(|source| storage(anyhow::Error::new(source)))?;
        if actual_ledger != prepared.ledger(0)? {
            return Err(inconsistent(anyhow::anyhow!(
                "control membership ledger does not match"
            )));
        }
        verify_prepared_registration_for_control_generation(
            database,
            self.admission_keys.as_ref(),
            manifest,
            prepared.credentials(),
        )
        .map_err(inconsistent)
    }
}

pub(super) fn inconsistent(source: anyhow::Error) -> SpaceControlGenerationError {
    SpaceControlGenerationError::Inconsistent { source }
}

pub(super) fn storage(source: anyhow::Error) -> SpaceControlGenerationError {
    SpaceControlGenerationError::Storage { source }
}

#[cfg(test)]
mod tests;
