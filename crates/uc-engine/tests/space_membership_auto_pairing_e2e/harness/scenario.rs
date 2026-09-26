//! 单场景证据。
//!
//! 每个场景第一行创建一次 [`TestScenario`]。nextest 为每个场景单独起进程，`cargo test` 默认单线程顺序运行，
//! 因此进程内同一时刻只有一个当前场景：测试台经它记录事件、阶段与临时目录，守卫释放时写出
//! `result.json` 与 `summary.txt`。场景 panic 时守卫同样写出工件，失败分类取测试台先记录的原因。

use super::*;

use std::path::PathBuf;
use std::sync::PoisonError;

use uc_testkit::{
    FailureKind, Scenario, ScenarioBudget, ScenarioConfig, ScenarioFailure, StageGuard,
    TempDirLease,
};

/// 单场景总预算；短于 nextest 对该二进制的 600 秒期限，给工件写入和清理留出时间。
const SCENARIO_BUDGET: Duration = Duration::from_secs(540);
const MAX_SCENARIO_NAME_LEN: usize = 64;

struct CurrentScenario {
    scenario: Scenario,
    failure: Option<ScenarioFailure>,
}

static CURRENT: Mutex<Option<CurrentScenario>> = Mutex::new(None);

fn current() -> MutexGuard<'static, Option<CurrentScenario>> {
    CURRENT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 场景守卫：创建时开始记录，释放时写出工件。
pub(crate) struct TestScenario {
    _private: (),
}

impl TestScenario {
    pub(crate) fn start() -> Self {
        let test_name = std::thread::current()
            .name()
            .unwrap_or("unnamed-scenario")
            .to_owned();
        let name: &'static str = Box::leak(scenario_name(&test_name).into_boxed_str());
        let reproduce: &'static str = Box::leak(
            format!(
                "bash scripts/testing/run-test-group.sh membership-e2e -E 'test(={test_name})'"
            )
            .into_boxed_str(),
        );
        let scenario = Scenario::start(ScenarioConfig::new(
            name,
            stable_seed(&test_name),
            ScenarioBudget::new(SCENARIO_BUDGET),
            reproduce,
            artifact_root(),
        ))
        .expect("start membership e2e scenario");
        let mut slot = current();
        assert!(slot.is_none(), "one membership e2e scenario per process");
        *slot = Some(CurrentScenario {
            scenario,
            failure: None,
        });
        Self { _private: () }
    }
}

impl Drop for TestScenario {
    fn drop(&mut self) {
        let Some(current) = current().take() else {
            return;
        };
        let panicking = std::thread::panicking();
        let result = if panicking {
            Err(current.failure.unwrap_or_else(|| {
                ScenarioFailure::new(FailureKind::ProductInvariant, "assertion-failed")
            }))
        } else {
            Ok(())
        };
        match current.scenario.finish(result) {
            Ok(_) => {}
            Err(failure) => {
                let summary = failure.summary().display().to_string();
                if !panicking {
                    panic!(
                        "membership e2e scenario did not finish cleanly: {:?}; summary: {summary}",
                        failure.failure()
                    );
                }
                tracing::error!(summary = %summary, "membership e2e scenario failed");
            }
        }
    }
}

/// 记录一个场景事件；没有当前场景时忽略。
pub(crate) fn record_event(kind: &'static str) {
    if let Some(current) = current().as_ref() {
        current.scenario.record_event(kind);
    }
}

/// 开始一个计时阶段，返回值释放时结束；没有当前场景时不计时。
pub(crate) fn stage(name: &'static str) -> Option<StageGuard> {
    current()
        .as_ref()
        .map(|current| current.scenario.stage(name))
}

/// 由当前场景分配并登记一个临时目录，场景结束时核对其已清理。
pub(crate) fn temp_dir(label: &'static str) -> TempDirLease {
    current()
        .as_mut()
        .expect("scenario must start with TestScenario::start()")
        .scenario
        .temp_dir(label)
        .expect("create scenario temporary directory")
}

/// 等待超过期限时先记录超时原因再失败，使工件保留准确的失败分类；panic 位置指向调用处。
#[track_caller]
pub(crate) fn ensure_before(
    deadline: tokio::time::Instant,
    condition: &'static str,
    message: impl FnOnce() -> String,
) {
    if tokio::time::Instant::now() < deadline {
        return;
    }
    if let Some(current) = current().as_mut() {
        if current.failure.is_none() {
            current.failure = Some(ScenarioFailure::new(FailureKind::ProductTimeout, condition));
        }
    }
    panic!("{}", message());
}

fn artifact_root() -> PathBuf {
    std::env::var_os("UC_TEST_ARTIFACTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-artifacts")
        })
        .join("membership-e2e")
}

/// 测试名转为 testkit 场景名：小写字母、数字与单个连字符，最长 64 字符。完整名称的种子保证工件目录唯一。
fn scenario_name(test_name: &str) -> String {
    let mut name = String::with_capacity(test_name.len());
    for character in test_name.chars() {
        let mapped = if character.is_ascii_alphanumeric() {
            character.to_ascii_lowercase()
        } else {
            '-'
        };
        if mapped == '-' && (name.is_empty() || name.ends_with('-')) {
            continue;
        }
        name.push(mapped);
    }
    name.truncate(MAX_SCENARIO_NAME_LEN);
    let trimmed = name.trim_end_matches('-');
    if trimmed.starts_with(|character: char| character.is_ascii_lowercase()) {
        trimmed.to_owned()
    } else {
        format!("e2e-{trimmed}")
            .chars()
            .take(MAX_SCENARIO_NAME_LEN)
            .collect::<String>()
            .trim_end_matches('-')
            .to_owned()
    }
}

/// FNV-1a，保证同一测试名在不同运行中得到同一种子。
fn stable_seed(test_name: &str) -> u64 {
    test_name.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}
