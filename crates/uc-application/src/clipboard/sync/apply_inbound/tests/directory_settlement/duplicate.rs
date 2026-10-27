//! 本机目标名称可以被重新使用，不能在目录发布后用URI代替内容身份去重。
use super::*;
use std::collections::BTreeMap;

#[tokio::test]
async fn directory_new_content_reusing_deleted_local_paths_finishes_its_claimed_commit() {
    assert_directory_replacement(false).await;
}

#[tokio::test]
async fn directory_different_content_is_not_skipped_when_sender_uri_matches_previous_local_output()
{
    assert_directory_replacement(true).await;
}

async fn assert_directory_replacement(sender_reuses_local_uri: bool) {
    let cache = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let fetches = Arc::new(AtomicUsize::new(0));
    let fetched = fetches.clone();
    let mut fetcher = MockBlobFetcher::new();
    fetcher
        .expect_fetch_blob_to_path()
        .times(2..=4)
        .returning(move |command| {
            let replacement = fetched.fetch_add(1, Ordering::SeqCst) >= 2;
            std::fs::write(
                &command.target_path,
                if replacement { b"world" } else { b"hello" },
            )
            .unwrap();
            Ok(crate::facade::blob_transfer::FetchBlobToPathResult {
                entry_id: command.entry_id,
                plaintext_hash: PlaintextHash::from_bytes(if replacement {
                    [8; 32]
                } else {
                    [5; 32]
                }),
                digest: BlobDigest::from_bytes([7; 32]),
                bytes_written: 5,
            })
        });
    let attempts = AttemptGate::empty();
    let materializer = FileCacheBlobMaterializer::new(
        Arc::new(fetcher),
        cache.path().to_owned(),
        Arc::new(FakeAtomicPublisher::default()),
    )
    .with_save_dir_resolver(Arc::new(FakeSaveDir {
        dir: destination.path().to_owned(),
    }))
    .with_directory_receive_attempt_ports(
        attempts.clone(),
        attempts.clone(),
        attempts.clone(),
        attempts.clone(),
    );
    let mut repo = MockEntryRepo::new();
    repo.expect_find_entry_id_by_snapshot_hash()
        .times(2)
        .returning(|_| Ok(None));
    let captures = Arc::new(AtomicUsize::new(0));
    let captured = captures.clone();
    let captured_snapshot = Arc::new(Mutex::new(None));
    let recorded_snapshot = captured_snapshot.clone();
    let captured_attempt = attempts.clone();
    let mut capture = MockCapture::new();
    capture
        .expect_capture()
        .times(1..=2)
        .returning(move |entry, _, snapshot| {
            assert_eq!(
                *captured_attempt.state.lock().unwrap(),
                AttemptState::Committing
            );
            *captured_attempt.state.lock().unwrap() = AttemptState::Completed;
            if captured.fetch_add(1, Ordering::SeqCst) == 0 {
                *recorded_snapshot.lock().unwrap() = Some(snapshot);
            }
            Ok(Some(entry))
        });
    let mut write = MockWrite::new();
    write.expect_write().times(1..=2).returning(|_, _| Ok(()));
    let receiver = build(repo, capture, write)
        .with_blob_materializer(Arc::new(materializer))
        .with_receive_attempt_ports(
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
        );
    let (plaintext, snapshot_hash) = two_root_wire_payload();
    let old_hash = snapshot_hash.clone();
    let first = receiver
        .execute(ApplyInboundInput {
            source_entry: None,
            from_device: DeviceId::new("source"),
            snapshot_hash,
            plaintext,
            resurface_intent: ClipboardWriteIntent::RemotePush,
            activation_owner: InboundActivationOwner::Inbound,
        })
        .await
        .unwrap();
    std::fs::remove_dir_all(destination.path().join("alpha")).unwrap();
    std::fs::remove_dir_all(destination.path().join("beta")).unwrap();
    // 端口模型开始另一条目；上一条目的完成状态不能成为新条目的假活动记录。
    attempts.exists.store(false, Ordering::SeqCst);
    let (mut blob_refs, manifest, mut snapshot) = two_root_payload();
    if sender_reuses_local_uri {
        snapshot = captured_snapshot.lock().unwrap().clone().unwrap();
    }
    for blob in &mut blob_refs {
        blob.ticket = BlobTicket::from_bytes(vec![8, 8, 8]);
    }
    snapshot.file_set_v1_component = Some(
        compute_file_set_component(&manifest, &BTreeMap::from([(0, [8; 32]), (1, [8; 32])]))
            .unwrap(),
    );
    let (plaintext, snapshot_hash) =
        encode_snapshot_with_blob_refs_and_file_set_to_v3_bytes(&snapshot, &blob_refs, &manifest)
            .unwrap();
    assert_ne!(
        snapshot_hash, old_hash,
        "内容身份确实不同，本机URI相同不代表重复"
    );
    let second = receiver
        .execute(ApplyInboundInput {
            source_entry: None,
            from_device: DeviceId::new("source"),
            snapshot_hash,
            plaintext,
            resurface_intent: ClipboardWriteIntent::RemotePush,
            activation_owner: InboundActivationOwner::Inbound,
        })
        .await;
    timeout(Duration::from_secs(10), receiver.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(first, ApplyOutcome::Applied { .. }));
    assert!(
        matches!(second, Ok(ApplyOutcome::Applied { .. })),
        "已发布且内容不同的目录必须完成原提交：{second:?}"
    );
    assert_eq!(captures.load(Ordering::SeqCst), 2);
    assert_eq!(fetches.load(Ordering::SeqCst), 4);
    assert_eq!(*attempts.state.lock().unwrap(), AttemptState::Completed);
    assert_eq!(
        std::fs::read(destination.path().join("alpha/one.txt")).unwrap(),
        b"world"
    );
    assert_eq!(
        std::fs::read(destination.path().join("beta/two.txt")).unwrap(),
        b"world"
    );
    assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 2);
}
