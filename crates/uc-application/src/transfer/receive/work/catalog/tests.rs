use super::{ReceiveSource, ReceiveWorkRegistry};
use crate::clipboard::sync::payload_codec::V3BlobRef;
use crate::transfer::receive::materialization::{ReceivedItem, ReceivedOutput};
use bytes::Bytes;
use std::sync::Arc;
use uc_core::file_transfer::OutboundProgressStatus;
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
        registry.merge_progress(&mut current);
        assert_eq!(current.items.len(), 3);
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
        assert_eq!(receipt.source.original_wire.is_none(), saved);
        assert_eq!(receipt.source.from_device, DeviceId::new("PRIVATE_DEVICE"));
        assert_eq!(receipt.source.expected_hash, "PRIVATE_HASH");
        assert_eq!(receipt.planned[2].blob_index, 2);
        assert!(receipt
            .planned
            .iter()
            .all(|item| item.kind == ReceiveItemKind::File));
        assert_eq!(format!("{:?}", receipt.source), "ReceiveSource");
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
    registry.merge_progress(&mut current);
    assert_eq!(
        current.items[0].status,
        TrackedFileTransferStatus::Completed
    );
    assert_eq!(current.items[1].status, TrackedFileTransferStatus::Failed);
}
