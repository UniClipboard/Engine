//! 加入方激活前核对邀请方续连路由与已验证成员历史的网络身份。
use uc_application::deps::PrepareJoinerActivationError;
use uc_core::membership::{AdmissionContinuationRoute, SpaceAdmissionRejectionReason};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::security::IdentityFingerprint;

use crate::network::iroh::space_admission::decode_space_admission_continuation_endpoint;

#[derive(Debug, thiserror::Error)]
pub(super) enum SponsorRouteIdentityError {
    #[error("sponsor continuation route is undecodable")]
    RouteUndecodable {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("sponsor endpoint fingerprint is unavailable")]
    FingerprintUnavailable {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("sponsor endpoint fingerprint differs from membership history")]
    Mismatch,
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl SponsorRouteIdentityError {
    pub fn fingerprint_unavailable() -> Self {
        Self::FingerprintUnavailable { source: None }
    }

    pub fn fingerprint_unavailable_from(source: impl Into<anyhow::Error>) -> Self {
        Self::FingerprintUnavailable {
            source: Some(source.into()),
        }
    }

    pub fn route_undecodable() -> Self {
        Self::RouteUndecodable { source: None }
    }

    pub fn route_undecodable_from(source: impl Into<anyhow::Error>) -> Self {
        Self::RouteUndecodable {
            source: Some(source.into()),
        }
    }
}

pub(super) fn verify_sponsor_route_identity(
    route: &AdmissionContinuationRoute,
    sponsor_fingerprint: &IdentityFingerprint,
    fingerprints: &dyn IdentityFingerprintFactoryPort,
) -> Result<(), SponsorRouteIdentityError> {
    let endpoint = decode_space_admission_continuation_endpoint(route.as_bytes())
        .map_err(SponsorRouteIdentityError::route_undecodable_from)?;
    let actual = fingerprints
        .from_public_key(endpoint.id.as_bytes())
        .map_err(SponsorRouteIdentityError::fingerprint_unavailable_from)?;
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
