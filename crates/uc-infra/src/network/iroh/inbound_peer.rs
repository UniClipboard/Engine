//! 入站对端身份与准入的共同判定；协议出口负责记录实际拒绝。
use std::sync::Arc;

use uc_application::deps::{MembershipLedgerError, PeerIdentityDirectoryPort};
use uc_core::ids::DeviceId;
use uc_core::membership::PeerAdmissionPort;
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::security::IdentityFingerprint;
use uc_observability_contract::diagnostics::connectivity::{
    record_inbound_peer_rejection, InboundPeerProtocol, InboundPeerRejectionReason,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InboundPeerRejection {
    IdentityUnresolved,
    IdentityAmbiguous,
    MemberReadFailed,
    FingerprintUnavailable,
    LedgerDenied,
    LedgerUnavailable,
    NotAccepting,
}

impl InboundPeerRejection {
    pub(crate) fn is_identity_failure(self) -> bool {
        matches!(
            self,
            Self::IdentityUnresolved
                | Self::IdentityAmbiguous
                | Self::MemberReadFailed
                | Self::FingerprintUnavailable
        )
    }

    fn diagnostic(self) -> InboundPeerRejectionReason {
        match self {
            Self::IdentityUnresolved => InboundPeerRejectionReason::IdentityUnresolved,
            Self::IdentityAmbiguous => InboundPeerRejectionReason::IdentityAmbiguous,
            Self::MemberReadFailed => InboundPeerRejectionReason::MemberReadFailed,
            Self::FingerprintUnavailable => InboundPeerRejectionReason::FingerprintUnavailable,
            Self::LedgerDenied => InboundPeerRejectionReason::LedgerDenied,
            Self::LedgerUnavailable => InboundPeerRejectionReason::LedgerUnavailable,
            Self::NotAccepting => InboundPeerRejectionReason::NotAccepting,
        }
    }
}

/// 身份解析失败；读取与派生失败保留原始来源，供需要传播错误的调用方使用。
#[derive(Debug, thiserror::Error)]
pub(crate) enum PeerIdentityError {
    #[error("peer identity is not a known member")]
    Unresolved,
    #[error("peer identity matches more than one member")]
    Ambiguous,
    #[error("member identities are unavailable")]
    MemberRead(#[source] MembershipLedgerError),
    #[error("identity fingerprint derivation failed")]
    Fingerprint(#[source] anyhow::Error),
}

impl PeerIdentityError {
    pub(crate) fn rejection(&self) -> InboundPeerRejection {
        match self {
            Self::Unresolved => InboundPeerRejection::IdentityUnresolved,
            Self::Ambiguous => InboundPeerRejection::IdentityAmbiguous,
            Self::MemberRead(_) => InboundPeerRejection::MemberReadFailed,
            Self::Fingerprint(_) => InboundPeerRejection::FingerprintUnavailable,
        }
    }
}

/// 把远端公钥解析为设备：候选只来自成员状态负责人发布的身份目录，不读取成员读模型。
pub(crate) struct PeerIdentityResolver {
    identities: Arc<dyn PeerIdentityDirectoryPort>,
    fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
}

impl PeerIdentityResolver {
    pub(crate) fn new(
        identities: Arc<dyn PeerIdentityDirectoryPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    ) -> Self {
        Self {
            identities,
            fingerprint_factory,
        }
    }

    pub(crate) async fn identify(
        &self,
        remote_public_key: &[u8; 32],
    ) -> Result<DeviceId, InboundPeerRejection> {
        self.resolve(remote_public_key)
            .await
            .map_err(|error| error.rejection())
    }

    pub(crate) async fn resolve(
        &self,
        remote_public_key: &[u8; 32],
    ) -> Result<DeviceId, PeerIdentityError> {
        let fingerprint = self
            .fingerprint_factory
            .from_public_key(remote_public_key)
            .map_err(PeerIdentityError::Fingerprint)?;
        let identities = self
            .identities
            .known_peer_identities()
            .await
            .map_err(PeerIdentityError::MemberRead)?;
        let mut found = None;
        for identity in identities {
            if identity.identity_fingerprint == fingerprint {
                match &found {
                    Some(device) if device != &identity.device_id => {
                        return Err(PeerIdentityError::Ambiguous);
                    }
                    None => found = Some(identity.device_id),
                    _ => {}
                }
            }
        }
        found.ok_or(PeerIdentityError::Unresolved)
    }

    pub(crate) fn fingerprint_matches(
        &self,
        remote_public_key: &[u8; 32],
        fingerprint: &IdentityFingerprint,
    ) -> bool {
        self.fingerprint_factory
            .from_public_key(remote_public_key)
            .is_ok_and(|derived| derived == *fingerprint)
    }
}

pub(crate) struct InboundPeerGate {
    protocol: InboundPeerProtocol,
    identity: PeerIdentityResolver,
    peer_admission: Arc<dyn PeerAdmissionPort>,
}

impl InboundPeerGate {
    pub(crate) fn new(
        protocol: InboundPeerProtocol,
        identities: Arc<dyn PeerIdentityDirectoryPort>,
        peer_admission: Arc<dyn PeerAdmissionPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    ) -> Self {
        Self {
            protocol,
            identity: PeerIdentityResolver::new(identities, fingerprint_factory),
            peer_admission,
        }
    }

    pub(crate) async fn identify(
        &self,
        remote_public_key: &[u8; 32],
    ) -> Result<DeviceId, InboundPeerRejection> {
        self.identity.identify(remote_public_key).await
    }

    pub(crate) async fn authorize(&self, device: &DeviceId) -> Result<(), InboundPeerRejection> {
        match self.peer_admission.is_admitted(device).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(InboundPeerRejection::LedgerDenied),
            Err(_) => Err(InboundPeerRejection::LedgerUnavailable),
        }
    }

    pub(crate) async fn admit(
        &self,
        remote_public_key: &[u8; 32],
    ) -> Result<DeviceId, InboundPeerRejection> {
        let device = self.identify(remote_public_key).await?;
        self.authorize(&device).await?;
        Ok(device)
    }

    pub(crate) fn record_rejection(&self, rejection: InboundPeerRejection) {
        record_inbound_rejection(self.protocol, rejection);
    }
}

pub(crate) fn record_inbound_rejection(
    protocol: InboundPeerProtocol,
    rejection: InboundPeerRejection,
) {
    record_inbound_peer_rejection(protocol, rejection.diagnostic());
}

/// 测试以内存成员表充当身份目录：候选与成员表中的成员一一对应，读取失败按目录不可用处理。
#[cfg(test)]
pub(crate) async fn identities_from_member_table(
    members: &dyn uc_core::membership::MemberRepositoryPort,
) -> Result<Vec<uc_application::deps::KnownPeerIdentity>, MembershipLedgerError> {
    Ok(members
        .list()
        .await
        .map_err(MembershipLedgerError::unavailable_from)?
        .into_iter()
        .map(|member| uc_application::deps::KnownPeerIdentity {
            device_id: member.device_id,
            identity_fingerprint: member.identity_fingerprint,
        })
        .collect())
}

#[cfg(test)]
pub(crate) fn member_table_directory(
    members: Arc<dyn uc_core::membership::MemberRepositoryPort>,
) -> Arc<dyn PeerIdentityDirectoryPort> {
    struct MemberTableDirectory(Arc<dyn uc_core::membership::MemberRepositoryPort>);

    #[async_trait::async_trait]
    impl PeerIdentityDirectoryPort for MemberTableDirectory {
        async fn known_peer_identities(
            &self,
        ) -> Result<Vec<uc_application::deps::KnownPeerIdentity>, MembershipLedgerError> {
            identities_from_member_table(self.0.as_ref()).await
        }
    }

    Arc::new(MemberTableDirectory(members))
}

/// 让测试用的内存成员表同时充当身份目录。
#[cfg(test)]
macro_rules! member_table_identity_directory {
    ($members:ty) => {
        #[async_trait::async_trait]
        impl uc_application::deps::PeerIdentityDirectoryPort for $members {
            async fn known_peer_identities(
                &self,
            ) -> Result<
                Vec<uc_application::deps::KnownPeerIdentity>,
                uc_application::deps::MembershipLedgerError,
            > {
                // rust-style: allow-qualified-path -- 宏在调用方模块展开，只能以完整路径定位辅助函数。
                crate::network::iroh::inbound_peer::identities_from_member_table(self).await
            }
        }
    };
}
#[cfg(test)]
pub(crate) use member_table_identity_directory;

#[cfg(test)]
mod tests;
