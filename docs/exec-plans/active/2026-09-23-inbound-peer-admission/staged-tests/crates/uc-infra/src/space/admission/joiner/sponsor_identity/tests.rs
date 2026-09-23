//! 邀请方成员历史身份必须等于其续连端点身份（切片 S4 暂存验收；落地路径即本文件在仓库中的相对路径）。
use iroh::{EndpointAddr, SecretKey};
use uc_application::deps::PrepareJoinerActivationError;
use uc_core::membership::{AdmissionContinuationRoute, SpaceAdmissionRejectionReason};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::security::IdentityFingerprint;

use super::{sponsor_identity_rejection, verify_sponsor_route_identity, SponsorRouteIdentityError};
use crate::network::iroh::encode_space_admission_route;
use crate::security::Sha256IdentityFingerprintFactory;

fn route_for(key: &SecretKey) -> AdmissionContinuationRoute {
    let bytes = encode_space_admission_route(&EndpointAddr::new(key.public()), None).unwrap();
    AdmissionContinuationRoute::from_bytes(bytes).unwrap()
}

fn fingerprint_of(key: &SecretKey) -> IdentityFingerprint {
    Sha256IdentityFingerprintFactory
        .from_public_key(key.public().as_bytes())
        .unwrap()
}

#[test]
fn matching_sponsor_identity_is_accepted() {
    let sponsor = SecretKey::generate();
    assert_eq!(
        verify_sponsor_route_identity(
            &route_for(&sponsor),
            &fingerprint_of(&sponsor),
            &Sha256IdentityFingerprintFactory,
        ),
        Ok(())
    );
}

#[test]
fn history_identity_of_another_key_is_a_mismatch() {
    // 现场假设的形态：成员历史保存旧身份，邀请方实际以新身份提供续连端点。
    let recorded = SecretKey::generate();
    let current = SecretKey::generate();
    assert_eq!(
        verify_sponsor_route_identity(
            &route_for(&current),
            &fingerprint_of(&recorded),
            &Sha256IdentityFingerprintFactory,
        ),
        Err(SponsorRouteIdentityError::Mismatch)
    );
}

#[test]
fn a_continuation_route_with_an_invitation_is_still_checked() {
    let sponsor = SecretKey::generate();
    let other = SecretKey::generate();
    let bytes = encode_space_admission_route(
        &EndpointAddr::new(sponsor.public()),
        uc_core::membership::InvitationId::from_bytes([7; 32]),
    )
    .unwrap();
    let route = AdmissionContinuationRoute::from_bytes(bytes).unwrap();
    assert_eq!(
        verify_sponsor_route_identity(
            &route,
            &fingerprint_of(&other),
            &Sha256IdentityFingerprintFactory
        ),
        Err(SponsorRouteIdentityError::Mismatch)
    );
}

#[test]
fn an_undecodable_route_is_rejected_not_skipped() {
    let sponsor = SecretKey::generate();
    let route = AdmissionContinuationRoute::from_bytes(b"continuation-route".to_vec()).unwrap();
    assert_eq!(
        verify_sponsor_route_identity(
            &route,
            &fingerprint_of(&sponsor),
            &Sha256IdentityFingerprintFactory
        ),
        Err(SponsorRouteIdentityError::RouteUndecodable)
    );
}

#[test]
fn every_failure_is_a_terminal_identity_conflict() {
    for error in [
        SponsorRouteIdentityError::Mismatch,
        SponsorRouteIdentityError::RouteUndecodable,
        SponsorRouteIdentityError::FingerprintUnavailable,
    ] {
        match sponsor_identity_rejection(error) {
            PrepareJoinerActivationError::Invalid { reason, source } => {
                assert_eq!(reason, SpaceAdmissionRejectionReason::IdentityConflict);
                assert!(
                    source.downcast_ref::<SponsorRouteIdentityError>().is_some(),
                    "the typed cause must stay in the source chain"
                );
            }
            other => panic!("{error:?} must not become retryable: {other:?}"),
        }
    }
}
