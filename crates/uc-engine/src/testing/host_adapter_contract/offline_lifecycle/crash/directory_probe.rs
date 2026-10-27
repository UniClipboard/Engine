//! 真实目录探测／发布回滚失败后强杀；旧保存目录的恢复责任不能丢失。
#[path = "directory_probe/publication.rs"]
mod publication;
use super::super::super::{
    mount_engine_rendezvous, next_engine_event_matching, persistent_engine_host, EmptyHostFiles,
    MemoryHostSecureStorage, ENGINE_TEST_LOCK,
};
use super::OwnedChild;
use crate::testing::directory_probe::DirectoryProbe;
use crate::{
    CreateSpaceInput, DeviceMembershipSummary, Engine, EngineConfig, EngineEvent,
    EntryReceiveProgressInput, FileSyncSettingsPatch, HostCapabilities, HostCapabilityError,
    HostClipboard, HostClipboardRepresentation, HostClipboardSnapshot, HostDirectories,
    JoinSpaceInput, ObserveClipboardChangeInput, Operation, OperationResult, SecretString,
    SettingsPatch, SettingsUpdateOutcome, SyncSettingsPatch,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, timeout};
use uc_core::ports::DIRECTORY_RECEIVE_STAGING_PREFIX;
use url::Url;
use wiremock::MockServer;

const CHILD_ENV: &str = "UC_ENGINE_DIRECTORY_PROBE_CHILD";
const CHILD_TEST: &str = "testing::host_adapter_contract::offline_lifecycle::crash::directory_probe::receiver_child_process";
const PASSPHRASE: &str = "directory probe crash fixture";

#[derive(Serialize, Deserialize)]
struct ChildInput {
    root: PathBuf,
    secure_storage: HashMap<String, Vec<u8>>,
    rendezvous_base_url: String,
    bind_port: u16,
    checkpoint: SocketAddr,
    old_save: PathBuf,
    new_save: PathBuf,
    publication_failure: bool,
}

struct Clipboard(Arc<Mutex<HostClipboardSnapshot>>);
impl HostClipboard for Clipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn write(&self, _: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        Ok(())
    }
}

async fn settings(engine: &Engine, save_dir: Option<&Path>) {
    let result = engine
        .execute(Operation::UpdateSettings(Box::new(SettingsPatch {
            sync: Some(SyncSettingsPatch {
                auto_sync_enabled: Some(false),
                sync_on_restore: Some(false),
                ..Default::default()
            }),
            file_sync: save_dir.map(|path| FileSyncSettingsPatch {
                auto_save_dir: Some(path.to_str().unwrap().into()),
                ..Default::default()
            }),
            ..Default::default()
        })))
        .await
        .unwrap();
    assert!(matches!(
        result,
        OperationResult::SettingsUpdated(SettingsUpdateOutcome::Updated(_))
    ));
}

