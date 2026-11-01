use super::{ReceiveSource, ReceiveWorkRegistry};
use crate::clipboard::sync::payload_codec::{
    decode_v3_bytes_to_snapshot_and_blob_refs, encode_snapshot_with_blob_refs_to_v3_bytes,
    V3BlobRef,
};
use crate::transfer::receive::materialization::{
    InboundFileSetManifest, InboundFileSetMember, ReceivedItem, ReceivedOutput,
};
use bytes::Bytes;
use std::num::TryFromIntError;
use std::sync::Arc;
use uc_core::clipboard::FileSetMemberKind;
use uc_core::file_transfer::{FileTransferCancellationReason, OutboundProgressStatus};
use uc_core::ids::{DeviceId, EntryId, FormatId, RepresentationId};
use uc_core::ports::blob::BlobTicket;
use uc_core::ports::{
    AttemptState, EntryReceiveProgress, ReceiveItemKind, TrackedFileTransferStatus,
};
use uc_core::ports::{ReceiveArtifact, ReceiveArtifactOwnership};
use uc_core::{ObservedClipboardRepresentation, SystemClipboardSnapshot};

fn source() -> ReceiveSource {
    ReceiveSource {
        source_entry: None,
        from_device: DeviceId::new("PRIVATE_DEVICE"),
        expected_hash: "PRIVATE_HASH".into(),
        original_wire: Some(Bytes::from_static(b"PRIVATE_ORIGINAL_WIRE")),
        representations: Vec::new(),
        manifest: None,
        blob_refs: (0..3)
            .map(|_| V3BlobRef {
                ticket: BlobTicket::from_bytes(vec![9]),
                entry_id: EntryId::from("PRIVATE_SOURCE"),
                filename: Some("PRIVATE_FILENAME".into()),
                mime: None,
                size_bytes: 7,
                representation_index: None,
            })
            .collect(),
    }
}
fn progress(state: AttemptState) -> EntryReceiveProgress {
    EntryReceiveProgress {
        snapshot_version: None,
        entry_id: "receiver".into(),
        attempt_id: "attempt".into(),
        state,
        total_bytes: 21,
        completed_bytes: 7,
        items_total: 1,
        items_completed: 1,
        items: Vec::new(),
    }
}

#[test]
fn query_includes_unstarted_items_and_requires_real_persistence_for_completed_downloads() {
    for saved in [false, true] {
        let registry = Arc::new(ReceiveWorkRegistry::default());
        let action = registry.action();
        action
            .register(&EntryId::from("receiver"), "attempt")
            .unwrap();
        action.accept_source(source());
        action.batch.plan(3);
        action.batch.item_result(
            "receiver:attempt:attempt:member:0",
            7,
            OutboundProgressStatus::Completed,
            None,
        );
        if saved {
            action.persisted(
                false,
                &[ReceivedItem {
                    blob_index: 0,
                    plaintext_hash: [1; 32],
                    bytes: 7,
                    output: ReceivedOutput::File(ReceiveArtifact {
                        item_id: "receiver:attempt:attempt:member:0".into(),
                        staged_path: "private-saved".into(),
                        final_path: "private-saved".into(),
                        ownership: ReceiveArtifactOwnership::UserDestination,
                    }),
                }],
            );
        }
        action.batch.settled(AttemptState::Failed);
        action.finish(Ok(()));
        drop(action);
        let mut current = progress(AttemptState::Failed);
        registry.merge_progress(&mut current).unwrap();
        assert_eq!(current.items.len(), 3);
        assert_eq!(current.items_total, 3);
        assert_eq!(current.total_bytes, 21);
        assert_eq!(current.items_completed, if saved { 1 } else { 0 });
        assert_eq!(current.completed_bytes, if saved { 7 } else { 0 });
        assert_eq!(
            current.items[0].status,
            if saved {
                TrackedFileTransferStatus::Completed
            } else {
                TrackedFileTransferStatus::Failed
            }
        );
        assert_eq!(current.items[1].status, TrackedFileTransferStatus::Failed);
        assert_eq!(current.items[2].status, TrackedFileTransferStatus::Failed);
        assert_ne!(current.items[0].item_id, current.items[1].item_id);
        assert!(!format!("{current:?}").contains("PRIVATE"));
        let state = registry.state.lock().unwrap();
        let receipt = &state.receipts[&EntryId::from("receiver")];
        assert_eq!(
            receipt.source.as_ref().unwrap().original_wire.is_none(),
            saved
        );
        assert_eq!(
            receipt.source.as_ref().unwrap().from_device,
            DeviceId::new("PRIVATE_DEVICE")
        );
        assert_eq!(
            receipt.source.as_ref().unwrap().expected_hash,
            "PRIVATE_HASH"
        );
        assert_eq!(receipt.planned[2].blob_index, 2);
        assert!(receipt
            .planned
            .iter()
            .all(|item| item.kind == ReceiveItemKind::File));
        assert_eq!(
            format!("{:?}", receipt.source.as_ref().unwrap()),
            "ReceiveSource"
        );
    }
}

