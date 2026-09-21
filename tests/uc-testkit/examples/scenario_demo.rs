use std::{env, process::ExitCode, time::Duration};

use uc_testkit::{FailureKind, Scenario, ScenarioBudget, ScenarioConfig};

const REPRODUCE: &str = "cargo run -p uc-testkit --example scenario_demo -- failure";

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let mode = env::args().nth(1).unwrap_or_else(|| "success".to_owned());
    let artifact_root = env::var_os("UC_TEST_ARTIFACTS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target/test-artifacts/manual"));
    let (name, budget) = match mode.as_str() {
        "success" => ("testkit-success-example", Duration::from_secs(1)),
        "failure" => ("testkit-failure-example", Duration::from_millis(40)),
        _ => return Err(format!("unknown mode: {mode}")),
    };
    let scenario = Scenario::start(ScenarioConfig::new(
        name,
        42,
        ScenarioBudget::new(budget),
        REPRODUCE,
        artifact_root,
    ))
    .map_err(|failure| failure.to_string())?;
    scenario.record_event("demo-started");

    if mode == "success" {
        scenario.record_event("condition-ready");
        scenario
            .wait_for_event("condition-ready", |event| event.kind() == "condition-ready")
            .await
            .map_err(|failure| failure.to_string())?;
        let completion = scenario
            .finish(Ok(()))
            .map_err(|failure| failure.to_string())?;
        println!("success artifact: {}", completion.artifact_dir().display());
        return Ok(());
    }

    let failure = {
        let _stage = scenario.stage("wait-never-ready");
        scenario
            .wait_for_event("never-ready", |_| false)
            .await
            .map(|_| "failure demo unexpectedly satisfied its condition".to_owned())
            .expect_err("failure branch always returns a message or timeout")
    };
    if failure.kind() != FailureKind::DriverProtocol {
        return Err(format!("unexpected failure kind: {}", failure.kind()));
    }

    let completion = scenario
        .finish(Err(failure))
        .expect_err("controlled failure must remain failed");
    println!("failure artifact: {}", completion.artifact_dir().display());
    Ok(())
}
