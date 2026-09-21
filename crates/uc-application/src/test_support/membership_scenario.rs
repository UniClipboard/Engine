use std::path::PathBuf;
use std::time::Duration;

use uc_testkit::{FailureKind, Scenario, ScenarioBudget, ScenarioConfig, ScenarioFailure};

pub(crate) fn scenario(
    name: &'static str,
    seed: u64,
    budget: Duration,
    reproduce: &'static str,
) -> Scenario {
    let artifact_root = PathBuf::from("../../target/test-artifacts/membership-recovery")
        .join(format!("process-{}", std::process::id()));
    Scenario::start(ScenarioConfig::new(
        name,
        seed,
        ScenarioBudget::new(budget),
        reproduce,
        artifact_root,
    ))
    .expect("membership scenario artifact directory should be available")
}

pub(crate) fn require(condition: bool, failure: &'static str) -> Result<(), ScenarioFailure> {
    condition
        .then_some(())
        .ok_or_else(|| ScenarioFailure::new(FailureKind::ProductInvariant, failure))
}

pub(crate) fn finish(scenario: Scenario, result: Result<(), ScenarioFailure>) {
    if let Err(failure) = scenario.finish(result) {
        panic!(
            "{}; artifacts: {}",
            failure,
            failure.artifact_dir().display()
        );
    }
}
