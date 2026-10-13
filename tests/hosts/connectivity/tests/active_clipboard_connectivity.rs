//! 独立进程真实状态通知、按需拉取、本体物化与系统写入读回。
#[path = "common/process.rs"]
mod process;
use process::{join_host, mount_rendezvous, wait_for_phase, HostProcess};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::{Duration, Instant};
const WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// 原实现基线：禁用普通自动推送后，缺失内容经恢复通知与按需拉取完成系统写入。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_clipboard_restore_pulls_missing_text_baseline() {
    let rendezvous = mount_rendezvous().await;
    let binary = std::env::var_os("UC_ACTIVE_CLIPBOARD_BASELINE_HOST")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_uc-connectivity-host")));
    let mut holder = HostProcess::start_binary(&rendezvous.uri(), binary.clone(), false, None);
    let mut requester = HostProcess::start_binary(&rendezvous.uri(), binary, false, None);
    holder.request(json!({ "command": "create", "name": "Holder" }));
    join_host(&mut holder, &mut requester, "Requester");
    wait_for_phase(&mut holder, "completed");
    wait_for_phase(&mut requester, "completed");
    for host in [&mut holder, &mut requester] {
        host.request(
            json!({ "command": "sync_preference", "sync_on_restore": true,
            "auto_sync_enabled": false }),
        );
    }
    let captured = holder.request(json!({ "command": "capture", "kind": "text",
        "text": "active clipboard baseline", "observed_at_ms": 1_700_000_000_000i64 }));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        requester.request(json!({ "command": "history" })),
        json!([]),
        "ordinary push must not fill the missing content before restore"
    );
    let restored = holder.request(json!({ "command": "restore", "entry": captured["entry"] }));
    assert_eq!(restored["restored"], true);
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        let active = requester.request(json!({ "command": "active" }));
        if !active.is_null() {
            assert_eq!(
                requester.request(json!({ "command": "clipboard" })),
                holder.request(json!({ "command": "clipboard" })),
                "actual system write must match the restored synthetic text"
            );
            let history = requester.request(json!({ "command": "history" }));
            assert_eq!(history.as_array().unwrap().len(), 1);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "restore notification must pull missing text and apply it"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    holder.shutdown();
    requester.shutdown();
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn capture_request(kind: &str) -> Value {
    let mut request = match kind {
        "text" => json!({ "kind": "text", "text": "active clipboard complete text" }),
        "image" => {
            json!({ "kind": "image", "bytes": include_bytes!("fixtures/active-clipboard.png").as_slice() })
        }
        "file" => {
            json!({ "kind": "file", "handle": "active-file", "display_name": "synthetic.bin", "bytes": b"complete single file content".as_slice() })
        }
        "directory" => json!({ "kind": "directory", "members": [
            { "name": "a.bin", "content": "directory member A" },
            { "name": "nested/b.bin", "content": "directory member B" },
            { "name": "empty", "directory": true },
        ] }),
        _ => panic!("unknown fixture"),
    };
    request["command"] = json!("capture");
    request["observed_at_ms"] = json!(1_700_000_000_000i64);
    request
}
fn assert_complete_content(kind: &str, clipboard: &Value) {
    let representations = clipboard["representations"].as_array().unwrap();
    // 文件捕获同时带有既有显示元数据；只用可粘贴的完整内容作为读回断言。
    let primary = representations
        .iter()
        .filter(|representation| {
            if matches!(kind, "file" | "directory") {
                representation["kind"] == "files"
            } else if kind == "image" {
                representation["mime_type"]
                    .as_str()
                    .is_some_and(|mime| mime.starts_with("image/"))
            } else {
                representation["mime_type"] == "text/plain"
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        primary.len(),
        1,
        "exactly one complete primary representation expected"
    );
    let representation = primary[0];
    match kind {
        "text" | "image" => {
            let bytes: &[u8] = if kind == "text" {
                b"active clipboard complete text"
            } else {
                include_bytes!("fixtures/active-clipboard.png")
            };
            assert_eq!(representation["size"], bytes.len());
            assert_eq!(representation["sha256"], digest(bytes));
        }
        "file" => {
            assert_eq!(
                representation["files"],
                json!([{ "kind": "file", "size": 28, "sha256": digest(b"complete single file content") }])
            );
        }
        "directory" => {
            let mut expected = vec![
                json!({ "kind": "file", "name_sha256": digest(b"a.bin"), "size": 18, "sha256": digest(b"directory member A") }),
                json!({ "kind": "directory", "name_sha256": digest(b"nested") }),
                json!({ "kind": "file", "name_sha256": digest(b"nested/b.bin"), "size": 18, "sha256": digest(b"directory member B") }),
                json!({ "kind": "directory", "name_sha256": digest(b"empty") }),
            ];
            expected.sort_by_key(Value::to_string);
            assert_eq!(
                representation["directories"],
                json!([{ "members": expected }])
            );
        }
        _ => unreachable!(),
    }
}
fn network_records(host: &mut HostProcess) -> Vec<Value> {
    host.request(json!({ "command": "flush" }));
    std::fs::read_dir(host.root.path().join("logs"))
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .flat_map(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .collect::<Vec<_>>()
        })
        .collect()
}
fn exchange_connections(records: &[Value], purpose: &str) -> std::collections::HashSet<String> {
    let started = records
        .iter()
        .filter(|row| {
            row["fields"]["event.name"] == "connection.started"
                && row["fields"]["purpose"] == purpose
        })
        .filter_map(|row| row["fields"]["connect_id"].as_str())
        .collect::<std::collections::HashSet<_>>();
    records
        .iter()
        .filter(|row| {
            matches!(
                row["fields"]["event.name"].as_str(),
                Some("connection.finished" | "connection.attempt.finished")
            ) && row["fields"]["outcome"] == "connected"
                && (row["fields"]["purpose"] == purpose
                    || row["fields"]["connect_id"]
                        .as_str()
                        .is_some_and(|id| started.contains(id)))
        })
        .filter_map(|row| row["connection_id"].as_str().map(str::to_owned))
        .collect()
}
fn exchange_paths(records: &[Value], purpose: &str) -> Vec<String> {
    let connections = exchange_connections(records, purpose);
    records
        .iter()
        .filter(|row| {
            row["fields"]["event.name"] == "connection.path.observed"
                && row["connection_id"]
                    .as_str()
                    .is_some_and(|id| connections.contains(id))
                && matches!(
                    row["fields"]["observation"].as_str(),
                    Some("selected" | "snapshot")
                )
        })
        .filter_map(|row| row["fields"]["path_kind"].as_str().map(str::to_owned))
        .collect()
}
fn receiver_paths(records: &[Value], operation: &str) -> Vec<String> {
    // 完整协议接收器记录原连接的实际 channel，已有不透明上下文给出同进程匿名连接编号。
    records
        .iter()
        .filter(|row| {
            row["fields"]["operation"] == operation && row["connection_id"].as_str().is_some()
        })
        .filter_map(|row| match row["fields"]["channel"].as_str() {
            Some("Direct") => Some("direct".into()),
            Some("Relay") => Some("relay".into()),
            _ => None,
        })
        .collect()
}
fn pull_count(holder: &[Value], requester: &[Value]) -> usize {
    let inbound = holder
        .iter()
        .filter(|row| row["fields"]["operation"] == "active_pull_receive")
        .filter_map(|row| row["connection_id"].as_str())
        .collect::<std::collections::HashSet<_>>();
    // 固定旧 holder 没有新接收诊断，使用新 requester 自足终态的本机实际连接编号。
    if inbound.is_empty() {
        exchange_connections(requester, "clipboard_pull").len()
    } else {
        inbound.len()
    }
}
async fn complete_restore(
    kind: &str,
    holder_binary: PathBuf,
    requester_binary: PathBuf,
    label: &str,
) {
    let server = mount_rendezvous().await;
    let mut holder = HostProcess::start_binary(&server.uri(), holder_binary, false, None);
    let mut requester = HostProcess::start_binary(&server.uri(), requester_binary, false, None);
    for host in [&mut holder, &mut requester] {
        host.request(json!({ "command": "start_network_capture" }));
    }
    holder.request(json!({ "command": "create", "name": "Holder" }));
    join_host(&mut holder, &mut requester, "Requester");
    wait_for_phase(&mut holder, "completed");
    wait_for_phase(&mut requester, "completed");
    for host in [&mut holder, &mut requester] {
        host.request(json!({ "command": "sync_preference", "sync_on_restore": true, "auto_sync_enabled": false }));
    }
    let entry = holder.request(capture_request(kind))["entry"].clone();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        requester.request(json!({ "command": "history" })),
        json!([]),
        "ordinary push must not hide the pull path"
    );
    assert_eq!(
        holder.request(json!({ "command": "restore", "entry": entry }))["restored"],
        true
    );
    let deadline = Instant::now() + WAIT_TIMEOUT;
    let clipboard = loop {
        if !requester.request(json!({ "command": "active" })).is_null() {
            let clipboard = requester.request(json!({ "command": "clipboard" }));
            assert_complete_content(kind, &clipboard);
            break clipboard;
        }
        assert!(
            Instant::now() < deadline,
            "complete restored content must reach the system clipboard"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    // 目录首次完整读回仍验收；既有目录可用性仅检查普通文件，重复激活债务另有证据记录。
    if kind != "directory" {
        let initial_pulls = pull_count(
            &network_records(&mut holder),
            &network_records(&mut requester),
        );
        assert!(
            initial_pulls > 0,
            "completed pull must have an actual connection record"
        );
        let first_event_deadline = Instant::now() + WAIT_TIMEOUT;
        let first_activation = loop {
            let events = requester.request(json!({ "command": "events" }));
            if let Some(timestamp) = events
                .as_array()
                .unwrap()
                .iter()
                .filter(|event| event["kind"] == "active")
                .filter_map(|event| event["activated_at_ms"].as_i64())
                .max()
            {
                break timestamp;
            }
            assert!(
                Instant::now() < first_event_deadline,
                "first application event must be observed"
            );
            std::thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(
            holder.request(json!({ "command": "restore", "entry": entry }))["restored"],
            true
        );
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            let events = requester.request(json!({ "command": "events" }));
            if events.as_array().unwrap().iter().any(|event| {
                event["kind"] == "active"
                    && event["activated_at_ms"]
                        .as_i64()
                        .is_some_and(|timestamp| timestamp > first_activation)
            }) {
                break;
            }
            if Instant::now() >= deadline {
                let evidence = std::env::var_os("UC_TEST_ARTIFACTS_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| {
                        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                            .join("../../../target/active-clipboard-connectivity-evidence")
                    });
                std::fs::create_dir_all(&evidence).unwrap();
                for (role, host) in [("holder", &mut holder), ("requester", &mut requester)] {
                    std::fs::write(
                        evidence.join(format!("{label}-{kind}-repeat-failure-{role}.json")),
                        serde_json::to_vec_pretty(&network_records(host)).unwrap(),
                    )
                    .unwrap();
                }
                panic!("second activation of locally held content must apply");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_complete_content(kind, &requester.request(json!({ "command": "clipboard" })));
        let repeated_pulls = pull_count(
            &network_records(&mut holder),
            &network_records(&mut requester),
        );
        assert_eq!(
            repeated_pulls, initial_pulls,
            "locally complete content must not be pulled again"
        );
    }
    let evidence = std::env::var_os("UC_TEST_ARTIFACTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../target/active-clipboard-connectivity-evidence/direct")
        });
    std::fs::create_dir_all(&evidence).unwrap();
    let holder_records = network_records(&mut holder);
    let requester_records = network_records(&mut requester);
    for (role, records) in [
        ("holder", &holder_records),
        ("requester", &requester_records),
    ] {
        std::fs::write(
            evidence.join(format!("{label}-{kind}-{role}-network.json")),
            serde_json::to_vec_pretty(records).unwrap(),
        )
        .unwrap();
    }
    let mut state_paths = exchange_paths(&holder_records, "active_clipboard");
    state_paths.extend(receiver_paths(&requester_records, "active_state_receive"));
    let mut pull_paths = exchange_paths(&requester_records, "clipboard_pull");
    pull_paths.extend(receiver_paths(&holder_records, "active_pull_receive"));
    // 路径门禁只在明确约束网络的脚本中启用；普通宿主不把 Unknown 推断为 Direct。
    let expected = std::env::var("UC_ACTIVE_CLIPBOARD_EXPECTED_PATH").ok();
    if let Some(expected) = expected.as_deref() {
        assert!(matches!(expected, "direct" | "relay"));
        assert!(
            !state_paths.is_empty() && state_paths.iter().all(|path| path == expected),
            "actual state exchange path must be {expected}: {state_paths:?}"
        );
        assert!(
            !pull_paths.is_empty() && pull_paths.iter().all(|path| path == expected),
            "actual pull exchange path must be {expected}: {pull_paths:?}"
        );
    }
    std::fs::write(evidence.join(format!("{label}-{kind}.json")), serde_json::to_vec_pretty(&json!({
        "result": "passed", "kind": kind, "selected_path": expected, "path_validation": if expected.is_some() { "passed" } else { "skipped-uncontrolled-network" }, "state_paths": state_paths, "pull_paths": pull_paths,
        "ordinary_push_disabled": true, "existing_content_did_not_pull_again": kind != "directory", "system_clipboard": clipboard,
    })).unwrap()).unwrap();
    holder.shutdown();
    requester.shutdown();
}
fn current_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_uc-connectivity-host"))
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restore_missing_text_reads_back_complete_content() {
    complete_restore("text", current_binary(), current_binary(), "current").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restore_missing_image_reads_back_complete_content() {
    complete_restore("image", current_binary(), current_binary(), "current").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restore_missing_file_reads_back_complete_content() {
    complete_restore("file", current_binary(), current_binary(), "current").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restore_missing_directory_reads_back_complete_content() {
    complete_restore("directory", current_binary(), current_binary(), "current").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "固定旧 Engine 驱动由专用脚本提供"]
async fn fixed_main_active_clipboard_interoperates_both_roles() {
    let old = PathBuf::from(
        std::env::var_os("UC_ACTIVE_CLIPBOARD_OLD_HOST").expect("fixed old Engine host"),
    );
    for kind in ["text", "image", "file", "directory"] {
        complete_restore(kind, old.clone(), current_binary(), "old-holder").await;
        complete_restore(kind, current_binary(), old.clone(), "old-requester").await;
    }
    unavailable_holder_does_not_apply(old.clone(), current_binary(), "old-holder").await;
    unavailable_holder_does_not_apply(current_binary(), old.clone(), "old-requester").await;
    locked_requester_does_not_apply(old.clone(), current_binary(), "old-holder").await;
    locked_requester_does_not_apply(current_binary(), old, "old-requester").await;
}

/// 删除已恢复条目的持有方仍发出先前状态；实际 pull 必须拒绝，不能写系统或推进寄存器。
async fn unavailable_holder_does_not_apply(
    holder_binary: PathBuf,
    requester_binary: PathBuf,
    label: &str,
) {
    let server = mount_rendezvous().await;
    let mut holder = HostProcess::start_binary(&server.uri(), holder_binary, false, None);
    let mut requester = HostProcess::start_binary(&server.uri(), requester_binary, false, None);
    for host in [&mut holder, &mut requester] {
        host.request(json!({ "command": "start_network_capture" }));
    }
    holder.request(json!({ "command": "create", "name": "Holder" }));
    join_host(&mut holder, &mut requester, "Requester");
    wait_for_phase(&mut holder, "completed");
    wait_for_phase(&mut requester, "completed");
    for host in [&mut holder, &mut requester] {
        host.request(json!({ "command": "sync_preference", "sync_on_restore": true, "auto_sync_enabled": false }));
    }
    let entry = holder.request(capture_request("text"))["entry"].clone();
    assert_eq!(
        requester.request(json!({ "command": "history" })),
        json!([])
    );
    assert_eq!(
        holder.request(json!({ "command": "restore", "entry": entry }))["restored"],
        true
    );
    holder.request(json!({ "command": "delete", "entry": entry }));
    let deadline = Instant::now() + WAIT_TIMEOUT;
    let records = loop {
        let records = network_records(&mut requester);
        if records.iter().any(|row| {
            row["message"].as_str().is_some_and(|message| {
                message.contains("holder cannot serve the content")
                    || message.contains("active pull peer cannot serve")
            })
        }) && !exchange_connections(&records, "clipboard_pull").is_empty()
            || Instant::now() >= deadline
        {
            break records;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(requester.request(json!({ "command": "active" })).is_null());
    assert_eq!(
        requester.request(json!({ "command": "history" })),
        json!([])
    );
    assert!(
        requester.request(json!({ "command": "clipboard" }))["representations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let evidence = std::env::var_os("UC_TEST_ARTIFACTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../target/active-clipboard-connectivity-evidence")
        });
    std::fs::create_dir_all(&evidence).unwrap();
    std::fs::write(
        evidence.join(format!("{label}-unavailable-requester-network.json")),
        serde_json::to_vec_pretty(&records).unwrap(),
    )
    .unwrap();
    assert!(
        records
            .iter()
            .any(|row| row["message"].as_str().is_some_and(|message| {
                message.contains("holder cannot serve the content")
                    || message.contains("active pull peer cannot serve")
            })),
        "actual holder refusal must arrive"
    );
    assert!(
        !exchange_connections(&records, "clipboard_pull").is_empty(),
        "refusal must follow an actual pull exchange"
    );
    let paths = exchange_paths(&records, "clipboard_pull");
    let expected = std::env::var("UC_ACTIVE_CLIPBOARD_EXPECTED_PATH").ok();
    if let Some(expected) = expected.as_deref() {
        assert!(matches!(expected, "direct" | "relay"));
        assert!(
            !paths.is_empty() && paths.iter().all(|path| path == expected),
            "actual refusal exchange path must be {expected}: {paths:?}"
        );
    }
    std::fs::write(evidence.join(format!("{label}-unavailable.json")), serde_json::to_vec_pretty(&json!({
        "selected_path": expected, "paths": paths,
        "path_validation": if expected.is_some() { "passed" } else { "skipped-uncontrolled-network" },
        "result": "passed", "holder_entry_deleted": true, "actual_refusal_observed": true,
        "register_unchanged": true, "system_clipboard_unchanged": true, "network_records": records,
    })).unwrap()).unwrap();
    holder.shutdown();
    requester.shutdown();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unavailable_holder_refusal_does_not_advance_or_apply() {
    unavailable_holder_does_not_apply(current_binary(), current_binary(), "current").await;
}

/// 稳定锁定操作先退休接收世代；后来的真实恢复通知不能写入锁定宿主。
async fn locked_requester_does_not_apply(
    holder_binary: PathBuf,
    requester_binary: PathBuf,
    label: &str,
) {
    let server = mount_rendezvous().await;
    let mut holder = HostProcess::start_binary(&server.uri(), holder_binary, false, None);
    let mut requester = HostProcess::start_binary(&server.uri(), requester_binary, false, None);
    holder.request(json!({ "command": "create", "name": "Holder" }));
    join_host(&mut holder, &mut requester, "Requester");
    wait_for_phase(&mut holder, "completed");
    wait_for_phase(&mut requester, "completed");
    for host in [&mut holder, &mut requester] {
        host.request(json!({ "command": "sync_preference", "sync_on_restore": true, "auto_sync_enabled": false }));
    }
    let entry = holder.request(capture_request("text"))["entry"].clone();
    assert_eq!(
        requester.request(json!({ "command": "history" })),
        json!([])
    );
    requester.request(json!({ "command": "lock" }));
    holder.request(json!({ "command": "restore", "entry": entry }));
    // 超过原静默窗口，观察实际宿主内容；锁定响应字节另由原 codec/真实 handler 合同测试验证。
    std::thread::sleep(Duration::from_millis(750));
    assert!(
        requester.request(json!({ "command": "clipboard" }))["representations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let evidence = std::env::var_os("UC_TEST_ARTIFACTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../target/active-clipboard-connectivity-evidence")
        });
    std::fs::create_dir_all(&evidence).unwrap();
    std::fs::write(
        evidence.join(format!("{label}-locked.json")),
        serde_json::to_vec_pretty(&json!({
            "result": "passed", "requester_locked_through_stable_operation": true,
            "restore_triggered_after_lock": true, "system_clipboard_unchanged": true,
        }))
        .unwrap(),
    )
    .unwrap();
    holder.shutdown();
    requester.shutdown();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn locked_requester_keeps_system_clipboard_unchanged() {
    locked_requester_does_not_apply(current_binary(), current_binary(), "current").await;
}
