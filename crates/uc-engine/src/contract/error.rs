use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineErrorCategory {
    InvalidInput,
    InvalidState,
    Unauthorized,
    NotFound,
    Conflict,
    Unavailable,
    DeadlineExceeded,
    Internal,
}

impl fmt::Display for EngineErrorCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::InvalidInput => "invalid_input",
            Self::InvalidState => "invalid_state",
            Self::Unauthorized => "unauthorized",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Unavailable => "unavailable",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::Internal => "internal",
        };
        formatter.write_str(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineError {
    code: u32,
    category: EngineErrorCategory,
    retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    admission_recovery: Option<super::AdmissionRecoverySummary>,
}

impl EngineError {
    pub fn new(code: u32, category: EngineErrorCategory, retryable: bool) -> Self {
        Self {
            code,
            category,
            retryable,
            admission_recovery: None,
        }
    }

    pub fn code(&self) -> u32 {
        self.code
    }

    pub fn category(&self) -> EngineErrorCategory {
        self.category
    }

    pub fn is_retryable(&self) -> bool {
        self.retryable
    }

    pub(crate) fn with_admission_recovery(
        mut self,
        summary: super::AdmissionRecoverySummary,
    ) -> Self {
        self.admission_recovery = Some(summary);
        self
    }

    pub(crate) fn admission_recovery(&self) -> Option<super::AdmissionRecoverySummary> {
        self.admission_recovery.clone()
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "engine error {} ({})", self.code, self.category)
    }
}

impl std::error::Error for EngineError {}
