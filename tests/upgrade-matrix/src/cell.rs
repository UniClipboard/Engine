//! 单元生命周期：持有 `uc_testkit::Scenario`，记录阶段、宿主身份与首次失败，结束时写出
//! `result.json`、`summary.txt` 与矩阵专属的 `cell.json`，并与期望登记比对。

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{Map, Value, json};
use uc_testkit::{
    FailureKind, Scenario, ScenarioBudget, ScenarioConfig, ScenarioFailure, StageGuard,
    TempDirLease,
};

use crate::{
    catalog::{self, CellSpec, Expected, Point},
    host::StderrTail,
    interop::{JOIN_REJECTED, JOIN_REJECTED_FACT},
    rendezvous,
};

/// 单元总预算：短于 nextest 期限，给工件写入与清理留出时间。
const PAIR_BUDGET: Duration = Duration::from_secs(900);
const CHAIN_BUDGET: Duration = Duration::from_secs(3_300);

pub(crate) struct CellRun {
    scenario: Scenario,
    stage: &'static str,
    rendezvous: String,
    hosts: Vec<Value>,
    facts: Map<String, Value>,
    stderr: BTreeMap<&'static str, StderrTail>,
    profiles: Vec<(&'static str, TempDirLease)>,
    call_errors: CallErrors,
}

/// 设备公开操作返回的错误：命令名与 Engine 错误码，写入 `cell.json` 供判定。
pub(crate) type CallErrors = Arc<Mutex<Vec<Value>>>;

/// 失败时从每台设备资料目录保留的 Engine 日志尾部上限。
const ENGINE_LOG_TAIL_BYTES: u64 = 512 * 1024;

impl CellRun {
    pub(crate) fn stage(&mut self, name: &'static str) -> StageGuard {
        self.stage = name;
        self.scenario.stage(name)
    }

    /// 失败后继续收集附加证据时，把单元结论的所在阶段恢复为首次失败的阶段。
    pub(crate) fn restore_failed_stage(&mut self, name: &'static str) {
        self.stage = name;
    }

    pub(crate) fn record_event(&self, kind: &'static str) {
        self.scenario.record_event(kind);
    }

    /// 设备资料目录由单元持有：跨版本进程复用，失败时先取出 Engine 日志再清理。
    pub(crate) fn profile_dir(&mut self, label: &'static str) -> Result<PathBuf, ScenarioFailure> {
        let lease = self.scenario.temp_dir(label)?;
        let path = lease.path().to_path_buf();
        self.profiles.push((label, lease));
        Ok(path)
    }

    fn engine_logs(&self) -> Vec<(String, Vec<u8>)> {
        let mut logs = Vec::new();
        for (label, lease) in &self.profiles {
            let Ok(entries) = fs::read_dir(lease.path().join("logs")) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if let Ok(bytes) = read_tail(&path) {
                    logs.push((format!("engine-{label}-{name}"), bytes));
                }
            }
        }
        logs
    }

    pub(crate) fn rendezvous(&self) -> &str {
        &self.rendezvous
    }

    pub(crate) fn call_errors(&self) -> CallErrors {
        Arc::clone(&self.call_errors)
    }

    pub(crate) fn stderr_tail(&mut self, label: &'static str) -> StderrTail {
        Arc::clone(
            self.stderr
                .entry(label)
                .or_insert_with(|| Arc::new(Mutex::new(Vec::new()))),
        )
    }

    /// 记录宿主身份并核对它与版本点一致，防止用错二进制。
    pub(crate) fn record_host(
        &mut self,
        label: &'static str,
        point: &Point,
        reply: &Value,
    ) -> Result<(), ScenarioFailure> {
        let host = &reply["host"];
        self.hosts.push(json!({
            "device": label,
            "point": point.id,
            "anchor": host["anchor"],
            "engine_rev": host["engine_rev"],
            "capabilities": host["capabilities"],
            "startup": reply["startup"],
            "refused": reply["error"],
            "refused_code": reply["code"],
        }));
        if host["anchor"] != point.id.as_str()
            || host["engine_rev"].as_str() != point.engine_rev.as_deref()
        {
            return Err(ScenarioFailure::new(
                FailureKind::FixtureInvalid,
                "host-anchor-mismatch",
            ));
        }
        Ok(())
    }

    pub(crate) fn fact(&mut self, key: &str, value: Value) {
        self.facts.insert(key.to_owned(), value);
    }
}

fn read_tail(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path)?;
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(
        length.saturating_sub(ENGINE_LOG_TAIL_BYTES),
    ))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn artifact_root() -> PathBuf {
    std::env::var_os("UC_TEST_ARTIFACTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-artifacts")
        })
        .join("upgrade-matrix")
}

