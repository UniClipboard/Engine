use std::sync::Arc;

use iroh::EndpointAddr;
use uc_core::ids::DeviceId;
use uc_core::ports::{PeerAddressError, PeerAddressRepositoryPort};

#[derive(Debug, thiserror::Error)]
pub(super) enum PeerAddressResolutionError {
    #[error("peer address repository failed")]
    Repository {
        #[source]
        source: PeerAddressError,
    },
    #[error("peer address encoding is invalid")]
    InvalidEncoding {
        #[source]
        source: postcard::Error,
    },
}

pub(super) struct PeerAddressResolver {
    repository: Arc<dyn PeerAddressRepositoryPort>,
}

impl PeerAddressResolver {
    pub(super) fn new(repository: Arc<dyn PeerAddressRepositoryPort>) -> Self {
        Self { repository }
    }

    pub(super) async fn resolve(
        &self,
        device: &DeviceId,
    ) -> Result<Option<EndpointAddr>, PeerAddressResolutionError> {
        let record = self
            .repository
            .get(device)
            .await
            .map_err(|source| PeerAddressResolutionError::Repository { source })?;
        record
            .map(|record| {
                use uc_observability_contract::diagnostics::connectivity::{
                    AddressRecordResult, NetworkRecorder, StoredAddressObservation,
                };
                let observation =
                    StoredAddressObservation::new(device.as_str(), Some(&record.addr_blob));
                let addr: EndpointAddr =
                    postcard::from_bytes(&record.addr_blob).map_err(|source| {
                        NetworkRecorder::current()
                            .address_record(&observation, AddressRecordResult::InvalidEncoding);
                        PeerAddressResolutionError::InvalidEncoding { source }
                    })?;
                let (summary, signature) = super::connection_diagnostics::candidate_summary(&addr);
                observation.in_scope(|| {
                    NetworkRecorder::current().address_loaded(
                        *addr.id.as_bytes(),
                        signature,
                        summary,
                        record.observed_at.timestamp_millis(),
                    )
                });
                Ok(addr)
            })
            .transpose()
    }
}

impl PeerAddressResolutionError {
    pub(super) const fn kind(&self) -> &'static str {
        match self {
            Self::Repository { .. } => "repository",
            Self::InvalidEncoding { .. } => "invalid_encoding",
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use chrono::Utc;
    use iroh::{EndpointAddr, SecretKey};
    use uc_core::ids::DeviceId;
    use uc_core::ports::{PeerAddressError, PeerAddressRecord, PeerAddressRepositoryPort};

    use super::{PeerAddressResolutionError, PeerAddressResolver};

    enum Reply {
        Missing,
        Record(Vec<u8>),
        Failed,
    }

    struct StubRepository(Reply);

    #[async_trait]
    impl PeerAddressRepositoryPort for StubRepository {
        async fn get(
            &self,
            device: &DeviceId,
        ) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
            match &self.0 {
                Reply::Missing => Ok(None),
                Reply::Record(addr_blob) => Ok(Some(PeerAddressRecord {
                    device_id: *device,
                    addr_blob: addr_blob.clone(),
                    observed_at: Utc::now(),
                })),
                Reply::Failed => Err(PeerAddressError::Internal("test failure".to_owned())),
            }
        }

        async fn upsert(&self, _record: &PeerAddressRecord) -> Result<(), PeerAddressError> {
            unreachable!()
        }

        async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
            unreachable!()
        }

        async fn remove(&self, _device: &DeviceId) -> Result<(), PeerAddressError> {
            unreachable!()
        }
    }

    fn resolver(reply: Reply) -> PeerAddressResolver {
        PeerAddressResolver::new(std::sync::Arc::new(StubRepository(reply)))
    }

    #[tokio::test]
    async fn resolves_stored_endpoint_address() {
        let expected = EndpointAddr::new(SecretKey::generate().public());
        let encoded = postcard::to_stdvec(&expected).unwrap();

        let actual = resolver(Reply::Record(encoded))
            .resolve(&DeviceId::new("peer"))
            .await
            .unwrap();

        assert_eq!(actual, Some(expected));
    }

    #[tokio::test]
    async fn preserves_missing_address_as_none() {
        let actual = resolver(Reply::Missing)
            .resolve(&DeviceId::new("peer"))
            .await
            .unwrap();

        assert_eq!(actual, None);
    }

    #[tokio::test]
    async fn preserves_repository_failure_source() {
        let error = resolver(Reply::Failed)
            .resolve(&DeviceId::new("peer"))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            PeerAddressResolutionError::Repository { .. }
        ));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[tokio::test]
    async fn preserves_invalid_encoding_source() {
        let error = resolver(Reply::Record(vec![0xff]))
            .resolve(&DeviceId::new("peer"))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            PeerAddressResolutionError::InvalidEncoding { .. }
        ));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[tokio::test]
    async fn stored_address_read_records_its_origin_without_exporting_the_address() {
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
        let address = EndpointAddr::new(SecretKey::generate().public())
            .with_ip_addr("192.0.2.77:4567".parse().expect("address"));
        let result = resolver(Reply::Record(
            postcard::to_stdvec(&address).expect("record"),
        ))
        .resolve(&DeviceId::new("PRIVATE_DEVICE"))
        .with_subscriber(subscriber)
        .await
        .expect("resolved");
        assert_eq!(result, Some(address.clone()));
        let records = exporter.get_emitted_logs().expect("logs");
        let record = records.iter().find(|entry| entry.record.attributes_iter().any(|(key, value)|
            key.as_str() == "event.name" && matches!(value, AnyValue::String(value) if value.as_str() == "address.loaded")
        )).expect("存储地址读取必须有来源记录");
        let payload = record
            .record
            .attributes_iter()
            .find_map(|(key, value)| match value {
                AnyValue::String(value) if key.as_str() == "payload" => Some(value.as_str()),
                _ => None,
            })
            .expect("payload");
        let fields = uc_observability_contract::diagnostics::connectivity::decode_local_record(
            "address.loaded",
            payload,
            "INFO",
        )
        .expect("typed fields");
        assert_eq!(fields["source"], "stored");
        assert_eq!(fields["direct_count"], 1);
        assert!(fields["observed_at_ms"].as_i64().is_some());
        assert!(!payload.contains("192.0.2.77"));
        assert!(!payload.contains("PRIVATE_DEVICE"));
        assert!(!payload.contains(&address.id.to_string()));
    }
}