#[tokio::test]
async fn delete_seals_late_actions_and_notifies_all_existing_actions_before_waiting() {
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let entry = EntryId::from("receiver");
    let late = registry.action();
    let a = registry.action();
    let b = registry.action();
    a.register(&entry, "a").unwrap();
    b.register(&entry, "b").unwrap();
    a.accept_source(source());
    let owner = registry.clone();
    let deleting = tokio::spawn(async move { owner.delete_scope(&entry).await });
    a.cancellation().cancelled().await;
    assert!(b.cancellation().is_cancelled());
    assert!(!deleting.is_finished());
    a.accept_source(source());
    assert!(registry.state.lock().unwrap().receipts.is_empty());
    a.finish(Ok(()));
    b.finish(Ok(()));
    drop(deleting.await.unwrap().unwrap());
    assert!(late.register(&EntryId::from("receiver"), "late").is_err());
    let fresh = registry.action();
    fresh.register(&EntryId::from("receiver"), "fresh").unwrap();
    fresh.accept_source(source());
    assert_eq!(registry.state.lock().unwrap().receipts.len(), 1);
    registry.close();
    let generation = registry.generation();
    assert!(registry.open(&generation));
    fresh.accept_source(source());
    assert!(registry.state.lock().unwrap().receipts.is_empty());
    fresh.finish(Ok(()));
}

#[test]
fn ordinary_cancel_keeps_original_source_but_full_success_releases_it() {
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let action = registry.action();
    action
        .register(&EntryId::from("receiver"), "attempt")
        .unwrap();
    registry
        .stop(&EntryId::from("receiver"), "attempt")
        .unwrap();
    action.accept_source(source());
    assert_eq!(registry.state.lock().unwrap().receipts.len(), 1);
    action.persisted(true, &[]);
    assert!(registry.state.lock().unwrap().receipts.is_empty());
    action.finish(Ok(()));
}

#[test]
fn only_the_explicitly_committed_item_proof_can_turn_download_completion_into_success() {
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let action = registry.action();
    action
        .register(&EntryId::from("receiver"), "attempt")
        .unwrap();
    action.accept_source(source());
    for index in 0..2 {
        action.batch.item_result(
            &format!("receiver:attempt:attempt:member:{index}"),
            7,
            OutboundProgressStatus::Completed,
            None,
        );
    }
    action.persisted(
        false,
        &[ReceivedItem {
            blob_index: 0,
            plaintext_hash: [1; 32],
            bytes: 7,
            output: ReceivedOutput::File(ReceiveArtifact {
                item_id: "saved".into(),
                staged_path: "saved".into(),
                final_path: "saved".into(),
                ownership: ReceiveArtifactOwnership::UserDestination,
            }),
        }],
    );
    action.finish(Ok(()));
    let mut current = progress(AttemptState::Failed);
    registry.merge_progress(&mut current).unwrap();
    assert_eq!(
        current.items[0].status,
        TrackedFileTransferStatus::Completed
    );
    assert_eq!(current.items[1].status, TrackedFileTransferStatus::Failed);
}

