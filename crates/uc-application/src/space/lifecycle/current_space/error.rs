#[derive(Debug, thiserror::Error)]
pub enum CurrentSpaceIdentityError {
    #[error("current Space identity is unavailable")]
    Unavailable {
        #[source]
        source: Option<anyhow::Error>,
    },

    #[error("current Space identity is inconsistent")]
    Inconsistent {
        #[source]
        source: Option<anyhow::Error>,
    },
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl CurrentSpaceIdentityError {
    pub fn unavailable() -> Self {
        Self::Unavailable { source: None }
    }

    pub fn unavailable_from(source: impl Into<anyhow::Error>) -> Self {
        Self::Unavailable {
            source: Some(source.into()),
        }
    }

    pub fn inconsistent() -> Self {
        Self::Inconsistent { source: None }
    }

    pub fn inconsistent_from(source: impl Into<anyhow::Error>) -> Self {
        Self::Inconsistent {
            source: Some(source.into()),
        }
    }
}
