use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{Ipv4Addr, UdpSocket};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[path = "common/process.rs"]
mod process;
use process::{join_host, mount_rendezvous, wait_for_active_join, wait_for_phase, HostProcess};

// 独立进程只以公开查询判断恢复完成；测试事件只负责精确等待真实回复。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retryable_failure_recovers_the_public_space_device_update() {
    let rendezvous = mount_rendezvous().await;
    let mut sponsor = HostProcess::start(&rendezvous.uri());
    let mut joiner = HostProcess::start(&rendezvous.uri());
    sponsor.request(json!({ "command": "create", "name": "Sponsor" }));
    let invitation = sponsor.request(json!({ "command": "invite" }))["invitation"]
        .as_str()
        .expect("invitation")
        .to_owned();
    let baseline = sponsor.request(json!({
        "command": "arm_membership_history_failures",
        "failure": "retryable",
        "count": 1,
    }))["after_sequence"]
        .as_u64()
        .expect("event baseline");
    joiner.request(json!({
        "command": "join",
        "name": "Joiner",
        "invitation": invitation,
    }));
    let failed = sponsor.request(json!({
        "command": "wait_space_work_event",
        "kind": "membership_history_sync_retryable_failure",
        "after_sequence": baseline,
    }));
    let failed_sequence = failed["sequence"].as_u64().expect("failure sequence");
    wait_for_phase(&mut sponsor, "retryable_failure");
    sponsor.request(json!({
        "command": "wait_space_work_event",
        "kind": "membership_history_sync_reply_received",
        "after_sequence": failed_sequence,
    }));
    let completed = wait_for_phase(&mut sponsor, "completed");
    assert_eq!(completed["reason"], Value::Null);
    assert_eq!(completed["recovery"], Value::Null);
    assert_eq!(completed["next_retry_at_ms"], Value::Null);
    sponsor.restart();
    let restarted = wait_for_phase(&mut sponsor, "completed");
    assert_eq!(restarted["next_retry_at_ms"], Value::Null);
    sponsor.shutdown();
    joiner.shutdown();
}

// 需处理故障必须持续可见，只有测试进程明确解除后才允许公开状态恢复。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attention_failure_stays_visible_until_recovery_then_completes() {
    let rendezvous = mount_rendezvous().await;
    let mut sponsor = HostProcess::start(&rendezvous.uri());
    let mut joiner = HostProcess::start(&rendezvous.uri());
    sponsor.request(json!({ "command": "create", "name": "Sponsor" }));
    let invitation = sponsor.request(json!({ "command": "invite" }))["invitation"]
        .as_str()
        .expect("invitation")
        .to_owned();
    let baseline = sponsor.request(json!({
        "command": "arm_membership_history_failures",
        "failure": "needs_attention",
        "count": 1_024,
    }))["after_sequence"]
        .as_u64()
        .expect("event baseline");
    joiner.request(json!({
        "command": "join",
        "name": "Joiner",
        "invitation": invitation,
    }));
    let failed = sponsor.request(json!({
        "command": "wait_space_work_event",
        "kind": "membership_history_sync_needs_attention",
        "after_sequence": baseline,
    }));
    let failed_sequence = failed["sequence"].as_u64().expect("failure sequence");
    let attention = wait_for_phase(&mut sponsor, "needs_attention");
    assert_eq!(attention["reason"], "device_state_rejected");
    assert_ne!(
        sponsor.request(json!({ "command": "eligibility" }))["device_trust"]["space_device_update"]
            ["phase"],
        "completed"
    );
    sponsor.request(json!({ "command": "clear_membership_history_failures" }));
    sponsor.request(json!({ "command": "opportunity" }));
    sponsor.request(json!({
        "command": "wait_space_work_event",
        "kind": "membership_history_sync_reply_received",
        "after_sequence": failed_sequence,
    }));
    wait_for_phase(&mut sponsor, "completed");
    sponsor.shutdown();
    joiner.shutdown();
}

