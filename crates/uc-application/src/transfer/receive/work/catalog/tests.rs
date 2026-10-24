use super::{ReceiveSource, ReceiveWorkRegistry};
use crate::clipboard::sync::payload_codec::V3BlobRef;
use crate::transfer::receive::materialization::{
    InboundFileSetManifest, InboundFileSetMember, ReceivedItem, ReceivedOutput,
};
use bytes::Bytes;
use std::num::TryFromIntError;
use std::sync::Arc;
use uc_core::clipboard::FileSetMemberKind;
use uc_core::file_transfer::{FileTransferCancellationReason, OutboundProgressStatus};
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::blob::BlobTicket;
use uc_core::ports::{
    AttemptState, EntryReceiveProgress, ReceiveItemKind, TrackedFileTransferStatus,
};
use uc_core::ports::{ReceiveArtifact, ReceiveArtifactOwnership};

fn source() -> ReceiveSource {
    ReceiveSource {
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
        action.retain_source(source());
        action.batch.plan(3, 21);
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
    a.retain_source(source());
    let owner = registry.clone();
    let deleting = tokio::spawn(async move { owner.delete_scope(&entry).await });
    a.cancellation().cancelled().await;
    assert!(b.cancellation().is_cancelled());
    assert!(!deleting.is_finished());
    a.retain_source(source());
    assert!(registry.state.lock().unwrap().receipts.is_empty());
    a.finish(Ok(()));
    b.finish(Ok(()));
    drop(deleting.await.unwrap().unwrap());
    assert!(late.register(&EntryId::from("receiver"), "late").is_err());
    let fresh = registry.action();
    fresh.register(&EntryId::from("receiver"), "fresh").unwrap();
    fresh.retain_source(source());
    assert_eq!(registry.state.lock().unwrap().receipts.len(), 1);
    registry.close();
    let generation = registry.generation();
    assert!(registry.open(&generation));
    fresh.retain_source(source());
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
    action.retain_source(source());
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
    action.retain_source(source());
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
    saved.retain_source(source());
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
        incoming.retain_source(new_source);
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
        incoming.retain_source(new_source);
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
        incoming.retain_source(source());
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
    action.retain_source(source());
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
    action.retain_source(original);
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
    action.retain_source(original);
    let mut current = progress(AttemptState::Receiving);
    let error = registry.merge_progress(&mut current).unwrap_err();
    assert!(error.chain().any(|source| source.is::<TryFromIntError>()));
    assert!(!format!("{error:?}").contains("PRIVATE"));
    action.finish(Ok(()));
}
