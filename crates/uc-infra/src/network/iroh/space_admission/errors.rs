use std::time::Duration;
use uc_application::deps::SpaceAdmissionTransportError;

use super::credential::SpaceAdmissionChannelCredentialError;
use super::crypto::ProofError;
use uc_sync_protocol::space_admission::WireError;

impl From<ProofError> for HandlerError {
    fn from(source: ProofError) -> Self {
        Self::AuthenticationProof {
            source: anyhow::Error::new(source),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(super) enum HandlerError {
    #[error("admission message is invalid")]
    Protocol {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("admission authentication was rejected")]
    Authentication {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("admission credential could not be used")]
    Credential(#[source] SpaceAdmissionChannelCredentialError),
    #[error("admission proof was rejected")]
    AuthenticationProof {
        #[source]
        source: anyhow::Error,
    },
    #[error("admission transport failed")]
    Transport {
        #[source]
        source: anyhow::Error,
    },
    #[error("admission peer upgrade required")]
    PeerUpgradeRequired,
    #[error("admission handling failed")]
    Application {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("admission acknowledgement missing")]
    Acknowledgement,
    #[error("admission deadline elapsed")]
    Timeout,
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl HandlerError {
    pub fn protocol() -> Self {
        Self::Protocol { source: None }
    }

    pub fn protocol_from(source: impl Into<anyhow::Error>) -> Self {
        Self::Protocol {
            source: Some(source.into()),
        }
    }

    pub fn authentication() -> Self {
        Self::Authentication { source: None }
    }

    pub fn authentication_from(source: impl Into<anyhow::Error>) -> Self {
        Self::Authentication {
            source: Some(source.into()),
        }
    }

    pub fn application() -> Self {
        Self::Application { source: None }
    }

    pub fn application_from(source: impl Into<anyhow::Error>) -> Self {
        Self::Application {
            source: Some(source.into()),
        }
    }
}

pub(super) fn map_request_wire_error(error: WireError) -> HandlerError {
    match error {
        WireError::UnsupportedLayout => HandlerError::PeerUpgradeRequired,
        other => map_server_wire_error(other),
    }
}

pub(super) fn map_server_wire_error(error: WireError) -> HandlerError {
    match error {
        WireError::Timeout => HandlerError::Timeout,
        WireError::Io(source) => HandlerError::Transport {
            source: anyhow::Error::new(source),
        },
        _ => HandlerError::protocol(),
    }
}

pub(super) fn map_reply_wire_error(error: WireError) -> SpaceAdmissionTransportError {
    match error {
        WireError::UnsupportedLayout => SpaceAdmissionTransportError::PeerUpgradeRequired,
        _ => SpaceAdmissionTransportError::protocol_rejected(),
    }
}

pub(super) const LEGACY_CLOSE_PROTOCOL: u32 = 0x51;
pub(super) const CLOSE_AUTHENTICATION: u32 = 0x52;
pub(super) const CLOSE_BUSY: u32 = 0x53;
pub(super) const CLOSE_PEER_UPGRADE_REQUIRED: u32 = 0x54;
pub(super) const CLOSE_PROTOCOL: u32 = 0x55;

pub(super) fn map_application_close_code(code: u64) -> Option<SpaceAdmissionTransportError> {
    match code {
        code if code == u64::from(CLOSE_PEER_UPGRADE_REQUIRED)
            || code == u64::from(LEGACY_CLOSE_PROTOCOL) =>
        {
            Some(SpaceAdmissionTransportError::PeerUpgradeRequired)
        }
        code if code == u64::from(CLOSE_AUTHENTICATION) => {
            Some(SpaceAdmissionTransportError::authentication_rejected())
        }
        code if code == u64::from(CLOSE_BUSY) => Some(SpaceAdmissionTransportError::deferred()),
        _ => None,
    }
}

/// Joiner 发出初次 hello、尚未收到 `OpaqueResponse` 时的关闭码解释。
///
/// 此时 sponsor 还没有进入凭据与证明校验；所有已发布版本对真实认证失败都关闭为
/// `CLOSE_AUTHENTICATION`。在这个阶段收到 `CLOSE_PROTOCOL`，只可能是 sponsor 读不懂本端
/// hello：a05–a08 按 `InitialHelloV1` 解析当前布局必然失败，且这些版本从未发送过版本不符信号。
pub(super) fn map_initial_hello_close_code(code: u64) -> Option<SpaceAdmissionTransportError> {
    if code == u64::from(CLOSE_PROTOCOL) {
        return Some(SpaceAdmissionTransportError::PeerUpgradeRequired);
    }
    map_application_close_code(code)
}

pub(super) async fn application_close_error(
    connection: &iroh::endpoint::Connection,
) -> Option<SpaceAdmissionTransportError> {
    application_close_code(connection)
        .await
        .and_then(map_application_close_code)
}

pub(super) async fn initial_hello_close_error(
    connection: &iroh::endpoint::Connection,
) -> Option<SpaceAdmissionTransportError> {
    application_close_code(connection)
        .await
        .and_then(map_initial_hello_close_code)
}

async fn application_close_code(connection: &iroh::endpoint::Connection) -> Option<u64> {
    let close_reason = match connection.close_reason() {
        Some(reason) => Some(reason),
        None => tokio::time::timeout(Duration::from_millis(100), connection.closed())
            .await
            .ok(),
    };
    let Some(iroh::endpoint::ConnectionError::ApplicationClosed(close)) = close_reason else {
        return None;
    };
    Some(close.error_code.into_inner())
}