fn saved_registry() -> Arc<ReceiveWorkRegistry> {
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let saved = registry.action();
    saved
        .register(&EntryId::from("receiver"), "attempt")
        .unwrap();
    saved.accept_source(source());
    saved.batch.item_result(
        "receiver:attempt:attempt:member:2",
        0,
        OutboundProgressStatus::Cancelled {
            reason: FileTransferCancellationReason::LocalUser,
        },
        None,
    );
    saved.persisted(
        false,
        &[ReceivedItem {
            blob_index: 0,
            plaintext_hash: [1; 32],
            bytes: 7,
            output: ReceivedOutput::File(ReceiveArtifact {
                item_id: "saved-original".into(),
                staged_path: "saved-original".into(),
                final_path: "saved-original".into(),
                ownership: ReceiveArtifactOwnership::UserDestination,
            }),
        }],
    );
    saved.finish(Ok(()));
    registry
}

#[test]
fn successive_uncommitted_sources_keep_original_item_identity_and_saved_results() {
    let registry = saved_registry();
    for attempt in ["second", "third"] {
        let incoming = registry.action();
        incoming
            .register(&EntryId::from("receiver"), attempt)
            .unwrap();
        let mut new_source = source();
        new_source.from_device = DeviceId::new("different");
        new_source.blob_refs.truncate(1);
        incoming.accept_source(new_source);
        incoming.finish(Ok(()));
        // 重复完成通知不能把回退后的保存事实替换为已丢弃的新下载事实。
        incoming.finish(Ok(()));
        let (original, landed) = registry
            .retry_material(&EntryId::from("receiver"), attempt)
            .unwrap();
        assert_eq!(original.from_device, DeviceId::new("PRIVATE_DEVICE"));
        assert_eq!(original.blob_refs.len(), 3);
        assert_eq!(landed.len(), 1);
        assert!(landed.contains_key(&0));
        let mut current = progress(AttemptState::Failed);
        current.attempt_id = attempt.into();
        registry.merge_progress(&mut current).unwrap();
        assert_eq!(
            current
                .items
                .iter()
                .map(|item| item.status)
                .collect::<Vec<_>>(),
            vec![
                TrackedFileTransferStatus::Completed,
                TrackedFileTransferStatus::Failed,
                TrackedFileTransferStatus::Cancelled,
            ]
        );
        assert_eq!(
            current.items[0].item_id,
            format!("receiver:attempt:{attempt}:member:0")
        );
    }
}

#[test]
fn actual_persistence_replaces_previous_recipe_and_full_success_releases_both_sources() {
    for complete in [false, true] {
        let registry = saved_registry();
        let incoming = registry.action();
        incoming
            .register(&EntryId::from("receiver"), "second")
            .unwrap();
        let mut new_source = source();
        new_source.from_device = DeviceId::new("different");
        new_source.blob_refs.truncate(2);
        incoming.accept_source(new_source);
        let new_saved = ReceivedItem {
            blob_index: 1,
            plaintext_hash: [2; 32],
            bytes: 7,
            output: ReceivedOutput::File(ReceiveArtifact {
                item_id: "new-saved".into(),
                staged_path: "new-saved".into(),
                final_path: "new-saved".into(),
                ownership: ReceiveArtifactOwnership::UserDestination,
            }),
        };
        incoming.persisted(complete, &[new_saved]);
        incoming.finish(Ok(()));
        let material = registry.retry_material(&EntryId::from("receiver"), "second");
        if complete {
            assert!(material.is_none());
            assert!(registry.state.lock().unwrap().receipts.is_empty());
        } else {
            let (original, landed) = material.unwrap();
            assert_eq!(original.from_device, DeviceId::new("different"));
            assert_eq!(original.blob_refs.len(), 2);
            assert_eq!(landed.len(), 1);
            assert!(landed.contains_key(&1));
        }
    }
}

