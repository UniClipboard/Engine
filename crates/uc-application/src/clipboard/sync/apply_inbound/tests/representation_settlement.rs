//! 完整接收及实际调度／保护；持久能力只注入明确失败，不替换接收步骤。
use super::*;
use crate::transfer::blob::test_support::local_file_probe;
use crate::transfer::blob::transport::BlobShutdownWait;
use crate::transfer::receive::materialization::ReceiveFailureSettlementError;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::timeout;

struct RejectNoEntry {
    gate: Arc<AttemptGate>,
    blocked: AtomicBool,
}
#[async_trait]
impl CommitInboundReceivePort for RejectNoEntry {
    async fn commit_inbound_receive(
        &self,
        settlement: &InboundReceiveSettlement,
    ) -> std::result::Result<(), InboundReceiveCommitError> {
        if self.blocked.load(Ordering::SeqCst)
            && matches!(settlement, InboundReceiveSettlement::NoEntry { .. })
        {
            return Err(InboundReceiveCommitError::Backend(Box::new(
                io::Error::from(io::ErrorKind::PermissionDenied),
            )));
        }
        self.gate.commit_inbound_receive(settlement).await
    }
}

#[tokio::test]
async fn representation_persistence_failure_retains_original_protection_and_both_sources_until_settled(
) {
    for missing in [true, false] {
        for begin_failure in [false, true] {
            let cache = tempfile::tempdir().unwrap();
            let gate = AttemptGate::empty();
            gate.fail_begin_failure
                .store(begin_failure, Ordering::SeqCst);
            let commit = Arc::new(RejectNoEntry {
                gate: gate.clone(),
                blocked: AtomicBool::new(!begin_failure),
            });
            let (facade, transport, protected) = local_file_probe(None);
            let mut repo = MockEntryRepo::new();
            repo.expect_find_entry_id_by_snapshot_hash()
                .returning(|_| Ok(None));
            let mut capture = MockCapture::new();
            let captured_entry = Arc::new(Mutex::new(None));
            let entry = captured_entry.clone();
            let held_at_capture = Arc::new(AtomicUsize::new(usize::MAX));
            let held = held_at_capture.clone();
            let guard = protected.clone();
            capture
                .expect_capture()
                .times(1)
                .returning(move |entry_id, _, _| {
                    *entry.lock().unwrap() = Some(entry_id);
                    held.store(guard.load(Ordering::SeqCst), Ordering::SeqCst);
                    if missing {
                        Ok(None)
                    } else {
                        Err(io::Error::from(io::ErrorKind::StorageFull).into())
                    }
                });
            let mut write = MockWrite::new();
            write.expect_write().times(0);
            let receiver = build(repo, capture, write)
                .with_blob_materializer(Arc::new(test_materializer(
                    facade,
                    cache.path().to_owned(),
                )))
                .with_receive_attempt_ports(
                    gate.clone(),
                    gate.clone(),
                    gate.clone(),
                    gate.clone(),
                    gate.clone(),
                    commit.clone(),
                    gate.clone(),
                );
            let snapshot = SystemClipboardSnapshot {
                ts_ms: 1,
                representations: vec![ObservedClipboardRepresentation::new(
                    RepresentationId::new(),
                    FormatId::from("image"),
                    Some(MimeType("image/png".into())),
                    Vec::new(),
                )],
                file_content_digests: Vec::new(),
                file_set_v1_component: None,
            };
            let refs = [V3BlobRef {
                ticket: BlobTicket::from_bytes(vec![1]),
                entry_id: EntryId::from("source"),
                filename: None,
                mime: Some("image/png".into()),
                size_bytes: 5,
                representation_index: Some(0),
            }];
            let (plaintext, snapshot_hash) =
                encode_snapshot_with_blob_refs_to_v3_bytes(&snapshot, &refs).unwrap();
            let error = receiver
                .execute(ApplyInboundInput {
                    source_entry: None,
                    from_device: DeviceId::new("peer"),
                    snapshot_hash,
                    plaintext,
                    resurface_intent: ClipboardWriteIntent::RemotePush,
                    activation_owner: InboundActivationOwner::Inbound,
                })
                .await
                .unwrap_err();
            let category_preserved = matches!(
                (&error, missing),
                (ApplyInboundError::Internal(_), true) | (ApplyInboundError::Capture(_), false)
            );
            let source = match &error {
                ApplyInboundError::Capture(source) | ApplyInboundError::Internal(source) => source,
                other => panic!("unexpected failure: {other:?}"),
            };
            let failure = source
                .chain()
                .find_map(|source| source.downcast_ref::<ReceiveFailureSettlementError>());
            let primary_preserved = failure.is_some_and(|failure| {
                if missing {
                    !failure.cause.chain().any(|source| source.is::<io::Error>())
                } else {
                    failure.cause.chain().any(|source| {
                        source
                            .downcast_ref::<io::Error>()
                            .is_some_and(|source| source.kind() == io::ErrorKind::StorageFull)
                    })
                }
            });
            let expected_settlement_kind = if begin_failure {
                io::ErrorKind::ReadOnlyFilesystem
            } else {
                io::ErrorKind::PermissionDenied
            };
            let settlement_preserved = failure.is_some_and(|failure| {
                failure.settlement.chain().any(|source| {
                    source
                        .downcast_ref::<io::Error>()
                        .is_some_and(|source| source.kind() == expected_settlement_kind)
                })
            });
            let retained = protected.load(Ordering::SeqCst);
            let stopping = timeout(Duration::from_millis(20), receiver.shutdown()).await;
            let while_stopping = protected.load(Ordering::SeqCst);
            commit.blocked.store(false, Ordering::SeqCst);
            gate.fail_begin_failure.store(false, Ordering::SeqCst);
            timeout(Duration::from_secs(10), receiver.shutdown())
                .await
                .unwrap()
                .unwrap();
            transport
                .shutdown(BlobShutdownWait::Complete)
                .await
                .unwrap();
            let entry = captured_entry.lock().unwrap().clone().unwrap();
            let settled = gate
                .get_entry_attempt(entry.as_ref())
                .await
                .unwrap()
                .unwrap();
            assert!(category_preserved, "原保存错误与缺失结果的公开分类不能互换");
            assert_eq!(held_at_capture.load(Ordering::SeqCst), 1);
            assert!(
                primary_preserved,
                "必须保留原保存失败／缺失结果，missing={missing}"
            );
            assert!(settlement_preserved, "必须保留实际持久交接失败来源");
            assert_eq!(retained, 1, "首次错误不能归还原保护和许可");
            assert_eq!(while_stopping, 1, "等待方退出仍由原任务持有资源");
            assert!(stopping.is_err(), "未完成NoEntry时关闭不能提前成功");
            assert_eq!(settled.state, AttemptState::Failed);
            assert_eq!(
                protected.load(Ordering::SeqCst),
                0,
                "持有错误对象不延长资源寿命"
            );
        }
    }
}
