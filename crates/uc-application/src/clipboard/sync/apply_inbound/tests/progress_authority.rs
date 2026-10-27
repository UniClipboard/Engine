//! 真实接收用例的CAS先后证明；存储port采用受控fixture，不冒充SQLite重启。
use super::{
    file_blob_input, ApplyInboundClipboardUseCase, AttemptGate, FakeAvailability, HostEventBus,
    InboundApplyCommonDeps, InboundReceiveAttemptDeps, MockBlobMaterializer, MockCapture,
    MockEntryRepo, NoopReceiveArtifactCleanup, RecordingLiveIndex, StoreOnlyPullDeps,
};
use crate::clipboard::entry_identity::EntryIdentityCoordinator;
use crate::transfer::progress::ProgressTaskAuthorityPort;
use crate::transfer::receive::reconciliation::ReceiveReadinessCoordinator;
use crate::transfer::receive::work::ReceiveWorkRegistry;
use async_trait::async_trait;
use std::io;
use std::sync::{atomic::AtomicUsize, Arc};
use tokio::sync::{Notify, Semaphore};
use uc_core::file_transfer::ProgressIdentity;
use uc_core::ids::EntryId;
use uc_core::ports::{AttemptError, BeginReceiveAttemptPort, BeginReceiveOutcome};
use uuid::Uuid;

struct HeldAcceptance {
    ledger: Arc<AttemptGate>,
    reject: bool,
    entered: Notify,
    release: Semaphore,
}
#[async_trait]
impl BeginReceiveAttemptPort for HeldAcceptance {
    async fn begin_first_receive(
        &self,
        entry: &str,
        attempt: &str,
        now: i64,
    ) -> Result<BeginReceiveOutcome, AttemptError> {
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        if self.reject {
            return Ok(BeginReceiveOutcome::Superseded);
        }
        self.ledger.begin_first_receive(entry, attempt, now).await
    }
    async fn begin_redelivery(
        &self,
        entry: &str,
        expected: &str,
        attempt: &str,
        now: i64,
    ) -> Result<BeginReceiveOutcome, AttemptError> {
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        if self.reject {
            return Ok(BeginReceiveOutcome::Superseded);
        }
        self.ledger
            .begin_redelivery(entry, expected, attempt, now)
            .await
    }
}

#[tokio::test]
async fn real_receive_handoffs_only_after_cas_begun_and_before_materialization() {
    for reject in [false, true] {
        let work = Arc::new(ReceiveWorkRegistry::default());
        let attempts = AttemptGate::empty();
        let acceptance = Arc::new(HeldAcceptance {
            ledger: attempts.clone(),
            reject,
            entered: Notify::new(),
            release: Semaphore::new(0),
        });
        let mut input = file_blob_input();
        let source = EntryId::from("00000000-0000-4000-8000-000000000001");
        let id = ProgressIdentity::new(*Uuid::parse_str(source.as_ref()).unwrap().as_bytes());
        input.source_entry = Some(source);
        let peer = input.from_device;
        let mut repo = MockEntryRepo::new();
        repo.expect_find_entry_id_by_snapshot_hash()
            .returning(|_| Ok(None));
        let capture = MockCapture::new();
        let mut materializer = MockBlobMaterializer::new();
        if !reject {
            let registry = work.clone();
            let ledger = attempts.clone();
            materializer
                .expect_materialize_plan()
                .times(1)
                .returning(move |_| {
                    let accepted = registry
                        .current_task(&peer, id)
                        .expect("authoritative before download");
                    let current = ledger.attempt_id.lock().unwrap();
                    assert_eq!(
                        accepted.descriptor.attempt.bytes(),
                        *Uuid::parse_str(&current).unwrap().as_bytes()
                    );
                    assert_eq!(accepted.descriptor.item_count, 1);
                    Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        "fixture ends after accepted handoff",
                    )
                    .into())
                });
        }
        let readiness = Arc::new(ReceiveReadinessCoordinator::new());
        readiness.mark_ready();
        let receiver = Arc::new(ApplyInboundClipboardUseCase::store_only_pull(
            StoreOnlyPullDeps {
                common: InboundApplyCommonDeps {
                    receive_retry: None,
                    receive_work: work.clone(),
                    entry_repo: Arc::new(repo),
                    capture: Arc::new(capture),
                    blob_materializer: Arc::new(materializer),
                    receive_attempts: InboundReceiveAttemptDeps {
                        get: attempts.clone(),
                        begin: acceptance.clone(),
                        claim_commit: attempts.clone(),
                        request_cancel: attempts.clone(),
                        begin_failure: attempts.clone(),
                        commit: attempts.clone(),
                        clock: attempts.clone(),
                    },
                    receive_artifact_cleanup: Arc::new(NoopReceiveArtifactCleanup),
                    receive_readiness: readiness,
                    host_event_emitter: Arc::new(HostEventBus::new()),
                    search_live_index: Arc::new(RecordingLiveIndex {
                        calls: AtomicUsize::new(0),
                    }),
                    availability: Arc::new(FakeAvailability { available: false }),
                    entry_identity_coordinator: Arc::new(EntryIdentityCoordinator::new()),
                },
            },
        ));
        let owner = receiver.clone();
        let applying = tokio::spawn(async move { owner.execute(input).await });
        acceptance.entered.notified().await;
        assert!(work.current_task(&peer, id).is_none());
        acceptance.release.add_permits(1);
        assert!(applying.await.unwrap().is_err());
        assert_eq!(work.current_task(&peer, id).is_none(), reject);
        receiver.shutdown().await.unwrap();
    }
}

