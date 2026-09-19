use std::net::TcpListener;
use std::time::Duration;

use tokio::time::{sleep, timeout};
use wiremock::MockServer;

use super::{
    mount_engine_rendezvous, next_engine_event_matching, persistent_engine_host,
    wait_entry_delivered, MemoryHostSecureStorage, ReadableHostFiles, RecordingHostFilesState,
    StaticHostClipboard, ENGINE_TEST_LOCK,
};
use crate::{
    CreateSpaceInput, DeviceMembershipSummary, DeviceReachabilitySummary, Engine, EngineConfig,
    EngineEvent, EngineState, HistoryEntryInput, HostCapabilities, HostClipboardSnapshot,
    HostDirectories, HostFileHandle, JoinSpaceInput, JoinSpaceStatusSummary, Operation,
    OperationResult, QueryHistoryInput, RefreshReason, SecretString, SendFilesInput, SendTextInput,
};

#[path = "offline_lifecycle/crash.rs"]
mod crash;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unavailable_network_does_not_block_local_resume_save_or_read() {
    let _guard = ENGINE_TEST_LOCK.lock().await;
    let unavailable = TcpListener::bind("127.0.0.1:0").unwrap();
    let unavailable_url = format!("http://{}", unavailable.local_addr().unwrap());
    drop(unavailable);

    let root = tempfile::tempdir().unwrap();
    let config = EngineConfig::new("2.0.0").with_rendezvous_base_url(unavailable_url);
    let (engine, _events) = timeout(
        Duration::from_secs(10),
        Engine::start(
            config,
            persistent_engine_host(root.path(), MemoryHostSecureStorage::default()),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    timeout(
        Duration::from_secs(10),
        engine.execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("offline local device".into()),
            passphrase: SecretString::new("offline-local-passphrase"),
            passphrase_confirmation: SecretString::new("offline-local-passphrase"),
        })),
    )
    .await
    .unwrap()
    .unwrap();

    engine.suspend().await.unwrap();
    timeout(Duration::from_secs(10), engine.resume())
        .await
        .unwrap()
        .unwrap();
    let OperationResult::EntrySent(saved) = timeout(
        Duration::from_secs(10),
        engine.execute(Operation::SendText(SendTextInput {
            text: "saved without any network service".into(),
            target_devices: Vec::new(),
        })),
    )
    .await
    .unwrap()
    .unwrap() else {
        panic!("expected local save");
    };
    let OperationResult::HistoryEntry(entry) = timeout(
        Duration::from_secs(10),
        engine.execute(Operation::GetHistoryEntry(HistoryEntryInput {
            entry_id: saved.entry_id,
        })),
    )
    .await
    .unwrap()
    .unwrap() else {
        panic!("expected local history entry");
    };
    assert_eq!(entry.content, "saved without any network service");
    timeout(Duration::from_secs(10), engine.shutdown_until_complete())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peer_restart_does_not_block_local_work_and_recovers_an_offline_file() {
    let _guard = ENGINE_TEST_LOCK.lock().await;
    let rendezvous = MockServer::start().await;
    mount_engine_rendezvous(&rendezvous).await;
    let sponsor_root = tempfile::tempdir().unwrap();
    let local_root = tempfile::tempdir().unwrap();
    let sponsor_storage = MemoryHostSecureStorage::default();
    let file_bytes = b"file saved while the paired receiver is offline".to_vec();
    let file_name = "offline-recovery.txt";
    let config = EngineConfig::new("2.0.0").with_rendezvous_base_url(rendezvous.uri());
    let (sponsor, _sponsor_events) = Engine::start(
        config.clone(),
        persistent_engine_host(sponsor_root.path(), sponsor_storage.clone()),
    )
    .await
    .unwrap();
    let local_host = HostCapabilities::new(
        HostDirectories::new(
            local_root.path().join("private"),
            local_root.path().join("cache"),
            local_root.path().join("temporary"),
            local_root.path().join("logs"),
        ),
        Box::new(MemoryHostSecureStorage::default()),
        Box::new(StaticHostClipboard {
            snapshot: HostClipboardSnapshot {
                observed_at_ms: 0,
                representations: Vec::new(),
            },
        }),
        Box::new(ReadableHostFiles {
            handle: "offline-file".into(),
            display_name: file_name.into(),
            mime_type: Some("text/plain".into()),
            bytes: file_bytes.clone(),
            state: std::sync::Arc::new(RecordingHostFilesState::default()),
        }),
    );
    let (local, mut local_events) = Engine::start(config.clone(), local_host).await.unwrap();
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
    match status {
        JoinSpaceStatusSummary::Active { .. } => {}
        JoinSpaceStatusSummary::Pending { .. } => {
            next_engine_event_matching(&mut local_events, |event| {
                matches!(
                    event,
                    EngineEvent::RefreshRequired {
                        reason: RefreshReason::StateInvalidated
                    }
                )
            })
            .await;
        }
        JoinSpaceStatusSummary::Rejected { reason, .. } => {
            panic!("join was rejected: {reason:?}");
        }
        JoinSpaceStatusSummary::Terminated { reason, .. } => {
            panic!("join was terminated: {reason:?}");
        }
    }
    let peer_id = timeout(Duration::from_secs(20), async {
        loop {
            let OperationResult::DeviceGroupChoices(summary) = local
                .execute(Operation::QueryDeviceGroupChoices)
                .await
                .unwrap()
            else {
                panic!("expected membership");
            };
            if summary.device_trust.local_membership == DeviceMembershipSummary::Active {
                if let Some(peer) = summary.device_trust.devices.iter().find(|device| {
                    !device.is_local && device.reachability == DeviceReachabilitySummary::Online
                }) {
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
            target_devices: vec![peer_id.clone()],
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

    let OperationResult::EntrySent(file) = local
        .execute(Operation::SendFiles(SendFilesInput {
            files: vec![HostFileHandle::new("offline-file")],
            target_devices: vec![peer_id.clone()],
        }))
        .await
        .unwrap()
    else {
        panic!("expected offline file save");
    };
    assert_eq!(file.total_accepted, 0);
    assert_eq!(file.total_offline, 1);

    let (restarted, _restarted_events) = Engine::start(
        config,
        persistent_engine_host(sponsor_root.path(), sponsor_storage),
    )
    .await
    .unwrap();
    wait_entry_delivered(&local, &file.entry_id, &peer_id).await;
    let received_entry_id = timeout(Duration::from_secs(30), async {
        loop {
            let OperationResult::HistoryPage { entries, .. } = restarted
                .execute(Operation::QueryHistory(QueryHistoryInput {
                    cursor: None,
                    limit: 20,
                    query: None,
                }))
                .await
                .unwrap()
            else {
                panic!("expected restarted peer history");
            };
            if let Some(entry) = entries
                .into_iter()
                .find(|entry| entry.preview.as_deref() == Some(file_name))
            {
                break entry.entry_id;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("restarted peer must receive the offline file");
    let OperationResult::EntryFileRead(received) = restarted
        .execute(Operation::ReadEntryFile(HistoryEntryInput {
            entry_id: received_entry_id,
        }))
        .await
        .unwrap()
    else {
        panic!("expected recovered file content");
    };
    assert_eq!(received.bytes, file_bytes);
    assert_eq!(received.file_name, file_name);

    restarted.shutdown_until_complete().await.unwrap();
    local.shutdown_until_complete().await.unwrap();
}
