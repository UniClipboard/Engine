#[derive(Debug, thiserror::Error)]
pub enum CurrentMemberSignatureError {
    #[error("current member signing state is unavailable")]
    Unavailable {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("current member signing state is invalid")]
    InvalidState {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("current member signing state could not be loaded")]
    Repository(#[source] anyhow::Error),
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl CurrentMemberSignatureError {
    pub fn unavailable() -> Self {
        Self::Unavailable { source: None }
    }

    pub fn unavailable_from(source: impl Into<anyhow::Error>) -> Self {
        Self::Unavailable {
            source: Some(source.into()),
        }
    }

    pub fn invalid_state() -> Self {
        Self::InvalidState { source: None }
    }

    pub fn invalid_state_from(source: impl Into<anyhow::Error>) -> Self {
        Self::InvalidState {
            source: Some(source.into()),
        }
    }
}
