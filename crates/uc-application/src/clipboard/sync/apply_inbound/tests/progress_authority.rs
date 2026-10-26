//! 真实接收用例的CAS先后证明；存储port采用受控fixture，不冒充SQLite重启。
use super::{
    file_blob_input, ApplyInboundClipboardUseCase, AttemptGate, FakeAvailability, HostEventBus,
    InboundApplyCommonDeps, InboundReceiveAttemptDeps, MockBlobMaterializer, MockCapture,
    MockEntryRepo, NoopReceiveArtifactCleanup, RecordingLiveIndex, StoreOnlyPullDeps,
};
use crate::clipboard::entry_identity::EntryIdentityCoordinator;
use crate::clipboard::sync::payload_codec::{
    decode_v3_bytes_to_snapshot_and_blob_refs, encode_snapshot_with_blob_refs_to_v3_bytes,
};
use crate::transfer::progress::{
    ProgressPageReply, ProgressSnapshotProviderPort, ProgressSnapshotQuery,
    ProgressTaskAuthorityPort, ProgressTaskDescriptor,
};
use crate::transfer::receive::reconciliation::ReceiveReadinessCoordinator;
use crate::transfer::receive::work::ReceiveWorkRegistry;
use async_trait::async_trait;
use std::io;
use std::sync::{atomic::AtomicUsize, Arc};
use tokio::sync::{Notify, Semaphore};
use tokio::time::{timeout, Duration};
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{ProgressIdentity, ProgressItemState, ProgressTerminalOutcome};
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::blob::BlobTicket;
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
    for (source_hint, reject) in [(false, false), (false, true), (true, false), (true, true)] {
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
        let blob_source = EntryId::from("00000000-0000-4000-8000-000000000002");
        let provider_roles = [DeviceId::new("provider-b"), DeviceId::new("provider-c")];
        let (snapshot, original) =
            decode_v3_bytes_to_snapshot_and_blob_refs(&input.plaintext).unwrap();
        // Application不解析ticket定位信息；受控材料化port把三份opaque能力映射到两个provider角色。
        let capabilities = (0..3)
            .map(|index| {
                (
                    BlobTicket::from_bytes(vec![200 + index, 42]),
                    provider_roles[index as usize % 2],
                )
            })
            .collect::<Vec<_>>();
        let references = capabilities
            .iter()
            .enumerate()
            .map(|(index, (ticket, _))| {
                let mut reference = original[0].clone();
                reference.ticket = ticket.clone();
                reference.entry_id = if source_hint {
                    blob_source.clone()
                } else {
                    source.clone()
                };
                reference.filename = Some(format!("handoff-{index}.bin"));
                reference
            })
            .collect::<Vec<_>>();
        let (plaintext, hash) =
            encode_snapshot_with_blob_refs_to_v3_bytes(&snapshot, &references).unwrap();
        input.plaintext = plaintext;
        input.snapshot_hash = hash;
        input.source_entry = source_hint.then_some(source.clone());
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
                .returning(move |plan| {
                    let (from_device, local_entry, _, blobs, directory, attempt, _, _, _) =
                        plan.into_parts();
                    assert_eq!(from_device, peer);
                    assert_ne!(local_entry, source);
                    assert!(directory.is_none());
                    assert_eq!(blobs, references);
                    for blob in &blobs {
                        let (_, provider) = capabilities
                            .iter()
                            .find(|(ticket, _)| ticket == &blob.ticket)
                            .expect("original provider capability");
                        assert_ne!(*provider, from_device);
                        assert!(registry.current_task(provider, id).is_none());
                    }
                    let accepted = registry
                        .current_task(&peer, id)
                        .expect("authoritative before download");
                    let current = ledger.attempt_id.lock().unwrap();
                    assert_eq!(
                        accepted.descriptor.attempt.bytes(),
                        *Uuid::parse_str(&current).unwrap().as_bytes()
                    );
                    assert_eq!(attempt.as_deref(), Some(current.as_str()));
                    assert_eq!(accepted.descriptor.item_count, 3);
                    assert!(accepted
                        .items
                        .iter()
                        .enumerate()
                        .all(|(index, item)| item.index == index as u16));
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
        let observation = CancellationToken::new();
        assert!(timeout(
            Duration::from_millis(10),
            work.next_progress_changes(&observation)
        )
        .await
        .is_err());
        acceptance.release.add_permits(1);
        assert!(applying.await.unwrap().is_err());
        assert_eq!(work.current_task(&peer, id).is_none(), reject);
        if !reject {
            let changes = timeout(
                Duration::from_secs(1),
                work.next_progress_changes(&observation),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(changes.len(), 1);
            assert_eq!(changes[0].peer, peer);
            let snapshot = changes[0].snapshot.as_ref().unwrap();
            assert!(snapshot.descriptor == work.current_task(&peer, id).unwrap().descriptor);
            assert_eq!(snapshot.items.len(), 3);
            assert!(snapshot.items.iter().all(|item| matches!(item, ProgressItemState::Terminal(value) if matches!(value.outcome, ProgressTerminalOutcome::Failed(_)))));
            let current = work
                .snapshot_page(
                    &peer,
                    ProgressSnapshotQuery {
                        source: id,
                        attempt: Some(snapshot.descriptor.attempt),
                        offset: 0,
                        revision: None,
                    },
                )
                .await
                .unwrap();
            assert!(
                matches!(current, ProgressPageReply::Current(page) if page.descriptor == snapshot.descriptor && page.items == snapshot.items)
            );
            for provider in provider_roles {
                assert!(work.current_task(&provider, id).is_none());
                assert!(matches!(
                    work.snapshot_page(
                        &provider,
                        ProgressSnapshotQuery {
                            source: id,
                            attempt: None,
                            offset: 0,
                            revision: None
                        }
                    )
                    .await
                    .unwrap(),
                    ProgressPageReply::NotRetained
                ));
            }
            if source_hint {
                let blob_id = ProgressIdentity::new(
                    *Uuid::parse_str(blob_source.as_ref()).unwrap().as_bytes(),
                );
                assert!(work.current_task(&peer, blob_id).is_none());
            }
        }
        // 观察真实事实不再制造一次变化；拒绝的CAS也不能生成通知资格。
        assert!(timeout(
            Duration::from_millis(10),
            work.next_progress_changes(&observation)
        )
        .await
        .is_err());
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
        encode_snapshot_with_blob_refs_to_v3_bytes,
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
            if request["directory"] != true {
                let count = request["items"].as_u64().unwrap_or(1) as usize;
                assert!((1..=1024).contains(&count));
                let (snapshot, refs) =
                    decode_v3_bytes_to_snapshot_and_blob_refs(&input.plaintext).unwrap();
                let refs: Vec<_> = (0..count)
                    .map(|index| {
                        let mut reference = refs[0].clone();
                        reference.filename = Some(format!("fixture-{index}.bin"));
                        reference
                    })
                    .collect();
                let (plaintext, hash) =
                    encode_snapshot_with_blob_refs_to_v3_bytes(&snapshot, &refs).unwrap();
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
        let encode = |task: crate::transfer::progress::ProgressSourceTask| {
            json!({
                "producer":task.descriptor.producer.bytes(),"revision":task.descriptor.revision,"source":task.descriptor.source.bytes(),"attempt":task.descriptor.attempt.bytes(),"item_count":task.descriptor.item_count,
                "closed":task.items.iter().enumerate().filter(|(_,item)|task.notification_closed.contains(item)).map(|(index,_)|index).collect::<Vec<_>>(),
                "items":task.items.iter().map(|item| json!({"kind":match item.kind { ProgressItemKind::File=>"file",ProgressItemKind::Representation=>"representation",ProgressItemKind::DirectoryUnit=>"directory" },"index":item.index})).collect::<Vec<_>>()
            })
        };
        let response = if request["op"] == "snapshot" {
            use crate::transfer::progress::{
                ProgressPageReply, ProgressSnapshotProviderPort, ProgressSnapshotQuery,
            };
            use uc_core::file_transfer::{ProgressItemState, ProgressTerminalOutcome};
            let peer = DeviceId::new(request["peer"].as_str().unwrap());
            let source =
                ProgressIdentity::new(serde_json::from_value(request["source"].clone()).unwrap());
            let query = ProgressSnapshotQuery {
                source,
                attempt: request["attempt"].as_array().map(|_| {
                    ProgressIdentity::new(
                        serde_json::from_value(request["attempt"].clone()).unwrap(),
                    )
                }),
                offset: request["offset"].as_u64().unwrap() as u16,
                revision: request["revision"].as_u64(),
            };
            match runtime.block_on(registry.snapshot_page(&peer, query)) {
                Ok(ProgressPageReply::Current(page) | ProgressPageReply::Superseded(page)) => {
                    json!({
                        "task": encode(registry.current_task(&peer, source).unwrap()),
                        "revision": page.revision, "offset": page.offset, "total":page.total,
                        "items": page.items.into_iter().map(|state| match state {
                            ProgressItemState::Terminal(value) => json!({
                                "kind":match value.item.kind { ProgressItemKind::File=>"file",ProgressItemKind::Representation=>"representation",ProgressItemKind::DirectoryUnit=>"directory" },
                                "index":value.item.index,"round":value.round,"sequence":value.sequence,
                                "result_revision":value.result_revision,"bytes":value.bytes,"total":value.total,
                                "failed":match value.outcome { ProgressTerminalOutcome::Failed(reason)=>Some(format!("{reason:?}")), _=>None },
                            }),
                            ProgressItemState::Active(_) => json!({"active":true}),
                        }).collect::<Vec<_>>()
                    })
                }
                Ok(ProgressPageReply::NotRetained) => json!({"not_retained":true}),
                _ => panic!("settled query fixture must retain its actual result"),
            }
        } else if request["op"] == "claim" {
            let peer = DeviceId::new(request["peer"].as_str().unwrap());
            let source =
                ProgressIdentity::new(serde_json::from_value(request["source"].clone()).unwrap());
            let attempt =
                ProgressIdentity::new(serde_json::from_value(request["attempt"].clone()).unwrap());
            let item = uc_core::file_transfer::ProgressItem {
                kind: match request["kind"].as_str().unwrap() {
                    "file" => ProgressItemKind::File,
                    "representation" => ProgressItemKind::Representation,
                    "directory" => ProgressItemKind::DirectoryUnit,
                    _ => panic!("invalid fixture kind"),
                },
                index: request["index"].as_u64().unwrap() as u16,
            };
            let expected = ProgressTaskDescriptor {
                producer: ProgressIdentity::new(
                    serde_json::from_value(request["producer"].clone()).unwrap(),
                ),
                revision: request["revision"].as_u64().unwrap(),
                source,
                attempt,
                item_count: u16::try_from(request["item_count"].as_u64().unwrap()).unwrap(),
            };
            match registry.claim_terminal(&peer, expected, item) {
                Ok(task) => json!({"task":encode(task)}),
                Err(reason) => json!({"refusal":format!("{reason:?}")}),
            }
        } else if request["op"] == "current" {
            let peer = DeviceId::new(request["peer"].as_str().unwrap());
            let source: [u8; 16] = serde_json::from_value(request["source"].clone()).unwrap();
            registry
                .current_task(&peer, ProgressIdentity::new(source))
                .map(encode)
                .unwrap_or(Value::Null)
        } else {
            json!({"accepted":true})
        };
        serde_json::to_writer(&mut connection, &response).unwrap();
        connection.write_all(b"\n").unwrap();
    }
}
