//! 进程资源周期记录的安装验收：真实安装 → 本地模块日志文件。
//!
//! 预先定义的失败方式（任一发生即测试失败）：
//! 1. 启动延迟内就写出资源记录（会打扰短生命周期进程和其他按行数断言的验收）；
//! 2. 关闭进程运行期超过预算（周期线程没有被立即唤醒停止）；
//! 3. （`--ignored`，仅 Linux/Android）启动延迟过后没有写出首条资源记录、记录出现计划之外的字段、
//!    数值字段被写成字符串，或匿名驻留不是正数。
//!
//! 运行期只能安装一次，两个测试不能在同一进程里运行；第二个测试单独用
//! `cargo test -p uc-observability-runtime --test process_census_install -- --ignored` 运行。

use std::time::{Duration, Instant};

use uc_observability_runtime::{
    DeploymentEnvironment, LocalLogConfig, ObservabilityConfig, ObservabilityResource,
    OperatingSystem, ProcessObservabilityRuntime, SignalResult,
};

#[cfg(any(target_os = "linux", target_os = "android"))]
const PLANNED_FIELDS: &[&str] = &[
    "rss_anon_kb",
    "rss_file_kb",
    "rss_shmem_kb",
    "vm_swap_kb",
    "vm_size_kb",
    "threads",
    "fd_total",
    "pss_anon_kb",
    "heap_in_use_kb",
    "heap_free_kb",
    "heap_mmap_kb",
];

fn census_row(directory: &std::path::Path) -> Option<serde_json::Value> {
    let mut text = String::new();
    for path in uc_observability_runtime::managed_log_files(directory).expect("files") {
        text.push_str(&std::fs::read_to_string(path).expect("file"));
    }
    text.lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("each line is JSON"))
        .filter(|row| row["source"] == "engine_module")
        .find(|row| {
            row["message"]
                .as_str()
                .is_some_and(|message| message.starts_with("process resource census"))
        })
}

fn install(directory: &std::path::Path) -> uc_observability_runtime::ProcessObservabilityHandle {
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.1.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Macos,
            "test",
        )
        .expect("resource"),
    )
    .with_local_logs(LocalLogConfig::new(directory));
    ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle()
}

#[test]
fn no_resource_record_is_written_during_the_startup_delay_and_shutdown_is_prompt() {
    let directory = tempfile::tempdir().expect("logs");
    let handle = install(directory.path());

    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
    assert!(
        census_row(directory.path()).is_none(),
        "the first record must wait for the startup delay"
    );

    let started = Instant::now();
    handle.shutdown(Duration::from_secs(5));
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
#[ignore = "waits for the 30 s startup delay; run explicitly with --ignored"]
fn the_first_resource_record_arrives_after_the_startup_delay() {
    let directory = tempfile::tempdir().expect("logs");
    let handle = install(directory.path());

    let deadline = Instant::now() + Duration::from_secs(60);
    let row = loop {
        assert_eq!(
            ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
            SignalResult::Completed
        );
        if let Some(row) = census_row(directory.path()) {
            break row;
        }
        assert!(
            Instant::now() < deadline,
            "no resource census record was written"
        );
        std::thread::sleep(Duration::from_millis(500));
    };

    let fields = row["fields"].as_object().expect("fields object");
    for (name, value) in fields {
        assert!(
            PLANNED_FIELDS.contains(&name.as_str()),
            "unplanned field {name}"
        );
        assert!(value.is_u64(), "{name} must stay numeric: {value}");
    }
    assert_eq!(row["message"], "process resource census");
    assert!(fields["rss_anon_kb"].as_u64().is_some_and(|kb| kb > 0));
    assert!(fields["threads"].as_u64().is_some_and(|count| count >= 1));
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    assert!(fields["heap_in_use_kb"].as_u64().is_some());

    handle.shutdown(Duration::from_secs(5));
}
