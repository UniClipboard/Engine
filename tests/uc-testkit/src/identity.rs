use serde::Serialize;

use crate::{FailureKind, ScenarioFailure};

const MAX_SCENARIO_NAME_LEN: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScenarioIdentity {
    name: String,
    seed: u64,
    artifact_id: String,
}

impl ScenarioIdentity {
    pub fn new(name: &str, seed: u64) -> Result<Self, ScenarioFailure> {
        if !valid_name(name) {
            return Err(ScenarioFailure::new(
                FailureKind::FixtureInvalid,
                "scenario-name-invalid",
            ));
        }

        Ok(Self {
            name: name.to_owned(),
            seed,
            artifact_id: format!("{name}-seed-{seed:016x}"),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn artifact_id(&self) -> &str {
        &self.artifact_id
    }
}

fn valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_SCENARIO_NAME_LEN {
        return false;
    }

    let mut previous_hyphen = false;
    for (index, byte) in name.bytes().enumerate() {
        let valid = byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-';
        if !valid || (index == 0 && !byte.is_ascii_lowercase()) {
            return false;
        }
        if byte == b'-' && previous_hyphen {
            return false;
        }
        previous_hyphen = byte == b'-';
    }

    !previous_hyphen
}
