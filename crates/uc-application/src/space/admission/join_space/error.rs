#[derive(Debug, thiserror::Error)]
pub enum JoinSpaceError {
    #[error("device name is required")]
    DeviceNameRequired,

    #[error("failed to save device name")]
    Settings(#[source] anyhow::Error),

    #[error("space admission is locked")]
    Locked,

    #[error("space admission state changed")]
    StateChanged,

    #[error("space admission requires recovery")]
    RecoveryRequired,

    #[error("space admission is unavailable")]
    Unavailable,

    #[error("the invitation cannot start a new admission")]
    InvalidInvitation,

    #[error("the previous local join cannot be superseded")]
    PreviousJoinCannotBeSuperseded {
        #[source]
        source: Option<anyhow::Error>,
    },

    #[error("the generated join material is invalid")]
    InvalidStartMaterial {
        #[source]
        source: Option<anyhow::Error>,
    },
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl JoinSpaceError {
    pub fn previous_join_cannot_be_superseded() -> Self {
        Self::PreviousJoinCannotBeSuperseded { source: None }
    }

    pub fn previous_join_cannot_be_superseded_from(source: impl Into<anyhow::Error>) -> Self {
        Self::PreviousJoinCannotBeSuperseded {
            source: Some(source.into()),
        }
    }

    pub fn invalid_start_material() -> Self {
        Self::InvalidStartMaterial { source: None }
    }

    pub fn invalid_start_material_from(source: impl Into<anyhow::Error>) -> Self {
        Self::InvalidStartMaterial {
            source: Some(source.into()),
        }
    }
}
