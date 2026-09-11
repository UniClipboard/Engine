use super::*;

// 只读取目标状态；禁止通过刷新或发送促成连接。
pub(super) async fn wait_online(engine: &Engine, peer_id: &str) {
    wait_online_within(engine, peer_id, Duration::from_secs(15)).await;
}

async fn wait_online_within(engine: &Engine, peer_id: &str, budget: Duration) {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if let Ok(OperationResult::PeerConnections(peers)) =
            engine.execute(Operation::QueryPeerConnections).await
        {
            if peers
                .iter()
                .any(|peer| peer.peer_id == peer_id && peer.connected)
            {
                return;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            if let Ok(OperationResult::DeviceGroupChoices(summary)) =
                engine.execute(Operation::QueryDeviceGroupChoices).await
            {
                if let Some(target) = summary
                    .device_trust
                    .devices
                    .iter()
                    .find(|device| device.device_id == peer_id)
                {
                    eprintln!(
                        "target state: reachability={:?} membership={:?} sync={:?} blocked={:?}",
                        target.reachability,
                        target.membership,
                        target.sync_relationship,
                        target.blocked_reason
                    );
                }
            }
            panic!("paired peer did not become online automatically within {budget:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// 名单中的 Active 不是普通通信资格。新成员的历史核对属于连接计时的前置条件。
pub(super) async fn wait_eligible(engine: &Engine, peer_id: &str) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        let OperationResult::DeviceGroupChoices(summary) = engine
            .execute(Operation::QueryDeviceGroupChoices)
            .await
            .unwrap()
        else {
            panic!("device group choices expected")
        };
        if summary.device_trust.devices.iter().any(|device| {
            device.device_id == peer_id
                && device.sync_relationship == uc_engine::DeviceSyncRelationshipSummary::Usable
        }) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "new member did not obtain normal communication eligibility within the membership recovery budget"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn paired_devices_cold_start_without_refresh() {
    let rendezvous = mount_rendezvous().await;
    let a_host = DeviceHarness::new(rendezvous.uri());
    let b_host = DeviceHarness::new(rendezvous.uri());
    let a = a_host.start_with_relay_fallback(false).await;
    let b = b_host.start_with_relay_fallback(false).await;
    let space_id = create_space(&a, "A").await.0;
    let b_id = join_through(&a, &b, "B", &space_id).await.self_device_id;
    wait_for_active_member_count(&a, 2).await;
    wait_for_active_member_count(&b, 2).await;
    let OperationResult::PeerConnections(peers) =
        b.execute(Operation::QueryPeerConnections).await.unwrap()
    else {
        panic!("expected peer connections")
    };
    let a_id = peers.first().expect("paired A").peer_id.clone();
    a.shutdown(SHUTDOWN_TIMEOUT).await.unwrap();
    b.shutdown(SHUTDOWN_TIMEOUT).await.unwrap();

    let (a, b) = tokio::join!(
        a_host.start_with_relay_fallback(false),
        b_host.start_with_relay_fallback(false)
    );
    tokio::join!(wait_online(&a, &b_id), wait_online(&b, &a_id));
    for (sender, receiver, target, text) in [
        (&a, &b, b_id, "automatic cold start A to B"),
        (&b, &a, a_id, "automatic cold start B to A"),
    ] {
        sender
            .execute(Operation::SendText(SendTextInput {
                text: text.to_owned(),
                target_devices: vec![target],
            }))
            .await
            .unwrap();
        wait_for_received_text(receiver, text).await;
    }
    a.shutdown(SHUTDOWN_TIMEOUT).await.unwrap();
    b.shutdown(SHUTDOWN_TIMEOUT).await.unwrap();
}

struct Pair {
    _rendezvous: MockServer,
    hosts: [DeviceHarness; 2],
    engines: [Engine; 2],
    ids: [String; 2],
    space: String,
}

impl Pair {
    async fn new() -> Self {
        let rendezvous = mount_rendezvous().await;
        let hosts = [
            DeviceHarness::new(rendezvous.uri()),
            DeviceHarness::new(rendezvous.uri()),
        ];
        let a = hosts[0].start_with_relay_fallback(false).await;
        let b = hosts[1].start_with_relay_fallback(false).await;
        let space = create_space(&a, "A").await.0;
        let b_id = join_through(&a, &b, "B", &space).await.self_device_id;
        wait_for_active_member_count(&a, 2).await;
        wait_for_active_member_count(&b, 2).await;
        let OperationResult::PeerConnections(peers) =
            b.execute(Operation::QueryPeerConnections).await.unwrap()
        else {
            panic!("peer connections expected")
        };
        let a_id = peers.first().unwrap().peer_id.clone();
        let pair = Self {
            _rendezvous: rendezvous,
            hosts,
            engines: [a, b],
            ids: [a_id, b_id],
            space,
        };
        pair.online().await;
        pair
    }

    async fn online(&self) {
        tokio::join!(
            wait_online(&self.engines[0], &self.ids[1]),
            wait_online(&self.engines[1], &self.ids[0])
        );
        for engine in &self.engines {
            wait_for_active_member_count(engine, 2).await;
        }
    }

    async fn transfer(&self, phase: &str) {
        for sender in 0..2 {
            let receiver = 1 - sender;
            let text = format!("automatic connection {phase} direction {sender}");
            let OperationResult::EntrySent(report) = self.engines[sender]
                .execute(Operation::SendText(SendTextInput {
                    text: text.clone(),
                    target_devices: vec![self.ids[receiver].clone()],
                }))
                .await
                .unwrap()
            else {
                panic!("send result expected")
            };
            assert_eq!(report.total_accepted, 1);
            wait_for_received_text(&self.engines[receiver], &text).await;
        }
    }

    async fn partition(&self, block: bool) {
        let endpoints = [
            query_endpoint_id(&self.engines[0], "A").await,
            query_endpoint_id(&self.engines[1], "B").await,
        ];
        for index in 0..2 {
            self.engines[index]
                .execute_dev(uc_engine::DevOperation::SetNetworkPartition {
                    blocked_endpoint_ids: if block {
                        vec![endpoints[1 - index]]
                    } else {
                        vec![]
                    },
                })
                .await
                .unwrap();
        }
    }

    async fn shutdown(&self) {
        for engine in &self.engines {
            engine.shutdown(SHUTDOWN_TIMEOUT).await.unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn either_peer_can_start_later_without_refresh() {
    for early in 0..2 {
        let mut pair = Pair::new().await;
        pair.shutdown().await;
        pair.engines[early] = pair.hosts[early].start_with_relay_fallback(false).await;
        // 明确制造对方尚未启动、首次尝试已可能失败的时间段；成功仍用条件等待。
        tokio::time::sleep(Duration::from_secs(6)).await;
        pair.engines[1 - early] = pair.hosts[1 - early].start_with_relay_fallback(false).await;
        pair.online().await;
        pair.transfer("later startup").await;
        pair.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn either_peer_restart_and_suspend_resume_reconnect_automatically() {
    let mut pair = Pair::new().await;
    for index in 0..2 {
        pair.engines[index]
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .unwrap();
        pair.engines[index] = pair.hosts[index].start_with_relay_fallback(false).await;
        pair.online().await;
        pair.transfer(&format!("restart {index}")).await;
        pair.engines[index].suspend().await.unwrap();
        assert!(pair.engines[index]
            .execute(Operation::NotifyConnectivityOpportunity {
                reason: uc_engine::ConnectivityOpportunity::Foreground,
            })
            .await
            .is_err());
        pair.engines[index].resume().await.unwrap();
        pair.online().await;
        pair.transfer(&format!("resume {index}")).await;
    }
    pair.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn network_partition_recovers_without_refresh_or_send() {
    let pair = Pair::new().await;
    pair.partition(true).await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    for engine in &pair.engines {
        let OperationResult::PeerConnections(peers) = engine
            .execute(Operation::QueryPeerConnections)
            .await
            .unwrap()
        else {
            panic!("peer connections expected")
        };
        assert!(peers.iter().all(|peer| !peer.connected));
    }
    pair.partition(false).await;
    pair.online().await;
    pair.transfer("network restored").await;
    pair.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn long_partition_recovers_without_refresh_or_host_notification() {
    let pair = Pair::new().await;
    pair.partition(true).await;
    // 等待跨过全部短退避级别；两端保持运行，不用进程启动或宿主通知帮助恢复。
    tokio::time::sleep(Duration::from_secs(110)).await;
    for engine in &pair.engines {
        let OperationResult::PeerConnections(peers) = engine
            .execute(Operation::QueryPeerConnections)
            .await
            .unwrap()
        else {
            panic!("peer connections expected")
        };
        assert!(peers.iter().all(|peer| !peer.connected));
    }
    pair.partition(false).await;
    tokio::join!(
        wait_online_within(&pair.engines[0], &pair.ids[1], Duration::from_secs(85)),
        wait_online_within(&pair.engines[1], &pair.ids[0], Duration::from_secs(85)),
    );
    pair.online().await;
    pair.transfer("long network interruption").await;
    pair.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn concurrent_manual_refreshes_share_automatic_recovery_without_disrupting_delivery() {
    let pair = Pair::new().await;
    pair.partition(true).await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    pair.partition(false).await;
    let (first, second) = tokio::join!(
        pair.engines[0].execute(Operation::RefreshPeerConnections),
        pair.engines[0].execute(Operation::RefreshPeerConnections),
    );
    for result in [first, second] {
        let OperationResult::PeerConnectionsRefreshed(report) = result.unwrap() else {
            panic!("refresh report expected")
        };
        assert_eq!(report.total, 1);
        assert_eq!(report.online, 1);
        assert_eq!(report.offline, 0);
        assert_eq!(report.errors, 0);
    }
    pair.online().await;
    pair.transfer("concurrent manual and automatic recovery")
        .await;
    pair.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn host_opportunities_are_nonblocking_and_recover_connections() {
    let pair = Pair::new().await;
    for reason in [
        uc_engine::ConnectivityOpportunity::Foreground,
        uc_engine::ConnectivityOpportunity::SystemWake,
        uc_engine::ConnectivityOpportunity::NetworkChanged,
    ] {
        pair.partition(true).await;
        tokio::time::sleep(Duration::from_secs(6)).await;
        pair.partition(false).await;
        for engine in &pair.engines {
            assert!(matches!(
                tokio::time::timeout(
                    Duration::from_secs(1),
                    engine.execute(Operation::NotifyConnectivityOpportunity { reason })
                )
                .await
                .unwrap()
                .unwrap(),
                OperationResult::ConnectivityOpportunityAccepted
            ));
        }
        pair.online().await;
        pair.transfer(&format!("opportunity {reason:?}")).await;
    }
    pair.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn offline_member_does_not_block_another_members_restart() {
    let pair = Pair::new().await;
    eprintln!("triple: original pair ready");
    let c_host = DeviceHarness::new(pair._rendezvous.uri());
    let c = c_host.start_with_relay_fallback(false).await;
    let c_id = join_through(&pair.engines[0], &c, "C", &pair.space)
        .await
        .self_device_id;
    for engine in [&pair.engines[0], &pair.engines[1], &c] {
        wait_for_active_member_count(engine, 3).await;
    }
    tokio::join!(
        wait_eligible(&pair.engines[0], &c_id),
        wait_eligible(&pair.engines[1], &c_id),
        wait_eligible(&c, &pair.ids[0]),
    );
    eprintln!("triple: A to C after admission");
    wait_online(&pair.engines[0], &c_id).await;
    eprintln!("triple: B to C after admission");
    wait_online(&pair.engines[1], &c_id).await;
    eprintln!("triple: C to A after admission");
    wait_online(&c, &pair.ids[0]).await;
    pair.engines[1].shutdown(SHUTDOWN_TIMEOUT).await.unwrap();
    c.shutdown(SHUTDOWN_TIMEOUT).await.unwrap();
    let c = c_host.start_with_relay_fallback(false).await;
    eprintln!("triple: A and C after restart with B offline");
    tokio::join!(
        wait_online(&pair.engines[0], &c_id),
        wait_online(&c, &pair.ids[0])
    );
    let text = "one offline member must not block automatic recovery";
    pair.engines[0]
        .execute(Operation::SendText(SendTextInput {
            text: text.into(),
            target_devices: vec![c_id],
        }))
        .await
        .unwrap();
    wait_for_received_text(&c, text).await;
    pair.engines[0].shutdown(SHUTDOWN_TIMEOUT).await.unwrap();
    c.shutdown(SHUTDOWN_TIMEOUT).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn removed_member_is_not_reconnected_by_host_opportunities() {
    let pair = Pair::new().await;
    pair.engines[0]
        .execute(Operation::RemoveMember(RemoveMemberInput {
            device_id: pair.ids[1].clone(),
        }))
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        let OperationResult::PeerConnections(peers) = pair.engines[0]
            .execute(Operation::QueryPeerConnections)
            .await
            .unwrap()
        else {
            panic!("expected peer connections")
        };
        if peers.iter().all(|peer| peer.peer_id != pair.ids[1]) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "removed peer remained visible"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    pair.engines[0]
        .execute(Operation::NotifyConnectivityOpportunity {
            reason: uc_engine::ConnectivityOpportunity::NetworkChanged,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let OperationResult::PeerConnections(peers) = pair.engines[0]
        .execute(Operation::QueryPeerConnections)
        .await
        .unwrap()
    else {
        panic!("expected peer connections")
    };
    assert!(peers.iter().all(|peer| peer.peer_id != pair.ids[1]));
    pair.shutdown().await;
}
