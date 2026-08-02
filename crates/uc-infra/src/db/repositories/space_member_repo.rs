use async_trait::async_trait;
use std::sync::Arc;

use uc_core::{DeviceId, MemberRepositoryPort, MembershipError, SpaceMember};

use crate::db::ports::DbExecutor;

use super::EncryptedRelationshipStore;

pub struct DieselSpaceMemberRepository<E> {
    store: Arc<EncryptedRelationshipStore<E>>,
}

impl<E> DieselSpaceMemberRepository<E> {
    pub fn new(store: Arc<EncryptedRelationshipStore<E>>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl<E> MemberRepositoryPort for DieselSpaceMemberRepository<E>
where
    E: DbExecutor,
{
    async fn get(
        &self,
        device_id_value: &DeviceId,
    ) -> Result<Option<SpaceMember>, MembershipError> {
        self.store
            .get_member(device_id_value)
            .await
            .map_err(|error| MembershipError::Repository(error.to_string()))
    }

    async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
        self.store
            .list_members()
            .await
            .map_err(|error| MembershipError::Repository(error.to_string()))
    }

    async fn save(&self, member: &SpaceMember) -> Result<(), MembershipError> {
        self.store
            .save_member(member)
            .await
            .map_err(|error| MembershipError::Repository(error.to_string()))
    }

    async fn remove(&self, device_id_value: &DeviceId) -> Result<bool, MembershipError> {
        self.store
            .remove_member(device_id_value)
            .await
            .map_err(|error| MembershipError::Repository(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::pool::init_db_pool;
    use crate::db::repositories::relationship_store::test_relationship_store;
    use chrono::Utc;
    use tempfile::{tempdir, TempDir};
    use uc_core::security::IdentityFingerprint;
    use uc_core::{DeviceId, MemberSyncPreferences, SpaceMember};

    fn make_repo() -> (
        DieselSpaceMemberRepository<Arc<DieselSqliteExecutor>>,
        TempDir,
    ) {
        let tempdir = tempdir().unwrap();
        let database_url = tempdir.path().join("space-member.sqlite");
        let pool = init_db_pool(database_url.to_str().unwrap()).unwrap();
        let repo = DieselSpaceMemberRepository::new(test_relationship_store(pool));
        (repo, tempdir)
    }

    /// Pad a short seed into a valid 16-char alphanumeric fingerprint.
    fn fixture_fingerprint(seed: &str) -> IdentityFingerprint {
        let mut raw: String = seed.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        raw.make_ascii_uppercase();
        while raw.len() < 16 {
            raw.push('A');
        }
        IdentityFingerprint::from_raw_string(&raw[..16]).unwrap()
    }

    fn fixture_member(id: &str) -> SpaceMember {
        SpaceMember {
            device_id: DeviceId::new(id),
            device_name: format!("device-{id}"),
            identity_fingerprint: fixture_fingerprint(&format!("FP{id}")),
            joined_at: Utc::now(),
            sync_preferences: MemberSyncPreferences::default(),
        }
    }

    #[tokio::test]
    async fn save_then_get_roundtrip() {
        let (repo, _tempdir) = make_repo();
        let member = fixture_member("dev-a");
        repo.save(&member).await.unwrap();

        let loaded = repo.get(&member.device_id).await.unwrap().unwrap();
        assert_eq!(loaded.device_id, member.device_id);
        assert_eq!(loaded.device_name, member.device_name);
        assert_eq!(loaded.identity_fingerprint, member.identity_fingerprint);
        assert_eq!(loaded.sync_preferences, member.sync_preferences);
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let (repo, _tempdir) = make_repo();
        let result = repo.get(&DeviceId::new("missing")).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn save_is_upsert() {
        let (repo, _tempdir) = make_repo();
        let mut member = fixture_member("dev-b");
        repo.save(&member).await.unwrap();

        member.device_name = "renamed".to_string();
        repo.save(&member).await.unwrap();

        let loaded = repo.get(&member.device_id).await.unwrap().unwrap();
        assert_eq!(loaded.device_name, "renamed");
    }

    #[tokio::test]
    async fn list_returns_all_saved() {
        let (repo, _tempdir) = make_repo();
        repo.save(&fixture_member("a")).await.unwrap();
        repo.save(&fixture_member("b")).await.unwrap();
        repo.save(&fixture_member("c")).await.unwrap();

        let mut members = repo.list().await.unwrap();
        members.sort_by(|x, y| x.device_id.as_str().cmp(y.device_id.as_str()));
        assert_eq!(members.len(), 3);
        assert_eq!(members[0].device_id.as_str(), "a");
        assert_eq!(members[2].device_id.as_str(), "c");
    }

    #[tokio::test]
    async fn remove_returns_true_when_present_false_when_absent() {
        let (repo, _tempdir) = make_repo();
        let member = fixture_member("dev-c");
        repo.save(&member).await.unwrap();

        let first = repo.remove(&member.device_id).await.unwrap();
        let second = repo.remove(&member.device_id).await.unwrap();
        assert!(first);
        assert!(!second);
        assert!(repo.get(&member.device_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn saved_member_name_is_not_persisted_in_plaintext() {
        let (repo, tempdir) = make_repo();
        let mut member = fixture_member("privacy-probe-member");
        member.device_name = "relationship-member-plaintext-probe-7f31".to_string();

        repo.save(&member).await.unwrap();

        let marker = member.device_name.as_bytes();
        for entry in std::fs::read_dir(tempdir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                let bytes = std::fs::read(&path).unwrap();
                assert!(
                    !bytes.windows(marker.len()).any(|window| window == marker),
                    "member name leaked into {}",
                    path.display()
                );
            }
        }
    }

    // NOTE: 2026-04-18-000001_create_space_member 里从 `paired_device` 搬迁数据的
    // `migration_copies_trusted_paired_devices_with_default_preferences` 测试在
    // phase 4b PR-5 随 `DROP TABLE paired_device` 一并删除 —— 迁移本身在 Phase 1
    // 已在生产库落地执行过，`paired_device` 表此后被 2026-04-20 迁移移除，fresh
    // DB 下该测试的前置插入语句无表可写。历史行为由 Phase 1 commit 5f5c6f4c 验证。
}