#[tokio::test]
async fn close_and_delete_discard_saved_backup_before_late_completion() {
    for deleting in [false, true] {
        let registry = saved_registry();
        let incoming = registry.action();
        let entry = EntryId::from("receiver");
        incoming.register(&entry, "second").unwrap();
        incoming.accept_source(source());
        if deleting {
            let owner = registry.clone();
            let deleting = tokio::spawn(async move { owner.delete_scope(&entry).await });
            incoming.cancellation().cancelled().await;
            incoming.finish(Ok(()));
            drop(deleting.await.unwrap().unwrap());
        } else {
            registry.close();
            assert!(registry.open(&registry.generation()));
            incoming.finish(Ok(()));
        }
        incoming.finish(Ok(()));
        assert!(registry.state.lock().unwrap().receipts.is_empty());
        assert!(incoming.previous_receipt.lock().unwrap().is_none());
    }
}

#[test]
fn query_uses_original_plan_totals_before_any_download_rows_exist() {
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let action = registry.action();
    action
        .register(&EntryId::from("receiver"), "attempt")
        .unwrap();
    action.accept_source(source());
    let mut current = progress(AttemptState::Receiving);
    current.items_total = 0;
    current.total_bytes = 0;
    registry.merge_progress(&mut current).unwrap();
    assert_eq!(current.items_total, 3);
    assert_eq!(current.total_bytes, 21);
    assert_eq!(current.items_completed, 0);
    assert_eq!(current.completed_bytes, 0);
    assert!(current
        .items
        .iter()
        .all(|item| item.status == TrackedFileTransferStatus::Pending));
    action.finish(Ok(()));
}

#[test]
fn directory_query_keeps_member_totals_but_completes_only_the_whole_unit() {
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let action = registry.action();
    action
        .register(&EntryId::from("receiver"), "attempt")
        .unwrap();
    let mut original = source();
    original.manifest = Some(InboundFileSetManifest {
        members: (0..3)
            .map(|index| InboundFileSetMember {
                root_index: 0,
                root_name: "PRIVATE_ROOT".into(),
                root_is_file: false,
                relative_path: format!("PRIVATE_MEMBER_{index}"),
                kind: FileSetMemberKind::File,
                blob_ref_index: Some(index),
            })
            .collect(),
    });
    action.accept_source(original);
    for state in [
        AttemptState::Receiving,
        AttemptState::Committing,
        AttemptState::Failing,
        AttemptState::Failed,
        AttemptState::Cancelled,
        AttemptState::Completed,
    ] {
        let mut current = progress(state);
        registry.merge_progress(&mut current).unwrap();
        assert_eq!(current.items.len(), 1);
        assert_eq!(current.items[0].kind, ReceiveItemKind::DirectoryUnit);
        assert_eq!(current.items_total, 3);
        assert_eq!(current.total_bytes, 21);
        assert_eq!(
            current.items_completed,
            if state == AttemptState::Completed {
                3
            } else {
                0
            }
        );
        assert_eq!(
            current.completed_bytes,
            if state == AttemptState::Completed {
                21
            } else {
                0
            }
        );
        assert!(!format!("{current:?}").contains("PRIVATE"));
    }
    action.finish(Ok(()));
}

#[test]
fn out_of_range_plan_bytes_are_reported_with_source_instead_of_clamping() {
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let action = registry.action();
    action
        .register(&EntryId::from("receiver"), "attempt")
        .unwrap();
    let mut original = source();
    original.blob_refs[0].size_bytes = u64::MAX;
    action.accept_source(original);
    let mut current = progress(AttemptState::Receiving);
    let error = registry.merge_progress(&mut current).unwrap_err();
    assert!(error.chain().any(|source| source.is::<TryFromIntError>()));
    assert!(!format!("{error:?}").contains("PRIVATE"));
    action.finish(Ok(()));
}