// 可重试失败和下次时间必须跨进程保存，重启后由 Engine 自己继续同一恢复流程。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retryable_failure_recovers_after_sponsor_restart() {
    let rendezvous = mount_rendezvous().await;
    let mut sponsor = HostProcess::start(&rendezvous.uri());
    let mut joiner = HostProcess::start(&rendezvous.uri());
    sponsor.request(json!({ "command": "create", "name": "Sponsor" }));
    let invitation = sponsor.request(json!({ "command": "invite" }))["invitation"]
        .as_str()
        .expect("invitation")
        .to_owned();
    let baseline = sponsor.request(json!({
        "command": "arm_membership_history_failures",
        "failure": "retryable",
        "count": 1,
    }))["after_sequence"]
        .as_u64()
        .expect("event baseline");
    joiner.request(json!({
        "command": "join",
        "name": "Joiner",
        "invitation": invitation,
    }));
    sponsor.request(json!({
        "command": "wait_space_work_event",
        "kind": "membership_history_sync_retryable_failure",
        "after_sequence": baseline,
    }));
    let before_restart = wait_for_phase(&mut sponsor, "retryable_failure");
    assert!(before_restart["next_retry_at_ms"].as_i64().is_some());
    sponsor.restart();
    let after_restart = wait_for_phase(&mut sponsor, "retryable_failure");
    assert_eq!(
        after_restart["next_retry_at_ms"],
        before_restart["next_retry_at_ms"]
    );
    sponsor.request(json!({
        "command": "wait_space_work_event",
        "kind": "membership_history_sync_reply_received",
        "after_sequence": 0,
    }));
    wait_for_phase(&mut sponsor, "completed");
    sponsor.shutdown();
    joiner.shutdown();
}

/// 三个独立 Engine 进程、真实 SQLite/MLS/QUIC：纯联系不能解除拒绝，正确历史和实际 ACK 才能。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_process_group_rejection_recovers_only_after_verified_history() {
    let rendezvous = mount_rendezvous().await;
    let mut a = HostProcess::start(&rendezvous.uri());
    // 场景只验证拒绝与历史恢复；禁用 relay 时，重启继续使用同一直接地址。
    let port_probe = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("reserve receiver port");
    let port = port_probe.local_addr().expect("receiver port").port();
    drop(port_probe);
    let mut b = HostProcess::start_binary(
        &rendezvous.uri(),
        PathBuf::from(env!("CARGO_BIN_EXE_uc-connectivity-host")),
        false,
        Some(port),
    );
    let mut c = HostProcess::start(&rendezvous.uri());
    a.request(json!({"command": "create", "name": "A"}));
    let invitation = a.request(json!({"command": "invite"}))["invitation"].clone();
    b.request(json!({"command": "join", "name": "B", "invitation": invitation}));
    wait_for_active_join(&mut b);
    wait_for_phase(&mut a, "completed");
    wait_for_phase(&mut b, "completed");
    wait_for_branch(&mut [&mut a, &mut b], 2);
    let peer =
        b.request(json!({"command": "eligibility"}))["device_trust"]["local_device_id"].clone();
    assert!(peer.is_string());
    for host in [&mut a, &mut c] {
        host.request(json!({"command": "set_membership_history_held", "held": true, "peer": peer}));
    }
    // 先形成完整的新分支，再让一次性篡改进入真实交换；不把准入期间的连接变化算作安全拒绝。
    let a_endpoint = a.request(json!({"command": "network_endpoint"}));
    let b_endpoint = b.request(json!({"command": "network_endpoint"}));
    a.request(json!({"command": "set_network_partition", "endpoint_ids": [b_endpoint]}));
    b.request(json!({"command": "set_network_partition", "endpoint_ids": [a_endpoint]}));
    let invitation = a.request(json!({"command": "invite"}))["invitation"].clone();
    c.request(json!({"command": "join", "name": "C", "invitation": invitation}));
    wait_for_active_join(&mut c);
    wait_for_branch(&mut [&mut a, &mut c], 3);
    a.request(json!({"command": "arm_group_update_tamper", "count": 1}));
    b.request(json!({"command": "set_network_partition", "endpoint_ids": []}));
    a.request(json!({"command": "set_network_partition", "endpoint_ids": []}));
    a.request(json!({"command": "opportunity"}));
    a.request(json!({"command": "wait_space_work_event", "kind": "group_update_tampered", "after_sequence": 0}));
    let rejected = wait_for_phase(&mut a, "needs_attention");
    assert_eq!(rejected["reason"], "device_security_update_rejected");
    b.restart();
    b.request(json!({"command": "opportunity"}));
    // 摘要与实际重新认证允许完成，但没有正确后缀的阶段仍持续可见。
    for _ in 0..5 {
        assert_eq!(
            a.request(json!({"command": "eligibility"}))["device_trust"]["space_device_update"]
                ["phase"],
            "needs_attention"
        );
        std::thread::sleep(Duration::from_millis(400));
    }
    for host in [&mut a, &mut c] {
        host.request(json!({"command": "set_membership_history_held", "held": false}));
    }
    a.request(json!({"command": "opportunity"}));
    b.request(json!({"command": "opportunity"}));
    wait_for_phase(&mut a, "completed");
    b.request(json!({"command": "wait_space_work_event", "kind": "group_update_applied", "after_sequence": 0}));
    a.shutdown();
    b.shutdown();
    c.shutdown();
}

