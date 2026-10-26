use super::{
    source, Arc, AttemptState, DeviceId, EntryId, OutboundProgressStatus, ReceiveWorkRegistry,
};
use crate::transfer::progress::{
    ProgressPageReply, ProgressReceiveError, ProgressSnapshotProviderPort, ProgressSnapshotQuery,
};
use uc_core::file_transfer::{
    ProgressIdentity, ProgressItemState, ProgressPhase, ProgressTerminalOutcome,
};
use uuid::Uuid;

fn query() -> ProgressSnapshotQuery {
    ProgressSnapshotQuery {
        source: ProgressIdentity::new(
            *Uuid::parse_str("00000000-0000-4000-8000-000000000001")
                .unwrap()
                .as_bytes(),
        ),
        attempt: None,
        offset: 0,
        revision: None,
    }
}
fn current(reply: ProgressPageReply) -> crate::transfer::progress::ProgressSnapshotPage {
    match reply {
        ProgressPageReply::Current(page) => page,
        _ => panic!("expected current page"),
    }
}
#[tokio::test]
async fn actual_receive_query_distinguishes_network_completion_from_commit_and_failure_cleanup() {
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let action = registry.action();
    let entry = EntryId::from("receiver");
    let attempt = "00000000-0000-4000-8000-000000000009";
    let mut source = source();
    source.source_entry = Some(EntryId::from("00000000-0000-4000-8000-000000000001"));
    let peer = source.from_device;
    let planned = source.planned(&entry, attempt);
    action.register(&entry, attempt).unwrap();
    action.prepare_plan(&source);
    assert!(matches!(
        registry.snapshot_page(&peer, query()).await.unwrap(),
        ProgressPageReply::NotRetained
    ));
    action.accept_source(source);
    action.batch.plan(3);
    let initial = current(registry.snapshot_page(&peer, query()).await.unwrap());
    assert_eq!(initial.items.len(), 3);
    assert!(initial.items.iter().all(|item| matches!(item, ProgressItemState::Active(value) if value.phase == ProgressPhase::Waiting)));
    action
        .batch
        .item_result(&planned[0].id, 7, OutboundProgressStatus::Completed, None);
    let processing = current(registry.snapshot_page(&peer, query()).await.unwrap());
    assert!(processing.revision > initial.revision);
    assert!(
        matches!(processing.items[0], ProgressItemState::Active(value) if value.phase == ProgressPhase::Processing && value.bytes == 7)
    );
    assert_eq!(
        current(registry.snapshot_page(&peer, query()).await.unwrap()).revision,
        processing.revision
    );
    action
        .batch
        .item_result(&planned[1].id, 0, OutboundProgressStatus::Failed, None);
    let cleaning = current(registry.snapshot_page(&peer, query()).await.unwrap());
    assert!(
        matches!(cleaning.items[1], ProgressItemState::Active(value) if value.phase == ProgressPhase::Processing)
    );
    action.finish(Err(Arc::new(
        crate::transfer::receive::work::ReceiveWorkError::Action(
            std::io::Error::other("actual receive action failed").into(),
        ),
    )));
    drop(action);
    let settled = current(registry.snapshot_page(&peer, query()).await.unwrap());
    assert!(settled.items.iter().all(|item| matches!(item, ProgressItemState::Terminal(value) if matches!(value.outcome, ProgressTerminalOutcome::Failed(_)))));
    assert!(matches!(
        registry
            .snapshot_page(&DeviceId::new("other"), query())
            .await
            .unwrap(),
        ProgressPageReply::NotRetained
    ));
    registry.close();
    assert!(matches!(
        registry.snapshot_page(&peer, query()).await.unwrap(),
        ProgressPageReply::AccessDenied
    ));
}
#[tokio::test]
async fn query_pages_refuse_changed_content_and_new_attempts_never_reuse_old_snapshot_revision() {
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let entry = EntryId::from("receiver");
    let mut source = source();
    source.source_entry = Some(EntryId::from("00000000-0000-4000-8000-000000000001"));
    source.blob_refs = (0..65).map(|_| source.blob_refs[0].clone()).collect();
    let peer = source.from_device;
    let action = registry.action();
    let attempt = "00000000-0000-4000-8000-000000000009";
    let planned = source.planned(&entry, attempt);
    action.register(&entry, attempt).unwrap();
    action.accept_source(source.clone());
    let first = current(registry.snapshot_page(&peer, query()).await.unwrap());
    assert_eq!(first.items.len(), 64);
    let next = ProgressSnapshotQuery {
        offset: 64,
        revision: Some(first.revision),
        ..query()
    };
    assert_eq!(
        current(registry.snapshot_page(&peer, next).await.unwrap())
            .items
            .len(),
        1
    );
    action
        .batch
        .item_result(&planned[0].id, 3, OutboundProgressStatus::InProgress, None);
    assert!(matches!(
        registry.snapshot_page(&peer, next).await,
        Err(ProgressReceiveError::Stale)
    ));
    action.finish(Ok(()));
    let replaced = registry.action();
    replaced
        .register(&entry, "00000000-0000-4000-8000-000000000002")
        .unwrap();
    replaced.accept_source(source);
    let expected = ProgressSnapshotQuery {
        attempt: Some(first.descriptor.attempt),
        ..query()
    };
    assert!(
        matches!(registry.snapshot_page(&peer, expected).await.unwrap(), ProgressPageReply::Superseded(page) if page.descriptor.attempt != first.descriptor.attempt && page.revision > first.revision)
    );
    assert!(matches!(
        registry.snapshot_page(&peer, next).await,
        Err(ProgressReceiveError::Stale)
    ));
    replaced.finish(Ok(()));
}

