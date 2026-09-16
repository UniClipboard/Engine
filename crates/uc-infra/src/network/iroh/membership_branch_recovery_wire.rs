use serde::{Deserialize, Serialize};
use uc_core::membership::{
    MemberInstanceId, MembershipBranchId, MembershipBranchRecoveryPackageV1, MembershipConflictId,
};

pub const MEMBERSHIP_BRANCH_RECOVERY_ALPN: &[u8] = b"uniclipboard/membership-branch-recovery/1";
const WIRE_VERSION: u16 = 1;
pub(crate) const MAX_RECOVERY_FRAME_SIZE: usize = 4 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) enum MembershipBranchRecoveryWireMessage {
    RequestGroupInfo {
        version: u16,
        conflict_id: MembershipConflictId,
        target_branch_id: MembershipBranchId,
        recipient_member: MemberInstanceId,
    },
    GroupInfo {
        version: u16,
        group_info: Vec<u8>,
    },
    SubmitExternalCommit {
        version: u16,
        conflict_id: MembershipConflictId,
        target_branch_id: MembershipBranchId,
        recipient_member: MemberInstanceId,
        external_commit: Vec<u8>,
    },
    RecoveryPackage {
        version: u16,
        package: MembershipBranchRecoveryPackageV1,
    },
    Rejected {
        version: u16,
    },
}

impl MembershipBranchRecoveryWireMessage {
    pub(crate) fn request_group_info(
        conflict_id: MembershipConflictId,
        target_branch_id: MembershipBranchId,
        recipient_member: MemberInstanceId,
    ) -> Self {
        Self::RequestGroupInfo {
            version: WIRE_VERSION,
            conflict_id,
            target_branch_id,
            recipient_member,
        }
    }

    pub(crate) fn group_info(group_info: Vec<u8>) -> Self {
        Self::GroupInfo {
            version: WIRE_VERSION,
            group_info,
        }
    }

    pub(crate) fn submit_external_commit(
        conflict_id: MembershipConflictId,
        target_branch_id: MembershipBranchId,
        recipient_member: MemberInstanceId,
        external_commit: Vec<u8>,
    ) -> Self {
        Self::SubmitExternalCommit {
            version: WIRE_VERSION,
            conflict_id,
            target_branch_id,
            recipient_member,
            external_commit,
        }
    }

    pub(crate) fn recovery_package(package: MembershipBranchRecoveryPackageV1) -> Self {
        Self::RecoveryPackage {
            version: WIRE_VERSION,
            package,
        }
    }

    pub(crate) const fn rejected() -> Self {
        Self::Rejected {
            version: WIRE_VERSION,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), MembershipBranchRecoveryWireError> {
        let (version, payload_is_valid) = match self {
            Self::RequestGroupInfo { version, .. } | Self::Rejected { version } => (*version, true),
            Self::GroupInfo {
                version,
                group_info,
            } => (*version, !group_info.is_empty()),
            Self::SubmitExternalCommit {
                version,
                external_commit,
                ..
            } => (*version, !external_commit.is_empty()),
            Self::RecoveryPackage { version, .. } => (*version, true),
        };
        if version != WIRE_VERSION || !payload_is_valid {
            return Err(invalid_wire(anyhow::anyhow!(
                "membership branch recovery frame is invalid"
            )));
        }
        Ok(())
    }
}

impl std::fmt::Debug for MembershipBranchRecoveryWireMessage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::RequestGroupInfo { .. } => "RequestGroupInfo([REDACTED])",
            Self::GroupInfo { .. } => "GroupInfo([REDACTED])",
            Self::SubmitExternalCommit { .. } => "SubmitExternalCommit([REDACTED])",
            Self::RecoveryPackage { .. } => "RecoveryPackage([REDACTED])",
            Self::Rejected { .. } => "Rejected",
        })
    }
}

#[derive(thiserror::Error)]
pub(crate) enum MembershipBranchRecoveryWireError {
    #[error("membership branch recovery frame is invalid")]
    Invalid {
        #[source]
        source: anyhow::Error,
    },
}

impl std::fmt::Debug for MembershipBranchRecoveryWireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MembershipBranchRecoveryWireError::Invalid")
    }
}

pub(crate) fn encode(
    message: &MembershipBranchRecoveryWireMessage,
) -> Result<Vec<u8>, MembershipBranchRecoveryWireError> {
    message.validate()?;
    let encoded =
        postcard::to_stdvec(message).map_err(|source| invalid_wire(anyhow::Error::new(source)))?;
    if encoded.len() > MAX_RECOVERY_FRAME_SIZE {
        return Err(invalid_wire(anyhow::anyhow!(
            "membership branch recovery frame is too large"
        )));
    }
    Ok(encoded)
}

pub(crate) fn decode(
    bytes: &[u8],
) -> Result<MembershipBranchRecoveryWireMessage, MembershipBranchRecoveryWireError> {
    if bytes.is_empty() || bytes.len() > MAX_RECOVERY_FRAME_SIZE {
        return Err(invalid_wire(anyhow::anyhow!(
            "membership branch recovery frame size is invalid"
        )));
    }
    let message: MembershipBranchRecoveryWireMessage =
        postcard::from_bytes(bytes).map_err(|source| invalid_wire(anyhow::Error::new(source)))?;
    message.validate()?;
    Ok(message)
}

fn invalid_wire(source: impl Into<anyhow::Error>) -> MembershipBranchRecoveryWireError {
    MembershipBranchRecoveryWireError::Invalid {
        source: source.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn request_round_trip_redacts_bindings() {
        let message = MembershipBranchRecoveryWireMessage::request_group_info(
            MembershipConflictId::from_bytes([0x11; 32]),
            MembershipBranchId::from_bytes([0x12; 32]),
            MemberInstanceId::from_bytes([0x13; 32]),
        );

        let decoded = decode(&encode(&message).unwrap()).unwrap();

        assert!(matches!(
            decoded,
            MembershipBranchRecoveryWireMessage::RequestGroupInfo { .. }
        ));
        assert_eq!(format!("{message:?}"), "RequestGroupInfo([REDACTED])");
    }

    #[test]
    fn malformed_frame_preserves_decode_source() {
        let error = decode(b"not-a-recovery-frame").unwrap_err();

        assert!(error.source().is_some());
        assert_eq!(
            error.to_string(),
            "membership branch recovery frame is invalid"
        );
        assert_eq!(
            format!("{error:?}"),
            "MembershipBranchRecoveryWireError::Invalid"
        );
    }

    #[test]
    fn external_commit_round_trip_keeps_two_phase_bindings_private() {
        let message = MembershipBranchRecoveryWireMessage::submit_external_commit(
            MembershipConflictId::from_bytes([0x21; 32]),
            MembershipBranchId::from_bytes([0x22; 32]),
            MemberInstanceId::from_bytes([0x23; 32]),
            vec![0x24],
        );

        let decoded = decode(&encode(&message).unwrap()).unwrap();

        assert!(matches!(
            decoded,
            MembershipBranchRecoveryWireMessage::SubmitExternalCommit { .. }
        ));
        assert_eq!(format!("{message:?}"), "SubmitExternalCommit([REDACTED])");
    }
}