async fn fixed_main_group_interop(legacy: bool) {
    let rendezvous = mount_rendezvous().await;
    let old = PathBuf::from(
        std::env::var_os("UC_GROUP_UPDATE_OLD_HOST")
            .expect("fixed main host binary must be provided"),
    );
    let current = PathBuf::from(env!("CARGO_BIN_EXE_uc-connectivity-host"));
    let mut a = HostProcess::start_binary(&rendezvous.uri(), old, false, None);
    let mut b = HostProcess::start_binary(&rendezvous.uri(), current.clone(), legacy, None);
    let mut c = HostProcess::start_binary(&rendezvous.uri(), current.clone(), legacy, None);
    let mut d = HostProcess::start_binary(&rendezvous.uri(), current, legacy, None);
    a.request(json!({"command": "create", "name": "OldA"}));
    let invitation = a.request(json!({"command": "invite"}))["invitation"].clone();
    b.request(json!({"command": "join", "name": "NewB", "invitation": invitation}));
    wait_for_active_join(&mut b);
    wait_for_phase(&mut a, "completed");
    wait_for_phase(&mut b, "completed");
    // 新 B 生成更新，保留旧 A：真实新→旧发送和旧 ACK。
    let invitation = b.request(json!({"command": "invite"}))["invitation"].clone();
    c.request(json!({"command": "join", "name": "NewC", "invitation": invitation}));
    wait_for_active_join(&mut c);
    wait_for_phase(&mut b, "completed");
    wait_for_phase(&mut a, "completed");
    // 旧 A 再生成更新，保留新 B/C：真实旧→新发送与新 ACK。
    let invitation = a.request(json!({"command": "invite"}))["invitation"].clone();
    d.request(json!({"command": "join", "name": "NewD", "invitation": invitation}));
    wait_for_active_join(&mut d);
    wait_for_phase(&mut a, "completed");
    wait_for_phase(&mut b, "completed");
    // 当前 head、分支和真实 MLS 效果共同证明两次增量历史的旧→新、新→旧互通。
    wait_for_branch(&mut [&mut a, &mut b, &mut c, &mut d], 4);
    let events = b.request(json!({"command": "space_work_events"}));
    let kinds = if legacy {
        [
            "group_update_legacy_wire_sent",
            "group_update_legacy_wire_acknowledged",
        ]
    } else {
        [
            "group_update_current_wire_sent",
            "group_update_current_wire_acknowledged",
        ]
    };
    for kind in kinds {
        assert!(
            events
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["kind"] == kind),
            "actual wire must prove each direction of the selected group protocol: {kind}"
        );
    }
    a.shutdown();
    b.shutdown();
    c.shutdown();
    d.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "需要固定 main 二进制 UC_GROUP_UPDATE_OLD_HOST；独立兼容验收显式执行"]
