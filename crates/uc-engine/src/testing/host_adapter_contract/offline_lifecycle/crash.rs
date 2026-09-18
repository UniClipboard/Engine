use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, timeout};
use wiremock::MockServer;

use super::super::{
    mount_engine_rendezvous, next_engine_event_matching, persistent_engine_host,
    wait_entry_delivered, MemoryHostSecureStorage, ReadableHostFiles, RecordingHostFilesState,
    StaticHostClipboard, ENGINE_TEST_LOCK,
};
use crate::{
    CreateSpaceInput, DeviceMembershipSummary, DeviceReachabilitySummary, Engine, EngineConfig,
    EngineEvent, EntryDeliveryStatusSummary, HistoryEntryInput, HostCapabilities,
    HostClipboardSnapshot, HostDirectories, HostFileHandle, JoinSpaceInput, JoinSpaceStatusSummary,
    Operation, OperationResult, QueryHistoryInput, SecretString, SendFilesInput,
};

const CHILD_ENV: &str = "UC_ENGINE_TRANSFER_CRASH_CHILD";
const CHILD_TEST: &str =
    "testing::host_adapter_contract::offline_lifecycle::crash::receiver_child_process";
const FILE_NAME: &str = "interrupted-transfer.bin";
const FILE_SIZE: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct ChildInput {
    root: PathBuf,
    secure_storage: HashMap<String, Vec<u8>>,
    rendezvous_base_url: String,
    bind_port: u16,
    checkpoint: SocketAddr,
}

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "仅由传输中进程终止合同作为子进程调用"]
async fn receiver_child_process() {
    assert!(matches!(std::env::var(CHILD_ENV).as_deref(), Ok("1")));
    let input: ChildInput = serde_json::from_reader(std::io::stdin().lock()).unwrap();
    let storage = MemoryHostSecureStorage {
        values: Arc::new(Mutex::new(input.secure_storage)),
    };
    let (_engine, mut events) = Engine::start(
        EngineConfig::new("2.0.0")
            .with_rendezvous_base_url(input.rendezvous_base_url)
            .with_test_relay_fallback(false)
            .with_test_iroh_bind_port(input.bind_port),
        persistent_engine_host(&input.root, storage),
    )
    .await
    .unwrap();
    let mut checkpoint = TcpStream::connect(input.checkpoint).await.unwrap();
    checkpoint.write_all(b"ready\n").await.unwrap();
    next_engine_event_matching(&mut events, |event| {
        matches!(event, EngineEvent::IncomingPending(_))
    })
    .await;
    checkpoint.write_all(b"inflight\n").await.unwrap();
    checkpoint.flush().await.unwrap();
    std::future::pending::<()>().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupted_file_transfer_recovers_after_receiver_process_restart() {
    let _guard = ENGINE_TEST_LOCK.lock().await;
    let rendezvous = MockServer::start().await;
    mount_engine_rendezvous(&rendezvous).await;
    let receiver_root = tempfile::tempdir().unwrap();
    let sender_root = tempfile::tempdir().unwrap();
    let receiver_storage = MemoryHostSecureStorage::default();
    let sender_storage = MemoryHostSecureStorage::default();
    let file_bytes = vec![0x5a; FILE_SIZE];
    let receiver_bind_port = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let receiver_config = EngineConfig::new("2.0.0")
        .with_rendezvous_base_url(rendezvous.uri())
        .with_test_relay_fallback(false)
        .with_test_iroh_bind_port(receiver_bind_port);
    let sender_config = EngineConfig::new("2.0.0")
        .with_rendezvous_base_url(rendezvous.uri())
        .with_test_relay_fallback(false);

    let (receiver, _receiver_events) = Engine::start(
        receiver_config.clone(),
        persistent_engine_host(receiver_root.path(), receiver_storage.clone()),
    )
    .await
    .unwrap();
    let sender_host = HostCapabilities::new(
        HostDirectories::new(
            sender_root.path().join("private"),
            sender_root.path().join("cache"),
            sender_root.path().join("temporary"),
            sender_root.path().join("logs"),
        ),
        Box::new(sender_storage),
        Box::new(StaticHostClipboard {
            snapshot: HostClipboardSnapshot {
                observed_at_ms: 0,
                representations: Vec::new(),
            },
        }),
        Box::new(ReadableHostFiles {
            handle: "interrupted-file".into(),
            display_name: FILE_NAME.into(),
            mime_type: Some("application/octet-stream".into()),
            bytes: file_bytes.clone(),
            state: Arc::new(RecordingHostFilesState::default()),
        }),
    );
    let (sender, mut sender_events) = Engine::start(sender_config, sender_host).await.unwrap();
    let sender = Arc::new(sender);

    receiver
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("transfer crash receiver".into()),
            passphrase: SecretString::new("transfer-crash-passphrase"),
            passphrase_confirmation: SecretString::new("transfer-crash-passphrase"),
        }))
        .await
        .unwrap();
    let OperationResult::InvitationIssued {
        invitation_code, ..
    } = receiver.execute(Operation::IssueInvitation).await.unwrap()
    else {
        panic!("expected invitation");
    };
    let OperationResult::JoinSpace(status) = sender
        .execute(Operation::JoinSpace(JoinSpaceInput {
            invitation_code,
            device_name: Some("transfer crash sender".into()),
            passphrase: SecretString::new("transfer-crash-passphrase"),
            preserve_unreadable_history: false,
        }))
        .await
        .unwrap()
    else {
        panic!("expected join result");
    };
    assert!(!matches!(status, JoinSpaceStatusSummary::Rejected { .. }));
    let receiver_id = timeout(Duration::from_secs(20), async {
        loop {
            next_engine_event_matching(&mut sender_events, |event| {
                matches!(event, EngineEvent::DeviceTrustChanged { revision } if *revision > 0)
            })
            .await;
            let OperationResult::DeviceGroupChoices(summary) = sender
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
    receiver.shutdown_until_complete().await.unwrap();
    drop(receiver);
    let checkpoint = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let input = ChildInput {
        root: receiver_root.path().to_owned(),
        secure_storage: receiver_storage.values().clone(),
        rendezvous_base_url: rendezvous.uri(),
        bind_port: receiver_bind_port,
        checkpoint: checkpoint.local_addr().unwrap(),
    };
    let mut child = OwnedChild(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD_TEST, "--ignored", "--quiet"])
            .env(CHILD_ENV, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    serde_json::to_writer(child.0.stdin.take().unwrap(), &input).unwrap();
    let (stream, _) = timeout(Duration::from_secs(20), checkpoint.accept())
        .await
        .unwrap()
        .unwrap();
    let mut stages = BufReader::new(stream).lines();
    assert_eq!(stages.next_line().await.unwrap().as_deref(), Some("ready"));

    let sending = tokio::spawn({
        let sender = Arc::clone(&sender);
        let receiver_id = receiver_id.clone();
        async move {
            sender
                .execute(Operation::SendFiles(SendFilesInput {
                    files: vec![HostFileHandle::new("interrupted-file")],
                    target_devices: vec![receiver_id],
                }))
                .await
        }
    });
    assert_eq!(
        timeout(Duration::from_secs(30), stages.next_line())
            .await
            .unwrap()
            .unwrap()
            .as_deref(),
        Some("inflight")
    );
    assert!(child.0.try_wait().unwrap().is_none());
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
    let OperationResult::EntrySent(sent) = timeout(Duration::from_secs(30), sending)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
    else {
        panic!("expected interrupted file save");
    };
    let OperationResult::EntryDelivery(delivery) = sender
        .execute(Operation::QueryEntryDelivery(HistoryEntryInput {
            entry_id: sent.entry_id.clone(),
        }))
        .await
        .unwrap()
    else {
        panic!("expected interrupted delivery state");
    };
    assert!(matches!(
        delivery.deliveries.as_slice(),
        [target]
            if matches!(
                target.status,
                EntryDeliveryStatusSummary::Pending | EntryDeliveryStatusSummary::Unreachable
            )
    ));

    timeout(Duration::from_secs(20), async {
        loop {
            let OperationResult::DeviceGroupChoices(summary) = sender
                .execute(Operation::QueryDeviceGroupChoices)
                .await
                .unwrap()
            else {
                panic!("expected device trust snapshot");
            };
            if summary.device_trust.devices.iter().any(|device| {
                device.device_id == receiver_id
                    && device.reachability == DeviceReachabilitySummary::Offline
            }) {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("sender must observe the terminated receiver offline");

    let (restarted, _events) = Engine::start(
        receiver_config,
        persistent_engine_host(receiver_root.path(), receiver_storage),
    )
    .await
    .unwrap();
    timeout(Duration::from_secs(20), async {
        loop {
            let OperationResult::DeviceGroupChoices(summary) = sender
                .execute(Operation::QueryDeviceGroupChoices)
                .await
                .unwrap()
            else {
                panic!("expected device trust snapshot");
            };
            if summary.device_trust.devices.iter().any(|device| {
                device.device_id == receiver_id
                    && device.reachability == DeviceReachabilitySummary::Online
            }) {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("sender must observe the restarted receiver online");
    wait_entry_delivered(&sender, &sent.entry_id, &receiver_id).await;
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
                panic!("expected restarted receiver history");
            };
            if let Some(entry) = entries
                .into_iter()
                .find(|entry| entry.preview.as_deref() == Some(FILE_NAME))
            {
                break entry.entry_id;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("restarted receiver must recover the interrupted file");
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
    assert_eq!(received.file_name, FILE_NAME);

    restarted.shutdown_until_complete().await.unwrap();
    sender.shutdown_until_complete().await.unwrap();
}
