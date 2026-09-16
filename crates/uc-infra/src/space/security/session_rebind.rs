use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::{RebindSpaceSessionPort, SpaceSessionRebindError};
use uc_core::crypto::model::EncryptionError;
use uc_core::ids::SpaceId;

use super::session::InMemorySession;

pub struct SpaceSessionRebindAdapter {
    session: Arc<InMemorySession>,
}

impl SpaceSessionRebindAdapter {
    pub fn new(session: Arc<InMemorySession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl RebindSpaceSessionPort for SpaceSessionRebindAdapter {
    async fn rebind_to_space(&self, space_id: &SpaceId) -> Result<(), SpaceSessionRebindError> {
        self.session
            .rebind_to_space(space_id)
            .map_err(map_rebind_error)
    }
}

fn map_rebind_error(error: EncryptionError) -> SpaceSessionRebindError {
    match error {
        EncryptionError::CorruptedKeySlot
        | EncryptionError::CorruptedBlob
        | EncryptionError::UnsupportedKeySlotVersion
        | EncryptionError::UnsupportedBlobVersion => SpaceSessionRebindError::Inconsistent,
        _ => SpaceSessionRebindError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::MasterKey;

    #[tokio::test]
    async fn rejects_rebind_when_session_is_not_unlocked() {
        let adapter = SpaceSessionRebindAdapter::new(Arc::new(InMemorySession::new()));

        let error = adapter
            .rebind_to_space(&SpaceId::new())
            .await
            .expect_err("locked session must reject rebind");

        assert_eq!(error, SpaceSessionRebindError::Unavailable);
    }

    #[tokio::test]
    async fn rebinds_unlocked_session_to_target_space() {
        let session = Arc::new(InMemorySession::new());
        session.set_master_key(MasterKey::from_bytes(&[7; 32]).unwrap());
        let adapter = SpaceSessionRebindAdapter::new(Arc::clone(&session));
        let target = SpaceId::new();

        adapter.rebind_to_space(&target).await.unwrap();

        assert_eq!(session.current_space_id().unwrap(), target);
    }
    #[test]
    fn concurrent_rebind_and_clear_never_restore_the_cleared_key() {
        let session = InMemorySession::new();
        let barrier = std::sync::Barrier::new(2);
        let all_locked = std::sync::atomic::AtomicBool::new(true);
        std::thread::scope(|threads| {
            threads.spawn(|| {
                for _ in 0..1000 {
                    barrier.wait();
                    let _ = session.rebind_to_space(&SpaceId::from("target"));
                    barrier.wait();
                }
            });
            for _ in 0..1000 {
                session.set_master_key(MasterKey::from_bytes(&[7; 32]).unwrap());
                barrier.wait();
                session.clear();
                barrier.wait();
                if session.is_ready() {
                    all_locked.store(false, std::sync::atomic::Ordering::SeqCst);
                }
            }
        });
        assert!(all_locked.load(std::sync::atomic::Ordering::SeqCst));
    }
}
