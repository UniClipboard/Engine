use crate::db::{ports::DbExecutor, repositories::EncryptedRelationshipStore};
use crate::space::SqliteMembershipLedger;
use async_trait::async_trait;
use std::sync::Arc;
use uc_application::deps::{
    ApplyMembershipProjectionError, ApplyMembershipProjectionPort, MembershipProjectionPlan,
};

/// 原子落实 Application 已验证的资料计划，不从旧移除记录推断删除资格。
pub struct MembershipProjectionAdapter<E> {
    ledger: Arc<SqliteMembershipLedger<E>>,
    relationships: Arc<EncryptedRelationshipStore<E>>,
}

impl<E> MembershipProjectionAdapter<E> {
    pub fn new(
        ledger: Arc<SqliteMembershipLedger<E>>,
        relationships: Arc<EncryptedRelationshipStore<E>>,
    ) -> Self {
        Self {
            ledger,
            relationships,
        }
    }
}

#[async_trait]
impl<E: DbExecutor> ApplyMembershipProjectionPort for MembershipProjectionAdapter<E> {
    async fn apply_membership_projection(
        &self,
        plan: MembershipProjectionPlan,
    ) -> Result<(), ApplyMembershipProjectionError> {
        self.ledger
            .apply_projection(&self.relationships, &plan)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        pool::init_db_pool,
        repositories::{test_relationship_store, DieselSpaceMemberRepository},
    };
    use chrono::Utc;
    use sha2::Digest;
    use uc_application::deps::{
        CommitMembershipLedgerPort, LoadedMembershipLedger, MembershipEffectKind,
        MembershipEffectPhase, MembershipLedgerMutation, MembershipProjectionPlan,
        PendingMembershipEffect,
    };
    use uc_core::membership::{
        AdmissionChangeFacts, MembershipActivationBaselineV2, MembershipCredential,
        MembershipEventId, VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
    };
    use uc_core::MemberRepositoryPort;
    use uc_core::{DeviceId, MemberSyncPreferences, SpaceMember};

    #[derive(Default)]
    struct Keys(std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>);
    impl uc_core::ports::SecureStoragePort for Keys {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, uc_core::ports::SecureStorageError> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }
        fn set(&self, key: &str, value: &[u8]) -> Result<(), uc_core::ports::SecureStorageError> {
            self.0
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.to_vec());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), uc_core::ports::SecureStorageError> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }

    #[tokio::test]
    async fn selected_branch_keeps_active_members_despite_old_completed_removal() {
        let directory = tempfile::tempdir().unwrap();
        let pool = init_db_pool(directory.path().join("control.sqlite").to_str().unwrap()).unwrap();
        let store = test_relationship_store(pool.clone());
        let members = Arc::new(DieselSpaceMemberRepository::new(store.clone()));
        let keys = Arc::new(crate::security::AdmissionKeyManager::new(
            Arc::new(Keys::default()),
            [71; 16],
        ));
        let ledger = Arc::new(crate::space::SqliteMembershipLedger::new(
            Arc::new(crate::db::executor::DieselSqliteExecutor::new(pool)),
            keys,
        ));
        let facts = |name: &str, byte| {
            let device_id = DeviceId::new(name);
            let credential =
                MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![byte; 32]);
            (
                AdmissionChangeFacts {
                    member_instance: credential.member_instance_id(&device_id),
                    device_id,
                    device_name: name.to_owned(),
                    identity_fingerprint:
                        uc_core::security::IdentityFingerprint::from_display_string(
                            "ABCD-EFGH-IJKL-MNOP",
                        )
                        .unwrap(),
                    transport_public_key: vec![byte],
                    transport_address_blob: vec![byte],
                    identity_signature: vec![byte],
                },
                credential,
            )
        };
        let (a, a_key) = facts("device-a", 1);
        let (b, b_key) = facts("device-b", 2);
        let history = VersionedMembershipHistory::from_activation_baseline(
            MembershipActivationBaselineV2::Established {
                lineage_id: "selected-space".to_owned(),
                head_event_id: MembershipEventId::from_hex(&"03".repeat(32)).unwrap(),
                head_depth: 0,
                current_members: vec![(a.clone(), a_key.clone()), (b.clone(), b_key)],
            },
        )
        .unwrap();
        let mut old_removal = history
            .create_unsigned_local_removal_event(
                a.member_instance,
                &a_key,
                b.member_instance,
                [4; 16],
                [5; 32],
            )
            .unwrap();
        old_removal.signature = vec![6];
        let mut loaded = LoadedMembershipLedger::no_current_space();
        loaded.revision = 1;
        loaded.membership_history = Some(history.encode_persisted_v2().unwrap());
        loaded.lineage_id = Some("selected-space".to_owned());
        loaded.local_device_id = Some(a.device_id);
        loaded.local_member_instance = Some(a.member_instance);
        loaded.local_join_active = true;
        loaded.effect_journal.insert(
            *old_removal.event_id().as_bytes(),
            PendingMembershipEffect {
                event_id: *old_removal.event_id().as_bytes(),
                kind: MembershipEffectKind::RemoveDevice,
                phase: MembershipEffectPhase::Activated,
                affected_device_ids: vec![b.device_id.clone()],
                payload: postcard::to_stdvec(&old_removal).unwrap(),
            },
        );
        members
            .save(&SpaceMember {
                device_id: b.device_id.clone(),
                device_name: b.device_name,
                identity_fingerprint: b.identity_fingerprint,
                joined_at: Utc::now(),
                sync_preferences: MemberSyncPreferences::default(),
            })
            .await
            .unwrap();
        let digest = <[u8; 32]>::from(sha2::Sha256::digest(
            loaded.membership_history.as_ref().unwrap(),
        ));
        let plan =
            MembershipProjectionPlan::from_verified_history(&loaded, &history, digest).unwrap();
        ledger
            .compare_and_commit(MembershipLedgerMutation {
                expected_revision: 0,
                expected_history_digest: None,
                replacement: loaded,
            })
            .await
            .unwrap();
        let maintenance = MembershipProjectionAdapter::new(ledger, store);
        maintenance
            .apply_membership_projection(plan.clone())
            .await
            .unwrap();
        assert!(
            members.get(&b.device_id).await.unwrap().is_some(),
            "新组仍有效的 B 不能被旧组移除记录清理"
        );
        members.remove(&b.device_id).await.unwrap();
        maintenance
            .apply_membership_projection(plan.clone())
            .await
            .unwrap();
        assert!(
            members.get(&b.device_id).await.unwrap().is_some(),
            "已损坏现场的有效成员资料必须能重建"
        );
        let mut outdated = plan;
        outdated.expected_revision = 0;
        outdated.members.clear();
        outdated.trusted_device_ids.clear();
        assert!(matches!(
            maintenance.apply_membership_projection(outdated).await,
            Err(ApplyMembershipProjectionError::Conflict)
        ));
        assert!(
            members.get(&b.device_id).await.unwrap().is_some(),
            "过期计划不得清理现有成员"
        );
    }
}
