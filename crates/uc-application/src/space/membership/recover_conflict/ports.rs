use std::collections::BTreeSet;

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberInstanceId, MembershipBranchId, MembershipBranchRecoveryPackageV1,
    MembershipBranchTransitionV1, MembershipConflictId, VersionedMembershipHistory,
};

#[derive(Clone)]
pub struct FetchMembershipBranchRecoveryInput {
    pub conflict_id: MembershipConflictId,
    pub target_branch_id: MembershipBranchId,
    pub recipient_member: MemberInstanceId,
    pub evidence_peer_device_ids: BTreeSet<DeviceId>,
}

#[derive(Debug, thiserror::Error)]
pub enum FetchMembershipBranchRecoveryError {
    #[error("membership branch recovery is temporarily unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
    #[error("membership branch recovery was rejected")]
    Rejected {
        #[source]
        source: anyhow::Error,
    },
}

#[async_trait]
pub trait FetchMembershipBranchRecoveryPort: Send + Sync {
    async fn fetch_membership_branch_recovery(
        &self,
        input: FetchMembershipBranchRecoveryInput,
    ) -> Result<MembershipBranchRecoveryPackageV1, FetchMembershipBranchRecoveryError>;
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
