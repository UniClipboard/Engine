//! 跨类别共用的公开操作与等待。

use super::*;

pub(crate) fn unix_time_ns(time: SystemTime) -> u64 {
    u64::try_from(
        time.duration_since(UNIX_EPOCH)
            .expect("wall clock after epoch")
            .as_nanos(),
    )
    .unwrap_or(u64::MAX)
}

pub(crate) fn otlp_string_attribute<'a>(
    attributes: &'a [opentelemetry_proto::tonic::common::v1::KeyValue],
    key: &str,
) -> Option<&'a str> {
    attributes
        .iter()
        .find(|attribute| attribute.key == key)
        .and_then(|attribute| attribute.value.as_ref())
        .and_then(|value| value.value.as_ref())
        .and_then(|value| match value {
            OtlpValue::StringValue(value) => Some(value.as_str()),
            _ => None,
        })
}

pub(crate) async fn wait_for_active_member_count(engine: &Engine, expected: u32) {
    let deadline = tokio::time::Instant::now() + ADMISSION_WAIT_TIMEOUT;
    loop {
        if let Ok(OperationResult::MembershipDiagnostics(summary)) =
            engine.execute(Operation::QueryMembershipDiagnostics).await
        {
            if summary.effective_member_count == expected {
                return;
            }
        }
        scenario::ensure_before(deadline, "wait-for-active-member-count", || {
            format!("active member count did not reach {expected}")
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub(crate) async fn wait_for_peer_refresh(engine: &Engine, label: &str) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        if let Ok(OperationResult::PeerConnectionsRefreshed(report)) =
            engine.execute(Operation::RefreshPeerConnections).await
        {
            if report.total > 0
                && report.online == report.total
                && report.offline == 0
                && report.errors == 0
            {
                return;
            }
        }
        scenario::ensure_before(deadline, "wait-for-peer-refresh", || {
            format!("{label} peer connection refresh timed out")
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub(crate) async fn query_endpoint_id(engine: &Engine, node: &str) -> [u8; 32] {
    let result = engine
        .execute_dev(uc_engine::DevOperation::QueryNetworkEndpointId)
        .await
        .unwrap_or_else(|error| panic!("node {node} endpoint query failed: {error}"));
    let uc_engine::DevOperationResult::NetworkEndpointId(endpoint_id) = result else {
        panic!("node {node} returned an unexpected endpoint result");
    };
    endpoint_id
}

pub(crate) async fn query_space_device_update(
    engine: &Engine,
) -> uc_engine::SpaceDeviceUpdateStatusSummary {
    let OperationResult::DeviceGroupChoices(summary) = engine
        .execute(Operation::QueryDeviceGroupChoices)
        .await
        .expect("query space device update")
    else {
        panic!("unexpected device group choices result");
    };
    summary.device_trust.space_device_update
}

pub(crate) async fn wait_for_space_work_event(
    engine: &Engine,
    after_sequence: u64,
    kind: uc_engine::DevSpaceWorkEventKind,
) -> uc_engine::DevSpaceWorkEvent {
    let result = tokio::time::timeout(
        ADMISSION_WAIT_TIMEOUT,
        engine.execute_dev(uc_engine::DevOperation::WaitForSpaceWorkEvent {
            after_sequence,
            kind,
        }),
    )
    .await
    .expect("Space work event was not observed")
    .expect("wait for Space work event");
    let uc_engine::DevOperationResult::SpaceWorkEvent(event) = result else {
        panic!("unexpected Space work event result");
    };
    event
}

pub(crate) async fn query_space_work_events(engine: &Engine) -> Vec<uc_engine::DevSpaceWorkEvent> {
    let result = engine
        .execute_dev(uc_engine::DevOperation::QuerySpaceWorkEvents)
        .await
        .expect("query Space work events");
    let uc_engine::DevOperationResult::SpaceWorkEvents(events) = result else {
        panic!("unexpected Space work events result");
    };
    events
}

pub(crate) async fn create_space(engine: &Engine, device_name: &str) -> (String, String) {
    let created = match engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some(device_name.to_owned()),
            passphrase: SecretString::new(PASSPHRASE),
            passphrase_confirmation: SecretString::new(PASSPHRASE),
        }))
        .await
        .expect("create space")
    {
        OperationResult::SpaceCreated {
            space_id,
            self_device_id,
            ..
        } => (space_id, self_device_id),
        other => panic!("unexpected create result: {other:?}"),
    };
    let OperationResult::SetupState(setup) = engine
        .execute(Operation::QuerySetupState)
        .await
        .expect("query created space")
    else {
        panic!("unexpected setup result after create");
    };
    assert_eq!(setup.space_id.as_deref(), Some(created.0.as_str()));
    created
}

pub(crate) struct JoinResult {
    pub(crate) self_device_id: String,
}

pub(crate) async fn join_through(
    sponsor: &Engine,
    joiner: &Engine,
    device_name: &str,
    expected_space_id: &str,
) -> JoinResult {
    let full_invitation = issue_invitation(sponsor).await;
    join_with_invitation(joiner, device_name, expected_space_id, full_invitation).await
}

pub(crate) async fn issue_invitation(sponsor: &Engine) -> String {
    issue_invitation_named(sponsor, "sponsor").await
}

pub(crate) async fn issue_invitation_named(sponsor: &Engine, sponsor_label: &str) -> String {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        match sponsor.execute(Operation::IssueInvitation).await {
            Ok(OperationResult::InvitationIssued {
                full_invitation, ..
            }) => return full_invitation,
            Ok(_) => panic!("unexpected invitation result"),
            Err(error) if error.is_retryable() && tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => panic!(
                "node {sponsor_label} issue admission invitation: {error}; retryable={}",
                error.is_retryable()
            ),
        }
    }
}

pub(crate) async fn join_with_invitation(
    joiner: &Engine,
    device_name: &str,
    expected_space_id: &str,
    full_invitation: String,
) -> JoinResult {
    let OperationResult::JoinSpace(status) = joiner
        .execute(Operation::JoinSpace(JoinSpaceInput {
            invitation_code: full_invitation,
            device_name: Some(device_name.to_owned()),
            passphrase: SecretString::new(PASSPHRASE),
            preserve_unreadable_history: false,
        }))
        .await
        .expect("start admission join")
    else {
        panic!("unexpected join result");
    };
    wait_for_completed_join(joiner, device_name, status, expected_space_id).await
}

pub(crate) async fn wait_for_completed_join(
    engine: &Engine,
    device_name: &str,
    mut status: JoinSpaceStatusSummary,
    expected_space_id: &str,
) -> JoinResult {
    let deadline = tokio::time::Instant::now() + ADMISSION_WAIT_TIMEOUT;
    loop {
        match status {
            JoinSpaceStatusSummary::Active { joined_space, .. } => {
                assert_eq!(joined_space.space_id, expected_space_id);
                return JoinResult {
                    self_device_id: joined_space.self_device_id,
                };
            }
            JoinSpaceStatusSummary::Rejected { reason, .. } => {
                panic!("admission was rejected: {reason:?}")
            }
            JoinSpaceStatusSummary::Terminated { reason, .. } => {
                panic!("admission was terminated locally: {reason:?}")
            }
            JoinSpaceStatusSummary::Pending { .. } => {}
            JoinSpaceStatusSummary::Processing { .. } => {}
            JoinSpaceStatusSummary::NeedsAttention { .. } => {
                panic!("admission requires explicit recovery")
            }
        }
        scenario::ensure_before(deadline, "wait-for-completed-join", || {
            format!("space admission timed out for {device_name}; last status: {status:?}")
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let snapshot = match engine.execute(Operation::QueryDeviceGroupChoices).await {
            Ok(OperationResult::DeviceGroupChoices(summary)) => summary.device_trust,
            Ok(_) => panic!("unexpected device trust result"),
            Err(_) => continue,
        };
        if let Some(current_join) = snapshot.current_join {
            status = current_join;
            continue;
        }
        let setup = match engine.execute(Operation::QuerySetupState).await {
            Ok(OperationResult::SetupState(setup)) => setup,
            Ok(_) => panic!("unexpected setup state result"),
            Err(_) => continue,
        };
        if setup.has_completed && setup.space_id.as_deref() == Some(expected_space_id) {
            let device = match engine.execute(Operation::QueryLocalDevice).await {
                Ok(OperationResult::LocalDevice(device)) => device,
                Ok(_) => panic!("unexpected local device result"),
                Err(_) => continue,
            };
            return JoinResult {
                self_device_id: device.device_id,
            };
        }
    }
}

pub(crate) async fn wait_for_received_text(engine: &Engine, expected_text: &str) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        if receiver_has_exact_text(engine, expected_text).await {
            return;
        }
        scenario::ensure_before(deadline, "wait-for-received-text", || {
            "content delivery timed out".to_owned()
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(crate) async fn receiver_has_exact_text(engine: &Engine, expected_text: &str) -> bool {
    let OperationResult::HistoryEntries(entries) = engine
        .execute(Operation::ListHistoryEntries(ListHistoryEntriesInput {
            limit: 100,
            offset: 0,
        }))
        .await
        .expect("list history entries")
    else {
        panic!("unexpected history list result");
    };
    for entry in entries {
        match engine
            .execute(Operation::GetHistoryEntry(HistoryEntryInput {
                entry_id: entry.entry_id,
            }))
            .await
            .expect("get history entry")
        {
            OperationResult::HistoryEntry(detail) if detail.content == expected_text => {
                return true
            }
            OperationResult::HistoryEntry(_) => {}
            other => panic!("unexpected history detail result: {other:?}"),
        }
    }
    false
}

pub(crate) async fn set_network_partition(engine: &Engine, blocked_endpoint_ids: Vec<[u8; 32]>) {
    let expected_count = blocked_endpoint_ids.len();
    let result = engine
        .execute_dev(uc_engine::DevOperation::SetNetworkPartition {
            blocked_endpoint_ids,
        })
        .await
        .expect("update test network partition");
    assert_eq!(
        result,
        uc_engine::DevOperationResult::NetworkPartitionUpdated {
            blocked_peer_count: expected_count,
        }
    );
}
