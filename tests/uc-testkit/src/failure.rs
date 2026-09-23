use std::fmt;

use serde::Serialize;
use thiserror::Error;

use crate::ScenarioEvent;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    ProductInvariant,
    ProductTimeout,
    EnvironmentUnavailable,
    ResourceCollision,
    DriverProtocol,
    FixtureInvalid,
    FrameworkArtifact,
    CleanupFailed,
}

impl fmt::Display for FailureKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::ProductInvariant => "product_invariant",
            Self::ProductTimeout => "product_timeout",
            Self::EnvironmentUnavailable => "environment_unavailable",
            Self::ResourceCollision => "resource_collision",
            Self::DriverProtocol => "driver_protocol",
            Self::FixtureInvalid => "fixture_invalid",
            Self::FrameworkArtifact => "framework_artifact",
            Self::CleanupFailed => "cleanup_failed",
        };
        formatter.write_str(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FailureReport {
    pub kind: FailureKind,
    pub condition: Option<String>,
    pub secondary: Vec<FailureKind>,
}

#[derive(Clone, Debug, Error)]
#[error("{kind}: {condition}")]
pub struct ScenarioFailure {
    kind: FailureKind,
    condition: String,
    last_event: Option<ScenarioEvent>,
    secondary: Vec<FailureKind>,
}

impl ScenarioFailure {
    pub fn new(kind: FailureKind, condition: &'static str) -> Self {
        Self {
            kind,
            condition: condition.to_owned(),
            last_event: None,
            secondary: Vec::new(),
        }
    }

    pub(crate) fn with_last_event(mut self, event: Option<ScenarioEvent>) -> Self {
        self.last_event = event;
        self
    }

    pub(crate) fn add_secondary(&mut self, kind: FailureKind) {
        if self.secondary.len() < 4 {
            self.secondary.push(kind);
        }
    }

    pub fn kind(&self) -> FailureKind {
        self.kind
    }

    pub fn condition(&self) -> Option<&str> {
        Some(&self.condition)
    }

    pub fn last_event(&self) -> Option<&ScenarioEvent> {
        self.last_event.as_ref()
    }

    pub(crate) fn report(&self) -> FailureReport {
        FailureReport {
            kind: self.kind,
            condition: Some(self.condition.clone()),
            secondary: self.secondary.clone(),
        }
    }
}
