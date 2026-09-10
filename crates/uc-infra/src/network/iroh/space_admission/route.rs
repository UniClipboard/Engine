use iroh::EndpointAddr;
use serde::{Deserialize, Serialize};
use uc_application::deps::SpaceAdmissionTransportError;
use uc_core::membership::{InvitationId, SpaceAdmissionRoute};

const DIAL_ROUTE_FORMAT_V1: u16 = 1;

#[derive(Serialize, Deserialize)]
struct AdmissionDialRouteV1 {
    format_version: u16,
    invitation_id: Option<[u8; 32]>,
    endpoint_addr: Vec<u8>,
}

pub(super) struct DecodedRoute {
    pub(super) invitation_id: Option<[u8; 32]>,
    pub(super) endpoint_addr: EndpointAddr,
}

pub fn encode_space_admission_route(
    endpoint_addr: &EndpointAddr,
    invitation_id: Option<InvitationId>,
) -> Result<Vec<u8>, SpaceAdmissionTransportError> {
    let endpoint_addr = postcard::to_stdvec(endpoint_addr)
        .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
    encode_space_admission_route_bytes(&endpoint_addr, invitation_id)
}

pub(crate) fn encode_space_admission_route_bytes(
    endpoint_addr: &[u8],
    invitation_id: Option<InvitationId>,
) -> Result<Vec<u8>, SpaceAdmissionTransportError> {
    postcard::to_stdvec(&AdmissionDialRouteV1 {
        format_version: DIAL_ROUTE_FORMAT_V1,
        invitation_id: invitation_id.map(|id| *id.as_bytes()),
        endpoint_addr: endpoint_addr.to_vec(),
    })
    .map_err(|_| SpaceAdmissionTransportError::Unavailable)
}

pub(super) fn decode_route(
    route: &SpaceAdmissionRoute,
    initial: bool,
) -> Result<DecodedRoute, SpaceAdmissionTransportError> {
    let wire: AdmissionDialRouteV1 = postcard::from_bytes(route.as_bytes())
        .map_err(|_| SpaceAdmissionTransportError::ProtocolRejected)?;
    if wire.format_version != DIAL_ROUTE_FORMAT_V1 || (initial && wire.invitation_id.is_none()) {
        return Err(SpaceAdmissionTransportError::ProtocolRejected);
    }
    let endpoint_addr = postcard::from_bytes(&wire.endpoint_addr)
        .map_err(|_| SpaceAdmissionTransportError::ProtocolRejected)?;
    Ok(DecodedRoute {
        invitation_id: wire.invitation_id,
        endpoint_addr,
    })
}

pub(crate) fn decode_space_admission_route(
    route: &[u8],
) -> Result<(EndpointAddr, Option<InvitationId>), SpaceAdmissionTransportError> {
    let route = SpaceAdmissionRoute::from_bytes(route.to_vec())
        .map_err(|_| SpaceAdmissionTransportError::ProtocolRejected)?;
    let decoded = decode_route(&route, true)?;
    Ok((
        decoded.endpoint_addr,
        decoded.invitation_id.and_then(InvitationId::from_bytes),
    ))
}
