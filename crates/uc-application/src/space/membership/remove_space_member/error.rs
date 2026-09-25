use uc_core::membership::MembershipEventId;

#[derive(Debug, thiserror::Error)]
pub enum RemoveSpaceMemberError {
    #[error("space is locked")]
    Locked,
    #[error("space membership recovery is required")]
    RecoveryRequired {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("the local device is not an active member")]
    LocalMemberRemoved,
    #[error("the target device is not an active member")]
    TargetNotFound,
    #[error("the local device cannot remove itself")]
    SelfTarget,
    #[error("space membership changed")]
    StateChanged,
    #[error("space member removal is unavailable")]
    Unavailable,
    #[error("member removal {change_id} was committed but follow-up is pending")]
    CommittedButPending { change_id: MembershipEventId },
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl RemoveSpaceMemberError {
    pub fn recovery_required() -> Self {
        Self::RecoveryRequired { source: None }
    }

    pub fn recovery_required_from(source: impl Into<anyhow::Error>) -> Self {
        Self::RecoveryRequired {
            source: Some(source.into()),
        }
    }
}