#[tokio::test]
async fn saved_success_survives_other_item_failure_and_delete_retires_query_before_cleanup_ends() {
    use super::{ReceivedItem, ReceivedOutput};
    use uc_core::ports::{ReceiveArtifact, ReceiveArtifactOwnership};
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let entry = EntryId::from("receiver");
    let attempt = "00000000-0000-4000-8000-000000000009";
    let mut source = source();
    source.source_entry = Some(EntryId::from("00000000-0000-4000-8000-000000000001"));
    let peer = source.from_device;
    let planned = source.planned(&entry, attempt);
    let action = registry.action();
    action.register(&entry, attempt).unwrap();
    action.accept_source(source);
    action.batch.plan(3);
    action
        .batch
        .item_result(&planned[0].id, 7, OutboundProgressStatus::Completed, None);
    action.persisted(
        false,
        &[ReceivedItem {
            blob_index: 0,
            plaintext_hash: [1; 32],
            bytes: 7,
            output: ReceivedOutput::File(ReceiveArtifact {
                item_id: planned[0].id.clone(),
                staged_path: "private-saved".into(),
                final_path: "private-saved".into(),
                ownership: ReceiveArtifactOwnership::UserDestination,
            }),
        }],
    );
    let saved = current(registry.snapshot_page(&peer, query()).await.unwrap());
    assert!(
        matches!(saved.items[0], ProgressItemState::Terminal(value) if value.outcome == ProgressTerminalOutcome::Completed)
    );
    let owner = registry.clone();
    let deleting = tokio::spawn(async move { owner.delete_scope(&entry).await });
    action.cancellation().cancelled().await;
    assert!(!deleting.is_finished());
    assert!(matches!(
        registry.snapshot_page(&peer, query()).await.unwrap(),
        ProgressPageReply::NotRetained
    ));
    action.finish(Ok(()));
    drop(deleting.await.unwrap().unwrap());
    assert!(matches!(
        registry.snapshot_page(&peer, query()).await.unwrap(),
        ProgressPageReply::NotRetained
    ));
}

#[tokio::test]
async fn successful_files_and_directory_remain_queryable_after_retry_receipt_is_released() {
    use super::InboundFileSetManifest;
    for directory in [false, true] {
        let registry = Arc::new(ReceiveWorkRegistry::default());
        let entry = EntryId::from("receiver");
        let attempt = "00000000-0000-4000-8000-000000000009";
        let mut source = source();
        source.source_entry = Some(EntryId::from("00000000-0000-4000-8000-000000000001"));
        if directory {
            source.manifest = Some(InboundFileSetManifest {
                members: Vec::new(),
            });
        }
        let peer = source.from_device;
        let planned = source.planned(&entry, attempt);
        let action = registry.action();
        action.register(&entry, attempt).unwrap();
        action.prepare_plan(&source);
        action.accept_source(source);
        action.batch.plan(planned.len());
        for plan in &planned {
            action.batch.item_result(
                &plan.id,
                plan.bytes,
                OutboundProgressStatus::Completed,
                None,
            );
        }
        let downloaded = current(registry.snapshot_page(&peer, query()).await.unwrap());
        assert!(downloaded.items.iter().all(|item| matches!(item, ProgressItemState::Active(value) if value.phase == ProgressPhase::Processing)));
        action.persisted(true, &[]);
        assert!(registry.state.lock().unwrap().receipts.is_empty());
        let committing = current(registry.snapshot_page(&peer, query()).await.unwrap());
        assert!(committing
            .items
            .iter()
            .all(|item| matches!(item, ProgressItemState::Active(_))));
        action.batch.settled(AttemptState::Completed);
        action.finish(Ok(()));
        drop(action);
        let completed = current(registry.snapshot_page(&peer, query()).await.unwrap());
        assert!(completed.revision > downloaded.revision);
        assert_eq!(completed.items.len(), if directory { 1 } else { 3 });
        assert!(completed.items.iter().all(|item| matches!(item, ProgressItemState::Terminal(value) if value.outcome == ProgressTerminalOutcome::Completed)));
        assert!(registry.state.lock().unwrap().receipts.is_empty());
    }
}
