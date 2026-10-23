//! 原消息作者 B 与实际提供端 C 不同，仍共享 C 的原下载额度。
use super::{dispatch, join, membership, pending, settled, start, PASSPHRASE};
use crate::engine::EngineRuntime;
use crate::testing::mount_engine_rendezvous;
use crate::testing::receive_exports::{ReceiveExports, ReleaseOnDrop};
use crate::{CreateSpaceInput, Operation, SecretString, SettingsPatch, SyncSettingsPatch};
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uc_application::facade::{FetchBlobToPathCommand, PublishBlobPathCommand};
use wiremock::MockServer;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ticket_provider_budget_is_shared_with_a_different_message_author() {
    let server = MockServer::start().await;
    mount_engine_rendezvous(&server).await;
    let roots = [tempdir().unwrap(), tempdir().unwrap(), tempdir().unwrap()];
    let exports = ReceiveExports::install(roots[0].path(), usize::MAX, 0);
    exports.paused_exports.lock().unwrap().extend([1, 2]);
    let _release = [
        ReleaseOnDrop(exports.clone()),
        ReleaseOnDrop(exports.clone()),
        ReleaseOnDrop(exports.clone()),
    ];
    let (receiver, mut receiver_events) = start(roots[0].path(), &server).await;
    let (sender, _sender_events) = start(roots[1].path(), &server).await;
    let (provider, _provider_events) = start(roots[2].path(), &server).await;
    receiver
        .execute(
            Operation::CreateSpace(CreateSpaceInput {
                device_name: Some("blob-quota-receiver".into()),
                passphrase: SecretString::new(PASSPHRASE),
                passphrase_confirmation: SecretString::new(PASSPHRASE),
            }),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    join(&receiver, &sender).await;
    membership(&receiver, 2, "receiver").await;
    membership(&sender, 2, "sender").await;
    join(&receiver, &provider).await;
    let receiver_id = membership(&receiver, 3, "receiver").await;
    let sender_id = membership(&sender, 3, "sender").await;
    let provider_id = membership(&provider, 3, "provider").await;
    assert!(
        sender_id != provider_id,
        "message author must differ from ticket provider"
    );
    for runtime in [&receiver, &sender, &provider] {
        runtime
            .execute(
                Operation::UpdateSettings(Box::new(SettingsPatch {
                    sync: Some(SyncSettingsPatch {
                        auto_sync_enabled: Some(false),
                        sync_on_restore: Some(false),
                        ..Default::default()
                    }),
                    ..Default::default()
                })),
                CancellationToken::new(),
            )
            .await
            .unwrap();
    }
    let receiver_facade = receiver.current_facade().await.unwrap();
    let sender_facade = sender.current_facade().await.unwrap();
    let provider_facade = provider.current_facade().await.unwrap();
    let contents = [
        b"C file from B message".as_slice(),
        b"C second file",
        b"C queued file",
        b"B independent file",
        b"B fourth active slot",
    ];
    let mut published = Vec::new();
    for (index, bytes) in contents.iter().enumerate() {
        let (root, facade) = if index >= 3 {
            (roots[1].path(), &sender_facade)
        } else {
            (roots[2].path(), &provider_facade)
        };
        let path = root.join(format!("source-{index}.bin"));
        std::fs::write(&path, bytes).unwrap();
        published.push(
            facade
                .publish_blob_path(PublishBlobPathCommand {
                    path,
                    entry_id: None,
                })
                .await
                .unwrap(),
        );
    }
    let send = tokio::spawn({
        let facade = Arc::clone(&sender_facade);
        let first = published[0].clone();
        let receiver_id = receiver_id.clone();
        async move { dispatch(&facade, &receiver_id, &first, contents[0].len()).await }
    });
    let first_entry = pending(&mut receiver_events, &sender_id).await;
    timeout(Duration::from_secs(20), exports.paused.notified())
        .await
        .unwrap();
    let second_path = roots[0].path().join("second-output.bin");
    let second = tokio::spawn({
        let facade = Arc::clone(&receiver_facade);
        let blob = published[1].clone();
        let path = second_path.clone();
        async move {
            facade
                .fetch_blob_to_path(FetchBlobToPathCommand {
                    cancellation: CancellationToken::new(),
                    ticket: blob.ticket,
                    entry_id: blob.entry_id,
                    target_path: path.into(),
                    transfer_context: None,
                })
                .await
        }
    });
    timeout(Duration::from_secs(20), exports.paused.notified())
        .await
        .unwrap();
    assert!(!second.is_finished());
    let third_path = roots[0].path().join("third-output.bin");
    let mut third = tokio::spawn({
        let facade = Arc::clone(&receiver_facade);
        let blob = published[2].clone();
        let path = third_path.clone();
        async move {
            facade
                .fetch_blob_to_path(FetchBlobToPathCommand {
                    cancellation: CancellationToken::new(),
                    ticket: blob.ticket,
                    entry_id: blob.entry_id,
                    target_path: path.into(),
                    transfer_context: None,
                })
                .await
        }
    });
    // 第三项已被驱动但原两项未归还许可；不读取内部调度计数。
    assert!(timeout(Duration::from_millis(250), &mut third)
        .await
        .is_err());
    let independent_path = roots[0].path().join("independent-output.bin");
    let independent = tokio::spawn({
        let facade = Arc::clone(&receiver_facade);
        let blob = published[3].clone();
        let path = independent_path.clone();
        async move {
            facade
                .fetch_blob_to_path(FetchBlobToPathCommand {
                    cancellation: CancellationToken::new(),
                    ticket: blob.ticket,
                    entry_id: blob.entry_id,
                    target_path: path.into(),
                    transfer_context: None,
                })
                .await
        }
    });
    timeout(Duration::from_secs(20), exports.paused.notified())
        .await
        .unwrap();
    assert!(!independent.is_finished());
    assert!(
        std::fs::read(&independent_path).unwrap() == contents[3],
        "another provider must export exact bytes while C waits"
    );
    // 三份真实导出仍持原许可；第四个活跃槽可供 B 使用，排队的 C 不得提前占全局许可。
    let fourth_path = roots[0].path().join("fourth-active-output.bin");
    let fourth = &published[4];
    timeout(
        Duration::from_secs(20),
        receiver_facade.fetch_blob_to_path(FetchBlobToPathCommand {
            cancellation: CancellationToken::new(),
            ticket: fourth.ticket.clone(),
            entry_id: fourth.entry_id.clone(),
            target_path: fourth_path.clone().into(),
            transfer_context: None,
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        std::fs::read(fourth_path).unwrap() == contents[4],
        "queued C work must leave the fourth global slot available"
    );
    assert!(timeout(Duration::from_millis(250), &mut third)
        .await
        .is_err());
    assert!(
        !third_path.exists(),
        "queued provider work must not export an output"
    );
    assert!(!second.is_finished());
    assert!(
        exports.exports.lock().unwrap().as_slice()
            == [
                published[0].digest,
                published[1].digest,
                published[3].digest,
                published[4].digest
            ],
        "only two C exports and two B exports may start"
    );
    assert_eq!(
        settled(&receiver, &first_entry, "receiving")
            .await
            .items_completed,
        0
    );
    exports.release.add_permits(3);
    timeout(Duration::from_secs(20), second)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(20), third)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(20), independent)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(20), send)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        settled(&receiver, &first_entry, "completed")
            .await
            .items_completed,
        1
    );
    assert!(
        std::fs::read(second_path).unwrap() == contents[1],
        "second C file must finish unchanged"
    );
    assert!(
        std::fs::read(third_path).unwrap() == contents[2],
        "queued C file must finish after original settlement"
    );
    assert!(
        exports.exports.lock().unwrap().as_slice()
            == [
                published[0].digest,
                published[1].digest,
                published[3].digest,
                published[4].digest,
                published[2].digest
            ],
        "queued C export must start only after original permits are returned"
    );
    drop((receiver_facade, sender_facade, provider_facade));
    for runtime in [&provider, &sender, &receiver] {
        runtime.shutdown(None).await.unwrap();
    }
}
