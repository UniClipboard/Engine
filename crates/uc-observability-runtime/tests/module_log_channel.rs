//! 模块日志通道的端到端验收：真实订阅者 → 本地日志文件 → 诊断导出。
//!
//! 预先定义的失败方式（任一发生即测试失败）：
//! 1. 错误链把未登记层（anyhow context、第三方错误）的正文写进文件，或丢掉已登记/结构化层；
//! 2. 自由文本字段（路径、设备名、对端标识等）未经审定就落盘；
//! 3. Detailed 窗口外记录 DEBUG，或窗口内漏记、窗口结束后仍记；
//! 4. 热记录点不限速，或限速丢弃没有计数、没有在下一条放行记录里给出 `suppressed`；
//! 5. 超长记录被静默丢弃而不是裁剪并计数；
//! 6. 超出字节预算后继续写入，或丢弃没有计数；
//! 7. 导出报告的计数与文件里的实际行数不符；
//! 8. 记录出现绝对路径或临时目录。
//!
//! 运行期只能安装一次，所以所有阶段放在同一个测试里依次执行。
// 本文件故意直接使用 tracing 日志宏：它验证运行期对未登记字段、内插消息和非 `uc_*` target 的处理，
// `uc_*!` 宏在编译期就拒绝这些写法，无法构造这些输入。clippy 只认 crate 级 allow（ADR-030）。
#![allow(clippy::disallowed_macros)]

use std::error::Error;
use std::time::Duration;

use uc_observability_contract::error_source::io_error_kind;
use uc_observability_contract::log_fields::log_id;
use uc_observability_contract::module_log::Sensitive;
use uc_observability_contract::uc_warn;
use uc_observability_runtime::{
    DeploymentEnvironment, DetailedCaptureRequest, LocalLogConfig, ObservabilityConfig,
    ObservabilityResource, OperatingSystem, ProcessObservabilityRuntime, SignalResult,
    StopCaptureResult,
};

const BUDGET_BYTES: u64 = 24_000;

#[derive(Debug, thiserror::Error)]
#[error("layer {kind}")]
struct Registered {
    kind: &'static str,
    #[source]
    source: Option<anyhow::Error>,
}

#[derive(Debug, thiserror::Error)]
#[error("{text}")]
struct Long {
    text: &'static str,
    #[source]
    inner: Option<Box<Long>>,
}

/// 未登记的第三方类型：其正文含敏感内容，绝不能出现在文件里。
#[derive(Debug, thiserror::Error)]
#[error("UNREGISTERED_EXTERNAL_SECRET_TEXT")]
struct Unregistered;

fn rows(directory: &std::path::Path) -> Vec<serde_json::Value> {
    let mut text = String::new();
    for path in uc_observability_runtime::managed_log_files(directory).expect("files") {
        text.push_str(&std::fs::read_to_string(path).expect("file"));
    }
    text.lines()
        .map(|line| serde_json::from_str(line).expect("each line is JSON"))
        .collect()
}

fn module_rows(directory: &std::path::Path) -> Vec<serde_json::Value> {
    rows(directory)
        .into_iter()
        .filter(|row| row["source"] == "engine_module")
        .collect()
}

/// 同一个记录点：限速按记录点计。
fn hot_event() {
    tracing::info!(target: "uc_infra::module_log_channel", step = "hot", "hot event");
}

fn flush() {
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
}

macro_rules! distinct_callsites {
    ($($index:literal)+) => {
        $(tracing::info!(target: "uc_infra::module_log_channel", step = "budget_fill", "budget fill {}", $index);)+
    };
}

