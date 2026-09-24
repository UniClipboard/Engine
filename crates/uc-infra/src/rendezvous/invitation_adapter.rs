//! Sponsor-side port adapter for [`PairingInvitationPort`].
//!
//! Drives two discovery channels concurrently per invitation:
//!
//! 1. **Cloud channel** — `RendezvousClient` POST. Best-effort; failure
//!    here means "cross-network joiners can't resolve via this code,"
//!    not "issue fails."
//! 2. **LAN channel** — window-scoped `MdnsPairingPublisher` instance.
//!    Started for every issued code; dropped on `consume_invitation` or
//!    when the adapter is dropped.
//!
//! Code provenance: when the cloud channel returns Ok, we adopt the
//! server-minted code (back-compat with the legacy "server is the
//! issuing authority" flow). When the cloud channel fails, we fall back
//! to local mint — that's the first-pair-no-WAN path. A future
//! migration (path 4-C in plan notes) will add `proposed_code` to the
//! cloud request and always pass the locally minted value, at which
//! point the conditional disappears.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, TimeZone, Utc};
use iroh::{Endpoint, EndpointAddr, TransportAddr};
use rand::RngCore;
use tokio::runtime::Handle as RuntimeHandle;
use tokio::sync::Mutex;
use tracing::{debug, instrument, warn};

use uc_core::pairing::invitation::InvitationCode;
use uc_core::ports::{
    CodeOrigin, ConsumeInvitationError, DeviceIdentityPort, InvitationError, IssuedInvitation,
    PairingInvitationAddressCandidate, PairingInvitationAddressQueryPort,
    PairingInvitationByAddressPort, PairingInvitationPort, SettingsPort,
};
use uc_core::settings::model::Settings;

use crate::network::iroh::filter_endpoint_addr;
use crate::network::iroh::runtime_consts;
use crate::pairing::{mint_invitation_code, MdnsPairingPublisher, PublisherHandle};

use super::client::{CreatePairingRequest, RendezvousClient, RendezvousHttpError};

/// TTL used when minting a code locally (cloud channel was unreachable).
/// Matches the typical TTL the rendezvous service returns for back-compat.
const LOCAL_MINT_TTL: ChronoDuration = ChronoDuration::seconds(300);

/// 在线目录登记比完整邀请多保留一分钟，避免客户端与目录服务的轻微时钟偏差
/// 把结构正确的成功响应误判为到期时间无效。完整邀请仍只在
/// [`LOCAL_MINT_TTL`] 窗口内有效，额外时间不会延长准入期限。
const DIRECTORY_REGISTRATION_TTL: ChronoDuration = ChronoDuration::seconds(360);

/// Rendezvous-backed adapter for [`PairingInvitationPort`].
///
/// Maintains a per-code map of live mDNS publisher handles so
/// `consume_invitation` can deterministically stop the LAN announce
/// without relying on TTL expiry.
pub struct RendezvousPairingInvitationAdapter {
    endpoint: Arc<Endpoint>,
    device_identity: Arc<dyn DeviceIdentityPort>,
    settings: Arc<dyn SettingsPort>,
    rendezvous: Arc<RendezvousClient>,
    /// Per-code live mDNS publisher handles plus their TTL. Dropping a
    /// value stops the underlying `swarm-discovery` announce thread.
    /// The TTL is used by [`Self::gc_expired_publishers`] (lazy GC,
    /// called on every issue / consume) so a publisher whose code
    /// expired without being consumed still gets released — no
    /// background timer needed.
    publishers: Mutex<HashMap<InvitationCode, (PublisherHandle, DateTime<Utc>)>>,
    #[cfg(test)]
    force_local_publication_failure: bool,
}

impl RendezvousPairingInvitationAdapter {
    pub fn new(
        endpoint: Arc<Endpoint>,
        device_identity: Arc<dyn DeviceIdentityPort>,
        settings: Arc<dyn SettingsPort>,
        rendezvous: Arc<RendezvousClient>,
    ) -> Self {
        Self {
            endpoint,
            device_identity,
            settings,
            rendezvous,
            publishers: Mutex::new(HashMap::new()),
            #[cfg(test)]
            force_local_publication_failure: false,
        }
    }

    #[cfg(test)]
    fn with_local_publication_failure(mut self) -> Self {
        self.force_local_publication_failure = true;
        self
    }

    async fn load_settings(&self) -> Result<Settings, InvitationError> {
        self.settings
            .load()
            .await
            .map_err(|err| InvitationError::Internal(format!("settings load failed: {err}")))
    }

    fn resolve_device_name(settings: &Settings) -> Result<String, InvitationError> {
        settings
            .general
            .device_name
            .as_ref()
            .filter(|n| !n.trim().is_empty())
            .cloned()
            .ok_or_else(|| {
                InvitationError::Internal(
                    "device_name missing from settings; user must set it before pairing"
                        .to_string(),
                )
            })
    }

    fn serialize_ticket(&self, allow_overlay: bool) -> Result<(String, String), InvitationError> {
        serialize_filtered_endpoint_ticket(self.endpoint.addr(), allow_overlay)
    }

    fn serialize_ticket_for_ip(
        &self,
        allow_overlay: bool,
        selected_ip: IpAddr,
    ) -> Result<(String, String), InvitationError> {
        serialize_endpoint_ticket_for_ip(self.endpoint.addr(), allow_overlay, selected_ip)
    }