/// 仅由跨crate组合证明启动；数据通道读取此进程里实际接收用例已交接的当前权威。
#[test]
#[ignore = "由run-progress-registration-proof驱动，以独立进程提供真实接收权威"]
fn registration_authority_worker() {
    use crate::clipboard::sync::payload_codec::{
        decode_v3_bytes_to_snapshot_and_blob_refs,
        encode_snapshot_with_blob_refs_and_file_set_to_v3_bytes,
    };
    use crate::transfer::receive::materialization::{InboundFileSetManifest, InboundFileSetMember};
    use serde_json::{json, Value};
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use uc_core::clipboard::FileSetMemberKind;
    use uc_core::file_transfer::ProgressItemKind;
    use uc_core::ids::DeviceId;
    let marker = std::env::var_os("UC_PROGRESS_AUTHORITY_WORKER_MARKER")
        .expect("proof driver supplies readiness marker");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    std::fs::write(marker, listener.local_addr().unwrap().to_string()).unwrap();
    for connection in listener.incoming() {
        let mut connection = connection.unwrap();
        let mut line = String::new();
        BufReader::new(&mut connection)
            .read_line(&mut line)
            .unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        if request["op"] == "accept" {
            let mut input = file_blob_input();
            input.from_device = DeviceId::new("progress-source");
            input.source_entry = Some(EntryId::from(Uuid::from_bytes([1; 16]).to_string()));
            if request["directory"] == true {
                let (snapshot, _) =
                    decode_v3_bytes_to_snapshot_and_blob_refs(&input.plaintext).unwrap();
                let manifest = InboundFileSetManifest {
                    members: vec![InboundFileSetMember {
                        root_index: 0,
                        root_name: "folder".into(),
                        root_is_file: false,
                        relative_path: String::new(),
                        kind: FileSetMemberKind::EmptyDirectory,
                        blob_ref_index: None,
                    }],
                };
                let (plaintext, hash) = encode_snapshot_with_blob_refs_and_file_set_to_v3_bytes(
                    &snapshot,
                    &[],
                    &manifest,
                )
                .unwrap();
                input.plaintext = plaintext;
                input.snapshot_hash = hash;
            }
            let attempts = AttemptGate::empty();
            let mut repo = MockEntryRepo::new();
            repo.expect_find_entry_id_by_snapshot_hash()
                .returning(|_| Ok(None));
            let mut materializer = MockBlobMaterializer::new();
            materializer
                .expect_materialize_plan()
                .times(1)
                .returning(|_| {
                    Err(io::Error::other("proof ends after accepted receive handoff").into())
                });
            let readiness = Arc::new(ReceiveReadinessCoordinator::new());
            readiness.mark_ready();
            let owner = ApplyInboundClipboardUseCase::store_only_pull(StoreOnlyPullDeps {
                common: InboundApplyCommonDeps {
                    receive_retry: None,
                    receive_work: registry.clone(),
                    entry_repo: Arc::new(repo),
                    capture: Arc::new(MockCapture::new()),
                    blob_materializer: Arc::new(materializer),
                    receive_attempts: InboundReceiveAttemptDeps {
                        get: attempts.clone(),
                        begin: attempts.clone(),
                        claim_commit: attempts.clone(),
                        request_cancel: attempts.clone(),
                        begin_failure: attempts.clone(),
                        commit: attempts.clone(),
                        clock: attempts,
                    },
                    receive_artifact_cleanup: Arc::new(NoopReceiveArtifactCleanup),
                    receive_readiness: readiness,
                    host_event_emitter: Arc::new(HostEventBus::new()),
                    search_live_index: Arc::new(RecordingLiveIndex {
                        calls: AtomicUsize::new(0),
                    }),
                    availability: Arc::new(FakeAvailability { available: false }),
                    entry_identity_coordinator: Arc::new(EntryIdentityCoordinator::new()),
                },
            });
            assert!(runtime.block_on(owner.execute(input)).is_err());
        }
        let response = if request["op"] == "current" {
            let peer = DeviceId::new(request["peer"].as_str().unwrap());
            let source: [u8; 16] = serde_json::from_value(request["source"].clone()).unwrap();
            registry.current_task(&peer,ProgressIdentity::new(source)).map(|task|json!({
                "producer":task.descriptor.producer.bytes(),"revision":task.descriptor.revision,"source":task.descriptor.source.bytes(),"attempt":task.descriptor.attempt.bytes(),"item_count":task.descriptor.item_count,
                "items":task.items.iter().map(|item| json!({"kind":match item.kind { ProgressItemKind::File=>"file",ProgressItemKind::Representation=>"representation",ProgressItemKind::DirectoryUnit=>"directory" },"index":item.index})).collect::<Vec<_>>()
            })).unwrap_or(Value::Null)
        } else {
            json!({"accepted":true})
        };
        serde_json::to_writer(&mut connection, &response).unwrap();
        connection.write_all(b"\n").unwrap();
    }
}
