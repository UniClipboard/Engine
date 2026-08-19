use std::sync::Arc;

use tracing::instrument;
use uc_core::ids::SpaceId;
use uc_core::ports::setup::SetupStatusPort;
use uc_core::ports::space::{IsSpaceUnlockedPort, SpaceAccessError, VerifyKeychainAccessPort};

const DEFAULT_SPACE_ID: &str = "space";

/// Narrow space-access ports consumed by [`EncryptionFacade`]. Each maps to one
/// facade method; the facade holds only the slices it calls (ports.md §8.1).
#[derive(Clone)]
pub struct EncryptionFacadeDeps {
    pub setup_status: Arc<dyn SetupStatusPort>,
    pub is_unlocked: Arc<dyn IsSpaceUnlockedPort>,
    pub verify_keychain_access: Arc<dyn VerifyKeychainAccessPort>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionStateView {
    pub initialized: bool,
    pub session_ready: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum EncryptionFacadeError {
    #[error("failed to load setup status: {0}")]
    SetupStatus(String),
    #[error("space access failed: {0}")]
    SpaceAccess(String),
}

pub struct EncryptionFacade {
    deps: EncryptionFacadeDeps,
}

impl EncryptionFacade {
    pub fn new(deps: EncryptionFacadeDeps) -> Self {
        Self { deps }
    }

    // 故意不挂 `#[instrument]`:`state()` 是高频只读查询(前端轮询 +
    // CLI / daemon 每个请求都会读一次),自身不做 I/O,也不推进流程。
    // 给它开 span 会被 sentry-tracing 上报成 transaction —— 14 天观测到
    // 96 万次,占 span 配额 ~27%。如要排障,出错路径用 `tracing::warn!`
    // / `error!` 即可,无需 root span。其他写动作(initialize / unlock /
    // lock / verify_keychain_access)仍保留 instrument。
    pub async fn state(&self) -> Result<EncryptionStateView, EncryptionFacadeError> {
        let (initialized, space_id) = self.setup_state().await?;
        let session_ready = if initialized {
            self.deps.is_unlocked.is_unlocked(&space_id).await
        } else {
            false
        };

        Ok(EncryptionStateView {
            initialized,
            session_ready,
        })
    }

    #[instrument(skip_all)]
    pub async fn verify_keychain_access(&self) -> Result<bool, EncryptionFacadeError> {
        self.deps
            .verify_keychain_access
            .verify_keychain_access()
            .await
            .map_err(space_access_error)
    }

    async fn setup_state(&self) -> Result<(bool, SpaceId), EncryptionFacadeError> {
        let status = self
            .deps
            .setup_status
            .get_status()
            .await
            .map_err(|err| EncryptionFacadeError::SetupStatus(err.to_string()))?;
        Ok((
            status.has_completed,
            status.space_id.unwrap_or_else(default_space_id),
        ))
    }
}

fn default_space_id() -> SpaceId {
    SpaceId::from(DEFAULT_SPACE_ID)
}

fn space_access_error(err: SpaceAccessError) -> EncryptionFacadeError {
    EncryptionFacadeError::SpaceAccess(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use std::sync::Mutex;
    use uc_core::crypto::domain::{ActiveSpace, Passphrase};
    use uc_core::ports::space::{InitializeSpacePort, LockSpacePort, ResumeSpaceSessionPort};
    use uc_core::setup::SetupStatus;

    use crate::test_support::CountingMobileConsumableBackfill;

    #[derive(Default)]
    struct FakeSetupStatus {
        status: Mutex<SetupStatus>,
    }

    #[async_trait]
    impl SetupStatusPort for FakeSetupStatus {
        async fn get_status(&self) -> anyhow::Result<SetupStatus> {
            Ok(self.status.lock().expect("status lock").clone())
        }

        async fn set_status(&self, status: &SetupStatus) -> anyhow::Result<()> {
            *self.status.lock().expect("status lock") = status.clone();
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeSpaceAccess {
        unlocked: Mutex<bool>,
        resume_returns_session: Mutex<bool>,
        verify_granted: Mutex<bool>,
        lock_calls: Mutex<u32>,
        init_already_initialized: Mutex<bool>,
        init_calls: Mutex<u32>,
        resume_space_ids: Mutex<Vec<SpaceId>>,
        is_unlocked_space_ids: Mutex<Vec<SpaceId>>,
    }

    #[async_trait]
    impl InitializeSpacePort for FakeSpaceAccess {
        async fn initialize(
            &self,
            space_id: &SpaceId,
            _passphrase: &Passphrase,
        ) -> Result<ActiveSpace, SpaceAccessError> {
            *self.init_calls.lock().expect("init calls lock") += 1;
            if *self.init_already_initialized.lock().expect("init flag") {
                return Err(SpaceAccessError::AlreadyInitialized);
            }
            Ok(ActiveSpace::new(space_id.clone()))
        }
    }

    #[async_trait]
    impl IsSpaceUnlockedPort for FakeSpaceAccess {
        async fn is_unlocked(&self, space_id: &SpaceId) -> bool {
            self.is_unlocked_space_ids
                .lock()
                .expect("is unlocked space ids lock")
                .push(space_id.clone());
            *self.unlocked.lock().expect("unlocked lock")
        }
    }

    #[async_trait]
    impl LockSpacePort for FakeSpaceAccess {
        async fn lock(&self, _space_id: &SpaceId) -> Result<(), SpaceAccessError> {
            *self.unlocked.lock().expect("unlocked lock") = false;
            *self.lock_calls.lock().expect("lock calls lock") += 1;
            Ok(())
        }
    }

    #[async_trait]
    impl ResumeSpaceSessionPort for FakeSpaceAccess {
        async fn try_resume_session(
            &self,
            space_id: &SpaceId,
        ) -> Result<Option<ActiveSpace>, SpaceAccessError> {
            self.resume_space_ids
                .lock()
                .expect("resume space ids lock")
                .push(space_id.clone());
            if *self.resume_returns_session.lock().expect("resume lock") {
                *self.unlocked.lock().expect("unlocked lock") = true;
                Ok(Some(ActiveSpace::new(space_id.clone())))
            } else {
                Ok(None)
            }
        }
    }

    #[async_trait]
    impl VerifyKeychainAccessPort for FakeSpaceAccess {
        async fn verify_keychain_access(&self) -> Result<bool, SpaceAccessError> {
            Ok(*self.verify_granted.lock().expect("verify lock"))
        }
    }

    fn facade_with(
        completed: bool,
        unlocked: bool,
        resume_returns_session: bool,
        verify_granted: bool,
    ) -> (
        EncryptionFacade,
        Arc<FakeSpaceAccess>,
        Arc<CountingMobileConsumableBackfill>,
    ) {
        let setup_status = Arc::new(FakeSetupStatus::default());
        setup_status
            .status
            .lock()
            .expect("status lock")
            .clone_from(&SetupStatus {
                has_completed: completed,
                space_id: completed.then(|| SpaceId::from("canonical-space")),
                re_pairing_required: false,
            });
        let space_access = Arc::new(FakeSpaceAccess::default());
        *space_access.unlocked.lock().expect("unlocked lock") = unlocked;
        *space_access
            .resume_returns_session
            .lock()
            .expect("resume lock") = resume_returns_session;
        *space_access.verify_granted.lock().expect("verify lock") = verify_granted;
        let backfill = Arc::new(CountingMobileConsumableBackfill::default());

        (
            EncryptionFacade::new(EncryptionFacadeDeps {
                setup_status,
                is_unlocked: space_access.clone(),
                verify_keychain_access: space_access.clone(),
            }),
            space_access,
            backfill,
        )
    }

    #[tokio::test]
    async fn state_reports_not_ready_when_setup_is_incomplete() {
        let (facade, _, _) = facade_with(false, true, false, false);

        let state = facade.state().await.expect("state");

        assert_eq!(
            state,
            EncryptionStateView {
                initialized: false,
                session_ready: false
            }
        );
    }

    #[tokio::test]
    async fn state_reports_session_ready_after_completed_setup() {
        let (facade, space_access, _) = facade_with(true, true, false, false);

        let state = facade.state().await.expect("state");

        assert_eq!(
            state,
            EncryptionStateView {
                initialized: true,
                session_ready: true
            }
        );
        assert_eq!(
            *space_access
                .is_unlocked_space_ids
                .lock()
                .expect("is unlocked space ids lock"),
            vec![SpaceId::from("canonical-space")]
        );
    }

    #[tokio::test]
    async fn verify_keychain_access_returns_grant_state() {
        let (facade, _, _) = facade_with(true, false, false, true);

        assert!(facade
            .verify_keychain_access()
            .await
            .expect("verify keychain"));
    }
}