#[test]
fn unchanged_v3_preserves_source_and_original_logical_positions_across_attempts() {
    let mut original = source();
    original.blob_refs.resize(4, original.blob_refs[0].clone());
    original.blob_refs[0].representation_index = Some(2);
    original.blob_refs[2].representation_index = Some(0);
    let snapshot = SystemClipboardSnapshot {
        ts_ms: 0,
        representations: (0..3)
            .map(|_| {
                ObservedClipboardRepresentation::new(
                    RepresentationId::new(),
                    FormatId::from("text"),
                    None,
                    Vec::new(),
                )
            })
            .collect(),
        file_content_digests: Vec::new(),
        file_set_v1_component: None,
    };
    let (wire, _) =
        encode_snapshot_with_blob_refs_to_v3_bytes(&snapshot, &original.blob_refs).unwrap();
    let (_, decoded) = decode_v3_bytes_to_snapshot_and_blob_refs(&wire).unwrap();
    assert_eq!(decoded, original.blob_refs);
    assert!(decoded
        .iter()
        .all(|blob| blob.entry_id == EntryId::from("PRIVATE_SOURCE")));
    original.blob_refs = decoded;
    let mut observed = Vec::new();
    for (entry, attempt) in [
        ("receiver-a", "first"),
        ("receiver-a", "second"),
        ("receiver-b", "first"),
    ] {
        let planned = original.planned(&EntryId::from(entry), attempt);
        assert_eq!(planned.len(), 4); // 相同名称、ticket、大小不合并逻辑项。
        for (position, (kind, suffix)) in [
            (ReceiveItemKind::Representation, "representation:2"),
            (ReceiveItemKind::File, "member:0"),
            (ReceiveItemKind::Representation, "representation:0"),
            (ReceiveItemKind::File, "member:1"),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(planned[position].blob_index, position);
            assert_eq!(planned[position].kind, kind);
            assert_eq!(
                planned[position].id,
                format!("{entry}:attempt:{attempt}:{suffix}")
            );
        }
        observed.push(planned.into_iter().map(|item| item.id).collect::<Vec<_>>());
    }
    assert_ne!(observed[0], observed[1]);
    assert_ne!(observed[0], observed[2]);
    original.manifest = Some(InboundFileSetManifest {
        members: Vec::new(),
    });
    let directory = original.planned(&EntryId::from("receiver-a"), "third");
    assert_eq!(directory.len(), 1);
    assert_eq!(directory[0].kind, ReceiveItemKind::DirectoryUnit);
}

#[test]
fn accepted_progress_is_current_idempotent_and_late_finished_actions_cannot_restore_old_attempt() {
    use crate::transfer::progress::ProgressTaskAuthorityPort;
    use uc_core::file_transfer::{ProgressIdentity, ProgressItemKind};
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let peer = DeviceId::new("PRIVATE_DEVICE");
    let entry = EntryId::from("local");
    let mut original = source();
    original.source_entry = Some(EntryId::from("00000000-0000-4000-8000-000000000001"));
    original.blob_refs[0].representation_index = Some(2);
    let source_id = ProgressIdentity::new(
        *uuid::Uuid::parse_str(original.source_entry.as_ref().unwrap().as_ref())
            .unwrap()
            .as_bytes(),
    );
    let first = registry.action();
    first
        .register(&entry, "00000000-0000-4000-8000-000000000009")
        .unwrap();
    first.prepare_plan(&original);
    assert!(registry.current_task(&peer, source_id).is_none());
    first.accept_source(original.clone());
    let accepted = registry.current_task(&peer, source_id).unwrap();
    assert_eq!(accepted.items[0].kind, ProgressItemKind::Representation);
    assert_eq!(accepted.items[0].index, 2);
    assert_eq!(accepted.items[1].index, 0);
    assert_eq!(accepted.items[2].index, 1);
    first.accept_source(original.clone());
    assert_eq!(
        registry
            .current_task(&peer, source_id)
            .unwrap()
            .descriptor
            .revision,
        accepted.descriptor.revision
    );
    first.finish(Ok(()));
    let next = registry.action();
    next.register(&entry, "00000000-0000-4000-8000-000000000002")
        .unwrap();
    next.accept_source(original.clone());
    let current = registry.current_task(&peer, source_id).unwrap();
    first.accept_source(original);
    assert_eq!(
        registry
            .current_task(&peer, source_id)
            .unwrap()
            .descriptor
            .attempt,
        current.descriptor.attempt
    );
    assert!(current.descriptor.revision > accepted.descriptor.revision);
    assert_eq!(current.descriptor.producer, accepted.descriptor.producer);
    next.finish(Ok(()));
}

