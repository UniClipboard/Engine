use super::id::AdmissionChannelPeerId;

macro_rules! define_redacted_artifact {
    ($name:ident, $max_size:expr) => {
        #[derive(PartialEq, Eq)]
        pub struct $name(Vec<u8>);

        impl $name {
            pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, AdmissionArtifactError> {
                if bytes.is_empty() {
                    return Err(AdmissionArtifactError::Empty);
                }
                if bytes.len() > $max_size {
                    return Err(AdmissionArtifactError::Oversized);
                }
                Ok(Self(bytes))
            }

            pub fn as_bytes(&self) -> &[u8] {
                &self.0
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter
                    .debug_struct(stringify!($name))
                    .field("byte_len", &self.0.len())
                    .finish_non_exhaustive()
            }
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionArtifactError {
    #[error("the admission artifact is empty")]
    Empty,
    #[error("the admission artifact exceeds its protocol limit")]
    Oversized,
}

define_redacted_artifact!(AdmissionKeyPackage, 1024 * 1024);
define_redacted_artifact!(AdmissionIdentitySignature, 64 * 1024);
define_redacted_artifact!(SpaceAdmissionRoute, 64 * 1024);
define_redacted_artifact!(AdmissionContinuationRoute, 64 * 1024);
define_redacted_artifact!(AdmissionSignedMembershipHistory, 4 * 1024 * 1024);
define_redacted_artifact!(AdmissionMlsCommit, 4 * 1024 * 1024);
define_redacted_artifact!(AdmissionMlsWelcome, 4 * 1024 * 1024);
define_redacted_artifact!(AdmissionSealedRecoveryMaterial, 4 * 1024 * 1024);
define_redacted_artifact!(AdmissionSourceSnapshot, 4 * 1024 * 1024);
define_redacted_artifact!(AdmissionEncryptedPasswordEquivalent, 1024 * 1024);
define_redacted_artifact!(AdmissionContinuationCredential, 1024 * 1024);
define_redacted_artifact!(AdmissionInvitationClaim, 1024 * 1024);
define_redacted_artifact!(AdmissionBaseSnapshot, 4 * 1024 * 1024);
define_redacted_artifact!(AdmissionStagedSecurityState, 4 * 1024 * 1024);
define_redacted_artifact!(AdmissionStagedTargetInput, 4 * 1024 * 1024);
define_redacted_artifact!(AdmissionStagedTarget, 4 * 1024 * 1024);
define_redacted_artifact!(AdmissionSealedSecurityState, 4 * 1024 * 1024);
define_redacted_artifact!(AdmissionActivatedSecurityState, 4 * 1024 * 1024);
define_redacted_artifact!(AdmissionSpaceTransition, 4 * 1024 * 1024);
define_redacted_artifact!(AdmissionSpaceTransitionResult, 4 * 1024 * 1024);
define_redacted_artifact!(AdmissionHelperSecurityState, 4 * 1024 * 1024);

#[derive(PartialEq, Eq)]
pub struct AdmissionHelperNonce([u8; 32]);

impl AdmissionHelperNonce {
    pub fn from_bytes(bytes: [u8; 32]) -> Option<Self> {
        if bytes == [0; 32] {
            None
        } else {
            Some(Self(bytes))
        }
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for AdmissionHelperNonce {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionHelperNonce([REDACTED])")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AdmissionPeerBinding {
    local_peer_id: AdmissionChannelPeerId,
    remote_peer_id: AdmissionChannelPeerId,
}

impl AdmissionPeerBinding {
    pub fn new(
        local_peer_id: AdmissionChannelPeerId,
        remote_peer_id: AdmissionChannelPeerId,
    ) -> Option<Self> {
        if local_peer_id == remote_peer_id {
            None
        } else {
            Some(Self {
                local_peer_id,
                remote_peer_id,
            })
        }
    }

    pub const fn local_peer_id(&self) -> AdmissionChannelPeerId {
        self.local_peer_id
    }

    pub const fn remote_peer_id(&self) -> AdmissionChannelPeerId {
        self.remote_peer_id
    }
}

impl std::fmt::Debug for AdmissionPeerBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionPeerBinding([REDACTED])")
    }
}

#[derive(PartialEq, Eq)]
pub struct AdmissionRecoveryPublicKey([u8; 32]);

impl AdmissionRecoveryPublicKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Option<Self> {
        if bytes == [0; 32] {
            None
        } else {
            Some(Self(bytes))
        }
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for AdmissionRecoveryPublicKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionRecoveryPublicKey([REDACTED])")
    }
}