    async fn create_pairing_with_ticket(
        &self,
        settings: Settings,
        endpoint_id: String,
        ticket: String,
    ) -> Result<IssuedInvitation, InvitationError> {
        let device_name = Self::resolve_device_name(&settings)?;
        let device_id = self.device_identity.current_device_id();
        let invitation_id = mint_invitation_id();
        let expires_at = Utc::now() + LOCAL_MINT_TTL;
        let endpoint_addr: EndpointAddr = serde_json::from_str(&ticket).map_err(|_| {
            InvitationError::Internal("failed to decode Sponsor endpoint address".to_owned())
        })?;
        let admission_route =
            crate::network::iroh::encode_space_admission_route(&endpoint_addr, Some(invitation_id))
                .map_err(|_| {
                    InvitationError::Internal("failed to encode Space admission route".to_owned())
                })?;
        let full_invitation = crate::space::encode_full_invitation(
            invitation_id,
            &admission_route,
            expires_at.timestamp_millis(),
        )
        .map_err(|_| InvitationError::Internal("failed to encode full invitation".to_owned()))?;

        let req = CreatePairingRequest {
            sponsor_device_id: device_id.as_str().to_string(),
            sponsor_device_name: device_name,
            sponsor_endpoint_id: endpoint_id.clone(),
            sponsor_ticket: full_invitation.as_str().to_owned(),
            code_length: crate::pairing::code_mint::INVITATION_CODE_LENGTH,
            ttl_secs: Some(DIRECTORY_REGISTRATION_TTL.num_seconds() as u32),
        };

        // ── Cloud channel (best-effort, gated by LAN-only mode) ────────
        // In LAN-only mode the user has explicitly opted out of cloud
        // discovery. We skip the cloud channel entirely (no HTTP request,
        // no metadata leak to the directory service) and mint the code
        // locally — the LAN channel will be the only publish surface.
        if runtime_consts::lan_only() {
            let code = InvitationCode::new(mint_invitation_code());
            debug!(
                %expires_at,
                "LAN-only mode: minted invitation locally, skipping cloud channel"
            );
            if let Err(err) = self
                .start_mdns_publisher(&code, &endpoint_id, full_invitation.as_str(), expires_at)
                .await
            {
                // mDNS is the only publish surface in LAN-only mode, so a
                // start failure means zero channels were initiated. The port
                // contract requires `Ok` only when at least one channel is
                // live, so surface the failure instead of returning an
                // undialable code.
                warn!(
                    failure_stage = "local_publication",
                    "mDNS publisher start failed in LAN-only mode; this invitation cannot be discovered",
                );
                return Err(map_local_publication_failure(err, None));
            }
            return build_issued_invitation(
                invitation_id,
                code,
                full_invitation,
                expires_at,
                CodeOrigin::LocallyMintedLanOnly,
            );
        }

        let (code, mut directory_failure) = match self.rendezvous.create_pairing(&req).await {
            Ok(parsed) => {
                let code = InvitationCode::new(parsed.code);
                Utc.timestamp_millis_opt(parsed.expires_at_ms)
                    .single()
                    .ok_or_else(|| {
                        invalid_directory_response(DirectoryResponseError::ExpiryOutOfRange)
                    })?;
                if parsed.expires_at_ms < expires_at.timestamp_millis() {
                    return Err(invalid_directory_response(
                        DirectoryResponseError::ExpiryPrecedesInvitation,
                    ));
                }
                debug!(%expires_at, "cloud channel issued invitation");
                (code, None)
            }
            Err(err) => {
                // Cloud unreachable: local mint + mDNS only.
                // Critical: we only fall back for transient/transport
                // errors. Unexpected status codes still surface as
                // Internal so production anomalies stay loud.
                if !is_cloud_recoverable(&err) {
                    return Err(map_create_err(err));
                }
                let code = InvitationCode::new(mint_invitation_code());
                warn!(
                    failure_stage = "directory_transport",
                    "cloud channel unreachable; minted invitation locally — only LAN joiners will resolve",
                );
                (code, Some(anyhow::Error::new(err)))
            }
        };

        // ── LAN channel (best-effort, window-scoped) ───────────────────
        if let Err(local_error) = self
            .start_mdns_publisher(&code, &endpoint_id, full_invitation.as_str(), expires_at)
            .await
        {
            warn!(
                failure_stage = "local_publication",
                directory_available = directory_failure.is_none(),
                "mDNS publisher start failed; LAN joiners will not resolve via this code",
            );
            // If the cloud channel also failed (local-mint fallback), mDNS
            // was the only remaining surface — zero channels initiated. The
            // port contract requires `Ok` only when at least one channel is
            // live, so surface the failure. When `cloud_ok` is true the cloud
            // channel still resolves the code, so the warning above suffices.
            if let Some(source) = directory_failure.take() {
                return Err(map_local_publication_failure(local_error, Some(source)));
            }
        }

        let code_origin = if directory_failure.is_none() {
            CodeOrigin::DirectoryIssued
        } else {
            CodeOrigin::LocallyMintedDirectoryUnreachable
        };
        build_issued_invitation(
            invitation_id,
            code,
            full_invitation,
            expires_at,
            code_origin,
        )
    }

    /// Starts a window-scoped mDNS publisher and stores its handle so
    /// `consume_invitation` can drop it later.
    ///
    /// The full invitation is hex encoded, then
    /// split into ordered bounded TXT attributes by the publisher. This
    /// preserves every dialable address without exceeding DNS-SD's
    /// per-attribute limit.
    async fn start_mdns_publisher(
        &self,
        code: &InvitationCode,
        endpoint_id: &str,
        full_invitation: &str,
        expires_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        #[cfg(test)]
        if self.force_local_publication_failure {
            return Err(anyhow::anyhow!("injected local publication failure"));
        }

        // Sweep stale handles before inserting; a sponsor that has
        // issued multiple codes without consuming them otherwise leaks
        // multicast sockets until process exit.
        self.gc_expired_publishers(Utc::now()).await;

        let ticket_hex = encode_mdns_ticket(full_invitation)?;
        // Pick the iroh endpoint's UDP port for the announce. The LAN IPs
        // we publish come from `if-addrs` inside the publisher, not from
        // iroh's filtered list — that keeps the publisher's "what to put
        // on the wire" identical regardless of which `TransportAddr`
        // variants iroh happens to surface today.
        let port = pick_endpoint_port(&self.endpoint.addr())?;
        let handle = MdnsPairingPublisher::start(
            &RuntimeHandle::current(),
            code.as_str(),
            endpoint_id,
            &ticket_hex,
            expires_at.timestamp_millis(),
            port,
        )?;
        self.publishers
            .lock()
            .await
            .insert(code.clone(), (handle, expires_at));
        Ok(())
    }

