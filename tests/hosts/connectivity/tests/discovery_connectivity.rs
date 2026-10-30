#![cfg(all(feature = "current-engine", target_os = "linux"))]
//! 真正重启两端并阻断旧路由，以mDNS负／正对照排除地址缓存和已有连接。
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[path = "common/process.rs"]
mod process;
use process::{join_host, mount_rendezvous, wait_for_phase, HostProcess, WAIT_TIMEOUT};

fn nft(args: &[&str]) {
    let output = Command::new("nft").args(args).output().unwrap();
    assert!(
        output.status.success(),
        "nft failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn records(host: &mut HostProcess, role: &str, phase: &str) -> Vec<Value> {
    host.request(json!({"command": "flush"}));
    let rows: Vec<Value> = std::fs::read_dir(host.root.path().join("logs"))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .flat_map(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .collect::<Vec<_>>()
        })
        .collect();
    let out = PathBuf::from(std::env::var_os("UC_TEST_ARTIFACTS_DIR").unwrap());
    std::fs::write(
        out.join(format!("{phase}-{role}.json")),
        serde_json::to_vec_pretty(&rows).unwrap(),
    )
    .unwrap();
    rows
}

fn contains(rows: &[Value], event: &str, source: &str) -> bool {
    rows.iter()
        .any(|row| row["fields"]["event.name"] == event && row["fields"]["source"] == source)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "通过run-discovery-connectivity.sh在独立网络namespace中执行"]
async fn cloud_outage_uses_only_mdns_after_both_processes_restart() {
    assert_eq!(
        std::env::var("UC_MDNS_DISCOVERY_ISOLATED").as_deref(),
        Ok("1")
    );
    assert!(std::env::var_os("UC_TEST_CLOUD_DISCOVERY").is_some());
    assert!(std::env::var_os("UC_MEMBERSHIP_HISTORY_TEST_RELAY").is_none());
    let server = mount_rendezvous().await;
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_uc-connectivity-host"));
    let mut sender = HostProcess::start_binary(&server.uri(), binary.clone(), false, Some(23101));
    let mut receiver = HostProcess::start_binary(&server.uri(), binary, false, Some(23102));
    sender.request(json!({"command": "create", "name": "D15 sender"}));
    join_host(&mut sender, &mut receiver, "D15 receiver");
    wait_for_phase(&mut sender, "completed");
    wait_for_phase(&mut receiver, "completed");
    let peer = receiver.request(json!({"command": "eligibility"}))["device_trust"]
        ["local_device_id"]
        .clone();
    let identities = [
        sender.request(json!({"command": "network_endpoint"})),
        receiver.request(json!({"command": "network_endpoint"})),
    ];
    // IPv4／IPv6旧端口都永远不可用；mDNS也在新进程启动前阻断。
    nft(&[
        "add",
        "rule",
        "inet",
        "discovery_test",
        "output",
        "udp",
        "dport",
        "{ 23101, 23102 }",
        "counter",
        "drop",
    ]);
    nft(&[
        "add",
        "rule",
        "inet",
        "discovery_test",
        "mdns",
        "udp",
        "dport",
        "5353",
        "counter",
        "drop",
    ]);
    let sender_keys = sender.stop_for_discovery_restart();
    let receiver_keys = receiver.stop_for_discovery_restart();
    for host in [&sender, &receiver] {
        std::fs::rename(
            host.root.path().join("logs"),
            host.root.path().join("bootstrap-logs"),
        )
        .unwrap();
    }
    sender.resume_on_port(sender_keys, 23201);
    receiver.resume_on_port(receiver_keys, 23202);
    assert_eq!(
        sender.request(json!({"command": "network_endpoint"})),
        identities[0]
    );
    assert_eq!(
        receiver.request(json!({"command": "network_endpoint"})),
        identities[1]
    );
    for host in [&mut sender, &mut receiver] {
        host.request(json!({"command": "start_network_capture"}));
    }
    sender.request(json!({"command": "send", "peer": peer, "text": "D15 blocked control"}));
    let deadline = Instant::now() + WAIT_TIMEOUT;
    let mut leaked = false;
    while Instant::now() < deadline {
        if receiver.request(json!({"command": "history"})) != json!([]) {
            leaked = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut blocked = records(&mut sender, "sender", "blocked");
    blocked.extend(records(&mut receiver, "receiver", "blocked"));
    assert!(!leaked, "mDNS关闭时不应通过缓存、旧连接或云端收到内容");
    assert!(!contains(&blocked, "address.discovered", "mdns"));
    assert!(
        contains(&blocked, "address.lookup.started", "dns"),
        "必须实际尝试Cloud，而非关闭Cloud装配"
    );
    assert!(!contains(&blocked, "address.discovered", "dns"));
    assert!(
        blocked.iter().any(|row| {
            let fields = &row["fields"];
            fields["event.name"] == "address.lookup.finished"
                && fields["source"] == "dns"
                && fields["error_count"]
                    .as_u64()
                    .is_some_and(|count| count > 0)
                && fields["result_count"] == 0
                && fields["interrupted"] == false
        }),
        "必须保留实际Cloud失败终态"
    );
    assert!(
        !blocked
            .iter()
            .any(|row| row["fields"]["event.name"] == "connection.path.observed"),
        "负对照不能有成功连接路径"
    );
    let firewall = Command::new("nft")
        .args(["-j", "list", "table", "inet", "discovery_test"])
        .output()
        .unwrap();
    assert!(firewall.status.success());
    let firewall: Value = serde_json::from_slice(&firewall.stdout).unwrap();
    let rules = firewall["nftables"].as_array().unwrap();
    for chain in ["output", "mdns"] {
        assert!(
            rules.iter().any(|row| row["rule"]["chain"] == chain
                && row["rule"]["expr"]
                    .as_array()
                    .is_some_and(|exprs| exprs.iter().any(|expr| expr["counter"]["packets"]
                        .as_u64()
                        .is_some_and(|packets| packets > 0)))),
            "{chain}阻断必须实际命中包"
        );
    }
    let out = PathBuf::from(std::env::var_os("UC_TEST_ARTIFACTS_DIR").unwrap());
    std::fs::write(
        out.join("blocked-firewall.json"),
        serde_json::to_vec_pretty(&firewall).unwrap(),
    )
    .unwrap();
    // 唯一网络变化是允许mDNS，旧端口及云隔离继续保持。
    nft(&["flush", "chain", "inet", "discovery_test", "mdns"]);
    sender.request(json!({"command": "opportunity"}));
    receiver.request(json!({"command": "opportunity"}));
    let text = "D15 mDNS only positive content";
    sender.request(json!({"command": "send", "peer": peer, "text": text}));
    let expected = format!("{:x}", Sha256::digest(text.as_bytes()));
    let deadline = Instant::now() + WAIT_TIMEOUT;
    let applied = loop {
        let clipboard = receiver.request(json!({"command": "clipboard"}));
        if clipboard["representations"].as_array().is_some_and(|reps| {
            reps.iter()
                .any(|rep| rep["sha256"] == expected && rep["size"] == text.len())
        }) {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut positive = records(&mut sender, "sender", "positive");
    positive.extend(records(&mut receiver, "receiver", "positive"));
    assert!(applied, "只放行mDNS后必须实际认证、同步并读回准确系统内容");
    assert!(contains(&positive, "address.discovered", "mdns"));
    assert!(!contains(&positive, "address.discovered", "dns"));
    assert!(positive.iter().any(
        |row| row["fields"]["event.name"] == "connection.path.observed"
            && row["fields"]["path_kind"] == "direct"
    ));
    sender.shutdown();
    receiver.shutdown();
}