async fn members_ready(engine: &Engine) {
    timeout(Duration::from_secs(25), async {
        loop {
            if let Ok(OperationResult::DeviceGroupChoices(choices)) =
                engine.execute(Operation::QueryDeviceGroupChoices).await
            {
                if choices.device_trust.local_membership == DeviceMembershipSummary::Active
                    && choices.device_trust.devices.iter().any(|device| {
                        !device.is_local && device.membership == DeviceMembershipSummary::Active
                    })
                {
                    return;
                }
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("both real members must become active");
}

fn staging_areas(directory: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(DIRECTORY_RECEIVE_STAGING_PREFIX))
        })
        .map(|entry| entry.path())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "仅由原生目录探测强杀合同作为子进程调用"]
async fn receiver_child_process() {
    assert!(matches!(std::env::var(CHILD_ENV).as_deref(), Ok("1")));
    // 模拟钥匙串只经私有管道移交，不写入磁盘或输出。
    let input: ChildInput = serde_json::from_reader(std::io::stdin().lock()).unwrap();
    let fixture = DirectoryProbe::install(&input.old_save, input.publication_failure);
    let storage = MemoryHostSecureStorage {
        values: Arc::new(Mutex::new(input.secure_storage)),
    };
    let (engine, mut events) = Engine::start(
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
    let EngineEvent::IncomingPending(pending) = next_engine_event_matching(&mut events, |event| {
        matches!(event, EngineEvent::IncomingPending(_))
    })
    .await
    else {
        panic!("directory receive must start")
    };
    if input.publication_failure {
        publication::wait_for_failed_rollback(
            &engine,
            &input.root,
            &input.old_save,
            &pending.entry_id,
            &fixture,
        )
        .await;
    } else {
        timeout(Duration::from_secs(20), fixture.entered.notified())
            .await
            .unwrap();
        assert!(
            fixture.supported.load(Ordering::SeqCst),
            "the real filesystem must support no-replace publication"
        );
        assert!(fixture
            .area
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|area| area.is_dir()));
    }
    settings(&engine, Some(&input.new_save)).await;
    let OperationResult::Settings(current) =
        engine.execute(Operation::QuerySettings).await.unwrap()
    else {
        panic!("current settings expected")
    };
    assert!(current.file_sync.auto_save_dir.as_deref() == input.new_save.to_str());
    let mut frame = serde_json::to_vec(&pending.entry_id).unwrap();
    frame.push(b'\n');
    checkpoint.write_all(&frame).await.unwrap();
    checkpoint.flush().await.unwrap();
    std::future::pending::<()>().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupted_directory_probe_recovers_the_previous_save_directory() {
    assert_directory_crash(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupted_directory_rollback_recovers_visible_root_and_preserves_reused_name() {
    assert_directory_crash(true).await;
}

async fn assert_directory_crash(publication_failure: bool) {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let server = MockServer::start().await;
    mount_engine_rendezvous(&server).await;
    let receiver_root = tempfile::tempdir().unwrap();
    let source_root = tempfile::tempdir().unwrap();
    let old_save = receiver_root.path().join("old-save");
    let new_save = receiver_root.path().join("new-save");
    std::fs::create_dir(&old_save).unwrap();
    std::fs::create_dir(&new_save).unwrap();
    std::fs::write(old_save.join("unrelated.bin"), b"keep unrelated").unwrap();
    let storage = MemoryHostSecureStorage::default();
    let bind_port = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let receiver_config = EngineConfig::new("2.0.0")
        .with_rendezvous_base_url(server.uri())
        .with_test_relay_fallback(false)
        .with_test_iroh_bind_port(bind_port);
    let (receiver, _events) = Engine::start(
        receiver_config.clone(),
        persistent_engine_host(receiver_root.path(), storage.clone()),
    )
    .await
    .unwrap();
    let clipboard = Arc::new(Mutex::new(HostClipboardSnapshot {
        observed_at_ms: 0,
        representations: Vec::new(),
    }));
    let source_host = HostCapabilities::new(
        HostDirectories::new(
            source_root.path().join("private"),
            source_root.path().join("cache"),
            source_root.path().join("temporary"),
            source_root.path().join("logs"),
        ),
        Box::new(MemoryHostSecureStorage::default()),
        Box::new(Clipboard(clipboard.clone())),
        Box::new(EmptyHostFiles),
    );
    let (source, _source_events) = Engine::start(
        EngineConfig::new("2.0.0")
            .with_rendezvous_base_url(server.uri())
            .with_test_relay_fallback(false),
        source_host,
    )
    .await
    .unwrap();
    let source = Arc::new(source);
    receiver
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("directory crash receiver".into()),
            passphrase: SecretString::new(PASSPHRASE),
            passphrase_confirmation: SecretString::new(PASSPHRASE),
        }))
        .await
        .unwrap();
    let OperationResult::InvitationIssued {
        invitation_code, ..
    } = receiver.execute(Operation::IssueInvitation).await.unwrap()
    else {
        panic!("invitation expected")
    };
    source
        .execute(Operation::JoinSpace(JoinSpaceInput {
            invitation_code,
            device_name: Some("directory crash source".into()),
            passphrase: SecretString::new(PASSPHRASE),
            preserve_unreadable_history: false,
        }))
        .await
        .unwrap();
    members_ready(&source).await;
    members_ready(&receiver).await;
    settings(&source, None).await;
    settings(&receiver, Some(&old_save)).await;
    receiver.shutdown_until_complete().await.unwrap();
    drop(receiver);
    let checkpoint = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let input = ChildInput {
        root: receiver_root.path().to_owned(),
        secure_storage: storage.values().clone(),
        rendezvous_base_url: server.uri(),
        bind_port,
        checkpoint: checkpoint.local_addr().unwrap(),
        old_save: old_save.clone(),
        new_save: new_save.clone(),
        publication_failure,
    };
    let mut child = OwnedChild(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD_TEST, "--ignored", "--quiet", "--nocapture"])
            .env(CHILD_ENV, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    serde_json::to_writer(child.0.stdin.take().unwrap(), &input).unwrap();
    let (stream, _) = timeout(Duration::from_secs(25), checkpoint.accept())
        .await
        .unwrap()
        .unwrap();
    let mut stages = BufReader::new(stream).lines();
    assert!(stages.next_line().await.unwrap().as_deref() == Some("ready"));
    let directory = source_root.path().join("directory-source");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("a.bin"), vec![0x61; 96 * 1024]).unwrap();
    std::fs::write(directory.join("b.bin"), vec![0x62; 96 * 1024]).unwrap();
    let mut roots = vec![Url::from_directory_path(&directory).unwrap().to_string()];
    if publication_failure {
        let second = source_root.path().join("directory-other");
        std::fs::create_dir(&second).unwrap();
        std::fs::write(second.join("c.bin"), vec![0x63; 96 * 1024]).unwrap();
        roots.push(Url::from_directory_path(second).unwrap().to_string());
        let third = source_root.path().join("directory-last");
        std::fs::create_dir(&third).unwrap();
        std::fs::write(third.join("d.bin"), vec![0x64; 96 * 1024]).unwrap();
        roots.push(Url::from_directory_path(third).unwrap().to_string());
    }
    *clipboard.lock().unwrap() = HostClipboardSnapshot {
        observed_at_ms: 1_700_000_100_000,
        representations: vec![HostClipboardRepresentation::Inline {
            format: "files".into(),
            mime_type: Some("text/uri-list".into()),
            bytes: roots.join("\r\n").into_bytes(),
        }],
    };
    let sending = tokio::spawn({
        let source = Arc::clone(&source);
        async move {
            source
                .execute(Operation::ObserveClipboardChange(
                    ObserveClipboardChangeInput { dispatch: true },
                ))
                .await
        }
    });
    let frame = timeout(Duration::from_secs(30), stages.next_line())
        .await
        .unwrap()
        .unwrap()
        .expect("child must finish the real probe and persist the new save directory");
    let entry_id: String = serde_json::from_str(&frame).unwrap();
    assert_eq!(staging_areas(&old_save).len(), 1);
    assert!(child.0.try_wait().unwrap().is_none());
    child.0.kill().unwrap();
    let exit = child.0.wait().unwrap();
    assert_eq!(
        exit.signal(),
        Some(9),
        "the receiver must actually be killed"
    );
    // 目录身份异步完成时，原观察动作可能没有同步报告；真实接收及探测已由子进程确认。
    let sent = timeout(Duration::from_secs(30), sending)
        .await
        .unwrap()
        .unwrap();
    assert!(
        sent.is_ok() || sent.as_ref().is_err_and(|error| error.code() == 1253),
        "原观察动作只能成功或返回异步目录尚无同步报告"
    );
    source.shutdown_until_complete().await.unwrap();
    // 已撤回的第二根保留私有staging证明；用户复用原目标不能被重启恢复删除。
    let user_root = old_save.join(if publication_failure {
        "directory-other"
    } else {
        "directory-source"
    });
    if publication_failure {
        assert_eq!(
            std::fs::read(old_save.join("directory-source/a.bin")).unwrap(),
            vec![0x61; 96 * 1024]
        );
        publication::assert_journal(&input.root, &entry_id, false);
    }
    std::fs::create_dir(&user_root).unwrap();
    std::fs::write(user_root.join("user.bin"), b"keep reused target").unwrap();
    let (restarted, _events) = Engine::start(
        receiver_config,
        persistent_engine_host(receiver_root.path(), storage),
    )
    .await
    .unwrap();
    timeout(Duration::from_secs(20), async {
        loop {
            if matches!(restarted.execute(Operation::QueryReceiveReadiness).await.unwrap(),
                OperationResult::ReceiveReadiness(readiness) if readiness.ready)
            {
                return;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("startup reconciliation must finish");
    let OperationResult::Settings(current) =
        restarted.execute(Operation::QuerySettings).await.unwrap()
    else {
        panic!("reopened settings expected")
    };
    assert!(current.file_sync.auto_save_dir.as_deref() == new_save.to_str());
    let progress = restarted
        .execute(Operation::QueryEntryReceiveProgress(
            EntryReceiveProgressInput {
                entry_id: entry_id.clone(),
            },
        ))
        .await
        .unwrap();
    let remaining = staging_areas(&old_save).len();
    let unrelated_kept =
        std::fs::read(old_save.join("unrelated.bin")).unwrap() == b"keep unrelated";
    let reused_kept = std::fs::read(user_root.join("user.bin")).unwrap() == b"keep reused target";
    if publication_failure {
        assert!(
            !old_save.join("directory-source").exists(),
            "restart must withdraw the actually published root"
        );
        publication::assert_journal(&input.root, &entry_id, true);
    }
    restarted.shutdown_until_complete().await.unwrap();
    assert!(
        matches!(progress, OperationResult::EntryReceiveProgress(Some(progress)) if progress.state == "failed")
    );
    assert_eq!(
        remaining, 0,
        "readiness must recover staging in the previous save directory"
    );
    assert!(
        unrelated_kept && reused_kept,
        "recovery must preserve unrelated and reused user paths"
    );
}