    /// Drops publisher handles whose codes have expired. Called lazily
    /// on every issue / consume — sufficient because a sponsor that
    /// stops issuing also stops needing the GC, and a sponsor that
    /// keeps issuing sweeps as a side effect of normal operation.
    async fn gc_expired_publishers(&self, now: DateTime<Utc>) {
        let mut map = self.publishers.lock().await;
        let before = map.len();
        map.retain(|_code, (_handle, exp)| *exp > now);
        let removed = before.saturating_sub(map.len());
        if removed > 0 {
            debug!(
                removed,
                remaining = map.len(),
                "mDNS publisher GC swept expired handles"
            );
        }
    }
}

fn build_issued_invitation(
    invitation_id: uc_core::membership::InvitationId,
    code: InvitationCode,
    full_invitation: uc_core::pairing::invitation::FullInvitation,
    expires_at: DateTime<Utc>,
    code_origin: CodeOrigin,
) -> Result<IssuedInvitation, InvitationError> {
    Ok(IssuedInvitation {
        invitation_id,
        code,
        full_invitation,
        expires_at,
        code_origin,
    })
}

#[derive(Debug, thiserror::Error)]
enum DirectoryResponseError {
    #[error("directory response body could not be parsed")]
    Parse {
        #[source]
        source: RendezvousHttpError,
    },
    #[error("directory response expiry is out of range")]
    ExpiryOutOfRange,
    #[error("directory response expiry precedes the invitation expiry")]
    ExpiryPrecedesInvitation,
}

impl DirectoryResponseError {
    fn diagnostic_reason(&self) -> &'static str {
        match self {
            Self::Parse { .. } => "body_parse_failed",
            Self::ExpiryOutOfRange => "expiry_out_of_range",
            Self::ExpiryPrecedesInvitation => "expiry_precedes_invitation",
        }
    }
}

fn invalid_directory_response(source: DirectoryResponseError) -> InvitationError {
    warn!(
        failure_stage = "directory_response",
        failure_reason = source.diagnostic_reason(),
        "rendezvous create response failed validation"
    );
    InvitationError::DirectoryInvalidResponse {
        source: anyhow::Error::new(source),
    }
}

fn mint_invitation_id() -> uc_core::membership::InvitationId {
    loop {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        if let Some(invitation_id) = uc_core::membership::InvitationId::from_bytes(bytes) {
            return invitation_id;
        }
    }
}

/// Encode the same full invitation used by cloud discovery for bounded mDNS publishing.
fn encode_mdns_ticket(full_invitation: &str) -> anyhow::Result<String> {
    if full_invitation.is_empty() {
        return Err(anyhow::anyhow!("full invitation is empty"));
    }
    Ok(hex::encode(full_invitation.as_bytes()))
}

/// Cloud-side errors we treat as "try LAN-only instead." Transport
/// failures and 5xx responses qualify. A malformed success response does
/// not: the directory may already have accepted the ticket, so minting a
/// different local code could advertise inconsistent credentials.
fn is_cloud_recoverable(err: &RendezvousHttpError) -> bool {
    matches!(
        err,
        RendezvousHttpError::Transport { .. } | RendezvousHttpError::ServiceUnavailable(_)
    )
}