async fn fixed_main_group_update_v2_interoperates_both_directions() {
    fixed_main_group_interop(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "需要固定 main 二进制 UC_GROUP_UPDATE_OLD_HOST；独立兼容验收显式执行"]
async fn fixed_main_group_update_v1_interoperates_both_directions() {
    fixed_main_group_interop(true).await;
}

fn wait_for_branch(hosts: &mut [&mut HostProcess], count: u32) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let snapshots = hosts
            .iter_mut()
            .map(|host| host.request(json!({"command": "membership_diagnostics"})))
            .collect::<Vec<_>>();
        if snapshots.iter().all(|snapshot| {
            snapshot["head_event_id"] == snapshots[0]["head_event_id"]
                && snapshot["branch_id"] == snapshots[0]["branch_id"]
                && snapshot["effective_member_count"] == count
                && snapshot["pending_effect_count"] == 0
        }) {
            return snapshots[0]["group_epoch"].as_u64().unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "separate processes must finish verified branch and actual local effects"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 独立进程重现真实 3→5 缺口、持久需求重启、原发送者离线与第三方连续恢复。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn separate_process_missing_predecessor_survives_restart_and_source_shutdown() {
    let started = Instant::now();
    let record_stage =
        |name: &str| eprintln!("缺口恢复验收阶段：{name}，耗时 {:?}", started.elapsed());
    record_stage("开始");
    let rendezvous = mount_rendezvous().await;
    let mut a = HostProcess::start(&rendezvous.uri());
    // 本场景关闭 relay；固定重启端口，隔离地址重新发现与密钥缺口恢复两种验收责任。
    // nextest 独占场景，接收方强杀并 wait 后再绑定同一 UDP 端口。
    let port_probe = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("reserve receiver port");
    let port = port_probe.local_addr().expect("receiver port").port();
    drop(port_probe);
    let mut b = HostProcess::start_binary(
        &rendezvous.uri(),
        PathBuf::from(env!("CARGO_BIN_EXE_uc-connectivity-host")),
        false,
        Some(port),
    );
    let mut c = HostProcess::start(&rendezvous.uri());
    let mut d = HostProcess::start(&rendezvous.uri());
    let mut e = HostProcess::start(&rendezvous.uri());
    for host in [&mut a, &mut b, &mut c, &mut d, &mut e] {
        host.request(json!({"command": "start_network_capture"}));
    }
    a.request(json!({"command": "create", "name": "A"}));
    join_host(&mut a, &mut b, "B");
    join_host(&mut a, &mut c, "C");
    let before = wait_for_branch(&mut [&mut a, &mut b, &mut c], 3);
    let b_peer =
        b.request(json!({"command": "eligibility"}))["device_trust"]["local_device_id"].clone();
    let a_endpoint = a.request(json!({"command": "network_endpoint"}));
    let b_endpoint = b.request(json!({"command": "network_endpoint"}));
    a.request(json!({"command": "set_network_partition", "endpoint_ids": [b_endpoint]}));
    b.request(json!({"command": "set_network_partition", "endpoint_ids": [a_endpoint]}));
    b.request(json!({"command": "set_membership_history_held", "held": true}));
    for host in [&mut c, &mut d, &mut e] {
        host.request(
            json!({"command": "set_membership_history_held", "held": true, "peer": b_peer}),
        );
    }
    join_host(&mut a, &mut d, "D");
    wait_for_branch(&mut [&mut a, &mut c, &mut d], 4);
    join_host(&mut c, &mut e, "E");
    let target = wait_for_branch(&mut [&mut c, &mut d, &mut e], 5);
    let missing = b.request(json!({"command": "wait_space_work_event", "kind": "group_update_missing_predecessor_observed", "after_sequence": 0}));
    // ACK 3 在恢复需求持久提交之后发送；跨过这个边界再 kill，而不是只看到下层缺口错误。
    b.request(json!({"command": "wait_space_work_event", "kind": "group_update_current_wire_acknowledged", "after_sequence": missing["sequence"]}));
    assert_eq!(
        b.request(json!({"command": "membership_diagnostics"}))["group_epoch"],
        before
    );
    record_stage("缺口需求已持久化并确认");
    let old_addresses = b.request(json!({"command":"network_addresses"}));
    a.shutdown();
    b.restart_with_crash(true);
    let new_addresses = b.request(json!({"command":"network_addresses"}));
    assert_eq!(
        old_addresses, new_addresses,
        "本场景隔离地址发现，重启必须保持广告地址"
    );
    record_stage("接收方强杀后重启");
    assert_eq!(
        b.request(json!({"command": "membership_diagnostics"}))["group_epoch"],
        before
    );
    for host in [&mut c, &mut d, &mut e] {
        host.request(json!({"command": "set_membership_history_held", "held": false}));
    }
    assert_eq!(
        wait_for_branch(&mut [&mut b, &mut c, &mut d, &mut e], 5),
        target
    );
    record_stage("真实目标 epoch 已安装");
    let mut previous = Value::Null;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let choices = c.request(json!({"command": "eligibility"}));
        let peer = choices["device_trust"]["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|device| device["device_id"] == b_peer)
            .expect("B remains a current member");
        if peer["group_update"] != previous {
            eprintln!(
                "缺口恢复验收发送待办：{}，耗时 {:?}",
                json!({"group_update": peer["group_update"], "relationship":peer["group_relationship"], "sync_state":peer["sync_relationship"]}),
                started.elapsed()
            );
            previous = peer["group_update"].clone();
        }
        if peer["group_update"].is_null() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "C must receive actual ACK for B's recovered update"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    record_stage("发送方实际 ACK 已结清");
    b.request(json!({"command": "wait_space_work_event", "kind": "group_update_applied", "after_sequence": 0}));
    b.shutdown();
    c.shutdown();
    d.shutdown();
    e.shutdown();
}

/// 两轮真实成员变化的历史核对复用同方向物理连接；只读既有脱敏诊断，不扩 Engine 查询。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn separate_process_membership_history_reuses_connections_across_rounds() {
    let rendezvous = mount_rendezvous().await;
    let mut a = HostProcess::start(&rendezvous.uri());
    let mut b = HostProcess::start(&rendezvous.uri());
    let mut c = HostProcess::start(&rendezvous.uri());
    for host in [&mut a, &mut b, &mut c] {
        host.request(json!({"command": "start_network_capture"}));
    }
    a.request(json!({"command": "create", "name": "A"}));
    join_host(&mut a, &mut b, "B");
    wait_for_branch(&mut [&mut a, &mut b], 2);
    a.request(json!({"command": "wait_space_work_event", "kind": "membership_history_sync_reply_received", "after_sequence": 0}));
    // 第三成员形成新的真实历史债务，不伪造 ACK 或直接调用同步内部步骤。
    join_host(&mut a, &mut c, "C");
    wait_for_branch(&mut [&mut a, &mut b, &mut c], 3);
    for host in [&mut a, &mut b, &mut c] {
        host.request(json!({"command": "flush"}));
    }
    let mut reports = Vec::new();
    for host in [&a, &b, &c] {
        reports.push(history_connection_report(host.root.path()));
    }
    // 新成员恢复之前已联系的 peer 至少经历两个历史请求，但每 peer 仅一条物理出站连接。
    let a_report = &reports[0];
    assert!(
        !a_report.is_empty(),
        "actual history dials must be recorded"
    );
    assert!(
        a_report.values().all(|connections| connections.len() == 1),
        "history connection reuse: {reports:?}"
    );
    assert!(
        reports[1]
            .values()
            .all(|connections| connections.len() == 1),
        "established member B direction isolation: {reports:?}"
    );
    // C 正在介绍身份，远端可能只接受临时交换；这些握手不属于已确认成员的保留池。
    let evidence = std::env::var_os("UC_TEST_ARTIFACTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../target/test-artifacts/membership-history-connectivity")
        });
    std::fs::create_dir_all(&evidence).unwrap();
    std::fs::write(
        evidence.join(
            if std::env::var_os("UC_MEMBERSHIP_HISTORY_TEST_RELAY").is_some() {
                "separate-process-relay-reuse.json"
            } else {
                "separate-process-reuse.json"
            },
        ),
        serde_json::to_vec_pretty(&reports).unwrap(),
    )
    .unwrap();
    a.shutdown();
    b.shutdown();
    c.shutdown();
}