fn stable_seed(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Actual {
    Pass,
    Fail {
        stage: String,
        kind: FailureKind,
        condition: String,
    },
    Skip {
        condition: String,
    },
}

fn classify(result: &Result<(), (ScenarioFailure, &'static str)>) -> Actual {
    match result {
        Ok(()) => Actual::Pass,
        Err((failure, _)) if failure.kind() == FailureKind::EnvironmentUnavailable => {
            Actual::Skip {
                condition: failure.condition().unwrap_or_default().to_owned(),
            }
        }
        Err((failure, stage)) => Actual::Fail {
            stage: (*stage).to_owned(),
            kind: failure.kind(),
            condition: failure.condition().unwrap_or_default().to_owned(),
        },
    }
}

/// 实际结果与登记是否一致；登记为“已知不兼容”却通过同样不一致。
fn matches(expected: &Expected, actual: &Actual, facts: &Map<String, Value>) -> bool {
    match (expected, actual) {
        (
            Expected::Pass {
                excluded_points, ..
            },
            Actual::Pass,
        ) => {
            let skipped: Vec<String> = facts
                .get("skipped_points")
                .and_then(Value::as_array)
                .map(|points| {
                    points
                        .iter()
                        .filter_map(|p| p.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            &skipped == excluded_points
        }
        (
            Expected::KnownIncompatible {
                stage, condition, ..
            },
            Actual::Fail {
                stage: s,
                condition: c,
                ..
            },
        ) => stage == s && condition == c,
        (
            Expected::Rejected {
                stage,
                rejection_reason,
                ..
            },
            Actual::Fail {
                stage: s,
                condition: c,
                ..
            },
        ) => stage == s && c == JOIN_REJECTED && rejected_with(facts, rejection_reason),
        (Expected::Skip { condition, .. }, Actual::Skip { condition: c }) => condition == c,
        _ => false,
    }
}

/// 加入方记录的公开加入状态为 `rejected`，且原因与登记一致。
fn rejected_with(facts: &Map<String, Value>, rejection_reason: &str) -> bool {
    facts.iter().any(|(key, value)| {
        key.starts_with(JOIN_REJECTED_FACT)
            && value["status"] == "rejected"
            && value["reason"] == rejection_reason
    })
}

fn actual_json(actual: &Actual) -> Value {
    match actual {
        Actual::Pass => json!({ "result": "pass" }),
        Actual::Fail {
            stage,
            kind,
            condition,
        } => json!({
            "result": "fail",
            "stage": stage,
            "kind": kind,
            "condition": condition,
        }),
        Actual::Skip { condition } => json!({ "result": "skip", "condition": condition }),
    }
}

fn write_private(path: &Path, bytes: &[u8]) {
    // 工件写入失败不改变单元结论；缺失的 cell.json 会被汇总脚本报告为缺失。
    let _ = fs::write(path, bytes);
}

/// 运行一个矩阵单元并在结果与登记不一致时让测试失败。
pub async fn run_cell(id: &'static str) {
    let spec = match catalog::cell(id) {
        Ok(spec) => spec,
        Err(message) => panic!("matrix cell {id} cannot be prepared: {message}"),
    };
    let name: &'static str = Box::leak(format!("upgrade-{id}").into_boxed_str());
    let reproduce: &'static str = Box::leak(
        format!(
            "bash scripts/testing/run-test-group.sh upgrade-matrix -E 'test(={}::{})'",
            spec.dimension.id(),
            test_name(id)
        )
        .into_boxed_str(),
    );
    let budget = match spec.versions {
        catalog::Versions::Chain(_) => CHAIN_BUDGET,
        catalog::Versions::Pair { .. } | catalog::Versions::Legacy { .. } => PAIR_BUDGET,
    };
    let scenario = match Scenario::start(ScenarioConfig::new(
        name,
        stable_seed(id),
        ScenarioBudget::new(budget),
        reproduce,
        artifact_root(),
    )) {
        Ok(scenario) => scenario,
        Err(failure) => panic!("matrix cell {id} cannot start its scenario: {failure}"),
    };
    let server = rendezvous::start().await;
    let mut run = CellRun {
        scenario,
        stage: "prepare",
        rendezvous: server.uri(),
        hosts: Vec::new(),
        facts: Map::new(),
        stderr: BTreeMap::new(),
        profiles: Vec::new(),
        call_errors: Arc::default(),
    };
    let result = crate::dimensions::run(&mut run, &spec).await;
    let stage = run.stage;
    let result = result.map_err(|failure| (failure, stage));
    let actual = classify(&result);
    let matched = matches(&spec.expected, &actual, &run.facts);
    let engine_logs = if actual == Actual::Pass {
        Vec::new()
    } else {
        run.engine_logs()
    };
    let CellRun {
        scenario,
        hosts,
        facts,
        stderr,
        profiles,
        call_errors,
        ..
    } = run;
    let call_errors = call_errors
        .lock()
        .map(|errors| errors.clone())
        .unwrap_or_default();
    // 资料目录在场景结束前释放，清理结果计入 result.json。
    drop(profiles);
    drop(server);
    let scenario_result = result.map_err(|(failure, _)| failure);
    let (artifact_dir, outcome) = match scenario.finish(scenario_result) {
        Ok(completion) => (completion.artifact_dir().to_path_buf(), "passed"),
        Err(failure) => (failure.artifact_dir().to_path_buf(), "failed"),
    };
    let record = json!({
        "schema_version": 1,
        "cell": spec.id,
        "dimension": spec.dimension.id(),
        "versions": versions_json(&spec),
        "inviter": spec.inviter.map(|inviter| format!("{inviter:?}").to_lowercase()),
        "expected": spec.expected_raw,
        "actual": actual_json(&actual),
        "matches": matched,
        "scenario_outcome": outcome,
        "hosts": hosts,
        "facts": facts,
        "call_errors": call_errors,
    });
    write_private(
        &artifact_dir.join("cell.json"),
        format!("{:#}\n", record).as_bytes(),
    );
    if actual != Actual::Pass {
        for (label, tail) in &stderr {
            if let Ok(bytes) = tail.lock() {
                write_private(
                    &artifact_dir.join(format!("host-{label}.stderr.log")),
                    &bytes,
                );
            }
        }
        for (name, bytes) in &engine_logs {
            write_private(&artifact_dir.join(name), bytes);
        }
    }
    assert!(
        matched,
        "matrix cell {id} does not match its registration: actual {}; artifacts: {}",
        actual_json(&actual),
        artifact_dir.display()
    );
}

fn versions_json(spec: &CellSpec) -> Value {
    match &spec.versions {
        catalog::Versions::Pair { from, to } => json!({ "from": from.id, "to": to.id }),
        catalog::Versions::Legacy { from, to } => {
            json!({ "from": from.id, "from_release": from.desktop_release, "to": to.id })
        }
        catalog::Versions::Chain(points) => {
            json!({ "chain": points.iter().map(|point| point.id.clone()).collect::<Vec<_>>() })
        }
    }
}

fn test_name(id: &str) -> String {
    let rest = id.split_once('-').map_or(id, |(_, rest)| rest);
    let parts: Vec<&str> = rest.split('-').collect();
    if parts.len() >= 2 {
        let mut name = format!("{}_to_{}", parts[0], parts[1]);
        for part in &parts[2..] {
            name.push('_');
            name.push_str(part);
        }
        name
    } else {
        rest.replace('-', "_")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejected_registration() -> Expected {
        Expected::Rejected {
            stage: "pair".into(),
            rejection_reason: "peer_upgrade_required".into(),
            reason: "incompatible admission protocols".into(),
            link: "docs".into(),
        }
    }

    fn join_rejected(stage: &str, condition: &str) -> Actual {
        Actual::Fail {
            stage: stage.into(),
            kind: FailureKind::ProductInvariant,
            condition: condition.into(),
        }
    }

    fn facts(status: &str, reason: &str) -> Map<String, Value> {
        let mut facts = Map::new();
        facts.insert(
            format!("{JOIN_REJECTED_FACT}b"),
            json!({ "status": status, "reason": reason }),
        );
        facts
    }

    #[test]
    fn rejected_registration_requires_the_stage_condition_and_public_reason() {
        let expected = rejected_registration();
        let actual = join_rejected("pair", JOIN_REJECTED);

        assert!(matches(
            &expected,
            &actual,
            &facts("rejected", "peer_upgrade_required")
        ));
        assert!(!matches(
            &expected,
            &actual,
            &facts("rejected", "authentication_rejected")
        ));
        assert!(!matches(
            &expected,
            &actual,
            &facts("terminated", "peer_upgrade_required")
        ));
        assert!(!matches(&expected, &actual, &Map::new()));
        assert!(!matches(
            &expected,
            &join_rejected("continue", JOIN_REJECTED),
            &facts("rejected", "peer_upgrade_required")
        ));
        assert!(!matches(
            &expected,
            &join_rejected("pair", "pairing-not-completed"),
            &facts("rejected", "peer_upgrade_required")
        ));
        assert!(!matches(
            &expected,
            &Actual::Pass,
            &facts("rejected", "peer_upgrade_required")
        ));
    }
}
