//! Diesel-backed [`PeerAddressRepositoryPort`] implementation.
//!
//! Persists one address record per device in the `peer_address` table. The
//! stored `addr_blob` is treated as opaque bytes on the persistence layer —
//! the caller (iroh adapter) owns the encoding.

use async_trait::async_trait;
use std::sync::Arc;

use uc_core::ids::DeviceId;
use uc_core::ports::{PeerAddressError, PeerAddressRecord, PeerAddressRepositoryPort};

use crate::db::ports::DbExecutor;

use super::EncryptedRelationshipStore;
use uc_observability_contract::diagnostics::connectivity::{
    AddressRecordResult, NetworkRecorder, StoredAddressObservation,
};

pub struct DieselPeerAddressRepository<E> {
    store: Arc<EncryptedRelationshipStore<E>>,
}

impl<E> DieselPeerAddressRepository<E> {
    pub fn new(store: Arc<EncryptedRelationshipStore<E>>) -> Self {
        use uc_observability_contract::diagnostics::connectivity::{
            LocalDiagnosticSource, SourceCapability, SourceCollection,
        };
        NetworkRecorder::current().register_source(
            LocalDiagnosticSource::AddressStorage,
            SourceCapability::Supported,
            SourceCollection::Enabled,
        );
        Self { store }
    }
}

