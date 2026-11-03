//! 基准入口；被测宿主、控制器和资源采样器分别运行。
#[cfg(unix)]
mod cleanup;
mod metrics;
mod report;
mod scenario;
#[cfg(unix)]
mod supervisor;
#[cfg(not(unix))]
mod supervisor {
    pub async fn run(
        _: &std::path::Path,
        _: &std::path::Path,
        _: &str,
        _: &str,
        _: bool,
    ) -> anyhow::Result<()> {
        anyhow::bail!("macOS required")
    }
}
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::{
    io::{self, Write},
    path::PathBuf,
};

const SCENARIO_VERSION: u32 = 2;

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    Environment,
    Run {
        #[arg(long)]
        host: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value = "text")]
        scenario: String,
    },
    Worker {
        #[arg(long)]
        host: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        scenario: String,
        #[arg(long, default_value = "none")]
        fault: String,
    },
    Collect {
        #[arg(long)]
        pid: u32,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        interval_ms: u64,
    },
    Compare {
        #[arg(long)]
        baseline: PathBuf,
        #[arg(long)]
        candidate: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Verify {
        #[arg(long)]
        output: PathBuf,
    },
    SelfTest {
        #[arg(long)]
        host: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        case: String,
    },
}
#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = execute().await {
        let _ = writeln!(io::stderr(), "benchmark failed: {error}");
        std::process::exit(1);
    }
}
async fn execute() -> Result<()> {
    match Args::parse().command {
        Action::Environment => {
            let count = report::competing_builds()?;
            writeln!(
                io::stdout(),
                "{}",
                serde_json::json!({"blocking_workloads":count})
            )?;
            Ok(())
        }
        Action::Run {
            host,
            output,
            scenario,
        } => supervisor::run(&host, &output, &scenario, "none", false).await,
        Action::Worker {
            host,
            output,
            root,
            scenario,
            fault,
        } => scenario::supervised_run(&host, &output, &root, &scenario, &fault).await,
        Action::Collect {
            pid,
            output,
            interval_ms,
        } => metrics::collect(pid, &output, interval_ms).await,
        Action::Verify { output } => report::verify(&output),
        Action::Compare {
            baseline,
            candidate,
            output,
        } => report::compare(&baseline, &candidate, &output),
        Action::SelfTest { host, output, case } => {
            let success_case = matches!(
                case.as_str(),
                "smoke" | "startup" | "disconnected" | "connected" | "full-text"
            );
            let scenario = match case.as_str() {
                "full-text" => "text",
                "startup" => "startup",
                "disconnected" => "disconnected",
                "connected" => "connected",
                _ => "smoke",
            };
            let fault = if success_case { "none" } else { &case };
            let result = supervisor::run(&host, &output, scenario, fault, true).await;
            let evidence = report::read(&output.join("result.json"))?;
            let passed = (result.is_ok() == success_case)
                && evidence["cleanup"] == "completed"
                && (success_case || report::read(&output.join("fault.json"))?["injected"] == case);
            report::write(
                &output.join("acceptance.json"),
                &serde_json::json!({"case":case,"passed":passed}),
            )?;
            anyhow::ensure!(passed, "self test failed");
            Ok(())
        }
    }
}
