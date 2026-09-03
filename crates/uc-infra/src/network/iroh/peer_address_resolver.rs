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
                postcard::from_bytes(&record.addr_blob)
                    .map_err(|source| PeerAddressResolutionError::InvalidEncoding { source })
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
}
