use crate::{cleanup, report, SCENARIO_VERSION};
use anyhow::{ensure, Context, Result};
use serde_json::json;
use std::{
    env,
    fs::{self, OpenOptions},
    os::fd::AsRawFd,
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncWriteExt;
use tokio::{
    process::Command,
    time::{sleep, timeout},
};
use uc_testkit::{
    CleanupStatus, FailureKind, Scenario, ScenarioBudget, ScenarioConfig, ScenarioFailure,
};

pub async fn run(
    host: &Path,
    output: &Path,
    kind: &str,
    fault: &str,
    acceptance_only: bool,
) -> Result<()> {
    ensure!(cfg!(target_os = "macos"), "macOS required");
    ensure!(
        matches!(
            kind,
            "startup" | "disconnected" | "connected" | "text" | "smoke"
        ),
        "unknown scenario"
    );
    ensure!(
        matches!(
            fault,
            "none" | "timeout" | "host-crash" | "collector-crash" | "controller-interrupt"
        ),
        "unknown fault"
    );
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(env::temp_dir().join("uc-benchmark.lock"))?;
    // 文件描述符持有期间保留排他锁，进程退出时由内核释放。
    ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "benchmark already running"
    );
    let recovered = cleanup::recover()?;
    if !acceptance_only {
        ensure!(
            report::competing_builds()? == 0,
            "competing build or simulator"
        );
    }
    fs::create_dir(output)?;
    report::write(
        &output.join("environment.json"),
        &report::environment(host)?,
    )?;
    let mut scenario = Scenario::start(ScenarioConfig::new(
        "engine-benchmark",
        374,
        ScenarioBudget::new(Duration::from_secs(900)),
        "uc-benchmark run --host HOST --output NEW_DIRECTORY",
        output,
    ))?;
    let root = scenario.temp_dir("profiles")?;
    cleanup::register(root.path(), None)?;
    let mut command = Command::new(env::current_exe()?);
    command
        .args(["worker", "--host"])
        .arg(host)
        .arg("--output")
        .arg(output)
        .arg("--root")
        .arg(root.path())
        .args(["--scenario", kind, "--fault", fault]);
    command
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut worker = command.spawn()?;
    let pid = worker.id().context("worker pid missing")? as i32;
    cleanup::register(root.path(), Some(pid))?;
    let mut parent_pipe = worker.stdin.take().context("worker control pipe missing")?;
    parent_pipe.write_all(b"S").await?;
    let outcome = tokio::select! {
        value = timeout(Duration::from_secs(if fault == "timeout" {2} else {900}),worker.wait()) => match value {Ok(Ok(status)) if status.success()=>"passed", Err(_)=>"timeout", _=>"failed"},
        _ = tokio::signal::ctrl_c() => "interrupted",
        _ = terminate.recv() => "interrupted",
        _ = async { if fault == "controller-interrupt" { sleep(Duration::from_secs(2)).await; } else { std::future::pending::<()>().await; } } => "controller-interrupt",
    };
    // 只终止本次新建的进程组，不按进程名清理。
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    sleep(Duration::from_millis(300)).await;
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    let waited = matches!(
        timeout(Duration::from_secs(10), worker.wait()).await,
        Ok(Ok(_))
    );
    let mut group_gone = false;
    for _ in 0..100 {
        if unsafe { libc::kill(-pid, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            group_gone = true;
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    scenario.record_external_resource(
        "process-group",
        "benchmark",
        if waited && group_gone {
            CleanupStatus::Completed
        } else {
            CleanupStatus::Failed
        },
    );
    drop(root);
    let work = report::read(&output.join("work.json")).unwrap_or(json!({"passed":false}));
    if matches!(outcome, "timeout" | "controller-interrupt") && fault == outcome {
        report::write(&output.join("fault.json"), &json!({"injected":fault}))?;
    }
    let metrics = if outcome == "passed" && work["passed"] == true {
        report::summarize(output).ok()
    } else {
        None
    };
    let valid = outcome == "passed" && work["passed"] == true && metrics.is_some();
    let completion = scenario.finish(if valid {
        Ok(())
    } else {
        Err(ScenarioFailure::new(
            FailureKind::DriverProtocol,
            "benchmark-failed",
        ))
    });
    let cleaned = match &completion {
        Ok(value) => value.report().cleanup == CleanupStatus::Completed,
        Err(value) => value.report().cleanup == CleanupStatus::Completed,
    };
    if cleaned {
        cleanup::complete()?;
    }
    if let Some(value) = &metrics {
        report::write(&output.join("metrics.json"), value)?;
    }
    let passed = outcome == "passed" && cleaned && work["passed"] == true && metrics.is_some();
    report::write(
        &output.join("result.json"),
        &json!({"schema_version":1,"scenario_version":SCENARIO_VERSION,"scenario":kind,"acceptance_only":acceptance_only,"recovered_stale_lease":recovered,"supervisor_outcome":outcome,"passed":passed,"cleanup":if cleaned {"completed"} else {"failed"},"work":work}),
    )?;
    report::manifest(output)?;
    ensure!(passed, "scenario failed");
    Ok(())
}