#[tokio::test]
async fn empty_directory_uses_authenticated_source_hint_and_delete_retires_its_mapping() {
    use crate::transfer::progress::ProgressTaskAuthorityPort;
    use uc_core::file_transfer::{ProgressIdentity, ProgressItemKind};
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let peer = DeviceId::new("PRIVATE_DEVICE");
    let mut original = source();
    original.source_entry = Some(EntryId::from("00000000-0000-4000-8000-000000000001"));
    original.blob_refs.clear();
    original.manifest = Some(InboundFileSetManifest {
        members: vec![InboundFileSetMember {
            root_index: 0,
            root_name: "folder".into(),
            root_is_file: false,
            relative_path: String::new(),
            kind: FileSetMemberKind::EmptyDirectory,
            blob_ref_index: None,
        }],
    });
    let mut bytes = [0; 16];
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes[15] = 1;
    let source_id = ProgressIdentity::new(bytes);
    let entry = EntryId::from("local");
    let action = registry.action();
    action
        .register(&entry, "00000000-0000-4000-8000-000000000002")
        .unwrap();
    action.accept_source(original.clone());
    let accepted = registry.current_task(&peer, source_id).unwrap();
    assert_eq!(accepted.items.len(), 1);
    assert_eq!(accepted.items[0].kind, ProgressItemKind::DirectoryUnit);
    action.persisted(true, &[]);
    assert!(registry.current_task(&peer, source_id).is_some());
    action.finish(Ok(()));
    let deleted = registry.delete_scope(&entry).await.unwrap();
    action.accept_source(original);
    assert!(registry.current_task(&peer, source_id).is_none());
    drop(deleted);
}

#[test]
fn actual_process_restart_creates_new_producer_and_does_not_restore_old_source_associations() {
    use crate::transfer::progress::ProgressTaskAuthorityPort;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};
    use uc_core::file_transfer::ProgressIdentity;
    const CHILD_MARKER: &str = "UC_PROGRESS_INCARNATION_FIXTURE";
    let source_id = ProgressIdentity::new(
        *uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000001")
            .unwrap()
            .as_bytes(),
    );
    if let Some(marker) = std::env::var_os(CHILD_MARKER) {
        let registry = Arc::new(ReceiveWorkRegistry::default());
        let peer = DeviceId::new("PRIVATE_DEVICE");
        assert!(registry.current_task(&peer, source_id).is_none());
        let action = registry.action();
        action
            .register(
                &EntryId::from("local"),
                "00000000-0000-4000-8000-000000000002",
            )
            .unwrap();
        let mut original = source();
        original.source_entry = Some(EntryId::from("00000000-0000-4000-8000-000000000001"));
        action.accept_source(original);
        let current = registry.current_task(&peer, source_id).unwrap();
        let mut observation = current.descriptor.producer.bytes().to_vec();
        observation.push(1); // 新进程首次读取确为未保留；文件只承载本测试的合成运行身份。
        std::fs::write(marker, observation).unwrap();
        loop {
            std::thread::park();
        } // 父进程真正终止进程，不以重新构造对象代替重启。
    }
    struct Process(Child);
    impl Drop for Process {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
            }
            let _ = self.0.wait();
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("synthetic-progress-incarnation");
    let mut producers = Vec::new();
    for _ in 0..2 {
        let mut process = Process(Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "transfer::receive::work::catalog::tests::actual_process_restart_creates_new_producer_and_does_not_restore_old_source_associations", "--nocapture"])
            .env(CHILD_MARKER, &marker).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        let observation = loop {
            if let Ok(observation) = std::fs::read(&marker) {
                if observation.len() == 17 {
                    break observation;
                }
            }
            assert!(
                process.0.try_wait().unwrap().is_none(),
                "child must remain alive until terminated"
            );
            assert!(
                Instant::now() < deadline,
                "child must publish accepted source state"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(observation[16], 1);
        producers.push(observation[..16].to_vec());
        process.0.kill().unwrap();
        process.0.wait().unwrap();
        std::fs::remove_file(&marker).unwrap();
    }
    assert_ne!(producers[0], producers[1]);
}

