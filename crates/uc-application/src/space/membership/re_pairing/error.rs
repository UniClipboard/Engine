#[derive(Debug, thiserror::Error)]
pub enum RePairingStateError {
    #[error("re-pairing state is unavailable")]
    Unavailable {
        #[source]
        source: Option<anyhow::Error>,
    },

    #[error("re-pairing state is inconsistent")]
    Inconsistent {
        #[source]
        source: Option<anyhow::Error>,
    },
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl RePairingStateError {
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
