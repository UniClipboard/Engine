//! 真实 SQLite、加密历史、Iroh 文件导入与完整准备负责人共同验证取消边界。
use super::*;
use crate::testing::active_pull_publication::{PublicationFixture, ReleaseOnDrop};
use crate::{
    ClipboardRestoreMode, CreateSpaceInput, JoinSpaceInput, Operation, OperationResult,
    RestoreClipboardInput, SecretString, SettingsPatch, SyncSettingsPatch,
};
use tokio::time::{timeout, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_pull_cancel_settles_real_file_a_without_starting_file_b() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let server = MockServer::start().await;
    mount_engine_rendezvous(&server).await;
    let holder_root = tempfile::tempdir().unwrap();
    let requester_root = tempfile::tempdir().unwrap();
    let fixture = PublicationFixture::install(holder_root.path());
    let _release_on_failure = ReleaseOnDrop(fixture.clone());
    let first = holder_root.path().join("file-a.bin");
    let second = holder_root.path().join("file-b.bin");
    std::fs::write(&first, b"real publication A").unwrap();
    std::fs::write(&second, b"must not publish B").unwrap();
    let uri = [first, second]
        .iter()
        .map(|path| url::Url::from_file_path(path).unwrap().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let holder_host = HostCapabilities::new(
        HostDirectories::new(
            holder_root.path().join("private"),
            holder_root.path().join("cache"),
            holder_root.path().join("temporary"),
            holder_root.path().join("logs"),
        ),
        Box::new(MemoryHostSecureStorage::default()),
        Box::new(StaticHostClipboard {
            snapshot: HostClipboardSnapshot {
                observed_at_ms: 1_700_000_000_000,
                representations: vec![HostClipboardRepresentation::Inline {
                    format: "files".into(),
                    mime_type: Some("text/uri-list".into()),
                    bytes: uri.into_bytes(),
                }],
            },
        }),
        Box::new(EmptyHostFiles),
    );
    let config = EngineConfig::new("1.2.3")
        .with_rendezvous_base_url(server.uri())
        .with_test_relay_fallback(false);
    let (holder, _holder_events) = Engine::start(config.clone(), holder_host).await.unwrap();
    let (requester, _requester_events) =
        Engine::start(config, empty_engine_host(requester_root.path()))
            .await
            .unwrap();
    holder
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("Holder".into()),
            passphrase: SecretString::new("real pull fixture"),
            passphrase_confirmation: SecretString::new("real pull fixture"),
        }))
        .await
        .unwrap();
    let OperationResult::InvitationIssued {
        invitation_code, ..
    } = holder.execute(Operation::IssueInvitation).await.unwrap()
    else {
        panic!("invitation expected")
    };
    requester
        .execute(Operation::JoinSpace(JoinSpaceInput {
            invitation_code,
            device_name: Some("Requester".into()),
            passphrase: SecretString::new("real pull fixture"),
            preserve_unreadable_history: false,
        }))
        .await
        .unwrap();
    timeout(std::time::Duration::from_secs(30), async {
        loop {
            match requester.execute(Operation::QueryDeviceGroupChoices).await {
                Ok(OperationResult::DeviceGroupChoices(choices))
                    if choices.device_trust.local_membership
                        == crate::DeviceMembershipSummary::Active =>
                {
                    break
                }
                Ok(OperationResult::DeviceGroupChoices(_)) => {}
                Err(error) if error.code() == 1103 => {} // 会话重建尚未完成，等待原稳定查询可用。
                other => panic!("join readiness failed: {other:?}"),
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    for engine in [&holder, &requester] {
        engine
            .execute(Operation::UpdateSettings(Box::new(SettingsPatch {
                sync: Some(SyncSettingsPatch {
                    sync_on_restore: Some(true),
                    auto_sync_enabled: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            })))
            .await
            .unwrap();
    }
    let OperationResult::ClipboardCaptured {
        entry_id: Some(entry_id),
    } = holder
        .execute(Operation::CaptureCurrentClipboard)
        .await
        .unwrap()
    else {
        panic!("two-file capture expected")
    };
    fixture.armed.store(true, Ordering::SeqCst);
    holder
        .execute(Operation::RestoreClipboard(RestoreClipboardInput {
            entry_id,
            mode: ClipboardRestoreMode::Standard,
        }))
        .await
        .unwrap();
    timeout(
        std::time::Duration::from_secs(8),
        fixture.started.notified(),
    )
    .await
    .unwrap();
    assert_eq!(fixture.real_imports.load(Ordering::SeqCst), 1);
    assert!(!fixture.finished.load(Ordering::SeqCst));
    // 请求方退休会关闭实际拉取连接；发布 A 仍由原本地工作负责人占用许可结算。
    requester.suspend().await.unwrap();
    timeout(std::time::Duration::from_secs(2), async {
        loop {
            if fixture
                .stop
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|stop| stop.check().is_err())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        !fixture.finished.load(Ordering::SeqCst),
        "网络取消不得冒充实际发布已完成"
    );
    // 持有方世代排空同样必须等待 A 实际结算，不能只等待网络 handler 退出。
    let mut holder_suspension = Box::pin(holder.suspend());
    assert!(
        timeout(
            std::time::Duration::from_millis(100),
            &mut holder_suspension
        )
        .await
        .is_err(),
        "holder generation cannot drain before actual publication finishes"
    );
    let released_at = Instant::now();
    fixture.release.add_permits(1);
    timeout(
        std::time::Duration::from_secs(5),
        fixture.completed.notified(),
    )
    .await
    .unwrap();
    timeout(std::time::Duration::from_secs(5), holder_suspension)
        .await
        .unwrap()
        .unwrap();
    assert!(fixture.cancelled.load(Ordering::SeqCst));
    assert_eq!(
        fixture.publications.load(Ordering::SeqCst),
        1,
        "B、后续编码和加密不能启动"
    );
    assert_eq!(fixture.real_imports.load(Ordering::SeqCst), 1);
    let evidence = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/active-clipboard-connectivity-evidence");
    std::fs::create_dir_all(&evidence).unwrap();
    std::fs::write(
        evidence.join("real-two-file-cancellation.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "result": "passed", "actual_imports": 1, "publication_attempts": 1,
            "network_ended_before_local_settlement": true, "preparation_cancelled": true,
            "holder_generation_waited_for_actual_settlement": true,
            "settlement_after_release_ms": released_at.elapsed().as_millis(),
        }))
        .unwrap(),
    )
    .unwrap();
    requester.shutdown_until_complete().await.unwrap();
    holder.shutdown_until_complete().await.unwrap();
}