#[test]
fn invalid_new_association_retires_old_progress_without_discarding_receive_source() {
    use crate::transfer::progress::ProgressTaskAuthorityPort;
    use uc_core::file_transfer::ProgressIdentity;
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let peer = DeviceId::new("PRIVATE_DEVICE");
    let entry = EntryId::from("local");
    let source_id = ProgressIdentity::new(
        *uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000001")
            .unwrap()
            .as_bytes(),
    );
    let mut original = source();
    original.source_entry = Some(EntryId::from("00000000-0000-4000-8000-000000000001"));
    let first = registry.action();
    first
        .register(&entry, "00000000-0000-4000-8000-000000000002")
        .unwrap();
    first.accept_source(original.clone());
    first.finish(Ok(()));
    assert!(registry.current_task(&peer, source_id).is_some());
    let second = registry.action();
    second
        .register(&entry, "00000000-0000-4000-8000-000000000003")
        .unwrap();
    original.source_entry = Some(EntryId::from("unrepresentable-legacy-source"));
    second.accept_source(original);
    assert!(registry.current_task(&peer, source_id).is_none());
    assert!(registry
        .retry_material(&entry, "00000000-0000-4000-8000-000000000003")
        .is_some());
    second.finish(Ok(()));
}

#[test]
fn repeated_handoff_from_older_live_entry_cannot_replace_newer_source_attempt() {
    use crate::transfer::progress::ProgressTaskAuthorityPort;
    use uc_core::file_transfer::ProgressIdentity;
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let peer = DeviceId::new("PRIVATE_DEVICE");
    let source_id = ProgressIdentity::new(
        *uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000001")
            .unwrap()
            .as_bytes(),
    );
    let mut original = source();
    original.source_entry = Some(EntryId::from("00000000-0000-4000-8000-000000000001"));
    let first = registry.action();
    first
        .register(
            &EntryId::from("older-local"),
            "00000000-0000-4000-8000-000000000009",
        )
        .unwrap();
    first.accept_source(original.clone());
    let second = registry.action();
    second
        .register(
            &EntryId::from("newer-local"),
            "00000000-0000-4000-8000-000000000002",
        )
        .unwrap();
    second.accept_source(original.clone());
    let current = registry.current_task(&peer, source_id).unwrap();
    first.accept_source(original);
    let after = registry.current_task(&peer, source_id).unwrap();
    assert_eq!(after.descriptor.attempt, current.descriptor.attempt);
    assert_eq!(after.descriptor.revision, current.descriptor.revision);
    first.finish(Ok(()));
    second.finish(Ok(()));
}

#[test]
fn legacy_blob_identity_requires_one_unambiguous_source_and_never_guesses_by_hash() {
    use crate::transfer::progress::ProgressTaskAuthorityPort;
    use uc_core::file_transfer::ProgressIdentity;
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let peer = DeviceId::new("PRIVATE_DEVICE");
    let entry = EntryId::from("local");
    let origin = EntryId::from("00000000-0000-4000-8000-000000000001");
    let source_id =
        ProgressIdentity::new(*uuid::Uuid::parse_str(origin.as_ref()).unwrap().as_bytes());
    let mut original = source();
    for blob in &mut original.blob_refs {
        blob.entry_id = origin.clone();
    }
    let first = registry.action();
    first
        .register(&entry, "00000000-0000-4000-8000-000000000002")
        .unwrap();
    first.accept_source(original.clone());
    first.finish(Ok(()));
    assert_eq!(
        registry.current_task(&peer, source_id).unwrap().items.len(),
        3
    );
    original.blob_refs[1].entry_id = EntryId::from("00000000-0000-4000-8000-000000000009");
    let second = registry.action();
    second
        .register(&entry, "00000000-0000-4000-8000-000000000003")
        .unwrap();
    second.accept_source(original);
    assert!(registry.current_task(&peer, source_id).is_none());
    assert!(registry
        .retry_material(&entry, "00000000-0000-4000-8000-000000000003")
        .is_some());
    second.finish(Ok(()));
}

mod query_snapshot;
