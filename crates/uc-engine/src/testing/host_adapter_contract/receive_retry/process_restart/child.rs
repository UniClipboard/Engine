//! 独立进程通过匿名stdin传递测试安全存储，检查跨地址空间恢复。
use super::super::super::MemoryHostSecureStorage;
use super::{persistent_engine_host, saved_files};
use crate::testing::receive_exports::{ReceiveExports, ReleaseOnDrop};
use crate::{
    Engine, EngineConfig, EngineEvent, Operation, OperationResult, OutboundProgressBatchState,
    OutboundProgressFreshness, OutboundProgressInput, RecoverSessionInput,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

const CHILD_TEST: &str =
    "testing::host_adapter_contract::receive_retry::process_restart::child::progress_restart_child";
#[derive(Serialize, Deserialize)]
pub(super) enum Mode {
    Sender { entry: String, peer: String },
    Receive,
    Saved { entry: String },
}
#[derive(Serialize, Deserialize)]
struct Input {
    root: PathBuf,
    secrets: HashMap<String, Vec<u8>>,
    rendezvous: String,
    checkpoint: SocketAddr,
    mode: Mode,
    bind_port: u16,
}
pub(super) struct Process(Child);
impl Process {
    pub(super) fn kill(&mut self) {
        self.0.kill().unwrap();
        self.0.wait().unwrap();
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
pub(super) async fn start(
    root: PathBuf,
    storage: &MemoryHostSecureStorage,
    rendezvous: String,
    bind_port: u16,
    mode: Mode,
) -> (Process, BufReader<TcpStream>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let input = Input {
        root,
        secrets: storage.values().clone(),
        rendezvous,
        checkpoint: listener.local_addr().unwrap(),
        mode,
        bind_port,
    };
    let mut process = Process(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD_TEST, "--ignored", "--quiet"])
            .env("UC_ENGINE_PROGRESS_RESTART_CHILD", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    serde_json::to_writer(process.0.stdin.take().unwrap(), &input).unwrap();
    let (stream, _) = timeout(Duration::from_secs(40), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut reader = BufReader::new(stream);
    assert_eq!(line(&mut reader).await, "ready");
    (process, reader)
}
pub(super) async fn line(reader: &mut BufReader<TcpStream>) -> String {
    let mut line = String::new();
    assert!(
        timeout(Duration::from_secs(30), reader.read_line(&mut line))
            .await
            .unwrap()
            .unwrap()
            > 0
    );
    line.trim_end().to_owned()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "仅由真实进程重启合同通过stdin匿名管道启动"]
async fn progress_restart_child() {
    assert_eq!(
        std::env::var("UC_ENGINE_PROGRESS_RESTART_CHILD").as_deref(),
        Ok("1")
    );
    let input: Input = serde_json::from_reader(std::io::stdin().lock()).unwrap();
    let storage = MemoryHostSecureStorage {
        values: Arc::new(Mutex::new(input.secrets)),
    };
    let exports = ReceiveExports::install(&input.root, usize::MAX, usize::MAX);
    let _release = ReleaseOnDrop(exports.clone());
    if matches!(input.mode, Mode::Receive) {
        exports.hold_terminals.store(true, Ordering::SeqCst);
    }
    let (engine, mut events) = Engine::start(
        EngineConfig::new("1.2.3")
            .with_rendezvous_base_url(input.rendezvous)
            .with_test_relay_fallback(false)
            .with_test_iroh_bind_port(input.bind_port),
        persistent_engine_host(&input.root, storage),
    )
    .await
    .unwrap();
    engine
        .execute(Operation::RecoverSession(RecoverSessionInput {
            allow_secure_storage_unlock: true,
        }))
        .await
        .unwrap();
    super::super::member_id(&engine).await;
    let mut socket = TcpStream::connect(input.checkpoint).await.unwrap();
    socket.write_all(b"ready\n").await.unwrap();
    match input.mode {
        Mode::Sender { entry, peer } => {
            let OperationResult::OutboundProgress(Some(view)) = engine
                .execute(Operation::QueryOutboundProgress(OutboundProgressInput {
                    entry_id: entry,
                    target_device_id: peer,
                }))
                .await
                .unwrap()
            else {
                panic!("persisted sender ownership")
            };
            assert_eq!(view.attempt_id, None);
            assert_eq!(view.state, OutboundProgressBatchState::Unconfirmed);
            assert_eq!(view.freshness, OutboundProgressFreshness::Unconfirmed);
            assert_eq!((view.items_total, view.items_unconfirmed), (3, 3));
            assert!(view.items.iter().all(|item| item.round.is_none()
                && item.completed_bytes.is_none()
                && item.total_bytes.is_none()));
            let OperationResult::OutboundProgressList(list) = engine
                .execute(Operation::ListOutboundProgress)
                .await
                .unwrap()
            else {
                panic!("history list")
            };
            assert!(list.contains(&view));
            socket.write_all(b"verified\n").await.unwrap();
        }
        Mode::Receive => {
            let EngineEvent::IncomingPending(pending) =
                super::super::super::next_engine_event_matching(&mut events, |event| {
                    matches!(event, EngineEvent::IncomingPending(_))
                })
                .await
            else {
                panic!("fresh incoming task")
            };
            let completed = super::super::progress(&engine, &pending.entry_id, "completed").await;
            timeout(Duration::from_secs(20), exports.terminal_held.notified())
                .await
                .unwrap();
            let files = saved_files(&engine, &pending.entry_id, &input.root).await;
            assert_eq!(files.len(), 3);
            for index in 3..6 {
                assert!(files.contains_key(format!("capacity-probe-file-{index}\n").as_bytes()));
            }
            assert_eq!(exports.exports.lock().unwrap().len(), 3);
            let checkpoint =
                serde_json::to_string(&(completed.entry_id, completed.attempt_id)).unwrap();
            socket.write_all(checkpoint.as_bytes()).await.unwrap();
            socket.write_all(b"\n").await.unwrap();
        }
        Mode::Saved { entry } => {
            let files = saved_files(&engine, &entry, &input.root).await;
            assert_eq!(files.len(), 3);
            for index in 3..6 {
                assert!(files.contains_key(format!("capacity-probe-file-{index}\n").as_bytes()));
            }
            assert!(exports.exports.lock().unwrap().is_empty());
            assert!(exports.protects.lock().unwrap().is_empty());
            socket.write_all(b"verified\n").await.unwrap();
        }
    }
    std::future::pending::<()>().await;
}
