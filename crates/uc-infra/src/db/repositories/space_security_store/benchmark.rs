use tempfile::TempDir;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    ContentKeyId, PendingGroupUpdate, ProtectionGroupId, RevocationRepositoryPort,
    SpaceKeyMaterial, SpaceKeyState,
};

use super::DieselSpaceSecurityStore;
use crate::db::executor::DieselSqliteExecutor;
use crate::db::pool::init_db_pool;
use crate::security::MasterKey;
use crate::space::InMemorySession;

/// 仅供正式性能套件创建不含用户资料的成员更新积压场景。
pub struct GroupUpdateDeliveryBenchmark {
    _directory: TempDir,
    repository: DieselSpaceSecurityStore<DieselSqliteExecutor>,
    space_id: SpaceId,
}

impl GroupUpdateDeliveryBenchmark {
    pub async fn new(material_bytes: usize) -> anyhow::Result<Self> {
        let directory = tempfile::tempdir()?;
        let database_path = directory.path().join("profile.sqlite");
        let database_url = database_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("benchmark database path is invalid"))?;
        let pool = init_db_pool(database_url)?;
        let session = InMemorySession::new();
        session.set_master_key(MasterKey::from_bytes(&[0x5a; 32])?);
        let repository = DieselSpaceSecurityStore::new(DieselSqliteExecutor::new(pool), session);
        let space_id = SpaceId::from_str("benchmark-space");
        let mut state = SpaceKeyState::legacy(space_id.clone());
        state.mark_migrating()?;
        state.mark_ready(
            ContentKeyId::from_string("benchmark-content-key")?,
            ProtectionGroupId::generate(),
        )?;
        let mut material = SpaceKeyMaterial::new(state, vec![0x51; material_bytes], Vec::new(), 1);
        material.add_pending_group_updates(
            (0..21).map(|index| {
                PendingGroupUpdate::persistent(
                    DeviceId::new(format!("benchmark-peer-{index}")),
                    br#"{"version":1,"group_epoch":1,"commit":[],"encrypted_key_catalog":[]}"#
                        .to_vec(),
                )
            }),
            2,
        );
        repository.save_space_material(&material).await?;
        let benchmark = Self {
            _directory: directory,
            repository,
            space_id,
        };
        benchmark.load_due().await?;
        Ok(benchmark)
    }

    pub async fn load_due(&self) -> anyhow::Result<()> {
        let due = self
            .repository
            .due_group_updates(&self.space_id, 3, None)
            .await?;
        if due.len() != 8 {
            return Err(anyhow::anyhow!(
                "benchmark load returned the wrong task count"
            ));
        }
        Ok(())
    }
}