fn history_connection_report(root: &std::path::Path) -> HashMap<String, Vec<String>> {
    let mut records = Vec::new();
    for entry in std::fs::read_dir(root.join("logs")).unwrap() {
        let path = entry.unwrap().path();
        if !path.is_file() {
            continue;
        }
        let Ok(data) = std::fs::read_to_string(path) else {
            continue;
        };
        records.extend(
            data.lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok()),
        );
    }
    let started = records
        .iter()
        .filter(|record| {
            record["fields"]["event.name"] == "connection.started"
                && record["fields"]["purpose"] == "membership_history"
        })
        .filter_map(|record| record["fields"]["connect_id"].as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut report = HashMap::<String, Vec<String>>::new();
    for record in &records {
        if record["fields"]["event.name"] != "connection.finished"
            || record["fields"]["outcome"] != "connected"
        {
            continue;
        }
        let Some(connect) = record["fields"]["connect_id"].as_str() else {
            continue;
        };
        if !started.contains(connect) {
            continue;
        }
        let peer = record["peer_ref"]
            .as_str()
            .expect("verified local peer correlation")
            .to_owned();
        let connection = record["connection_id"]
            .as_str()
            .expect("actual physical connection correlation")
            .to_owned();
        report.entry(peer).or_default().push(connection);
    }
    for connections in report.values_mut() {
        connections.sort();
        connections.dedup();
    }
    report
}
