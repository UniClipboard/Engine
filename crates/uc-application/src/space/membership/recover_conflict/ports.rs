use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberInstanceId, MembershipBranchId, MembershipBranchRecoveryPackageV1,
    MembershipBranchTransitionV1, MembershipConflictId, VersionedMembershipHistory,
};

#[derive(Clone)]
pub struct MembershipBranchRecoveryRequest {
    pub peer_device_id: DeviceId,
    pub conflict_id: MembershipConflictId,
    pub target_branch_id: MembershipBranchId,
    pub recipient_member: MemberInstanceId,
}

#[derive(Clone)]
pub struct MembershipBranchRecoveryCommit {
    pub request: MembershipBranchRecoveryRequest,
    pub external_commit: Vec<u8>,
}

#[derive(thiserror::Error)]
pub enum MembershipBranchRecoveryChannelError {
    #[error("membership branch recovery peer is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
    #[error("membership branch recovery peer rejected the request")]
    Rejected {
        #[source]
        source: anyhow::Error,
    },
    #[error("membership branch recovery response is invalid")]
    Invalid {
        #[source]
        source: anyhow::Error,
    },
}

impl std::fmt::Debug for MembershipBranchRecoveryChannelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable { .. } => "MembershipBranchRecoveryChannelError::Unavailable",
            Self::Rejected { .. } => "MembershipBranchRecoveryChannelError::Rejected",
            Self::Invalid { .. } => "MembershipBranchRecoveryChannelError::Invalid",
        })
    }
}

/// 单个认证 peer 的两阶段恢复信道；不选择 peer、不解释 MLS，也不持久化流程状态。
#[async_trait]
pub trait MembershipBranchRecoveryChannelPort: Send + Sync {
    async fn request_membership_branch_group_info(
        &self,
        request: MembershipBranchRecoveryRequest,
    ) -> Result<Vec<u8>, MembershipBranchRecoveryChannelError>;

    async fn submit_membership_branch_external_commit(
        &self,
        request: MembershipBranchRecoveryCommit,
    ) -> Result<MembershipBranchRecoveryPackageV1, MembershipBranchRecoveryChannelError>;
}

pub struct PreparedMembershipBranchRecoveryRecipient {
    pub external_commit: Vec<u8>,
    pub staged_mls_state: Vec<u8>,
}

#[derive(thiserror::Error)]
pub enum PrepareMembershipBranchRecoveryRecipientError {
    #[error("membership branch recovery recipient is temporarily unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
    #[error("membership branch recovery recipient material is invalid")]
    Invalid {
        #[source]
        source: anyhow::Error,
    },
}

impl std::fmt::Debug for PrepareMembershipBranchRecoveryRecipientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable { .. } => {
                "PrepareMembershipBranchRecoveryRecipientError::Unavailable"
            }
            Self::Invalid { .. } => "PrepareMembershipBranchRecoveryRecipientError::Invalid",
        })
    }
}

/// 只从目标 GroupInfo 生成 recipient staged MLS state；不进行网络或持久化。
#[async_trait]
pub trait PrepareMembershipBranchRecoveryRecipientPort: Send + Sync {
    async fn prepare_membership_branch_recovery_recipient(
        &self,
        group_info: Vec<u8>,
    ) -> Result<
        PreparedMembershipBranchRecoveryRecipient,
        PrepareMembershipBranchRecoveryRecipientError,
    >;
}

#[derive(Clone)]
pub struct PrepareMembershipBranchTransitionInput {
    pub transition_id: [u8; 32],
    pub conflict_id: MembershipConflictId,
    pub target_branch_id: MembershipBranchId,
    pub package: MembershipBranchRecoveryPackageV1,
}

#[derive(Debug, thiserror::Error)]
pub enum PrepareMembershipBranchTransitionError {
    #[error("membership branch transition preparation is temporarily unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
    #[error("membership branch transition preparation failed")]
    Invalid {
        #[source]
        source: anyhow::Error,
    },
}

/// 只生成无外部副作用的 `Prepared` 计划；generation 写入由后续阶段执行。
#[async_trait]
pub trait PrepareMembershipBranchTransitionPort: Send + Sync {
    async fn prepare_membership_branch_transition(
        &self,
        input: PrepareMembershipBranchTransitionInput,
    ) -> Result<MembershipBranchTransitionV1, PrepareMembershipBranchTransitionError>;
}

#[derive(Clone)]
pub struct PrepareMembershipBranchRecoveryMaterialInput {
    pub conflict_id: MembershipConflictId,
    pub target_branch_id: MembershipBranchId,
    pub recipient_member: MemberInstanceId,
    pub target_history: VersionedMembershipHistory,
    pub external_commit: Vec<u8>,
}

pub struct PreparedMembershipBranchRecoveryMaterial {
    pub target_staged_space_material: Vec<u8>,
    pub sealed_mls_recovery_material: Vec<u8>,
    pub encrypted_content_key_catalog: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum PrepareMembershipBranchRecoveryMaterialError {
    #[error("membership branch recovery material is temporarily unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
    #[error("membership branch recovery material is invalid")]
    Invalid {
        #[source]
        source: anyhow::Error,
    },
}

#[async_trait]
pub trait PrepareMembershipBranchRecoveryMaterialPort: Send + Sync {
    async fn export_membership_branch_recovery_group_info(
        &self,
    ) -> Result<Vec<u8>, PrepareMembershipBranchRecoveryMaterialError>;

    async fn prepare_membership_branch_recovery_material(
        &self,
        input: PrepareMembershipBranchRecoveryMaterialInput,
    ) -> Result<
        PreparedMembershipBranchRecoveryMaterial,
        PrepareMembershipBranchRecoveryMaterialError,
    >;

    async fn commit_membership_branch_recovery_material(
        &self,
        target_staged_space_material: Vec<u8>,
    ) -> Result<(), PrepareMembershipBranchRecoveryMaterialError>;
}

#[derive(Clone)]
pub struct BeginMembershipBranchRecoveryInput {
    pub source_device_id: DeviceId,
    pub conflict_id: MembershipConflictId,
    pub target_branch_id: MembershipBranchId,
    pub recipient_member: MemberInstanceId,
}

#[derive(Clone)]
pub struct IssueMembershipBranchRecoveryInput {
    pub source_device_id: DeviceId,
    pub conflict_id: MembershipConflictId,
    pub target_branch_id: MembershipBranchId,
    pub recipient_member: MemberInstanceId,
    pub external_commit: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum IssueMembershipBranchRecoveryError {
    #[error("membership branch recovery request is rejected")]
    Rejected {
        #[source]
        source: anyhow::Error,
    },
    #[error("membership branch recovery issuer is temporarily unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
    #[error("membership branch recovery issuer state is corrupt")]
    Corrupt {
        #[source]
        source: anyhow::Error,
    },
}

#[async_trait]
pub trait IssueMembershipBranchRecoveryPort: Send + Sync {
    async fn begin_membership_branch_recovery(
        &self,
        input: BeginMembershipBranchRecoveryInput,
    ) -> Result<Vec<u8>, IssueMembershipBranchRecoveryError>;

    async fn issue_membership_branch_recovery(
        &self,
        input: IssueMembershipBranchRecoveryInput,
    ) -> Result<MembershipBranchRecoveryPackageV1, IssueMembershipBranchRecoveryError>;
}
