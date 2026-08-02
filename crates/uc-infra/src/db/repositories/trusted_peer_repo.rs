use async_trait::async_trait;
use std::sync::Arc;

use uc_core::{DeviceId, TrustedPeer, TrustedPeerError, TrustedPeerRepositoryPort};

use crate::db::ports::DbExecutor;

use super::EncryptedRelationshipStore;

pub struct DieselTrustedPeerRepository<E> {
    store: Arc<EncryptedRelationshipStore<E>>,
}

impl<E> DieselTrustedPeerRepository<E> {
    pub fn new(store: Arc<EncryptedRelationshipStore<E>>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl<E> TrustedPeerRepositoryPort for DieselTrustedPeerRepository<E>
where
    E: DbExecutor,
{
    async fn get(
        &self,
        peer_device_id_value: &DeviceId,
    ) -> Result<Option<TrustedPeer>, TrustedPeerError> {
        self.store
            .get_trusted_peer(peer_device_id_value)
            .await
            .map_err(|error| TrustedPeerError::Repository(error.to_string()))
    }

    async fn list(&self) -> Result<Vec<TrustedPeer>, TrustedPeerError> {
        self.store
            .list_trusted_peers()
            .await
            .map_err(|error| TrustedPeerError::Repository(error.to_string()))
    }

    async fn save(&self, peer: &TrustedPeer) -> Result<(), TrustedPeerError> {
        self.store
            .save_trusted_peer(peer)
            .await
            .map_err(|error| TrustedPeerError::Repository(error.to_string()))
    }

    async fn remove(&self, peer_device_id_value: &DeviceId) -> Result<bool, TrustedPeerError> {
        self.store
            .remove_trusted_peer(peer_device_id_value)
            .await
            .map_err(|error| TrustedPeerError::Repository(error.to_string()))
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
    use uc_core::{DeviceId, TrustedPeer};

    fn make_repo() -> (
        DieselTrustedPeerRepository<Arc<DieselSqliteExecutor>>,
        TempDir,
    ) {
        let tempdir = tempdir().unwrap();
        let database_url = tempdir.path().join("trusted-peer.sqlite");
        let pool = init_db_pool(database_url.to_str().unwrap()).unwrap();
        let repo = DieselTrustedPeerRepository::new(test_relationship_store(pool));
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

    fn fixture_peer(peer: &str, local: &str) -> TrustedPeer {
        TrustedPeer {
            local_device_id: DeviceId::new(local),
            peer_device_id: DeviceId::new(peer),
            peer_fingerprint: fixture_fingerprint(&format!("FP{peer}")),
            trusted_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn save_then_get_roundtrip() {
        let (repo, _tempdir) = make_repo();
        let peer = fixture_peer("peer-a", "local-1");
        repo.save(&peer).await.unwrap();

        let loaded = repo.get(&peer.peer_device_id).await.unwrap().unwrap();
        assert_eq!(loaded.peer_device_id, peer.peer_device_id);
        assert_eq!(loaded.local_device_id, peer.local_device_id);
        assert_eq!(loaded.peer_fingerprint, peer.peer_fingerprint);
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
        let mut peer = fixture_peer("peer-b", "local-1");
        repo.save(&peer).await.unwrap();

        let rotated = fixture_fingerprint("ROTATEDFP");
        peer.peer_fingerprint = rotated.clone();
        repo.save(&peer).await.unwrap();

        let loaded = repo.get(&peer.peer_device_id).await.unwrap().unwrap();
        assert_eq!(loaded.peer_fingerprint, rotated);
    }

    #[tokio::test]
    async fn list_returns_all_saved() {
        let (repo, _tempdir) = make_repo();
        repo.save(&fixture_peer("a", "local-1")).await.unwrap();
        repo.save(&fixture_peer("b", "local-1")).await.unwrap();
        repo.save(&fixture_peer("c", "local-1")).await.unwrap();

        let mut peers = repo.list().await.unwrap();
        peers.sort_by(|x, y| x.peer_device_id.as_str().cmp(y.peer_device_id.as_str()));
        assert_eq!(peers.len(), 3);
        assert_eq!(peers[0].peer_device_id.as_str(), "a");
        assert_eq!(peers[2].peer_device_id.as_str(), "c");
    }

    #[tokio::test]
    async fn remove_returns_true_when_present_false_when_absent() {
        let (repo, _tempdir) = make_repo();
        let peer = fixture_peer("peer-c", "local-1");
        repo.save(&peer).await.unwrap();

        let first = repo.remove(&peer.peer_device_id).await.unwrap();
        let second = repo.remove(&peer.peer_device_id).await.unwrap();
        assert!(first);
        assert!(!second);
        assert!(repo.get(&peer.peer_device_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn saved_peer_fingerprint_is_not_persisted_in_plaintext() {
        let (repo, tempdir) = make_repo();
        let peer = fixture_peer("peer-privacy-probe", "local-privacy-probe");
        let marker = peer.peer_fingerprint.as_raw();

        repo.save(&peer).await.unwrap();

        for entry in std::fs::read_dir(tempdir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                let bytes = std::fs::read(&path).unwrap();
                assert!(
                    !bytes
                        .windows(marker.len())
                        .any(|window| window == marker.as_bytes()),
                    "peer fingerprint leaked into {}",
                    path.display()
                );
            }
        }
    }
}
