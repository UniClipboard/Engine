use std::sync::Arc;

use tracing::instrument;
use uc_core::ports::pairing_invitation::{
    InvitationError, PairingInvitationAddressCandidate, PairingInvitationAddressQueryPort,
};

use super::QueryPairingInvitationAddressesError;

pub(crate) struct QueryPairingInvitationAddressesUseCase {
    addresses: Arc<dyn PairingInvitationAddressQueryPort>,
}

impl QueryPairingInvitationAddressesUseCase {
    pub(crate) fn new(addresses: Arc<dyn PairingInvitationAddressQueryPort>) -> Self {
        Self { addresses }
    }

    #[instrument(skip_all, fields(count = tracing::field::Empty))]
    pub(crate) async fn execute(
        &self,
    ) -> Result<Vec<PairingInvitationAddressCandidate>, QueryPairingInvitationAddressesError> {
        let candidates = self
            .addresses
            .list_invitation_addresses()
            .await
            .map_err(map_error)?;
        tracing::Span::current().record("count", candidates.len());
        Ok(candidates)
    }
}

fn map_error(error: InvitationError) -> QueryPairingInvitationAddressesError {
    match error {
        InvitationError::NetworkNotStarted => {
            QueryPairingInvitationAddressesError::NetworkNotStarted
        }
        InvitationError::ServiceUnavailable => {
            QueryPairingInvitationAddressesError::ServiceUnavailable
        }
        InvitationError::AddressNotAvailable(ip) => {
            QueryPairingInvitationAddressesError::AddressNotAvailable(ip)
        }
        InvitationError::Internal(message) => {
            QueryPairingInvitationAddressesError::Internal(message)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use async_trait::async_trait;

    use super::*;

    struct FixedAddresses(Result<Vec<PairingInvitationAddressCandidate>, InvitationError>);

    #[async_trait]
    impl PairingInvitationAddressQueryPort for FixedAddresses {
        async fn list_invitation_addresses(
            &self,
        ) -> Result<Vec<PairingInvitationAddressCandidate>, InvitationError> {
            match &self.0 {
                Ok(addresses) => Ok(addresses.clone()),
                Err(InvitationError::NetworkNotStarted) => Err(InvitationError::NetworkNotStarted),
                Err(InvitationError::ServiceUnavailable) => {
                    Err(InvitationError::ServiceUnavailable)
                }
                Err(InvitationError::AddressNotAvailable(ip)) => {
                    Err(InvitationError::AddressNotAvailable(*ip))
                }
                Err(InvitationError::Internal(message)) => {
                    Err(InvitationError::Internal(message.clone()))
                }
            }
        }
    }

    #[tokio::test]
    async fn returns_available_invitation_addresses() {
        let expected = vec![PairingInvitationAddressCandidate {
            ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2)),
            port: 443,
        }];
        let query = QueryPairingInvitationAddressesUseCase::new(Arc::new(FixedAddresses(Ok(
            expected.clone(),
        ))));

        let addresses = query.execute().await.unwrap();

        assert_eq!(addresses, expected);
    }

    #[tokio::test]
    async fn preserves_network_not_started_error() {
        let query = QueryPairingInvitationAddressesUseCase::new(Arc::new(FixedAddresses(Err(
            InvitationError::NetworkNotStarted,
        ))));

        let error = query.execute().await.unwrap_err();

        assert!(matches!(
            error,
            QueryPairingInvitationAddressesError::NetworkNotStarted
        ));
    }
}
