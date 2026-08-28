use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use uc_core::membership::InvitationId;
use uc_core::pairing::invitation::FullInvitation;

const FULL_INVITATION_PREFIX: &str = "ucspace1_";
const FULL_INVITATION_FORMAT_V1: u16 = 1;
const MAX_ROUTE_LEN: usize = 64 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum FullInvitationCodecError {
    #[error("the full invitation route is invalid")]
    InvalidRoute,
    #[error("the full invitation encoding is invalid")]
    InvalidEncoding,
    #[error("the full invitation version is unsupported")]
    UnsupportedVersion,
    #[error("the full invitation has expired")]
    Expired,
}

#[derive(Serialize, Deserialize)]
struct FullInvitationV1 {
    format_version: u16,
    invitation_id: [u8; 32],
    route: Vec<u8>,
    expires_at_ms: i64,
}

pub(crate) struct DecodedFullInvitation {
    invitation_id: InvitationId,
    route: Vec<u8>,
    expires_at_ms: i64,
}

impl DecodedFullInvitation {
    pub(crate) const fn invitation_id(&self) -> InvitationId {
        self.invitation_id
    }

    pub(crate) fn route(&self) -> &[u8] {
        &self.route
    }

    pub(crate) const fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }
}

pub(crate) fn encode_full_invitation(
    invitation_id: InvitationId,
    route: &[u8],
    expires_at_ms: i64,
) -> Result<FullInvitation, FullInvitationCodecError> {
    validate_route(route)?;
    let encoded = postcard::to_stdvec(&FullInvitationV1 {
        format_version: FULL_INVITATION_FORMAT_V1,
        invitation_id: *invitation_id.as_bytes(),
        route: route.to_vec(),
        expires_at_ms,
    })
    .map_err(|_| FullInvitationCodecError::InvalidEncoding)?;
    FullInvitation::new(format!(
        "{FULL_INVITATION_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(encoded)
    ))
    .map_err(|_| FullInvitationCodecError::InvalidEncoding)
}

pub(crate) fn decode_full_invitation(
    invitation: &FullInvitation,
    now_ms: i64,
) -> Result<DecodedFullInvitation, FullInvitationCodecError> {
    let encoded = invitation
        .as_str()
        .strip_prefix(FULL_INVITATION_PREFIX)
        .ok_or(FullInvitationCodecError::InvalidEncoding)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| FullInvitationCodecError::InvalidEncoding)?;
    let decoded: FullInvitationV1 =
        postcard::from_bytes(&bytes).map_err(|_| FullInvitationCodecError::InvalidEncoding)?;
    if decoded.format_version != FULL_INVITATION_FORMAT_V1 {
        return Err(FullInvitationCodecError::UnsupportedVersion);
    }
    let invitation_id = InvitationId::from_bytes(decoded.invitation_id)
        .ok_or(FullInvitationCodecError::InvalidEncoding)?;
    validate_route(&decoded.route)?;
    if now_ms >= decoded.expires_at_ms {
        return Err(FullInvitationCodecError::Expired);
    }
    Ok(DecodedFullInvitation {
        invitation_id,
        route: decoded.route,
        expires_at_ms: decoded.expires_at_ms,
    })
}

pub(crate) fn decode_invitation_entry(
    value: &str,
    now_ms: i64,
) -> Result<Option<DecodedFullInvitation>, FullInvitationCodecError> {
    if !value.starts_with(FULL_INVITATION_PREFIX) {
        return Ok(None);
    }
    let invitation = FullInvitation::new(value.to_owned())
        .map_err(|_| FullInvitationCodecError::InvalidEncoding)?;
    decode_full_invitation(&invitation, now_ms).map(Some)
}

fn validate_route(route: &[u8]) -> Result<(), FullInvitationCodecError> {
    if route.is_empty() || route.len() > MAX_ROUTE_LEN {
        Err(FullInvitationCodecError::InvalidRoute)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invitation_id() -> InvitationId {
        InvitationId::from_bytes([0x51; 32]).expect("valid invitation id")
    }

    #[test]
    fn full_invitation_rejects_invalid_route_and_expiry() {
        assert_eq!(
            encode_full_invitation(invitation_id(), &[], 100),
            Err(FullInvitationCodecError::InvalidRoute)
        );
        assert_eq!(
            encode_full_invitation(invitation_id(), &[0x52; MAX_ROUTE_LEN + 1], 100),
            Err(FullInvitationCodecError::InvalidRoute)
        );

        let invitation =
            encode_full_invitation(invitation_id(), b"route", 100).expect("valid full invitation");
        assert!(matches!(
            decode_full_invitation(&invitation, 100),
            Err(FullInvitationCodecError::Expired)
        ));
    }

    #[test]
    fn full_invitation_rejects_malformed_and_unknown_versions() {
        let malformed = FullInvitation::new("not-a-full-invitation").expect("bounded fixture");
        assert!(matches!(
            decode_full_invitation(&malformed, 0),
            Err(FullInvitationCodecError::InvalidEncoding)
        ));

        let encoded = postcard::to_stdvec(&FullInvitationV1 {
            format_version: FULL_INVITATION_FORMAT_V1 + 1,
            invitation_id: *invitation_id().as_bytes(),
            route: b"route".to_vec(),
            expires_at_ms: 100,
        })
        .expect("future version fixture");
        let future = FullInvitation::new(format!(
            "{FULL_INVITATION_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(encoded)
        ))
        .expect("bounded fixture");
        assert!(matches!(
            decode_full_invitation(&future, 0),
            Err(FullInvitationCodecError::UnsupportedVersion)
        ));
    }
}
