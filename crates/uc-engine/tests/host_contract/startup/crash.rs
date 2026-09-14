use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use uc_engine::{
    CreateSpaceInput, Engine, EngineConfig, HistoryEntryInput, Operation, OperationResult,
    SecretString, SendTextInput,
};

use super::{host, MemorySecureStorage};

const CHILD_ENV: &str = "UC_ENGINE_LIFECYCLE_CRASH_CHILD";
const CHILD_CONTENT: &str = "confirmed content from the terminated process";

#[derive(Serialize, Deserialize)]
struct ChildInput {
    root: PathBuf,
    secure_storage: HashMap<String, Vec<u8>>,
    checkpoint: SocketAddr,
}

#[derive(Serialize, Deserialize)]
struct Checkpoint {
    entry_id: String,
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
#[ignore = "仅由进程终止合同作为子进程调用"]
async fn child_process() {
    assert!(matches!(std::env::var(CHILD_ENV).as_deref(), Ok("1")));
    // 宿主密钥只经匿名管道进入子进程内存，不生成明文密钥文件或输出到测试日志。
    let input: ChildInput = serde_json::from_reader(std::io::stdin().lock()).unwrap();
    let storage = MemorySecureStorage {
        values: Arc::new(Mutex::new(input.secure_storage)),
        ..MemorySecureStorage::default()
    };
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(&input.root, Box::new(storage)),
    )
    .await
    .unwrap();
    let OperationResult::EntrySent(sent) = engine
        .execute(Operation::SendText(SendTextInput {
            text: CHILD_CONTENT.into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap()
    else {
        panic!("child save was not confirmed");
    };
    let mut checkpoint = TcpStream::connect(input.checkpoint).await.unwrap();
    checkpoint
        .write_all(
            &serde_json::to_vec(&Checkpoint {
                entry_id: sent.entry_id,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    checkpoint.shutdown().await.unwrap();
    // 保持真实 Engine 存活，父进程直接终止，不经过 shutdown 或 Rust 析构。
    std::future::pending::<()>().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirmed_content_survives_process_termination_and_immediate_reopen() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("process termination test".into()),
            passphrase: SecretString::new("process-termination-passphrase"),
            passphrase_confirmation: SecretString::new("process-termination-passphrase"),
        }))
        .await
        .unwrap();
    let OperationResult::EntrySent(original) = engine
        .execute(Operation::SendText(SendTextInput {
            text: "confirmed content before the child process".into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap()
    else {
        panic!("initial save was not confirmed");
    };
    engine.shutdown_until_complete().await.unwrap();
    drop(engine);

    let checkpoint = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let input = ChildInput {
        root: root.path().to_owned(),
        secure_storage: storage.values().clone(),
        checkpoint: checkpoint.local_addr().unwrap(),
    };
    let mut child = OwnedChild(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "startup::crash::child_process",
                "--ignored",
                "--quiet",
            ])
            .env(CHILD_ENV, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    serde_json::to_writer(child.0.stdin.take().unwrap(), &input).unwrap();
    let saved: Checkpoint = timeout(Duration::from_secs(20), async {
        let (stream, _) = checkpoint.accept().await.unwrap();
        let mut bytes = Vec::new();
        stream.take(4096).read_to_end(&mut bytes).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    })
    .await
    .unwrap();
    assert!(child.0.try_wait().unwrap().is_none());
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());

    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage)),
    )
    .await
    .unwrap();
    for (entry_id, expected) in [
        (
            original.entry_id,
            "confirmed content before the child process",
        ),
        (saved.entry_id, CHILD_CONTENT),
    ] {
        let OperationResult::HistoryEntry(entry) = engine
            .execute(Operation::GetHistoryEntry(HistoryEntryInput { entry_id }))
            .await
            .unwrap()
        else {
            panic!("confirmed entry was not recovered");
        };
        assert_eq!(entry.content, expected);
    }
    engine
        .execute(Operation::SendText(SendTextInput {
            text: "content saved after process recovery".into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap();
    engine.shutdown_until_complete().await.unwrap();
}
