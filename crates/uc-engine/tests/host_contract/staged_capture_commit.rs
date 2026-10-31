//! 真实 Engine/SQLite/暂存文件链路：入站大图提交后必须自行生成缩略图。

use std::time::{Duration, Instant};

use uc_engine::{
    AppendMobileFileUploadInput, ApplyMobileSyncDocumentInput, BeginMobileFileUploadInput,
    CreateSpaceInput, Engine, EngineConfig, FinishMobileFileUploadInput, HistoryEntryInput,
    MobileSyncDocument, MobileSyncDocumentApplyOutcome, MobileSyncItemType, Operation,
    OperationResult, SecretString,
};

use super::{startup::host, MemorySecureStorage};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn staged_mobile_image_materializes_after_its_atomic_capture_commit() {
    tracing_subscriber::fmt()
        .with_env_filter("error,uc_mobile_lan=debug,uc_application=debug,uc_infra_storage=debug,uc_infra_content=debug")
        .with_test_writer()
        .try_init()
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let (engine, _) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(MemorySecureStorage::default())),
    )
    .await
    .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("staged image acceptance".into()),
            passphrase: SecretString::new("staged-image-acceptance-passphrase"),
            passphrase_confirmation: SecretString::new("staged-image-acceptance-passphrase"),
        }))
        .await
        .unwrap();
    let image =
        include_bytes!("../../../../tests/hosts/connectivity/tests/fixtures/active-clipboard.png");
    assert!(image.len() > 16 * 1024);
    let source = "did_staged_image_acceptance".to_string();
    let OperationResult::MobileFileUploadStarted(handle) = engine
        .execute(Operation::BeginMobileFileUpload(
            BeginMobileFileUploadInput {
                data_name: "acceptance.png".into(),
                media_type: "image/png".into(),
                source_device_id: source.clone(),
                transfer_id: "mobile-lan:staged-acceptance".into(),
                total_bytes: Some(image.len() as u64),
            },
        ))
        .await
        .unwrap()
    else {
        panic!("mobile upload did not start")
    };
    engine
        .execute(Operation::AppendMobileFileUpload(
            AppendMobileFileUploadInput {
                handle: handle.clone(),
                bytes: image.to_vec(),
            },
        ))
        .await
        .unwrap();
    engine
        .execute(Operation::FinishMobileFileUpload(
            FinishMobileFileUploadInput {
                handle,
                media_type: "image/png".into(),
            },
        ))
        .await
        .unwrap();
    let OperationResult::MobileSyncDocumentApplied(MobileSyncDocumentApplyOutcome::Applied {
        entry_id,
        ..
    }) = engine
        .execute(Operation::ApplyMobileSyncDocument(Box::new(
            ApplyMobileSyncDocumentInput {
                source_device_id: source,
                document: MobileSyncDocument {
                    item_type: MobileSyncItemType::Image,
                    text: "acceptance.png".into(),
                    data_name: Some("acceptance.png".into()),
                    has_data: true,
                    size: image.len() as u64,
                    hash: None,
                    content_id: None,
                },
            },
        )))
        .await
        .unwrap()
    else {
        panic!("mobile image was not captured")
    };
    let entry = HistoryEntryInput { entry_id };
    let started = Instant::now();
    let mut busy = 0;
    let outcome = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match engine
                .execute(Operation::ReadEntryThumbnail(entry.clone()))
                .await
            {
                Ok(OperationResult::ThumbnailRead(thumbnail)) => break thumbnail,
                Err(error) if error.code() == 1465 => {
                    busy += 1;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                other => panic!("unexpected thumbnail outcome: {other:?}"),
            }
        }
    })
    .await;
    if outcome.is_err() {
        engine.shutdown(Duration::from_secs(5)).await.unwrap();
        panic!("staged image did not materialize after {busy} busy reads");
    }
    let thumbnail = outcome.unwrap();
    assert!(!thumbnail.bytes.is_empty());
    assert_eq!(thumbnail.media_type.as_deref(), Some("image/webp"));
    assert!(matches!(
        engine
            .execute(Operation::GetHistoryEntryResource(entry))
            .await
            .unwrap(),
        OperationResult::HistoryEntryResource(_)
    ));
    engine.shutdown(Duration::from_secs(5)).await.unwrap();
    if let Ok(path) = std::env::var("UC_STAGED_IMAGE_E2E_RECEIPT") {
        std::fs::write(path, serde_json::to_vec_pretty(&serde_json::json!({
            "passed": true, "original_bytes": image.len(), "busy_reads": busy,
            "elapsed_ms": started.elapsed().as_millis(),
            "boundary": "public Engine operations; real SQLite/staging/blob/thumbnail; no original read before thumbnail readiness"
        })).unwrap()).unwrap();
    }
}
