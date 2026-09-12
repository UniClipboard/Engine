use std::time::Duration;

use tokio::time::{sleep, timeout};
use wiremock::MockServer;

use super::{
    empty_engine_host, mount_engine_rendezvous, next_engine_event_matching, ENGINE_TEST_LOCK,
};
use crate::{
    CreateSpaceInput, DeviceMembershipSummary, Engine, EngineConfig, EngineEvent, EngineState,
    HistoryEntryInput, JoinSpaceInput, JoinSpaceStatusSummary, Operation, OperationResult,
    SecretString, SendTextInput,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn paired_peer_shutdown_does_not_block_local_resume_read_or_save() {
    let _guard = ENGINE_TEST_LOCK.lock().await;
    let rendezvous = MockServer::start().await;
    mount_engine_rendezvous(&rendezvous).await;
    let sponsor_root = tempfile::tempdir().unwrap();
    let local_root = tempfile::tempdir().unwrap();
    let config = EngineConfig::new("2.0.0").with_rendezvous_base_url(rendezvous.uri());
    let (sponsor, _sponsor_events) =
        Engine::start(config.clone(), empty_engine_host(sponsor_root.path()))
            .await
            .unwrap();
    let (local, mut local_events) = Engine::start(config, empty_engine_host(local_root.path()))
        .await
        .unwrap();
    sponsor
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("offline lifecycle sponsor".into()),
            passphrase: SecretString::new("offline-lifecycle-passphrase"),
            passphrase_confirmation: SecretString::new("offline-lifecycle-passphrase"),
        }))
        .await
        .unwrap();
    let OperationResult::InvitationIssued {
        invitation_code, ..
    } = sponsor.execute(Operation::IssueInvitation).await.unwrap()
    else {
        panic!("expected invitation");
    };
    let OperationResult::JoinSpace(status) = local
        .execute(Operation::JoinSpace(JoinSpaceInput {
            invitation_code,
            device_name: Some("offline lifecycle local".into()),
            passphrase: SecretString::new("offline-lifecycle-passphrase"),
            preserve_unreadable_history: false,
        }))
        .await
        .unwrap()
    else {
        panic!("expected join result");
    };
    assert!(!matches!(status, JoinSpaceStatusSummary::Rejected { .. }));
    let peer_id = timeout(Duration::from_secs(20), async {
        loop {
            next_engine_event_matching(&mut local_events, |event| {
                matches!(
                    event, EngineEvent::DeviceTrustChanged { revision } if *revision > 0
                )
            })
            .await;
            let OperationResult::DeviceGroupChoices(summary) = local
                .execute(Operation::QueryDeviceGroupChoices)
                .await
                .unwrap()
            else {
                panic!("expected membership");
            };
            if summary.device_trust.local_membership == DeviceMembershipSummary::Active {
                if let Some(peer) = summary
                    .device_trust
                    .devices
                    .iter()
                    .find(|device| !device.is_local)
                {
                    break peer.device_id.clone();
                }
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let OperationResult::EntrySent(saved) = local
        .execute(Operation::SendText(SendTextInput {
            text: "confirmed before peer shutdown".into(),
            target_devices: vec![peer_id],
        }))
        .await
        .unwrap()
    else {
        panic!("expected saved entry");
    };
    assert_eq!(
        saved.total_accepted, 1,
        "prove the peer was reachable before shutdown"
    );

    sponsor.shutdown_until_complete().await.unwrap();
    drop(sponsor);
    // 对端已经实际关闭，恢复全过程没有其他设备能回应；保留默认同步设置。
    local.suspend().await.unwrap();
    timeout(Duration::from_secs(10), local.resume())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(local.lifecycle_state().await, EngineState::Running);
    let OperationResult::HistoryEntry(entry) = local
        .execute(Operation::GetHistoryEntry(HistoryEntryInput {
            entry_id: saved.entry_id,
        }))
        .await
        .unwrap()
    else {
        panic!("expected original entry");
    };
    assert_eq!(entry.content, "confirmed before peer shutdown");
    let OperationResult::EntrySent(saved) = timeout(
        Duration::from_secs(20),
        local.execute(Operation::SendText(SendTextInput {
            text: "confirmed while paired peer remains offline".into(),
            target_devices: Vec::new(),
        })),
    )
    .await
    .unwrap()
    .unwrap() else {
        panic!("expected offline save");
    };
    let OperationResult::HistoryEntry(entry) = local
        .execute(Operation::GetHistoryEntry(HistoryEntryInput {
            entry_id: saved.entry_id,
        }))
        .await
        .unwrap()
    else {
        panic!("expected offline entry");
    };
    assert_eq!(entry.content, "confirmed while paired peer remains offline");
    local.shutdown_until_complete().await.unwrap();
}
