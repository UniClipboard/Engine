mod material;
mod persistence;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{TimeZone as _, Utc};
use sha2::Digest as _;
use uc_application::deps::{
    AdmissionSpaceTransitionPreparationV2, AdvanceMembershipBranchTransitionInput,
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, MembershipLedgerMutation,
    PeerReconciliationRecord,
};
use uc_core::membership::{
    MembershipBranchTransitionPhaseV1, RelationshipStateResetPort, RevocationRepositoryPort,
};
use uc_core::ports::atomic_publish::AtomicPublishPort;
use uc_core::ports::security::current_profile::CurrentProfilePort;
use uc_core::ports::PeerAddressRecord;
use uc_core::{MemberSyncPreferences, SpaceMember, TrustedPeer};

use self::material::PreparedAdmissionControl;
use self::persistence::{
    acquire_lease, checkpoint_database, compact_database, database_digest, open_existing_pool,
    open_pool, remove_directory_if_present, sync_directory, verify_sqlite, write_new_database,
    TargetSessionSubkeyDeriver,
};
use super::{ActiveRuntimeManifestV3, AdmissionKeyManager, ProfileRuntimeLayout};
use crate::db::executor::DieselSqliteExecutor;
use crate::db::repositories::{DieselSpaceSecurityStore, EncryptedRelationshipStore};
use crate::fs::FsAtomicPublisher;
use crate::space::{
    install_prepared_registration_for_control_generation,
    rebind_registration_to_control_generation, verify_prepared_registration_for_control_generation,
    DefaultSpaceAccessAdapter, InMemorySession, SqliteMembershipLedger,
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

        self.prepare_with_session(&prepared, manifest, target_session.as_ref())
            .await
    }

    /// SameSpace admission 保留活动 MasterKey/keyslot，只准备新的完整控制世代。
    pub async fn prepare_same_space_admission(
        &self,
        input: &AdmissionSpaceTransitionPreparationV2,
        source: &ActiveRuntimeManifestV3,
        target: &ActiveRuntimeManifestV3,
    ) -> Result<PreparedSpaceControlGeneration, SpaceControlGenerationError> {
        let _guard = self.prepare_lock.lock().await;
        if source.layout().space_id() != target.layout().space_id()
            || source.layout().profile_data_generation()
                != target.layout().profile_data_generation()
            || source.keyslot_generation() != target.keyslot_generation()
            || source.layout().space_control_generation()
                == target.layout().space_control_generation()
        {
            return Err(inconsistent(anyhow::anyhow!(
                "same-space control generation input is inconsistent"
            )));
        }
        let prepared = PreparedAdmissionControl::try_from_input(input, target)?;
        let target_session = self
            .space_access
            .retained_control_session(prepared.space_id())
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        target_session
            .install_space_material(prepared.security_material())
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;

        self.prepare_with_session(&prepared, target, target_session.as_ref())
            .await
    }

    /// 为 Device Reset 建立独立的可写 control snapshot。
    ///
    /// snapshot 只包含来源 control SQLite；profile database/blob 不在本模块依赖
    /// 图中。目标在原子发布前完成 credential scope 重绑与 SQLite 校验，随后由
    /// Application 通过现有 repository 正常重建成员、关系和安全状态。
    pub async fn prepare_device_reset_snapshot(
        &self,
        source: &ActiveRuntimeManifestV3,
        target: &ActiveRuntimeManifestV3,
        source_pool: &crate::db::pool::DbPool,
    ) -> Result<PreparedSpaceControlGeneration, SpaceControlGenerationError> {
        self.prepare_retained_control_snapshot(source, target, source_pool, true)
            .await
    }

    /// 为 membership branch 建立同 Space、同 keyslot 的独立 control seed。
    pub async fn prepare_membership_branch_snapshot(
        &self,
        source: &ActiveRuntimeManifestV3,
        target: &ActiveRuntimeManifestV3,
        source_pool: &crate::db::pool::DbPool,
    ) -> Result<PreparedSpaceControlGeneration, SpaceControlGenerationError> {
        self.prepare_retained_control_snapshot(source, target, source_pool, false)
            .await
    }

    async fn prepare_retained_control_snapshot(
        &self,
        source: &ActiveRuntimeManifestV3,
        target: &ActiveRuntimeManifestV3,
        source_pool: &crate::db::pool::DbPool,
        space_changes: bool,
    ) -> Result<PreparedSpaceControlGeneration, SpaceControlGenerationError> {
        let _guard = self.prepare_lock.lock().await;
        if (source.layout().space_id() != target.layout().space_id()) != space_changes
            || source.keyslot_generation() != target.keyslot_generation()
            || source.layout().profile_data_generation()
                != target.layout().profile_data_generation()
            || source.layout().space_control_generation()
                == target.layout().space_control_generation()
        {
            return Err(inconsistent(anyhow::anyhow!(
                "retained control snapshot input is inconsistent"
            )));
        }
        self.space_access
            .retained_control_session(source.layout().space_id())
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;

        let layout = ProfileRuntimeLayout::v3(&self.profile_root, target);
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
            rebind_registration_to_control_generation(
                final_database,
                self.admission_keys.as_ref(),
                source,
                target,
            )
            .map_err(inconsistent)?;
            compact_database(final_database)?;
            verify_sqlite(final_database)?;
            return Ok(PreparedSpaceControlGeneration {
                manifest: target.clone(),
                database_digest: database_digest(final_database)?,
            });
        }
        if final_database.exists() {
            return Err(inconsistent(anyhow::anyhow!(
                "retained control destination is not a database"
            )));
        }

        let work_directory =
            generation_parent.join(format!(".retained-control-{}.tmp", uuid::Uuid::new_v4()));
        std::fs::create_dir(&work_directory)
            .map_err(|source| storage(anyhow::Error::new(source)))?;
        let work_database = work_directory.join("control.sqlite");
        let scratch = work_directory.join("source.snapshot.tmp");
        let result = async {
            let snapshot =
                crate::config_migration::db_snapshot::snapshot_to_bytes(source_pool, &scratch)
                    .map_err(|source| storage(anyhow::Error::new(source)))?;
            write_new_database(&work_database, &snapshot)?;
            rebind_registration_to_control_generation(
                &work_database,
                self.admission_keys.as_ref(),
                source,
                target,
            )
            .map_err(inconsistent)?;
            compact_database(&work_database)?;
            verify_sqlite(&work_database)?;
            let digest = database_digest(&work_database)?;
            FsAtomicPublisher
                .publish_into_free_name(&work_directory, final_directory)
                .await
                .map_err(|source| storage(anyhow::Error::new(source)))?;
            sync_directory(generation_parent)?;
            Ok(PreparedSpaceControlGeneration {
                manifest: target.clone(),
                database_digest: digest,
            })
        }
        .await;
        if result.is_err() {
            let _ = remove_directory_if_present(&work_directory);
        }
        result
    }

    async fn prepare_with_session(
        &self,
        prepared: &PreparedAdmissionControl,
        manifest: &ActiveRuntimeManifestV3,
        target_session: &InMemorySession,
    ) -> Result<PreparedSpaceControlGeneration, SpaceControlGenerationError> {
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
            self.verify_database(final_database, prepared, manifest, target_session)
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
            self.build_database(&work_database, prepared, manifest, target_session)
                .await?;
            compact_database(&work_database)?;
            self.verify_database(&work_database, prepared, manifest, target_session)
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

    /// 从已认证 transition state 中的 manifest 与介质摘要恢复 prepared proof。
    ///
    /// 初次构造已经通过完整 production repository 回读；重启恢复不再需要把
    /// admission material 复制进 transition schema，而是以认证摘要重新验证同一
    /// 不可变 SQLite generation。
    pub async fn reopen_prepared(
        &self,
        manifest: &ActiveRuntimeManifestV3,
        expected_database_digest: &[u8; 32],
    ) -> Result<PreparedSpaceControlGeneration, SpaceControlGenerationError> {
        if expected_database_digest == &[0; 32] {
            return Err(inconsistent(anyhow::anyhow!(
                "prepared control database digest is invalid"
            )));
        }
        let layout = ProfileRuntimeLayout::v3(&self.profile_root, manifest);
        let database = layout.control_database();
        if !database.is_file() {
            return Err(inconsistent(anyhow::anyhow!(
                "prepared control database is missing"
            )));
        }
        verify_sqlite(database)?;
        let actual = database_digest(database)?;
        if &actual != expected_database_digest {
            return Err(inconsistent(anyhow::anyhow!(
                "prepared control database digest does not match"
            )));
        }
        Ok(PreparedSpaceControlGeneration {
            manifest: manifest.clone(),
            database_digest: actual,
        })
    }

    /// 完成 Reset 已由 Application 修改的可写目标，并形成 promotion proof。
    ///
    /// Reset target 在 `stage` 后不是 admission-style 不可变候选：成员、关系和
    /// MLS/security 都由现有 rebuild use case 写入。本操作在这些写入结束后独占
    /// checkpoint、凭据 scope 重绑、正式 security repository 回读和介质摘要，
    /// 调用方不能自己拼装 SQLite 验证步骤。
    pub async fn finalize_device_reset_target(
        &self,
        source: &ActiveRuntimeManifestV3,
        target: &ActiveRuntimeManifestV3,
        active_pool: &crate::db::pool::DbPool,
    ) -> Result<PreparedSpaceControlGeneration, SpaceControlGenerationError> {
        let _guard = self.prepare_lock.lock().await;
        if source.layout().space_id() == target.layout().space_id()
            || source.keyslot_generation() != target.keyslot_generation()
            || source.layout().profile_data_generation()
                != target.layout().profile_data_generation()
            || source.layout().space_control_generation()
                == target.layout().space_control_generation()
        {
            return Err(inconsistent(anyhow::anyhow!(
                "device reset control generation input is inconsistent"
            )));
        }
        let layout = ProfileRuntimeLayout::v3(&self.profile_root, target);
        let database = layout.control_database();
        if !database.is_file() {
            return Err(inconsistent(anyhow::anyhow!(
                "device reset control database is missing"
            )));
        }
        rebind_registration_to_control_generation(
            database,
            self.admission_keys.as_ref(),
            source,
            target,
        )
        .map_err(inconsistent)?;
        checkpoint_database(active_pool, database)?;
        verify_sqlite(database)?;

        let target_session = self
            .space_access
            .retained_control_session(target.layout().space_id())
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        let executor = Arc::new(DieselSqliteExecutor::new(active_pool.clone()));
        let security = DieselSpaceSecurityStore::new(executor, target_session.as_ref().clone());
        let material = security
            .load_space_material(target.layout().space_id())
            .await
            .map_err(|source| storage(anyhow::Error::new(source)))?
            .ok_or_else(|| {
                inconsistent(anyhow::anyhow!(
                    "device reset target security material is missing"
                ))
            })?;
        if material.state().space_id() != target.layout().space_id() {
            return Err(inconsistent(anyhow::anyhow!(
                "device reset target security material does not match"
            )));
        }
        checkpoint_database(active_pool, database)?;
        verify_sqlite(database)?;
        Ok(PreparedSpaceControlGeneration {
            manifest: target.clone(),
            database_digest: database_digest(database)?,
        })
    }

    /// 验证 branch recovery package 能在当前活动 Space 中恢复目标 material。
    pub fn verify_membership_branch_recovery(
        &self,
        input: &AdvanceMembershipBranchTransitionInput,
    ) -> Result<(), SpaceControlGenerationError> {
        if !input.transition.validate()
            || input.recovery_package.conflict_id() != input.transition.conflict_id()
            || input.recovery_package.target_branch_id() != input.transition.target_branch_id()
        {
            return Err(inconsistent(anyhow::anyhow!(
                "membership branch recovery binding is invalid"
            )));
        }
        self.space_access
            .prepare_recovered_membership_branch_material(
                &input.recipient_staged_mls_state,
                input.recovery_package.sealed_mls_recovery_material(),
                input.recovery_package.encrypted_content_key_catalog(),
            )
            .map(|_| ())
            .map_err(|source| inconsistent(anyhow::Error::new(source)))
    }

    /// 把已验证的 branch material、目标成员关系与 target ledger 一次写入
    /// 不可见的 control generation；profile payload 不参与。
    pub async fn stage_membership_branch_target(
        &self,
        input: &AdvanceMembershipBranchTransitionInput,
        source: &ActiveRuntimeManifestV3,
        target: &ActiveRuntimeManifestV3,
    ) -> Result<(), SpaceControlGenerationError> {
        let _guard = self.prepare_lock.lock().await;
        self.validate_membership_branch_binding(input, source, target, false)?;
        let layout = ProfileRuntimeLayout::v3(&self.profile_root, target);
        let database = layout.control_database();
        if !database.is_file() {
            return Err(inconsistent(anyhow::anyhow!(
                "membership branch target database is missing"
            )));
        }
        let target_session = self
            .space_access
            .retained_control_session(source.layout().space_id())
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        let material = self
            .space_access
            .prepare_recovered_membership_branch_material(
                &input.recipient_staged_mls_state,
                input.recovery_package.sealed_mls_recovery_material(),
                input.recovery_package.encrypted_content_key_catalog(),
            )
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        target_session
            .install_space_material(&material)
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        let (members, trusted_peers, peer_addresses) =
            branch_relationships(input, target.layout().space_id())?;
        let encoded_history = input
            .target_history
            .encode_persisted_v2()
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        let local_member = input.recovery_package.recipient_member();
        let local_facts = input
            .target_history
            .admission_facts_for(local_member)
            .ok_or_else(|| inconsistent(anyhow::anyhow!("branch recipient facts are missing")))?;

        {
            let pool = open_existing_pool(database)?;
            let executor = Arc::new(DieselSqliteExecutor::new(pool));
            let security = DieselSpaceSecurityStore::new(
                Arc::clone(&executor),
                target_session.as_ref().clone(),
            );
            security
                .save_space_material(&material)
                .await
                .map_err(|source| storage(anyhow::Error::new(source)))?;

            let relationships = EncryptedRelationshipStore::new(
                Arc::clone(&executor),
                Arc::new(TargetSessionSubkeyDeriver::new(
                    target_session.as_ref().clone(),
                )),
                Arc::clone(&self.current_profile),
            );
            relationships
                .clear_all_relationships()
                .await
                .map_err(|source| storage(anyhow::Error::new(source)))?;
            for member in &members {
                relationships
                    .save_member(member)
                    .await
                    .map_err(|source| storage(anyhow::Error::new(source)))?;
            }
            for peer in &trusted_peers {
                relationships
                    .save_trusted_peer(peer)
                    .await
                    .map_err(|source| storage(anyhow::Error::new(source)))?;
            }
            for address in &peer_addresses {
                relationships
                    .save_peer_address(address)
                    .await
                    .map_err(|source| storage(anyhow::Error::new(source)))?;
            }

            let ledger = SqliteMembershipLedger::new(executor, Arc::clone(&self.admission_keys));
            let current = ledger
                .load()
                .await
                .map_err(|source| storage(anyhow::Error::new(source)))?;
            let mut replacement = current.clone();
            let stored = replacement
                .membership_branch_transitions
                .get_mut(input.transition.transition_id())
                .ok_or_else(|| {
                    inconsistent(anyhow::anyhow!("branch target checkpoint is missing"))
                })?;
            advance_branch_checkpoint_to_staged(stored, &input.transition)?;
            replacement.membership_history = Some(encoded_history);
            replacement.local_device_id = Some(local_facts.device_id.clone());
            replacement.local_member_instance = Some(local_member);
            replacement.local_join_active = true;
            replacement.peer_reconciliation = input
                .target_history
                .active_members()
                .into_iter()
                .filter(|member| member != &local_member)
                .map(|member| {
                    let facts = input
                        .target_history
                        .admission_facts_for(member)
                        .ok_or_else(|| {
                            inconsistent(anyhow::anyhow!("branch target member facts are missing"))
                        })?;
                    Ok((
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
                    ))
                })
                .collect::<Result<_, SpaceControlGenerationError>>()?;
            replacement.revision = current
                .revision
                .checked_add(1)
                .ok_or_else(|| inconsistent(anyhow::anyhow!("branch ledger revision overflow")))?;
            ledger
                .compare_and_commit(MembershipLedgerMutation {
                    expected_revision: current.revision,
                    expected_history_digest: current
                        .membership_history
                        .as_deref()
                        .map(|bytes| sha2::Sha256::digest(bytes).into()),
                    replacement,
                })
                .await
                .map_err(|source| storage(anyhow::Error::new(source)))?;
        }
        verify_sqlite(database)
    }

    /// 对完整 branch target 进行 production 回读并形成 promotion proof。
    pub async fn finalize_membership_branch_target(
        &self,
        input: &AdvanceMembershipBranchTransitionInput,
        source: &ActiveRuntimeManifestV3,
        target: &ActiveRuntimeManifestV3,
    ) -> Result<PreparedSpaceControlGeneration, SpaceControlGenerationError> {
        let _guard = self.prepare_lock.lock().await;
        self.validate_membership_branch_binding(input, source, target, true)?;
        let layout = ProfileRuntimeLayout::v3(&self.profile_root, target);
        let database = layout.control_database();
        verify_sqlite(database)?;
        let target_session = self
            .space_access
            .retained_control_session(source.layout().space_id())
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        let expected_material = self
            .space_access
            .prepare_recovered_membership_branch_material(
                &input.recipient_staged_mls_state,
                input.recovery_package.sealed_mls_recovery_material(),
                input.recovery_package.encrypted_content_key_catalog(),
            )
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        target_session
            .install_space_material(&expected_material)
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        let (mut expected_members, mut expected_peers, mut expected_addresses) =
            branch_relationships(input, target.layout().space_id())?;
        {
            let pool = open_existing_pool(database)?;
            let executor = Arc::new(DieselSqliteExecutor::new(pool));
            let security = DieselSpaceSecurityStore::new(
                Arc::clone(&executor),
                target_session.as_ref().clone(),
            );
            let actual_material = security
                .load_space_material(target.layout().space_id())
                .await
                .map_err(|source| storage(anyhow::Error::new(source)))?
                .ok_or_else(|| {
                    inconsistent(anyhow::anyhow!(
                        "branch target security material is missing"
                    ))
                })?;
            if actual_material != expected_material {
                return Err(inconsistent(anyhow::anyhow!(
                    "branch target security material does not match"
                )));
            }
            let relationships = EncryptedRelationshipStore::new(
                Arc::clone(&executor),
                Arc::new(TargetSessionSubkeyDeriver::new(
                    target_session.as_ref().clone(),
                )),
                Arc::clone(&self.current_profile),
            );
            let mut actual_members = relationships
                .list_members()
                .await
                .map_err(|source| storage(anyhow::Error::new(source)))?;
            let mut actual_peers = relationships
                .list_trusted_peers()
                .await
                .map_err(|source| storage(anyhow::Error::new(source)))?;
            let mut actual_addresses = relationships
                .list_peer_addresses()
                .await
                .map_err(|source| storage(anyhow::Error::new(source)))?;
            actual_members.sort_by(|left, right| left.device_id.cmp(&right.device_id));
            actual_peers.sort_by(|left, right| left.peer_device_id.cmp(&right.peer_device_id));
            actual_addresses.sort_by(|left, right| left.device_id.cmp(&right.device_id));
            expected_members.sort_by(|left, right| left.device_id.cmp(&right.device_id));
            expected_peers.sort_by(|left, right| left.peer_device_id.cmp(&right.peer_device_id));
            expected_addresses.sort_by(|left, right| left.device_id.cmp(&right.device_id));
            if actual_members != expected_members
                || actual_peers != expected_peers
                || actual_addresses != expected_addresses
            {
                return Err(inconsistent(anyhow::anyhow!(
                    "branch target relationships do not match"
                )));
            }
            let ledger = SqliteMembershipLedger::new(executor, Arc::clone(&self.admission_keys));
            let actual = ledger
                .load()
                .await
                .map_err(|source| storage(anyhow::Error::new(source)))?;
            let expected_history = input
                .target_history
                .encode_persisted_v2()
                .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
            if actual.membership_history.as_deref() != Some(expected_history.as_slice())
                || actual.local_member_instance != Some(input.recovery_package.recipient_member())
                || actual
                    .membership_branch_transitions
                    .get(input.transition.transition_id())
                    != Some(&input.transition)
            {
                return Err(inconsistent(anyhow::anyhow!(
                    "branch target ledger does not match"
                )));
            }
        }
        compact_database(database)?;
        verify_sqlite(database)?;
        Ok(PreparedSpaceControlGeneration {
            manifest: target.clone(),
            database_digest: database_digest(database)?,
        })
    }

    fn validate_membership_branch_binding(
        &self,
        input: &AdvanceMembershipBranchTransitionInput,
        source: &ActiveRuntimeManifestV3,
        target: &ActiveRuntimeManifestV3,
        target_is_staged: bool,
    ) -> Result<(), SpaceControlGenerationError> {
        let expected_phase = if target_is_staged {
            MembershipBranchTransitionPhaseV1::TargetStaged
        } else {
            MembershipBranchTransitionPhaseV1::TargetVerified
        };
        if !input.transition.validate()
            || input.transition.phase() != expected_phase
            || input.transition.source_generation() != source.layout().space_control_generation()
            || input.transition.target_generation() != target.layout().space_control_generation()
            || source.layout().space_id() != target.layout().space_id()
            || source.keyslot_generation() != target.keyslot_generation()
            || source.layout().profile_data_generation()
                != target.layout().profile_data_generation()
            || input.recovery_package.conflict_id() != input.transition.conflict_id()
            || input.recovery_package.target_branch_id() != input.transition.target_branch_id()
            || input.target_history.lineage_id() != target.layout().space_id().as_ref()
        {
            return Err(inconsistent(anyhow::anyhow!(
                "membership branch control binding is invalid"
            )));
        }
        Ok(())
    }

    /// 删除仍未被 active manifest 引用的 prepared control generation。
    /// 调用方必须先在 transition activation 的同一租约下证明它未生效。
    pub(crate) fn discard_prepared(
        &self,
        prepared: &PreparedSpaceControlGeneration,
    ) -> Result<(), SpaceControlGenerationError> {
        let layout = ProfileRuntimeLayout::v3(&self.profile_root, prepared.manifest());
        let database = layout.control_database();
        if !database.exists() {
            return Ok(());
        }
        verify_sqlite(database)?;
        if database_digest(database)? != *prepared.database_digest() {
            return Err(inconsistent(anyhow::anyhow!(
                "discarded control database digest does not match"
            )));
        }
        let directory = database
            .parent()
            .ok_or_else(|| storage(anyhow::anyhow!("control generation directory is missing")))?;
        let parent = directory
            .parent()
            .ok_or_else(|| storage(anyhow::anyhow!("control generation parent is missing")))?;
        let _lease = acquire_lease(parent)?;
        remove_directory_if_present(directory)?;
        sync_directory(parent)
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

fn advance_branch_checkpoint_to_staged(
    stored: &mut uc_core::membership::MembershipBranchTransitionV1,
    expected_verified: &uc_core::membership::MembershipBranchTransitionV1,
) -> Result<(), SpaceControlGenerationError> {
    let expected_staged = expected_verified
        .advance(MembershipBranchTransitionPhaseV1::TargetStaged)
        .ok_or_else(|| inconsistent(anyhow::anyhow!("branch target phase is invalid")))?;
    while stored.phase() != MembershipBranchTransitionPhaseV1::TargetStaged {
        let next = match stored.phase() {
            MembershipBranchTransitionPhaseV1::Prepared => {
                MembershipBranchTransitionPhaseV1::SourceBackedUp
            }
            MembershipBranchTransitionPhaseV1::SourceBackedUp => {
                MembershipBranchTransitionPhaseV1::TargetVerified
            }
            MembershipBranchTransitionPhaseV1::TargetVerified => {
                MembershipBranchTransitionPhaseV1::TargetStaged
            }
            MembershipBranchTransitionPhaseV1::TargetStaged
            | MembershipBranchTransitionPhaseV1::Promoted
            | MembershipBranchTransitionPhaseV1::RuntimeRestored
            | MembershipBranchTransitionPhaseV1::Completed => {
                return Err(inconsistent(anyhow::anyhow!(
                    "branch target checkpoint cannot be staged"
                )))
            }
        };
        *stored = stored
            .advance(next)
            .ok_or_else(|| inconsistent(anyhow::anyhow!("branch target checkpoint is invalid")))?;
    }
    if stored != &expected_staged {
        return Err(inconsistent(anyhow::anyhow!(
            "branch target checkpoint does not match"
        )));
    }
    Ok(())
}

fn branch_relationships(
    input: &AdvanceMembershipBranchTransitionInput,
    space_id: &uc_core::ids::SpaceId,
) -> Result<(Vec<SpaceMember>, Vec<TrustedPeer>, Vec<PeerAddressRecord>), SpaceControlGenerationError>
{
    if input.target_history.lineage_id() != space_id.as_ref() {
        return Err(inconsistent(anyhow::anyhow!(
            "branch target lineage does not match"
        )));
    }
    let local_member = input.recovery_package.recipient_member();
    let local_facts = input
        .target_history
        .admission_facts_for(local_member)
        .ok_or_else(|| inconsistent(anyhow::anyhow!("branch recipient facts are missing")))?;
    let timestamp = Utc
        .timestamp_millis_opt(0)
        .single()
        .ok_or_else(|| inconsistent(anyhow::anyhow!("branch relationship timestamp is invalid")))?;
    let mut members = Vec::new();
    let mut trusted_peers = Vec::new();
    let mut peer_addresses = Vec::new();
    for member in input.target_history.active_members() {
        let facts = input
            .target_history
            .admission_facts_for(member)
            .ok_or_else(|| inconsistent(anyhow::anyhow!("branch member facts are missing")))?;
        members.push(SpaceMember {
            device_id: facts.device_id.clone(),
            device_name: facts.device_name.clone(),
            identity_fingerprint: facts.identity_fingerprint.clone(),
            joined_at: timestamp,
            sync_preferences: MemberSyncPreferences::default(),
        });
        if member != local_member {
            trusted_peers.push(TrustedPeer {
                local_device_id: local_facts.device_id.clone(),
                peer_device_id: facts.device_id.clone(),
                peer_fingerprint: facts.identity_fingerprint.clone(),
                trusted_at: timestamp,
            });
            peer_addresses.push(PeerAddressRecord {
                device_id: facts.device_id.clone(),
                addr_blob: facts.transport_address_blob.clone(),
                observed_at: timestamp,
            });
        }
    }
    Ok((members, trusted_peers, peer_addresses))
}

pub(super) fn inconsistent(source: anyhow::Error) -> SpaceControlGenerationError {
    SpaceControlGenerationError::Inconsistent { source }
}

pub(super) fn storage(source: anyhow::Error) -> SpaceControlGenerationError {
    SpaceControlGenerationError::Storage { source }
}

#[cfg(test)]
mod tests;
