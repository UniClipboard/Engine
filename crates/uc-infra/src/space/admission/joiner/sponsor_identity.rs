//! 加入方激活前核对邀请方续连路由与已验证成员历史的网络身份。
use uc_application::deps::PrepareJoinerActivationError;
use uc_core::membership::{AdmissionContinuationRoute, SpaceAdmissionRejectionReason};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::security::IdentityFingerprint;

use crate::network::iroh::space_admission::decode_space_admission_continuation_endpoint;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(super) enum SponsorRouteIdentityError {
    #[error("sponsor continuation route is undecodable")]
    RouteUndecodable,
    #[error("sponsor endpoint fingerprint is unavailable")]
    FingerprintUnavailable,
    #[error("sponsor endpoint fingerprint differs from membership history")]
    Mismatch,
}

pub(super) fn verify_sponsor_route_identity(
    route: &AdmissionContinuationRoute,
    sponsor_fingerprint: &IdentityFingerprint,
    fingerprints: &dyn IdentityFingerprintFactoryPort,
) -> Result<(), SponsorRouteIdentityError> {
    let endpoint = decode_space_admission_continuation_endpoint(route.as_bytes())
        .map_err(|_| SponsorRouteIdentityError::RouteUndecodable)?;
    let actual = fingerprints
        .from_public_key(endpoint.id.as_bytes())
        .map_err(|_| SponsorRouteIdentityError::FingerprintUnavailable)?;
    if actual != *sponsor_fingerprint {
        return Err(SponsorRouteIdentityError::Mismatch);
    }
    Ok(())
}

pub(super) fn sponsor_identity_rejection(
    error: SponsorRouteIdentityError,
) -> PrepareJoinerActivationError {
    PrepareJoinerActivationError::invalid_for(
        SpaceAdmissionRejectionReason::IdentityConflict,
        error,
    )
}

#[cfg(test)]
mod tests;