#[test]
fn module_log_channel_records_renders_limits_and_exports_with_visible_counts() {
    let directory = tempfile::tempdir().expect("logs");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.1.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Macos,
            "test",
        )
        .expect("resource"),
    )
    .with_local_logs(
        LocalLogConfig::new(directory.path()).with_module_log_budget_bytes(BUDGET_BYTES),
    );
    let outcome = ProcessObservabilityRuntime::install(config).expect("install");
    let handle = outcome.handle();

    // 阶段一：错误链渲染（失败方式 1）。
    let io = std::io::Error::from_raw_os_error(13);
    let chained = Registered {
        kind: "outer",
        source: Some(anyhow::Error::new(io).context("fixed action phrase")),
    };
    tracing::warn!(target: "uc_infra::module_log_channel", error = &chained as &dyn Error, "chain rendering");
    let parse = serde_json::from_str::<serde_json::Value>("{ not json").expect_err("invalid json");
    tracing::warn!(target: "uc_infra::module_log_channel", error = &parse as &dyn Error, "serde rendering");
    tracing::warn!(target: "uc_infra::module_log_channel", error = &Unregistered as &dyn Error, "unregistered rendering");
    flush();
    let rows_after_chain = module_rows(directory.path());
    let chain_row = rows_after_chain
        .iter()
        .find(|row| row["message"] == "chain rendering")
        .expect("chain row");
    // 仓库错误类型不再登记正文；自有类型与 anyhow 的 context 层都记为固定占位。
    assert_eq!(chain_row["error.chain"][0], "<opaque>");
    assert_eq!(chain_row["error.chain"][1], "<opaque>");
    let io_layer = chain_row["error.chain"][2].as_str().expect("io layer");
    assert!(
        io_layer.starts_with("io error kind=PermissionDenied os_code=13"),
        "{io_layer}"
    );
    assert_eq!(chain_row["error.opaque_layers"], 2);
    let serde_row = rows_after_chain
        .iter()
        .find(|row| row["message"] == "serde rendering")
        .expect("serde row");
    assert!(
        serde_row["error.chain"][0]
            .as_str()
            .expect("json layer")
            .starts_with("json error category=Syntax"),
        "{serde_row}"
    );
    let unregistered_row = rows_after_chain
        .iter()
        .find(|row| row["message"] == "unregistered rendering")
        .expect("unregistered row");
    assert_eq!(
        unregistered_row["error.chain"],
        serde_json::json!(["<opaque>"])
    );
    let text = serde_json::to_string(&rows_after_chain).expect("rows");
    assert!(!text.contains("UNREGISTERED_EXTERNAL_SECRET_TEXT"));
    assert!(
        !text.contains("fixed action phrase"),
        "anyhow context layers are opaque"
    );

    // 阶段二：自由文本字段默认拒绝，审定过的固定词字段保留（失败方式 2、8）。
    let path = directory.path().join("secret-file.txt");
    tracing::info!(
        target: "uc_infra::module_log_channel",
        path = %path.display(),
        device_name = "MyPhone123",
        peer = %"peer-0123456789abcdef",
        error_kind = "fixed_kind",
        entry_id = %"entry-1",
        count = 7u64,
        note = %Sensitive("wrapped-but-unreviewed-name"),
        "field policy"
    );
    flush();
    let rows_after_fields = module_rows(directory.path());
    let field_row = rows_after_fields
        .iter()
        .find(|row| row["message"] == "field policy")
        .expect("field row");
    assert_eq!(field_row["fields"]["error_kind"], "fixed_kind");
    assert_eq!(field_row["fields"]["entry_id"], "entry-1");
    assert_eq!(field_row["fields"]["count"], 7);
    for omitted in ["path", "device_name", "peer", "note"] {
        assert_eq!(field_row["fields"][omitted], "<omitted>", "{omitted}");
    }
    // 阶段二补充：`uc_*!` 宏写出的记录与旧写法落盘结果一致，且错误链照常渲染。
    let typed_io = std::io::Error::from(std::io::ErrorKind::NotFound);
    uc_warn!(
        target: "uc_infra::module_log_channel",
        error_kind = "fixed_kind",
        entry_id = log_id(&"entry-typed"),
        io_error_kind = io_error_kind(&typed_io),
        error = &typed_io as &dyn Error,
        "typed event"
    );
    flush();
    let typed_rows = module_rows(directory.path());
    let typed_row = typed_rows
        .iter()
        .find(|row| row["message"] == "typed event")
        .expect("typed row");
    assert_eq!(typed_row["fields"]["error_kind"], "fixed_kind");
    assert_eq!(typed_row["fields"]["entry_id"], "entry-typed");
    assert_eq!(typed_row["fields"]["io_error_kind"], "NotFound");
    assert!(typed_row["error.chain"][0]
        .as_str()
        .expect("io layer")
        .starts_with("io error kind=NotFound"));
    // 宏展开在调用处：源码位置属于调用方文件，而不是契约 crate 内的宏定义。
    assert!(
        typed_row["location"]
            .as_str()
            .expect("location")
            .contains("module_log_channel.rs"),
        "{typed_row}"
    );
    let all_text = std::fs::read_dir(directory.path())
        .expect("dir")
        .map(|entry| std::fs::read_to_string(entry.expect("entry").path()).expect("file"))
        .collect::<String>();
    for leaked in [
        "MyPhone123",
        "peer-0123456789abcdef",
        "secret-file.txt",
        "wrapped-but-unreviewed-name",
    ] {
        assert!(!all_text.contains(leaked), "leaked {leaked}");
    }
    assert!(!all_text.contains(directory.path().to_string_lossy().as_ref()));

    // 阶段三：Detailed 窗口的 DEBUG 门（失败方式 3）。
    tracing::debug!(target: "uc_infra::module_log_channel", step = "before_window", "debug outside window");
    let capture = handle
        .start_local_diagnostic_capture(DetailedCaptureRequest {
            duration: Duration::from_secs(30),
        })
        .expect("start capture");
    let capture_id = capture.capture_id.clone().expect("capture id");
    tracing::debug!(target: "uc_infra::module_log_channel", step = "in_window", "debug inside window");
    tracing::trace!(target: "uc_infra::module_log_channel", step = "in_window", "trace inside window");
    assert_eq!(
        handle
            .stop_local_diagnostic_capture(&capture_id)
            .expect("stop capture"),
        StopCaptureResult::Stopped
    );
    tracing::debug!(target: "uc_infra::module_log_channel", step = "after_window", "debug after window");
    flush();
    let rows_after_window = module_rows(directory.path());
    let messages: Vec<&str> = rows_after_window
        .iter()
        .filter_map(|row| row["message"].as_str())
        .collect();
    assert!(!messages.contains(&"debug outside window"), "{messages:?}");
    assert!(messages.contains(&"debug inside window"), "{messages:?}");
    assert!(!messages.contains(&"trace inside window"));
    assert!(!messages.contains(&"debug after window"));
    let inside = rows_after_window
        .iter()
        .find(|row| row["message"] == "debug inside window")
        .expect("inside row");
    assert_eq!(inside["capture_mode"], "detailed");
    assert_eq!(inside["capture_id"], capture_id.as_str());

    // 阶段四：超长记录裁剪并计数（失败方式 5）。
    let text = "L".repeat(250);
    let leaked_text: &'static str = Box::leak(text.into_boxed_str());
    let mut long = Long {
        text: leaked_text,
        inner: None,
    };
    for _ in 0..19 {
        long = Long {
            text: leaked_text,
            inner: Some(Box::new(long)),
        };
    }
    tracing::warn!(target: "uc_infra::module_log_channel", error = &long as &dyn Error, "long chain");
    flush();
    let rows_after_long = module_rows(directory.path());
    let long_row = rows_after_long
        .iter()
        .find(|row| row["message"] == "long chain")
        .expect("long row");
    let chain = long_row["error.chain"].as_array().expect("chain");
    assert_eq!(chain.len(), 17, "{long_row}");
    assert_eq!(chain[16], "<more layers omitted>", "{long_row}");

    // 记录整体超过上限：字段被丢弃并标记 truncated，计数加一。
    tracing::warn!(
        target: "uc_infra::module_log_channel",
        cause = leaked_text,
        context = leaked_text,
        dependency = leaked_text,
        emitter = leaked_text,
        event = leaked_text,
        issue = leaked_text,
        msg_kind = leaked_text,
        plan = leaked_text,
        operation = leaked_text,
        table = leaked_text,
        origin = leaked_text,
        recovery_state = leaked_text,
        reject_reason = leaked_text,
        existing_status = leaked_text,
        file_paths_source = leaked_text,
        error_class = leaked_text,
        source_class = leaked_text,
        step = leaked_text,
        "oversized record"
    );
    flush();
    let oversized = module_rows(directory.path())
        .into_iter()
        .find(|row| row["message"] == "oversized record")
        .expect("oversized row");
    assert_eq!(oversized["truncated"], true, "{oversized}");
    assert_eq!(oversized["fields"], serde_json::json!({}), "{oversized}");

    // 阶段五：热记录点限速，丢弃计数并在下一条放行记录里给出 suppressed（失败方式 4）。
    for _ in 0..200 {
        hot_event();
    }
    std::thread::sleep(Duration::from_millis(1500));
    hot_event();
    flush();
    let hot: Vec<_> = module_rows(directory.path())
        .into_iter()
        .filter(|row| row["message"] == "hot event")
        .collect();
    assert!(hot.len() <= 24, "burst plus refill only, got {}", hot.len());
    assert!(
        hot.iter()
            .any(|row| row["suppressed"].as_u64().is_some_and(|count| count >= 150)),
        "the first admitted record after the burst must report the suppressed count"
    );

    // 阶段六：字节预算耗尽后停止写入并计数（失败方式 6）。
    distinct_callsites!(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49);
    flush();

    // 导出与计数一致（失败方式 7）。
    let report = handle
        .prepare_local_diagnostic_export(Duration::from_secs(5))
        .expect("export");
    assert_eq!(report.flush, SignalResult::Completed);
    let counts = report.module_logs;
    let final_rows = module_rows(directory.path());
    assert_eq!(counts.emitted as usize, final_rows.len(), "{counts:?}");
    assert!(counts.rate_limited >= 150, "{counts:?}");
    assert!(counts.budget_dropped > 0, "{counts:?}");
    assert_eq!(counts.truncated, 1, "{counts:?}");
    assert!(counts.opaque_error_layers >= 2, "{counts:?}");
    let written: usize = final_rows
        .iter()
        .map(|row| serde_json::to_vec(row).expect("row").len() + 1)
        .sum();
    assert!(
        (written as u64) <= BUDGET_BYTES + 4096,
        "written {written} exceeds the budget"
    );
    handle.shutdown(Duration::from_secs(5));
}