/// Extract the first IP-bound port the endpoint surfaces. Used by the
/// mDNS publisher as the announced service port — joiners will connect
/// to whatever port maps to whichever IP they pick.
///
/// Real iroh endpoints always have at least one IP `TransportAddr`
/// online by the time we're issuing invitations, so the `None` case is
/// a defensive guard for tests / very early init.
fn pick_endpoint_port(addr: &EndpointAddr) -> anyhow::Result<u16> {
    addr.addrs
        .iter()
        .find_map(|a| match a {
            TransportAddr::Ip(sa) => Some(sa.port()),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("endpoint exposes no IP transport addresses"))
}

fn serialize_filtered_endpoint_ticket(
    addr: EndpointAddr,
    allow_overlay: bool,
) -> Result<(String, String), InvitationError> {
    let addr = filter_endpoint_addr(addr, allow_overlay);
    if addr.addrs.is_empty() {
        return Err(InvitationError::NoPublishableAddress {
            source: anyhow::Error::msg("endpoint has no address after product filtering"),
        });
    }
    serialize_endpoint_addr(addr)
}

/// Build a ticket that only carries the single address matching
/// `selected_ip`.
///
/// **Ordering matters**: the product address filter runs *first*, so an
/// IP that the filter drops (overlay-network rules with
/// `allow_overlay=false`, link-local, Clash fake-ip 198.18.0.0/15) will
/// surface as `AddressNotAvailable` rather than slipping into the ticket.
/// This is intentional — the dev tool reuses the product filter so
/// observing "what gets published if we pick this IP" stays aligned with
/// the runtime that production peers see.
fn serialize_endpoint_ticket_for_ip(
    addr: EndpointAddr,
    allow_overlay: bool,
    selected_ip: IpAddr,
) -> Result<(String, String), InvitationError> {
    let addr = filter_endpoint_addr(addr, allow_overlay);
    let EndpointAddr { id, addrs } = addr;
    let selected: Vec<TransportAddr> = addrs
        .into_iter()
        .filter(|addr| match addr {
            TransportAddr::Ip(socket) => socket.ip() == selected_ip,
            _ => false,
        })
        .collect();
    if selected.is_empty() {
        return Err(InvitationError::AddressNotAvailable(selected_ip));
    }
    serialize_endpoint_addr(EndpointAddr::from_parts(id, selected))
}

fn list_invitation_address_candidates(
    addr: EndpointAddr,
    allow_overlay: bool,
) -> Result<Vec<PairingInvitationAddressCandidate>, InvitationError> {
    let addr = filter_endpoint_addr(addr, allow_overlay);
    let candidates: Vec<PairingInvitationAddressCandidate> = addr
        .ip_addrs()
        .map(|socket| PairingInvitationAddressCandidate {
            ip: socket.ip(),
            port: socket.port(),
        })
        .collect();
    if candidates.is_empty() {
        return Err(InvitationError::NetworkNotStarted);
    }
    Ok(candidates)
}

fn serialize_endpoint_addr(addr: EndpointAddr) -> Result<(String, String), InvitationError> {
    let endpoint_id = addr.id.to_string();
    let ticket = serde_json::to_string(&addr)
        .map_err(|err| InvitationError::Internal(format!("endpoint addr serialize: {err}")))?;
    Ok((endpoint_id, ticket))
}

#[async_trait]
impl PairingInvitationPort for RendezvousPairingInvitationAdapter {
    #[instrument(skip_all)]
    async fn issue_invitation(&self) -> Result<IssuedInvitation, InvitationError> {
        let settings = self.load_settings().await?;
        let (endpoint_id, ticket) =
            self.serialize_ticket(settings.network.allow_overlay_network_addrs)?;
        self.create_pairing_with_ticket(settings, endpoint_id, ticket)
            .await
    }

    #[instrument(skip_all)]
    async fn consume_invitation(
        &self,
        code: &InvitationCode,
    ) -> Result<(), ConsumeInvitationError> {
        // Stop the LAN announce immediately (deterministic, not TTL-bound).
        // Dropping the handle stops `swarm-discovery`'s actor; this also
        // releases the multicast socket so a follow-up `issue_invitation`
        // can bind fresh. Also sweep any stale entries piggy-backed on
        // this call.
        self.gc_expired_publishers(Utc::now()).await;
        if self.publishers.lock().await.remove(code).is_some() {
            debug!("mDNS publisher stopped for consumed invitation");
        }

        match self.rendezvous.consume_pairing(code.as_str()).await {
            Ok(()) => {
                debug!("cloud channel invitation consumed");
                Ok(())
            }
            Err(err) => Err(map_consume_err(err)),
        }
    }
}

#[async_trait]
impl PairingInvitationAddressQueryPort for RendezvousPairingInvitationAdapter {
    #[instrument(skip_all)]
    async fn list_invitation_addresses(
        &self,
    ) -> Result<Vec<PairingInvitationAddressCandidate>, InvitationError> {
        let settings = self.load_settings().await?;
        list_invitation_address_candidates(
            self.endpoint.addr(),
            settings.network.allow_overlay_network_addrs,
        )
    }
}

#[async_trait]
impl PairingInvitationByAddressPort for RendezvousPairingInvitationAdapter {
    #[instrument(skip_all, fields(selected_ip = %selected_ip))]
    async fn issue_invitation_for_address(
        &self,
        selected_ip: IpAddr,
    ) -> Result<IssuedInvitation, InvitationError> {
        let settings = self.load_settings().await?;
        let (endpoint_id, ticket) = self
            .serialize_ticket_for_ip(settings.network.allow_overlay_network_addrs, selected_ip)?;
        self.create_pairing_with_ticket(settings, endpoint_id, ticket)
            .await
    }
}

// ── Error mappers ───────────────────────────────────────────────────────────

fn map_create_err(err: RendezvousHttpError) -> InvitationError {
    match err {
        // Transport + 5xx both mean "try again later" from the caller's POV.
        RendezvousHttpError::Transport { .. } | RendezvousHttpError::ServiceUnavailable(_) => {
            InvitationError::ServiceUnavailable
        }
        // A create call hitting 404/410/409 would mean the rendezvous API
        // contract broke (create isn't keyed by a code on the client side).
        // Report as Internal so the anomaly is visible in logs.
        RendezvousHttpError::NotFound
        | RendezvousHttpError::Gone
        | RendezvousHttpError::Conflict
        | RendezvousHttpError::Unexpected { .. } => InvitationError::DirectoryRejected {
            source: anyhow::Error::new(err),
        },
        RendezvousHttpError::Parse { .. } => {
            invalid_directory_response(DirectoryResponseError::Parse { source: err })
        }
    }
}

fn map_local_publication_failure(
    local_error: anyhow::Error,
    directory_failure: Option<anyhow::Error>,
) -> InvitationError {
    match directory_failure {
        Some(source) => InvitationError::DirectoryTransportFailed {
            source: source.context("local invitation publication also failed"),
        },
        None => InvitationError::LocalPublicationFailed {
            source: local_error,
        },
    }
}

fn map_consume_err(err: RendezvousHttpError) -> ConsumeInvitationError {
    match err {
        // Server groups `pairing_not_found` + `pairing_expired` under 404,
        // and `pairing_already_consumed` under 409. Caller treats all three
        // as benign (the code's lifecycle is already terminal), so they
        // collapse to NotFound.
        RendezvousHttpError::NotFound
        | RendezvousHttpError::Gone
        | RendezvousHttpError::Conflict => ConsumeInvitationError::NotFound,
        RendezvousHttpError::Transport { .. } | RendezvousHttpError::ServiceUnavailable(_) => {
            ConsumeInvitationError::ServiceUnavailable
        }
        RendezvousHttpError::Unexpected { status, slug } => ConsumeInvitationError::Internal(
            format!("rendezvous rejected consume ({status}, slug={slug})"),
        ),
        RendezvousHttpError::Parse { .. } => {
            ConsumeInvitationError::Internal("rendezvous response parse failed".to_owned())
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing::MdnsPublisherError;
    use std::io::Write;
    use std::net::SocketAddr;
    use std::sync::Mutex as StdMutex;

    use async_trait::async_trait;
    use chrono::DateTime;
    use iroh::{EndpointAddr, SecretKey, TransportAddr};
    use serde_json::json;
    use tracing::Level;
    use uc_core::ids::DeviceId;
    use uc_core::settings::model::Settings;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    mod provider_dependency_evidence;

    #[derive(Clone, Default)]
    struct CapturedWriter(Arc<StdMutex<Vec<u8>>>);

    impl CapturedWriter {
        fn dump(&self) -> String {
            String::from_utf8(self.0.lock().expect("lock captured logs").clone())
                .expect("captured logs should be UTF-8")
        }
    }

    impl Write for CapturedWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("lock captured logs")
                .extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedWriter {
        type Writer = CapturedWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    struct FakeDeviceIdentity(DeviceId);
    impl DeviceIdentityPort for FakeDeviceIdentity {
        fn current_device_id(&self) -> DeviceId {
            self.0.clone()
        }
    }

    struct InMemorySettings {
        inner: StdMutex<Settings>,
    }
    impl InMemorySettings {
        fn with_device_name(name: Option<&str>) -> Arc<Self> {
            let mut s = Settings::default();
            s.general.device_name = name.map(String::from);
            Arc::new(Self {
                inner: StdMutex::new(s),
            })
        }
    }
    #[async_trait]
    impl SettingsPort for InMemorySettings {
        async fn load(&self) -> anyhow::Result<Settings> {
            Ok(self.inner.lock().unwrap().clone())
        }
        async fn save(&self, s: &Settings) -> anyhow::Result<()> {
            *self.inner.lock().unwrap() = s.clone();
            Ok(())
        }
    }

    async fn loopback_endpoint() -> Arc<Endpoint> {
        // Loopback-only endpoint: no relay, no external discovery. That
        // leaves a single direct IPv4 address in `EndpointAddr.addrs`, which
        // is enough to satisfy `serialize_ticket`'s "addrs non-empty" guard.
        let ep = Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .expect("bind loopback endpoint");
        Arc::new(ep)
    }

    fn make_adapter(
        endpoint: Arc<Endpoint>,
        settings: Arc<dyn SettingsPort>,
        base_url: impl Into<String>,
    ) -> RendezvousPairingInvitationAdapter {
        RendezvousPairingInvitationAdapter::new(
            endpoint,
            Arc::new(FakeDeviceIdentity(DeviceId::new("device-a"))),
            settings,
            Arc::new(RendezvousClient::with_base_url(base_url)),
        )
    }

    /// Asserts an invitation came from the local-mint fallback path (cloud
    /// channel unreachable / misbehaving) rather than being adopted from a
    /// server response. Two signals distinguish them:
    ///
    /// 1. 格式：六位数字，分为两个三位组。
    /// 2. TTL — the local path sets `expires_at = now + LOCAL_MINT_TTL`, so
    ///    the value must land in the window bracketed by the call. A
    ///    server-minted expiry would carry the response's own timestamp.
    fn assert_locally_minted(
        issued: &IssuedInvitation,
        before: DateTime<Utc>,
        after: DateTime<Utc>,
    ) {
        let code = issued.code.as_str();
        assert_eq!(code.len(), 7, "local-mint code is XXX-XXX");
        let (left, right) = code.split_once('-').expect("local-mint code has a hyphen");
        assert_eq!(left.len(), 3);
        assert_eq!(right.len(), 3);
        assert!(left
            .bytes()
            .chain(right.bytes())
            .all(|b| b.is_ascii_digit()));
        assert!(
            issued.expires_at >= before + LOCAL_MINT_TTL
                && issued.expires_at <= after + LOCAL_MINT_TTL,
            "expires_at {} outside local-mint window [{}, {}]",
            issued.expires_at,
            before + LOCAL_MINT_TTL,
            after + LOCAL_MINT_TTL,
        );
        // These tests exercise the recoverable-cloud-failure fallback, so the
        // code's provenance must reflect a directory outage (not LAN-only).
        assert_eq!(
            issued.code_origin,
            CodeOrigin::LocallyMintedDirectoryUnreachable,
            "fallback mint should record a directory-unreachable origin"
        );
    }

    // ── issue_invitation ─────────────────────────────────────────────────

    #[test]
    fn serialize_ticket_filters_bad_virtual_addrs_but_keeps_allowed_overlay() {
        let addr = EndpointAddr::from_parts(
            SecretKey::generate().public(),
            [
                TransportAddr::Ip("100.79.191.42:61743".parse::<SocketAddr>().unwrap()),
                TransportAddr::Ip("198.18.0.1:61743".parse::<SocketAddr>().unwrap()),
                TransportAddr::Ip("169.254.1.2:61743".parse::<SocketAddr>().unwrap()),
                TransportAddr::Ip("192.168.31.72:61743".parse::<SocketAddr>().unwrap()),
            ],
        );

        let (_, ticket) = serialize_filtered_endpoint_ticket(addr, true).expect("ticket");
        let decoded: EndpointAddr = serde_json::from_str(&ticket).expect("decode ticket");
        let ips: Vec<String> = decoded
            .ip_addrs()
            .map(|addr| addr.ip().to_string())
            .collect();

        assert!(ips.contains(&"100.79.191.42".to_string()));
        assert!(ips.contains(&"192.168.31.72".to_string()));
        assert!(!ips.contains(&"198.18.0.1".to_string()));
        assert!(!ips.contains(&"169.254.1.2".to_string()));
    }

    #[test]
    fn serialize_ticket_for_selected_ip_keeps_only_that_ip() {
        let selected_ip = "100.79.191.42".parse().unwrap();
        let addr = EndpointAddr::from_parts(
            SecretKey::generate().public(),
            [
                TransportAddr::Ip("100.79.191.42:61743".parse::<SocketAddr>().unwrap()),
                TransportAddr::Ip("192.168.31.72:61743".parse::<SocketAddr>().unwrap()),
            ],
        );

        let (_, ticket) =
            serialize_endpoint_ticket_for_ip(addr, true, selected_ip).expect("ticket");
        let decoded: EndpointAddr = serde_json::from_str(&ticket).expect("decode ticket");
        let sockets: Vec<SocketAddr> = decoded.ip_addrs().copied().collect();

        assert_eq!(
            sockets,
            vec!["100.79.191.42:61743".parse::<SocketAddr>().unwrap()]
        );
    }

    #[test]
    fn serialize_ticket_for_selected_ip_rejects_absent_ip() {
        let selected_ip = "100.79.191.42".parse().unwrap();
        let addr = EndpointAddr::from_parts(
            SecretKey::generate().public(),
            [TransportAddr::Ip(
                "192.168.31.72:61743".parse::<SocketAddr>().unwrap(),
            )],
        );

        let err = serialize_endpoint_ticket_for_ip(addr, true, selected_ip).unwrap_err();
        assert!(matches!(
            err,
            InvitationError::AddressNotAvailable(ip) if ip == selected_ip
        ));
    }

    #[test]
    fn list_invitation_address_candidates_uses_ticket_filter() {
        let addr = EndpointAddr::from_parts(
            SecretKey::generate().public(),
            [
                TransportAddr::Ip("100.79.191.42:61743".parse::<SocketAddr>().unwrap()),
                TransportAddr::Ip("198.18.0.1:61743".parse::<SocketAddr>().unwrap()),
                TransportAddr::Ip("192.168.31.72:61743".parse::<SocketAddr>().unwrap()),
            ],
        );

        let candidates = list_invitation_address_candidates(addr, true).expect("candidates");
        let rendered: Vec<String> = candidates
            .iter()
            .map(|candidate| format!("{}:{}", candidate.ip, candidate.port))
            .collect();

        assert_eq!(
            rendered,
            vec![
                "100.79.191.42:61743".to_string(),
                "192.168.31.72:61743".to_string(),
            ]
        );
    }

    #[test]
    fn full_invitation_round_trips_identity_route_and_expiry() {
        let invitation_id =
            uc_core::membership::InvitationId::from_bytes([0x41; 32]).expect("valid invitation id");
        let route = br#"{"node":"sponsor"}"#;
        let expires_at_ms = 1_800_000_000_000_i64;

        let invitation = crate::space::encode_full_invitation(invitation_id, route, expires_at_ms)
            .expect("full invitation should encode");
        let decoded = crate::space::decode_full_invitation(&invitation, expires_at_ms - 1)
            .expect("full invitation should decode");

        assert!(invitation.as_str().starts_with("ucspace1_"));
        assert_eq!(decoded.invitation_id(), invitation_id);
        assert_eq!(decoded.route(), route);
        assert_eq!(decoded.expires_at_ms(), expires_at_ms);

        let mdns_ticket = encode_mdns_ticket(invitation.as_str()).expect("encode mDNS ticket");
        let mdns_bytes = hex::decode(mdns_ticket).expect("decode mDNS ticket");
        assert_eq!(mdns_bytes, invitation.as_str().as_bytes());
    }

    #[tokio::test]
    async fn issue_invitation_happy_path() {
        let ep = loopback_endpoint().await;
        let expected_sponsor_id = ep.id();
        let server = MockServer::start().await;
        // The directory alias must outlive the opaque full invitation. Give
        // the response a margin so setup time cannot make the two five-minute
        // windows race at millisecond precision.
        let directory_expires_at_ms =
            (Utc::now() + LOCAL_MINT_TTL + chrono::Duration::minutes(1)).timestamp_millis();
        Mock::given(method("POST"))
            .and(path("/v1/pairings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": "ABCD-EFGH",
                "expiresAtMs": directory_expires_at_ms,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let adapter = make_adapter(
            ep,
            InMemorySettings::with_device_name(Some("mac")),
            server.uri(),
        );
        let before = Utc::now();
        let issued = adapter.issue_invitation().await.expect("happy path");
        let after = Utc::now();

        assert_eq!(issued.code.as_str(), "ABCD-EFGH");
        assert!(
            issued.expires_at >= before + LOCAL_MINT_TTL
                && issued.expires_at <= after + LOCAL_MINT_TTL
        );
        let decoded = crate::space::decode_full_invitation(
            &issued.full_invitation,
            issued.expires_at.timestamp_millis() - 1,
        )
        .expect("issued full invitation should decode");
        let (decoded_route, route_invitation_id) =
            crate::network::iroh::space_admission::decode_space_admission_route(decoded.route())
                .expect("decode Sponsor admission route");
        crate::pairing::invitation_resolver::validate_invitation_route(
            issued.full_invitation.as_str(),
        )
        .expect("the joiner resolver should accept the issued route");
        assert_eq!(decoded.invitation_id(), issued.invitation_id);
        assert_eq!(route_invitation_id, Some(issued.invitation_id));
        assert_eq!(decoded_route.id, expected_sponsor_id);
        assert!(!decoded_route.addrs.is_empty());
        assert_eq!(
            decoded.expires_at_ms(),
            issued.expires_at.timestamp_millis()
        );
    }

    #[tokio::test]
    async fn issue_invitation_includes_required_body_fields() {
        use wiremock::matchers::body_partial_json;

        let ep = loopback_endpoint().await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/pairings"))
            .and(body_partial_json(json!({
                "sponsorDeviceId": "device-a",
                "sponsorDeviceName": "mac",
                "codeLength": 6,
                "ttlSecs": 360,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": "ABCD-EFGH",
                "expiresAtMs": 1_900_000_000_000_i64,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let adapter = make_adapter(
            ep,
            InMemorySettings::with_device_name(Some("mac")),
            server.uri(),
        );
        let issued = adapter.issue_invitation().await.expect("body matches");
        let requests = server
            .received_requests()
            .await
            .expect("recorded rendezvous requests");
        let body: serde_json::Value =
            serde_json::from_slice(&requests[0].body).expect("decode request body");
        let published = body["sponsorTicket"]
            .as_str()
            .expect("opaque Sponsor ticket");

        assert_eq!(published, issued.full_invitation.as_str());
    }

    #[tokio::test]
    async fn issue_invitation_falls_back_to_local_mint_on_5xx() {
        // A 5xx from the cloud channel is recoverable (`is_cloud_recoverable`):
        // the sponsor mints a code locally and announces it on LAN instead of
        // failing. This is the first-pair-no-WAN path — the cloud directory is
        // down, but a same-LAN joiner still resolves the code via mDNS.
        let ep = loopback_endpoint().await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/pairings"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let adapter = make_adapter(
            ep,
            InMemorySettings::with_device_name(Some("mac")),
            server.uri(),
        );
        let before = Utc::now();
        let issued = adapter
            .issue_invitation()
            .await
            .expect("5xx is recoverable: falls back to local mint");
        let after = Utc::now();

        assert_locally_minted(&issued, before, after);
    }

    #[tokio::test]
    async fn issue_invitation_classifies_directory_rejection() {
        let ep = loopback_endpoint().await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/pairings"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": { "code": "invalid_request" }
            })))
            .mount(&server)
            .await;

        let adapter = make_adapter(
            ep,
            InMemorySettings::with_device_name(Some("mac")),
            server.uri(),
        );
        let err = adapter.issue_invitation().await.unwrap_err();
        assert!(matches!(err, InvitationError::DirectoryRejected { .. }));
    }

    #[tokio::test]
    async fn issue_invitation_rejects_malformed_success_response() {
        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_ansi(false)
            .with_max_level(Level::WARN)
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);
        let ep = loopback_endpoint().await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/pairings"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
            .mount(&server)
            .await;

        let adapter = make_adapter(
            ep,
            InMemorySettings::with_device_name(Some("mac")),
            server.uri(),
        );
        let error = adapter.issue_invitation().await.expect_err(
            "the server may have registered the ticket, so a different local code is unsafe",
        );

        assert!(matches!(
            error,
            InvitationError::DirectoryInvalidResponse { .. }
        ));
        let logs = writer.dump();
        assert!(logs.contains("failure_stage=\"directory_response\""));
        assert!(logs.contains("failure_reason=\"body_parse_failed\""));
        assert!(!logs.contains("not-json"));
    }

    #[tokio::test]
    async fn issue_invitation_falls_back_to_local_mint_on_transport_failure() {
        let ep = loopback_endpoint().await;
        // Point at a port guaranteed to reject — no server running there.
        // Transport failure is recoverable, so the sponsor mints locally
        // rather than surfacing an error.
        let adapter = make_adapter(
            ep,
            InMemorySettings::with_device_name(Some("mac")),
            "http://127.0.0.1:1",
        );
        let before = Utc::now();
        let issued = adapter
            .issue_invitation()
            .await
            .expect("transport failure is recoverable: falls back to local mint");
        let after = Utc::now();

        assert_locally_minted(&issued, before, after);
    }

    #[tokio::test]
    async fn issue_invitation_classifies_transport_when_local_publication_also_fails() {
        let ep = loopback_endpoint().await;
        let adapter = make_adapter(
            ep,
            InMemorySettings::with_device_name(Some("device")),
            "http://127.0.0.1:1",
        )
        .with_local_publication_failure();

        let error = adapter.issue_invitation().await.unwrap_err();

        assert!(matches!(
            error,
            InvitationError::DirectoryTransportFailed { .. }
        ));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn local_publication_failure_has_its_own_stage() {
        let error = map_local_publication_failure(
            anyhow::Error::new(MdnsPublisherError::SocketBind(
                "private socket and interface detail".to_owned(),
            )),
            None,
        );

        assert!(matches!(
            error,
            InvitationError::LocalPublicationFailed { .. }
        ));
        assert_eq!(error.to_string(), "local invitation publication failed");
        assert_eq!(format!("{error:?}"), "LocalPublicationFailed");
        let source = std::error::Error::source(&error).expect("local publication source");
        assert!(matches!(
            source.downcast_ref::<MdnsPublisherError>(),
            Some(MdnsPublisherError::SocketBind(_))
        ));
    }

    #[test]
    fn no_publishable_address_has_a_stable_private_error() {
        let addr = EndpointAddr::from_parts(
            SecretKey::generate().public(),
            [TransportAddr::Ip(
                "169.254.1.2:61743".parse::<SocketAddr>().unwrap(),
            )],
        );

        let error = serialize_filtered_endpoint_ticket(addr, false).unwrap_err();

        assert!(matches!(
            error,
            InvitationError::NoPublishableAddress { .. }
        ));
        assert_eq!(format!("{error:?}"), "NoPublishableAddress");
        assert!(!error.to_string().contains("169.254"));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[tokio::test]
    async fn issue_invitation_rejects_missing_device_name() {
        let ep = loopback_endpoint().await;
        let server = MockServer::start().await;
        // Even if the server would accept, we should short-circuit before
        // sending — assert no request by leaving no `.mount(&server)`.
        let adapter = make_adapter(ep, InMemorySettings::with_device_name(None), server.uri());
        let err = adapter.issue_invitation().await.unwrap_err();
        let msg = match err {
            InvitationError::Internal(m) => m,
            other => panic!("expected Internal, got {other:?}"),
        };
        assert!(msg.contains("device_name"), "msg was {msg}");
    }

    #[tokio::test]
    async fn issue_invitation_maps_invalid_expires_at_to_internal() {
        let ep = loopback_endpoint().await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/pairings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": "ABCD-EFGH",
                "expiresAtMs": i64::MAX,  // out-of-range for chrono
            })))
            .mount(&server)
            .await;

        let adapter = make_adapter(
            ep,
            InMemorySettings::with_device_name(Some("mac")),
            server.uri(),
        );
        let err = adapter.issue_invitation().await.unwrap_err();
        assert!(matches!(
            err,
            InvitationError::DirectoryInvalidResponse { .. }
        ));
    }

    #[tokio::test]
    async fn issue_invitation_rejects_directory_expiry_before_full_invitation() {
        let ep = loopback_endpoint().await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/pairings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": "ABCD-EFGH",
                "expiresAtMs": 1_i64,
            })))
            .mount(&server)
            .await;

        let adapter = make_adapter(
            ep,
            InMemorySettings::with_device_name(Some("mac")),
            server.uri(),
        );

        let error = adapter
            .issue_invitation()
            .await
            .expect_err("directory alias cannot expire before the full invitation");

        assert!(matches!(
            error,
            InvitationError::DirectoryInvalidResponse { .. }
        ));
    }

    #[test]
    fn invalid_directory_response_preserves_source_and_logs_safe_reason() {
        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_ansi(false)
            .with_max_level(Level::WARN)
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);

        let error = invalid_directory_response(DirectoryResponseError::ExpiryPrecedesInvitation);

        assert!(matches!(
            error,
            InvitationError::DirectoryInvalidResponse { .. }
        ));
        let source = std::error::Error::source(&error).expect("validation source");
        assert_eq!(
            source.to_string(),
            "directory response expiry precedes the invitation expiry"
        );
        let logs = writer.dump();
        assert!(logs.contains("failure_stage=\"directory_response\""));
        assert!(logs.contains("failure_reason=\"expiry_precedes_invitation\""));
        assert!(!logs.contains("invitation expiry precedes"));
    }

    // ── consume_invitation ───────────────────────────────────────────────

    #[tokio::test]
    async fn consume_invitation_happy_path_is_204_ok() {
        use wiremock::matchers::body_partial_json;
        let ep = loopback_endpoint().await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/pairings/consume"))
            .and(body_partial_json(json!({ "code": "ABCD-EFGH" })))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let adapter = make_adapter(
            ep,
            InMemorySettings::with_device_name(Some("mac")),
            server.uri(),
        );
        adapter
            .consume_invitation(&InvitationCode::new("ABCD-EFGH"))
            .await
            .expect("consume happy path");
    }

    #[tokio::test]
    async fn consume_invitation_maps_404_to_not_found() {
        let ep = loopback_endpoint().await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/pairings/consume"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let adapter = make_adapter(
            ep,
            InMemorySettings::with_device_name(Some("mac")),
            server.uri(),
        );
        let err = adapter
            .consume_invitation(&InvitationCode::new("GONE-CODE"))
            .await
            .unwrap_err();
        assert!(matches!(err, ConsumeInvitationError::NotFound));
    }

    #[tokio::test]
    async fn consume_invitation_maps_409_to_not_found() {
        // Server returns 409 `pairing_already_consumed` when a second
        // sponsor client wins the race. Caller treats as benign, so we
        // collapse to NotFound like the existing expired branch.
        let ep = loopback_endpoint().await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/pairings/consume"))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;

        let adapter = make_adapter(
            ep,
            InMemorySettings::with_device_name(Some("mac")),
            server.uri(),
        );
        let err = adapter
            .consume_invitation(&InvitationCode::new("OLD-CODE"))
            .await
            .unwrap_err();
        assert!(matches!(err, ConsumeInvitationError::NotFound));
    }

    #[tokio::test]
    async fn consume_invitation_maps_5xx_to_service_unavailable() {
        let ep = loopback_endpoint().await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/pairings/consume"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let adapter = make_adapter(
            ep,
            InMemorySettings::with_device_name(Some("mac")),
            server.uri(),
        );
        let err = adapter
            .consume_invitation(&InvitationCode::new("BUSY-CODE"))
            .await
            .unwrap_err();
        assert!(matches!(err, ConsumeInvitationError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn consume_invitation_maps_transport_failure_to_service_unavailable() {
        let ep = loopback_endpoint().await;
        let adapter = make_adapter(
            ep,
            InMemorySettings::with_device_name(Some("mac")),
            "http://127.0.0.1:1",
        );
        let err = adapter
            .consume_invitation(&InvitationCode::new("ANY"))
            .await
            .unwrap_err();
        assert!(matches!(err, ConsumeInvitationError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn consume_invitation_maps_other_4xx_to_internal_with_slug() {
        let ep = loopback_endpoint().await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/pairings/consume"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": { "code": "malformed_code" }
            })))
            .mount(&server)
            .await;

        let adapter = make_adapter(
            ep,
            InMemorySettings::with_device_name(Some("mac")),
            server.uri(),
        );
        let err = adapter
            .consume_invitation(&InvitationCode::new("WEIRD"))
            .await
            .unwrap_err();
        let msg = match err {
            ConsumeInvitationError::Internal(m) => m,
            other => panic!("expected Internal, got {other:?}"),
        };
        assert!(msg.contains("malformed_code"), "msg was {msg}");
        assert!(msg.contains("400"));
    }
}
