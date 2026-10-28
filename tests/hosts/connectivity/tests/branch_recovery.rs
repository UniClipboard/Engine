#![cfg(feature = "branch-recovery-tests")]
//! 独立进程中的真实成员分叉与目标分支恢复。
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::{Duration, Instant},
};

use serde_json::{json, Value};
#[cfg(all(feature = "branch-recovery-fault-tests", unix))]
use std::{io::Read, os::unix::net::UnixListener};

#[path = "common/process.rs"]
mod process;
use process::{join_host, mount_rendezvous, HostProcess};

fn host_log_records(host: &mut HostProcess) -> Vec<Value> {
    host.request(json!({"command": "flush"}));
    std::fs::read_dir(host.root.path().join("logs"))
        .expect("diagnostic directory")
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .flat_map(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn recovery_paths(host: &mut HostProcess) -> HashMap<String, HashSet<String>> {
    let records = host_log_records(host);
    let mut paths = HashMap::new();
    for row in &records {
        if row["fields"]["event.name"] == "connection.protocol.negotiated"
            && row["fields"]["protocol"] == "membership_branch_recovery"
        {
            if let Some(id) = row["connection_id"].as_str() {
                let observed = paths.entry(id.to_owned()).or_insert_with(HashSet::new);
                if let Some(path @ ("direct" | "relay")) = row["fields"]["initial_path"].as_str() {
                    observed.insert(path.to_owned());
                }
            }
        }
    }
    for row in &records {
        if row["fields"]["event.name"] != "connection.path.observed"
            || !matches!(
                row["fields"]["observation"].as_str(),
                Some("selected" | "snapshot")
            )
        {
            continue;
        }
        if let (Some(id), Some(path @ ("direct" | "relay"))) = (
            row["connection_id"].as_str(),
            row["fields"]["path_kind"].as_str(),
        ) {
            if let Some(observed) = paths.get_mut(id) {
                observed.insert(path.to_owned());
            }
        }
    }
    paths
}

fn completed_conflict_rounds(host: &mut HostProcess) -> HashSet<u64> {
    let records = host_log_records(host);
    records
        .iter()
        .filter(|row| {
            row["fields"]["event.name"] == "runtime.work.finished"
                && row["fields"]["step"] == "maintenance_conflicts"
                && row["fields"]["uc.outcome"] == "ok"
        })
        .filter_map(|row| row["fields"]["maintenance_round"].as_u64())
        .filter(|round| {
            [
                "membership.maintenance.started",
                "membership.maintenance.finished",
            ]
            .iter()
            .all(|name| {
                records.iter().any(|row| {
                    row["fields"]["event.name"] == *name
                        && row["fields"]["maintenance_round"].as_u64() == Some(*round)
                })
            })
        })
        .filter(|round| {
            let start = records.iter().find(|row| {
                row["fields"]["event.name"] == "runtime.work.started"
                    && row["fields"]["step"] == "maintenance_conflicts"
                    && row["fields"]["maintenance_round"].as_u64() == Some(*round)
            });
            let end = records.iter().find(|row| {
                row["fields"]["event.name"] == "runtime.work.finished"
                    && row["fields"]["step"] == "maintenance_conflicts"
                    && row["fields"]["maintenance_round"].as_u64() == Some(*round)
            });
            let Some((start, end)) = start
                .and_then(|row| row["timestamp"].as_str())
                .zip(end.and_then(|row| row["timestamp"].as_str()))
            else {
                return false;
            };
            records
                .iter()
                .filter(|row| {
                    row["fields"]["event.name"] == "connection.protocol.negotiated"
                        && row["fields"]["protocol"] == "membership_branch_recovery"
                        && row["timestamp"]
                            .as_str()
                            .is_some_and(|timestamp| start <= timestamp && timestamp <= end)
                })
                .count()
                >= 2
        })
        .collect()
}

fn snapshot(host: &mut HostProcess) -> Value {
    host.request(json!({"command": "membership_diagnostics"}))
}

fn wait_branch(hosts: &mut [&mut HostProcess], count: u64, stage: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let snapshots: Vec<_> = hosts.iter_mut().map(|host| snapshot(host)).collect();
        if snapshots.iter().all(|value| {
            value["branch_id"] == snapshots[0]["branch_id"]
                && value["head_event_id"] == snapshots[0]["head_event_id"]
                && value["effective_member_count"] == count
                && value["pending_effect_count"] == 0
        }) && (stage != "recovered"
            || snapshots
                .last()
                .is_some_and(|value| value["pending_conflict_count"] == 0))
        {
            return snapshots[0].clone();
        }
        if Instant::now() >= deadline {
            let facts: Vec<_> = snapshots
                .iter()
                .map(|value| {
                    json!({
                        "same_branch": value["branch_id"] == snapshots[0]["branch_id"],
                        "same_head": value["head_event_id"] == snapshots[0]["head_event_id"],
                        "members": value["effective_member_count"],
                        "pending_effects": value["pending_effect_count"],
                        "conflicts": value["pending_conflict_count"],
                    })
                })
                .collect();
            panic!("成员历史未收敛: stage={stage}, expected_members={count}, facts={facts:?}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn separate_processes_restore_selected_membership_branch() {
    run_scenario(false, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn completed_recovery_survives_recipient_process_crash() {
    run_scenario(true, None).await;
}

#[cfg(feature = "branch-recovery-fault-tests")]
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn recipient_prepared_survives_process_crash() {
    run_scenario(false, Some("recipient_prepared")).await;
}

#[cfg(feature = "branch-recovery-fault-tests")]
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn recipient_completed_survives_process_crash() {
    run_scenario(false, Some("recipient_completed")).await;
}

#[cfg(feature = "branch-recovery-fault-tests")]
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn target_prepared_survives_process_crash() {
    run_scenario(false, Some("target_prepared")).await;
}

#[cfg(feature = "branch-recovery-fault-tests")]
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn target_committed_survives_process_crash() {
    run_scenario(false, Some("target_committed")).await;
}

#[cfg(all(feature = "branch-recovery-fault-tests", unix))]
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn retried_commit_returns_the_original_recovery_package_after_issuer_crash() {
    run_scenario(false, Some("target_committed_compare")).await;
}

#[cfg(all(feature = "branch-recovery-fault-tests", unix))]
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn conflicting_commit_is_rejected_after_issuer_crash() {
    run_scenario(false, Some("target_committed_conflicting")).await;
}

#[cfg(all(feature = "branch-recovery-fault-tests", unix))]
fn receive_package_bytes(listener: &UnixListener) -> Vec<u8> {
    listener
        .set_nonblocking(true)
        .expect("configure capture socket");
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .expect("capture timeout");
                let mut bytes = Vec::new();
                stream
                    .take(32 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .expect("capture package");
                assert!(!bytes.is_empty() && bytes.len() <= 32 * 1024 * 1024);
                return bytes;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "未收到测试进程比较值");
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("capture socket failed: {error}"),
        }
    }
}

#[cfg(feature = "branch-recovery-fault-tests")]
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn space_reset_waits_for_started_issuer_commit() {
    run_scenario(false, Some("target_committed_reset")).await;
}

#[cfg(feature = "branch-recovery-fault-tests")]
#[tokio::test]
async fn space_reset_rejects_a_late_network_response_from_old_space() {
    run_scenario(false, Some("network_reset")).await;
}

#[cfg(feature = "branch-recovery-fault-tests")]
#[tokio::test]
async fn same_device_rejoin_rejects_old_recovery_response() {
    run_scenario(false, Some("network_rejoin")).await;
}

async fn run_scenario(crash_after_completion: bool, crash_at: Option<&str>) {
    let rendezvous = mount_rendezvous().await;
    let old_binary = std::env::var_os("UC_BRANCH_RECOVERY_OLD_HOST").map(PathBuf::from);
    let old_side = std::env::var("UC_BRANCH_RECOVERY_OLD_SIDE").ok();
    let expected_path = std::env::var("UC_BRANCH_RECOVERY_EXPECT_PATH").ok();
    if let Some(path) = &expected_path {
        assert!(matches!(path.as_str(), "direct" | "relay"), "未知网络路径");
    }
    assert_eq!(
        old_binary.is_some(),
        old_side.is_some(),
        "旧端路径与角色必须同时提供"
    );
    if let Some(side) = &old_side {
        assert!(
            matches!(side.as_str(), "issuer" | "recipient"),
            "未知旧端角色"
        );
    }
    let start = |side: &str| match (&old_binary, old_side.as_deref()) {
        (Some(binary), Some(selected)) if selected == side => {
            HostProcess::start_binary(&rendezvous.uri(), binary.clone(), false, None)
        }
        _ => HostProcess::start(&rendezvous.uri()),
    };
    // 目标分支两个有效来源都使用旧端，避免选择证据peer时绕开旧版签发方。
    let mut a = start("issuer");
    let mut b = start("issuer");
    let mut c = start("recipient");
    let mut d = HostProcess::start(&rendezvous.uri());
    let mut e = HostProcess::start(&rendezvous.uri());
    if expected_path.is_some() {
        if old_side.as_deref() == Some("recipient") {
            for host in [&mut a, &mut b] {
                host.request(json!({"command": "start_network_capture"}));
            }
        } else {
            c.request(json!({"command": "start_network_capture"}));
        }
    } else if crash_at.is_none() && old_side.is_none() {
        c.request(json!({"command": "start_network_capture"}));
    }
    if let Some(side) = old_side.as_deref() {
        let old_hosts: Vec<&HostProcess> = if side == "issuer" {
            vec![&a, &b]
        } else {
            vec![&c]
        };
        for host in old_hosts {
            assert_eq!(
                host.host_info["engine_rev"],
                "5cbca8fc24a278fa7b01ae611bf4b925914bef13"
            );
            assert!(host.host_info["capabilities"]
                .as_array()
                .is_some_and(|caps| caps.iter().any(|cap| cap == "branch-recovery-tests")));
        }
    }
    a.request(json!({"command": "create", "name": "A"}));
    join_host(&mut a, &mut b, "B");
    join_host(&mut a, &mut c, "C");
    join_host(&mut a, &mut d, "D");
    join_host(&mut a, &mut e, "E");
    wait_branch(&mut [&mut a, &mut b, &mut c, &mut d, &mut e], 5, "initial");
    #[cfg(feature = "branch-recovery-fault-tests")]
    let rejoin_invitations = if crash_at == Some("network_rejoin") {
        Some([
            a.request(json!({"command": "invite"}))["invitation"].clone(),
            b.request(json!({"command": "invite"}))["invitation"].clone(),
        ])
    } else {
        None
    };
    let ids: Vec<_> = [&mut a, &mut b, &mut c, &mut d, &mut e]
        .into_iter()
        .map(|host| host.request(json!({"command": "network_endpoint"})))
        .collect();
    let device_d =
        d.request(json!({"command": "eligibility"}))["device_trust"]["local_device_id"].clone();
    let device_e =
        e.request(json!({"command": "eligibility"}))["device_trust"]["local_device_id"].clone();
    assert!(device_d.is_string() && device_e.is_string());
    for host in [&mut a, &mut b, &mut d] {
        host.request(json!({"command": "set_network_partition", "endpoint_ids": [ids[2], ids[4]]}));
    }
    for host in [&mut c, &mut e] {
        host.request(
            json!({"command": "set_network_partition", "endpoint_ids": [ids[0], ids[1], ids[3]]}),
        );
    }
    b.request(json!({"command": "remove", "peer": device_d}));
    c.request(json!({"command": "remove", "peer": device_e}));
    let target = wait_branch(&mut [&mut b], 4, "target");
    let other = wait_branch(&mut [&mut c], 4, "other");
    assert_ne!(target["branch_id"], other["branch_id"]);
    let selected = format!("b:{}", target["branch_id"].as_str().expect("branch id"));
    let recipient_gate = c.root.path().join("branch-recovery-test-gate");
    let issuer_gates = [
        a.root.path().join("branch-recovery-test-gate"),
        b.root.path().join("branch-recovery-test-gate"),
    ];
    #[cfg(all(feature = "branch-recovery-fault-tests", unix))]
    let captures = if matches!(
        crash_at,
        Some("target_committed_compare" | "target_committed_conflicting")
    ) {
        for gate in issuer_gates.iter().chain(std::iter::once(&recipient_gate)) {
            std::fs::create_dir_all(gate).expect("create capture gate");
        }
        std::fs::write(recipient_gate.join("arm"), "recipient_completed")
            .expect("arm recipient capture");
        Some((
            UnixListener::bind(issuer_gates[0].join("capture.sock")).expect("issuer socket"),
            UnixListener::bind(issuer_gates[1].join("capture.sock")).expect("issuer socket"),
            UnixListener::bind(recipient_gate.join("capture.sock")).expect("recipient socket"),
        ))
    } else {
        None
    };
    if matches!(crash_at, Some("network_reset" | "network_rejoin")) {
        for gate in &issuer_gates {
            std::fs::create_dir_all(gate).expect("create network gate");
            std::fs::write(gate.join("network_arm"), b"armed").expect("arm network gate");
        }
    } else if let Some(phase) = crash_at {
        let armed_phase = if matches!(
            phase,
            "target_committed_reset" | "target_committed_compare" | "target_committed_conflicting"
        ) {
            "target_committed"
        } else {
            phase
        };
        let gates: Vec<_> = if phase.starts_with("target_") {
            issuer_gates.iter().collect()
        } else {
            vec![&recipient_gate]
        };
        for gate in gates {
            std::fs::create_dir_all(gate).expect("create checkpoint gate");
            std::fs::write(gate.join("arm"), armed_phase).expect("arm checkpoint gate");
        }
    }
    for host in [&mut a, &mut b, &mut c, &mut d, &mut e] {
        host.request(json!({"command": "set_network_partition", "endpoint_ids": []}));
        host.request(json!({"command": "opportunity"}));
    }
    let prior_rounds = if crash_at.is_none() && old_side.is_none() {
        Some(completed_conflict_rounds(&mut c))
    } else {
        None
    };
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let choices = c.request(json!({"command": "eligibility"}));
        let issue = choices["issues"].as_array().and_then(|issues| {
            issues.iter().find(|issue| {
                issue["choices"].as_array().is_some_and(|choices| {
                    choices.iter().any(|choice| choice["choice_id"] == selected)
                })
            })
        });
        if let Some(issue) = issue {
            let result = c.request(json!({"command": "choose_device_group", "issue_id": issue["issue_id"], "choice_id": selected, "revision": choices["revision"]}));
            match result["outcome"].as_str() {
                Some("completed" | "pending" | "already_completed") => break,
                Some("state_changed") => {}
                _ => panic!("组选择未被接受: outcome={:?}", result["outcome"]),
            }
        }
        assert!(Instant::now() < deadline, "没有发现目标分支冲突");
        std::thread::sleep(Duration::from_millis(100));
    }
    #[cfg(feature = "branch-recovery-fault-tests")]
    if matches!(crash_at, Some("network_reset" | "network_rejoin")) {
        let deadline = Instant::now() + Duration::from_secs(90);
        while !issuer_gates
            .iter()
            .any(|gate| gate.join("network_reached").exists())
        {
            assert!(Instant::now() < deadline, "未暂停真实网络响应");
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut rejoined_state = None;
        if crash_at == Some("network_reset") {
            c.send_without_waiting(json!({"command": "reset_space"}));
            assert_eq!(c.read_response()["reset_polled"], true);
            let reset = c.read_response();
            assert!(reset.get("ok").is_some(), "Space重置失败: {reset}");
        } else {
            let recipient = c.request(json!({"command": "eligibility"}))["device_trust"]
                ["local_device_id"]
                .clone();
            assert!(recipient.is_string());
            let selected = usize::from(!issuer_gates[0].join("network_reached").exists());
            let sponsor = if selected == 0 { &mut a } else { &mut b };
            wait_branch(&mut [sponsor], 4, "before_removal");
            sponsor.request(json!({"command": "remove", "peer": recipient}));
            wait_branch(&mut [sponsor], 3, "removed");
            let invitation = &rejoin_invitations.as_ref().expect("rejoin invitations")[selected];
            c.request(json!({"command": "join", "name": "C", "invitation": invitation}));
            process::wait_for_active_join(&mut c);
            assert_eq!(
                c.request(json!({"command": "eligibility"}))["device_trust"]["local_device_id"],
                recipient,
                "重新加入必须保留同一设备身份"
            );
            wait_branch(&mut [sponsor, &mut c], 4, "rejoined");
            rejoined_state = Some(snapshot(&mut c));
        }
        for gate in &issuer_gates {
            std::fs::remove_file(gate.join("network_arm")).expect("disarm network gate");
            std::fs::write(gate.join("network_release"), b"release").expect("release network gate");
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while !issuer_gates
            .iter()
            .any(|gate| gate.join("network_attempted").exists())
        {
            assert!(Instant::now() < deadline, "旧响应未尝试写入");
            std::thread::sleep(Duration::from_millis(20));
        }
        let expected_count = if crash_at == Some("network_reset") {
            1
        } else {
            4
        };
        let current = wait_branch(&mut [&mut c], expected_count, "after_late_response");
        if crash_at == Some("network_reset") {
            assert_ne!(current["branch_id"], target["branch_id"]);
            assert_eq!(current["pending_conflict_count"], 0);
        } else if let Some(rejoined) = rejoined_state {
            assert_eq!(current["branch_id"], rejoined["branch_id"]);
            assert_eq!(current["head_event_id"], rejoined["head_event_id"]);
            assert_eq!(
                current["pending_conflict_count"],
                rejoined["pending_conflict_count"]
            );
        }
        for host in [&mut a, &mut b, &mut c, &mut d, &mut e] {
            host.shutdown();
        }
        return;
    }
    if let Some(phase) = crash_at {
        let deadline = Instant::now() + Duration::from_secs(90);
        #[cfg(all(feature = "branch-recovery-fault-tests", unix))]
        let mut original_package = None;
        if phase.starts_with("target_") {
            while !issuer_gates
                .iter()
                .any(|gate| gate.join("reached").exists())
            {
                assert!(Instant::now() < deadline, "未到达签发方恢复检查点");
                std::thread::sleep(Duration::from_millis(20));
            }
            let selected = usize::from(!issuer_gates[0].join("reached").exists());
            #[cfg(all(feature = "branch-recovery-fault-tests", unix))]
            if let Some((first, second, _)) = &captures {
                original_package = Some(receive_package_bytes(if selected == 0 {
                    first
                } else {
                    second
                }));
            }
            #[cfg(all(feature = "branch-recovery-fault-tests", unix))]
            if phase == "target_committed_conflicting" {
                std::fs::write(recipient_gate.join("mutate_commit"), b"enabled")
                    .expect("arm conflicting commit");
            }
            let blocked: Vec<_> = [0, 1, 3, 4]
                .into_iter()
                .filter(|index| *index != selected)
                .map(|index| ids[index].clone())
                .collect();
            // 丢失响应后只允许原签发方继续提供恢复，不能由另一个候选掩盖重试失败。
            c.request(json!({"command": "set_network_partition", "endpoint_ids": blocked}));
            for gate in &issuer_gates {
                std::fs::remove_file(gate.join("arm")).expect("disarm checkpoint gate");
            }
            std::fs::write(issuer_gates[1 - selected].join("release"), b"release")
                .expect("release other issuer");
            #[cfg(feature = "branch-recovery-fault-tests")]
            if phase == "target_committed_reset" {
                let host = if selected == 0 { &mut a } else { &mut b };
                host.send_without_waiting(json!({"command": "reset_space"}));
                assert_eq!(host.read_response()["reset_polled"], true);
                assert!(
                    !host.has_response_within(Duration::from_millis(250)),
                    "Space重置不能在已开始的签发工作结算前结束"
                );
                std::fs::write(issuer_gates[selected].join("release"), b"release")
                    .expect("release issuer checkpoint");
                let reset = host.read_response();
                assert!(reset.get("ok").is_some(), "Space重置失败: {reset}");
                let new_space = wait_branch(&mut [host], 1, "new_space");
                assert_ne!(new_space["branch_id"], target["branch_id"]);
                assert_eq!(new_space["pending_conflict_count"], 0);
                for host in [&mut a, &mut b, &mut c, &mut d, &mut e] {
                    host.shutdown();
                }
                return;
            }
            if selected == 0 {
                a.restart_with_crash(true);
            } else {
                b.restart_with_crash(true);
            }
        } else {
            while !recipient_gate.join("reached").exists() {
                assert!(Instant::now() < deadline, "未到达接收方恢复检查点");
                std::thread::sleep(Duration::from_millis(20));
            }
            std::fs::remove_file(recipient_gate.join("arm")).expect("disarm checkpoint gate");
            c.restart_with_crash(true);
        }
        c.request(json!({"command": "opportunity"}));
        b.request(json!({"command": "opportunity"}));
        #[cfg(all(feature = "branch-recovery-fault-tests", unix))]
        if phase == "target_committed_conflicting" {
            while !recipient_gate.join("rejected").exists() {
                assert!(Instant::now() < deadline, "不同commit未收到签发方拒绝");
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(recipient_gate.join("mutated").exists());
            std::fs::remove_file(recipient_gate.join("mutate_commit"))
                .expect("disarm conflicting commit");
            c.request(json!({"command": "opportunity"}));
        }
        #[cfg(all(feature = "branch-recovery-fault-tests", unix))]
        if let (Some(original), Some((_, _, recipient))) = (original_package, &captures) {
            let recovery_deadline = Instant::now() + Duration::from_secs(90);
            while !recipient_gate.join("reached").exists() {
                assert!(
                    Instant::now() < recovery_deadline,
                    "重试后未持久化接收方恢复包"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            let received = receive_package_bytes(recipient);
            assert!(original == received, "重试后恢复包与原签发结果不一致");
            std::fs::remove_file(recipient_gate.join("arm")).expect("disarm recipient capture");
            std::fs::write(recipient_gate.join("release"), b"release")
                .expect("release recipient capture");
        }
    }
    let recovered = wait_branch(&mut [&mut b, &mut c], 4, "recovered");
    assert_eq!(recovered["branch_id"], target["branch_id"]);
    assert_eq!(snapshot(&mut c)["pending_conflict_count"], 0);
    if let Some(prior_rounds) = prior_rounds {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if completed_conflict_rounds(&mut c)
                .difference(&prior_rounds)
                .next()
                .is_some()
            {
                break;
            }
            assert!(Instant::now() < deadline, "恢复后缺少关联的维护轮次结算");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    if let Some(expected) = expected_path {
        let mut paths = HashMap::new();
        if old_side.as_deref() == Some("recipient") {
            for host in [&mut a, &mut b] {
                // 连接编号只在单进程内唯一，分别保存每个进程的结果。
                for (_, observed) in recovery_paths(host) {
                    paths.insert(paths.len(), observed);
                }
            }
        } else {
            for (_, observed) in recovery_paths(&mut c) {
                paths.insert(paths.len(), observed);
            }
        }
        assert!(paths.len() >= 2, "需要至少两条已协商的恢复连接");
        assert!(
            paths
                .values()
                .all(|observed| observed.len() == 1 && observed.contains(&expected)),
            "恢复连接缺少唯一的预期选路证据"
        );
        eprintln!(
            "恢复连接路径验证通过: path={expected}, count={}",
            paths.len()
        );
    }
    if crash_after_completion {
        // 已完成的接收端不执行正常关闭，重启后仍须从持久记录恢复相同分支。
        c.restart_with_crash(true);
        c.request(json!({"command": "opportunity"}));
        let restarted = wait_branch(&mut [&mut b, &mut c], 4, "recovered");
        assert_eq!(restarted["branch_id"], target["branch_id"]);
        assert_eq!(snapshot(&mut c)["pending_conflict_count"], 0);
    }
    for host in [&mut a, &mut b, &mut c, &mut d, &mut e] {
        host.shutdown();
    }
}
