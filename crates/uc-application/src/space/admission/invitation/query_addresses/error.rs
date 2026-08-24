use std::net::IpAddr;

#[derive(Debug, thiserror::Error)]
pub enum QueryPairingInvitationAddressesError {
    #[error("pairing network is not started")]
    NetworkNotStarted,

    #[error("pairing invitation service is unavailable")]
    ServiceUnavailable,

    #[error("pairing invitation address is not available: {0}")]
    AddressNotAvailable(IpAddr),

    #[error("failed to query pairing invitation addresses: {0}")]
    Internal(String),
}