#[async_trait]
impl<E> PeerAddressRepositoryPort for DieselPeerAddressRepository<E>
where
    E: DbExecutor,
{
    async fn get(&self, device: &DeviceId) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
        let result = self
            .store
            .get_peer_address(device)
            .await
            .map_err(|error| PeerAddressError::Internal(error.to_string()));
        let observation = StoredAddressObservation::new(
            device.as_str(),
            result
                .as_ref()
                .ok()
                .and_then(|record| record.as_ref())
                .map(|record| record.addr_blob.as_slice()),
        );
        let outcome = match &result {
            Ok(Some(record)) => AddressRecordResult::Loaded {
                observed_at_ms: record.observed_at.timestamp_millis(),
            },
            Ok(None) => AddressRecordResult::Missing,
            Err(_) => AddressRecordResult::ReadFailed,
        };
        NetworkRecorder::current().address_record(&observation, outcome);
        result
    }

    async fn upsert(&self, record: &PeerAddressRecord) -> Result<(), PeerAddressError> {
        let result = self
            .store
            .save_peer_address(record)
            .await
            .map_err(|error| PeerAddressError::Internal(error.to_string()));
        let observation =
            StoredAddressObservation::new(record.device_id.as_str(), Some(&record.addr_blob));
        NetworkRecorder::current().address_record(
            &observation,
            if result.is_ok() {
                AddressRecordResult::Saved {
                    observed_at_ms: record.observed_at.timestamp_millis(),
                }
            } else {
                AddressRecordResult::SaveFailed
            },
        );
        result
    }

    async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
        self.store
            .list_peer_addresses()
            .await
            .map_err(|error| PeerAddressError::Internal(error.to_string()))
    }

    async fn remove(&self, device: &DeviceId) -> Result<(), PeerAddressError> {
        self.store
            .remove_peer_address(device)
            .await
            .map(|_| ())
            .map_err(|error| PeerAddressError::Internal(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::pool::init_db_pool;
    use crate::db::repositories::relationship_store::test_relationship_store;
    use chrono::{TimeZone, Utc};
    use tempfile::{tempdir, TempDir};

    fn make_repo() -> (
        DieselPeerAddressRepository<Arc<DieselSqliteExecutor>>,
        TempDir,
    ) {
        let tempdir = tempdir().unwrap();
        let database_url = tempdir.path().join("peer-address.sqlite");
        let pool = init_db_pool(database_url.to_str().unwrap()).unwrap();
        let repo = DieselPeerAddressRepository::new(test_relationship_store(pool));
        (repo, tempdir)
    }

    fn fixture_record(id: &str, blob: &[u8]) -> PeerAddressRecord {
        PeerAddressRecord {
            device_id: DeviceId::new(id),
            addr_blob: blob.to_vec(),
            observed_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        }
    }

    #[tokio::test]
    async fn upsert_then_get_roundtrip() {
        use opentelemetry::logs::AnyValue;
        use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
        use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
        use tracing::instrument::WithSubscriber;
        use tracing_subscriber::layer::SubscriberExt;
        let exporter = InMemoryLogExporter::default();
        let provider = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let subscriber =
            tracing_subscriber::registry().with(OpenTelemetryTracingBridge::new(&provider));
        let (repo, _tempdir) = make_repo();
        let rec = fixture_record("dev-a", b"iroh-addr-blob-a");
        repo.upsert(&rec).with_subscriber(subscriber).await.unwrap();

        let loaded = repo.get(&rec.device_id).await.unwrap().unwrap();
        assert_eq!(loaded, rec);
        let rows = exporter.get_emitted_logs().expect("logs");
        assert!(rows.iter().any(|row| row.record.attributes_iter().any(|(key, value)|
            key.as_str() == "event.name" && matches!(value, AnyValue::String(value) if value.as_str() == "address.record.saved")
        )), "真实存储成功必须留下经过编码的保存证据");
        assert!(!format!("{rows:?}").contains("iroh-addr-blob-a"));
        assert!(!format!("{rows:?}").contains("dev-a"));
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let (repo, _tempdir) = make_repo();
        let result = repo.get(&DeviceId::new("missing")).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn upsert_is_last_write_wins() {
        let (repo, _tempdir) = make_repo();
        let mut rec = fixture_record("dev-b", b"v1");
        repo.upsert(&rec).await.unwrap();

        rec.addr_blob = b"v2-bigger-blob".to_vec();
        rec.observed_at = Utc.timestamp_opt(1_700_100_000, 0).unwrap();
        repo.upsert(&rec).await.unwrap();

        let loaded = repo.get(&rec.device_id).await.unwrap().unwrap();
        assert_eq!(loaded.addr_blob, b"v2-bigger-blob".to_vec());
        assert_eq!(loaded.observed_at, rec.observed_at);
    }

    #[tokio::test]
    async fn list_returns_all_rows() {
        let (repo, _tempdir) = make_repo();
        repo.upsert(&fixture_record("a", b"addr-a")).await.unwrap();
        repo.upsert(&fixture_record("b", b"addr-b")).await.unwrap();
        repo.upsert(&fixture_record("c", b"addr-c")).await.unwrap();

        let mut rows = repo.list().await.unwrap();
        rows.sort_by(|x, y| x.device_id.as_str().cmp(y.device_id.as_str()));
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].device_id.as_str(), "a");
        assert_eq!(rows[2].device_id.as_str(), "c");
    }

    #[tokio::test]
    async fn remove_is_idempotent() {
        let (repo, _tempdir) = make_repo();
        let rec = fixture_record("dev-c", b"addr-c");
        repo.upsert(&rec).await.unwrap();

        repo.remove(&rec.device_id).await.unwrap();
        repo.remove(&rec.device_id).await.unwrap();
        assert!(repo.get(&rec.device_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_on_empty_db_is_empty_vec() {
        let (repo, _tempdir) = make_repo();
        let rows = repo.list().await.unwrap();
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn saved_address_is_not_persisted_in_plaintext() {
        let (repo, tempdir) = make_repo();
        let marker = b"relationship-address-plaintext-probe-4d92";
        let record = fixture_record("address-privacy-probe", marker);

        repo.upsert(&record).await.unwrap();

        for entry in std::fs::read_dir(tempdir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                let bytes = std::fs::read(&path).unwrap();
                assert!(
                    !bytes.windows(marker.len()).any(|window| window == marker),
                    "peer address leaked into {}",
                    path.display()
                );
            }
        }
    }
}
