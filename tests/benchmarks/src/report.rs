use crate::SCENARIO_VERSION;
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    env,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::Path,
    process::Command,
};

pub fn write(path: &Path, value: &Value) -> Result<()> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    Ok(())
}
pub fn read(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn hash(path: &Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(fs::read(path)?)))
}
fn command(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program).args(args).output()?;
    ensure!(output.status.success(), "fingerprint command failed");
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
pub fn competing_builds() -> Result<usize> {
    let processes = command("ps", &["-axo", "comm="])?;
    let active_processes = processes
        .lines()
        .filter(|line| {
            matches!(
                Path::new(line.trim())
                    .file_name()
                    .and_then(|name| name.to_str()),
                Some(
                    "rustc"
                        | "cargo"
                        | "xcodebuild"
                        | "emulator"
                        | "qemu-system-aarch64-headless"
                        | "qemu-system-x86_64-headless"
                        | "qemu-system-aarch64"
                        | "qemu-system-x86_64"
                )
            )
        })
        .count();
    // 空闲窗口不代表设备在运行；同时检查没有打开窗口的模拟设备。
    let devices: Value = serde_json::from_str(&command(
        "xcrun",
        &["simctl", "list", "devices", "booted", "--json"],
    )?)?;
    let mut booted = 0;
    for runtime in devices["devices"]
        .as_object()
        .context("simulator device list missing")?
        .values()
    {
        booted += runtime
            .as_array()
            .context("simulator runtime list invalid")?
            .iter()
            .filter(|device| device["state"] == "Booted")
            .count();
    }
    Ok(active_processes + booted)
}
pub fn environment(host: &Path) -> Result<Value> {
    Ok(
        json!({"competing_builds":competing_builds()?,"host_sha256":hash(host)?,"harness_sha256":hash(&env::current_exe()?)?,
        "source_commit":command("git",&["rev-parse","HEAD"])?,"dirty":!command("git",&["status","--porcelain"])?.is_empty(),
        "lock_sha256":hash(Path::new("Cargo.lock"))?,"rustc":command("rustc",&["-Vv"])?,
        "os":command("sw_vers",&["-productVersion"])?,"architecture":env::consts::ARCH,
        "cpu":command("sysctl",&["-n","machdep.cpu.brand_string"])?,"logical_cpus":command("sysctl",&["-n","hw.logicalcpu"])?,
        "power":command("pmset",&["-g","batt"])?,"load":command("sysctl",&["-n","vm.loadavg"])?,
        "scenario_version":SCENARIO_VERSION,"cpu_time_format":"mach-timebase-ns-v1","measurement":"connectivity-host-process","dataset_version":1,"seed":374,"network":"local-direct",
        "machine_id":format!("{:x}",Sha256::digest(command("scutil",&["--get","LocalHostName"])?.as_bytes())),
        "host_manifest":read(&host.with_extension("manifest.json")).unwrap_or(Value::Null)}),
    )
}
fn artifact_files(
    root: &Path,
    directory: &Path,
    files: &mut serde_json::Map<String, Value>,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        ensure!(
            !entry.file_type()?.is_symlink(),
            "artifact symlink rejected"
        );
        let path = entry.path();
        if path.is_dir() {
            artifact_files(root, &path, files)?;
        } else if path.is_file() {
            let name = path
                .strip_prefix(root)?
                .to_str()
                .context("artifact name missing")?;
            if matches!(name, "manifest.json" | "acceptance.json") {
                continue;
            }
            files.insert(name.into(), json!(hash(&path)?));
        }
    }
    Ok(())
}
pub fn manifest(output: &Path) -> Result<()> {
    let mut files = serde_json::Map::new();
    artifact_files(output, output, &mut files)?;
    write(
        &output.join("manifest.json"),
        &json!({"schema_version":1,"sha256":files}),
    )
}
pub fn verify(output: &Path) -> Result<()> {
    let manifest = read(&output.join("manifest.json"))?;
    ensure!(manifest["schema_version"] == 1, "unsupported manifest");
    let mut actual = serde_json::Map::new();
    artifact_files(output, output, &mut actual)?;
    ensure!(
        manifest["sha256"] == json!(actual),
        "artifact hash or file set mismatch"
    );
    for required in [
        "result.json",
        "environment.json",
        "work.json",
        "metrics.json",
        "a.jsonl",
        "b.jsonl",
    ] {
        ensure!(actual.contains_key(required), "required artifact missing");
    }
    let result = read(&output.join("result.json"))?;
    // uc-testkit 回执承载清理状态；缺失或未完成都不能算通过。
    let receipt = scenario_receipt(output).unwrap_or(Value::Null);
    ensure!(
        result["passed"] == true
            && result["cleanup"] == "completed"
            && receipt["cleanup"] == "completed",
        "run failed"
    );
    summarize(output)?;
    Ok(())
}
fn scenario_receipt(output: &Path) -> Result<Value> {
    let mut receipts = fs::read_dir(output)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let receipt = entry.path().join("result.json");
            receipt.is_file().then_some(receipt)
        });
    let receipt = receipts.next().context("scenario receipt missing")?;
    ensure!(receipts.next().is_none(), "multiple scenario receipts");
    read(&receipt)
}
pub fn summarize(output: &Path) -> Result<Value> {
    let work = read(&output.join("work.json"))?;
    let mut result = serde_json::Map::new();
    for role in ["a", "b"] {
        let mut samples = Vec::new();
        for line in BufReader::new(fs::File::open(output.join(format!("{role}.jsonl")))?).lines() {
            let line = line?;
            ensure!(
                line.len() <= 4096 && samples.len() < 19000,
                "sample bound exceeded"
            );
            samples.push(serde_json::from_str::<Value>(&line)?);
        }
        ensure!(samples.len() >= 2, "insufficient samples");
        for pair in samples.windows(2) {
            ensure!(
                pair[0]["process_start_abstime"] == pair[1]["process_start_abstime"],
                "process identity changed"
            );
            ensure!(
                pair[1]["monotonic_ns"].as_u64() > pair[0]["monotonic_ns"].as_u64(),
                "sample time not increasing"
            );
            ensure!(
                pair[1]["cpu_time_ns"].as_u64() >= pair[0]["cpu_time_ns"].as_u64(),
                "CPU counter decreased"
            );
        }
        let mut rss: Vec<u64> = samples
            .iter()
            .map(|x| x["resident_bytes"].as_u64().context("RSS missing"))
            .collect::<Result<_>>()?;
        rss.sort();
        let cpu = |index: usize| {
            samples[index]["cpu_time_ns"]
                .as_u64()
                .context("CPU missing")
        };
        let windows = work["windows"]
            .as_array()
            .context("phase windows missing")?;
        let mut phases = serde_json::Map::new();
        for pair in windows.windows(2) {
            let start = pair[0]["monotonic_ns"]
                .as_u64()
                .context("phase start missing")?;
            let end = pair[1]["monotonic_ns"]
                .as_u64()
                .context("phase end missing")?;
            let selected: Vec<&Value> = samples
                .iter()
                .filter(|row| {
                    row["monotonic_ns"]
                        .as_u64()
                        .is_some_and(|t| t >= start && t <= end)
                })
                .collect();
            if selected.len() >= 2 {
                let mut resident: Vec<u64> = selected
                    .iter()
                    .filter_map(|row| row["resident_bytes"].as_u64())
                    .collect();
                resident.sort();
                let delta = selected[selected.len() - 1]["cpu_time_ns"]
                    .as_u64()
                    .context("phase CPU missing")?
                    - selected[0]["cpu_time_ns"]
                        .as_u64()
                        .context("phase CPU missing")?;
                phases.insert(pair[0]["phase"].as_str().context("phase name missing")?.into(),json!({"samples":selected.len(),"median_rss":(resident[(resident.len()-1)/2] as f64 + resident[resident.len()/2] as f64)/2.0,"observed_max_rss":resident.last(),"cpu_delta_ns":delta}));
            }
        }
        let max_gap = samples
            .windows(2)
            .filter_map(|pair| {
                Some(
                    pair[1]["monotonic_ns"]
                        .as_u64()?
                        .saturating_sub(pair[0]["monotonic_ns"].as_u64()?),
                )
            })
            .max();
        result.insert(role.into(),json!({"phases":phases,"max_sample_gap_ns":max_gap,"sample_count":samples.len(),"median_rss":(rss[(rss.len()-1)/2] as f64 + rss[rss.len()/2] as f64)/2.0,"observed_max_rss":rss.last(),"cpu_delta_ns":cpu(samples.len()-1)?.checked_sub(cpu(0)?).context("CPU counter decreased")?,"startup_ns":work[format!("{role}_startup_ns")]}));
    }
    Ok(json!(result))
}
pub fn compare(baseline: &Path, candidate: &Path, output: &Path) -> Result<()> {
    verify(baseline)?;
    verify(candidate)?;
    let a = summarize(baseline)?;
    let b = summarize(candidate)?;
    let ae = read(&baseline.join("environment.json"))?;
    let be = read(&candidate.join("environment.json"))?;
    ensure!(
        ae["cpu_time_format"] == "mach-timebase-ns-v1"
            && be["cpu_time_format"] == "mach-timebase-ns-v1",
        "legacy CPU units require corrected analysis"
    );
    let measurement_eligible = read(&baseline.join("result.json"))?["acceptance_only"] == false
        && read(&candidate.join("result.json"))?["acceptance_only"] == false
        && !ae["host_manifest"].is_null()
        && !be["host_manifest"].is_null();
    for environment in [&ae, &be] {
        if !environment["host_manifest"].is_null() {
            ensure!(
                environment["host_manifest"]["binary_sha256"] == environment["host_sha256"],
                "binary provenance mismatch"
            );
        }
    }
    if measurement_eligible {
        for field in ["host_source_sha256", "features", "rustc", "profile"] {
            ensure!(
                ae["host_manifest"][field] == be["host_manifest"][field],
                "build conditions differ"
            );
        }
    }
    for field in [
        "os",
        "architecture",
        "cpu",
        "logical_cpus",
        "machine_id",
        "dataset_version",
        "scenario_version",
        "network",
        "harness_sha256",
    ] {
        ensure!(ae[field] == be[field], "incompatible environments");
    }
    ensure!(
        read(&baseline.join("result.json"))?["scenario"]
            == read(&candidate.join("result.json"))?["scenario"],
        "scenario mismatch"
    );
    fs::create_dir(output)?;
    write(
        &output.join("comparison.json"),
        &json!({"schema_version":1,"baseline":a,"candidate":b,"threshold_enforced":false,"measurement_eligible":measurement_eligible}),
    )?;
    let mut text=format!("# Benchmark comparison\n\nObserved host process costs. No performance gate.\n\nMeasurement eligible: {measurement_eligible}. Acceptance-only data is not a reference baseline.\n\n| Role | Metric | Baseline | Candidate | Change |\n|---|---|---:|---:|---:|\n");
    for role in ["a", "b"] {
        for key in [
            "median_rss",
            "observed_max_rss",
            "cpu_delta_ns",
            "startup_ns",
        ] {
            let av = a[role][key].as_f64().context("metric missing")?;
            let bv = b[role][key].as_f64().context("metric missing")?;
            text.push_str(&format!(
                "| {role} | {key} | {av:.0} | {bv:.0} | {:.0} |\n",
                bv - av
            ));
        }
    }
    fs::write(output.join("report.md"), text)?;
    Ok(())
}
