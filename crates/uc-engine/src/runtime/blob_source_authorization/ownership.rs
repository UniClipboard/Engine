//! 三个真实生产Runtime的消息源、提供端与接收结果分别归属；不注入进度或伪造身份。
use super::{dispatch, join, membership, pending, settled, start, PASSPHRASE};
use crate::engine::EngineRuntime;
use crate::runtime::ProductionRuntime;
use crate::testing::{
    mount_engine_rendezvous,
    receive_exports::{ReceiveExports, ReleaseOnDrop},
};
use crate::{
    BlobResourceInput, CreateSpaceInput, HistoryEntryInput, Operation, OperationResult,
    OutboundProgressBatchState, OutboundProgressInput, OutboundProgressPhase,
    OutboundProgressState, OutboundProgressSummary, ReceiveItemFailureReason, SecretString,
    SettingsPatch, SyncSettingsPatch,
};
use std::path::Path;
use std::time::Duration;
use tempfile::tempdir;
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;
use uc_application::facade::PublishBlobPathCommand;
use url::Url;
use wiremock::MockServer;

pub(super) async fn outbound(
    runtime: &ProductionRuntime,
    source: &str,
    peer: &str,
    expected: impl Fn(&OutboundProgressSummary) -> bool,
) -> OutboundProgressSummary {
    timeout(Duration::from_secs(20), async {
        loop {
            let OperationResult::OutboundProgress(view) = runtime
                .execute(
                    Operation::QueryOutboundProgress(OutboundProgressInput {
                        entry_id: source.into(),
                        target_device_id: peer.into(),
                    }),
                    CancellationToken::new(),
                )
                .await
                .unwrap()
            else {
                panic!("complete source progress")
            };
            if let Some(view) = view {
                if expected(&view) {
                    return view;
                }
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("original authenticated sender must observe its own receiver")
}

pub(super) async fn assert_saved(
    runtime: &ProductionRuntime,
    entry: &str,
    root: &Path,
    content: &[u8],
) {
    let OperationResult::HistoryEntryResource(resource) = runtime
        .execute(
            Operation::GetHistoryEntryResource(HistoryEntryInput {
                entry_id: entry.into(),
            }),
            CancellationToken::new(),
        )
        .await
        .unwrap()
    else {
        panic!("saved file URI resource")
    };
    let uri_bytes = match (resource.inline_data, resource.blob_id) {
        (Some(bytes), _) => bytes,
        (None, Some(blob_id)) => {
            let OperationResult::BlobRead(blob) = runtime
                .execute(
                    Operation::ReadBlob(BlobResourceInput { blob_id }),
                    CancellationToken::new(),
                )
                .await
                .unwrap()
            else {
                panic!("saved file URI blob")
            };
            blob.bytes
        }
        _ => panic!("saved URI missing"),
    };
    let uri = std::str::from_utf8(&uri_bytes).unwrap().trim();
    let saved = Url::parse(uri)
        .unwrap()
        .to_file_path()
        .unwrap()
        .canonicalize()
        .unwrap();
    assert!(saved.starts_with(root.canonicalize().unwrap()));
    assert_eq!(std::fs::read(saved).unwrap(), content);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn third_party_provider_results_and_two_cold_receivers_keep_separate_ownership() {
    for provider_is_source in [true, false] {
        let server = MockServer::start().await;
        mount_engine_rendezvous(&server).await;
        let roots = [tempdir().unwrap(), tempdir().unwrap(), tempdir().unwrap()];
        let held = ReceiveExports::install(roots[0].path(), usize::MAX, 0);
        let provider_exports = ReceiveExports::install(roots[2].path(), usize::MAX, usize::MAX);
        let _release = [
            ReleaseOnDrop(held.clone()),
            ReleaseOnDrop(provider_exports.clone()),
        ];
        let (receiver, mut receiver_events) = start(roots[0].path(), &server).await;
        let (sender, _) = start(roots[1].path(), &server).await;
        let (provider, mut provider_events) = start(roots[2].path(), &server).await;
        receiver
            .execute(
                Operation::CreateSpace(CreateSpaceInput {
                    device_name: Some("ownership receiver".into()),
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
        assert_ne!(sender_id, provider_id);
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
        timeout(Duration::from_secs(30), async {
        loop {
            if matches!(sender.execute(Operation::QueryPeerConnections, CancellationToken::new()).await.unwrap(),
                OperationResult::PeerConnections(peers) if [&receiver_id, &provider_id].iter().all(|id|
                    peers.iter().any(|peer| &peer.peer_id == *id && peer.connected))) {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("both real recipients must be reachable before dispatch");
        let sender_facade = sender.current_facade().await.unwrap();
        let provider_facade = provider.current_facade().await.unwrap();
        let content = b"actual C bytes carried in a B clipboard message";
        let path = roots[2].path().join("provider-body.bin");
        std::fs::write(&path, content).unwrap();
        let publisher = if provider_is_source {
            &provider_facade
        } else {
            &sender_facade
        };
        let published = publisher
            .publish_blob_path(PublishBlobPathCommand {
                path,
                entry_id: None,
            })
            .await
            .unwrap();
        dispatch(&sender_facade, &receiver_id, &published, content.len()).await;
        let receiver_entry = pending(&mut receiver_events, &sender_id).await;
        let provider_entry = pending(&mut provider_events, &sender_id).await;
        timeout(Duration::from_secs(20), held.paused.notified())
            .await
            .unwrap();
        // Core 入站准入只接受对端；自指票据不能因缓存命中越过准入。
        let provider_done = settled(
            &provider,
            &provider_entry,
            if provider_is_source {
                "failed"
            } else {
                "completed"
            },
        )
        .await;
        if provider_is_source {
            assert_eq!(
                provider_done.items[0].failure_reason,
                Some(ReceiveItemFailureReason::AccessDenied)
            );
            assert!(provider_exports.exports.lock().unwrap().is_empty());
        }
        let source = published.entry_id.as_ref();
        let other = outbound(&sender, source, &provider_id, |view| {
            view.state
                == if provider_is_source {
                    OutboundProgressBatchState::Failed
                } else {
                    OutboundProgressBatchState::Completed
                }
        })
        .await;
        assert_eq!(
            other.attempt_id.as_deref(),
            Some(provider_done.attempt_id.as_str())
        );
        let waiting = outbound(&sender, source, &receiver_id, |view| {
            view.items.first().is_some_and(|item| {
                item.last_known_phase == Some(OutboundProgressPhase::Processing)
            })
        })
        .await;
        assert_eq!(
            (
                waiting.items_total,
                waiting.items_completed,
                waiting.items_failed,
                waiting.items_cancelled
            ),
            (1, 0, 0, 0)
        );
        assert_ne!(waiting.attempt_id, other.attempt_id);
        assert_eq!(
            held.cache_misses.lock().unwrap().as_slice(),
            [published.digest]
        );
        if provider_is_source {
            assert!(
                provider_exports.cache_misses.lock().unwrap().is_empty(),
                "self ticket is refused before reading cache"
            );
        } else {
            assert_eq!(
                provider_exports.cache_misses.lock().unwrap().as_slice(),
                [published.digest],
                "second recipient must really download cold content"
            );
        }
        assert_eq!(held.exports.lock().unwrap().as_slice(), [published.digest]);
        held.release.add_permits(1);
        let receiver_done = settled(&receiver, &receiver_entry, "completed").await;
        let done = outbound(&sender, source, &receiver_id, |view| {
            view.items_completed == 1
        })
        .await;
        assert_eq!(
            done.attempt_id.as_deref(),
            Some(receiver_done.attempt_id.as_str())
        );
        assert_eq!(done.items[0].state, OutboundProgressState::Completed);
        assert_eq!(done.attempt_id, waiting.attempt_id);
        let preserved = outbound(&sender, source, &provider_id, |view| {
            view.state == other.state
        })
        .await;
        assert_eq!(preserved.attempt_id, other.attempt_id);
        assert_eq!(preserved.items, other.items);
        assert_ne!(done.attempt_id, other.attempt_id);
        assert_saved(&receiver, &receiver_entry, roots[0].path(), content).await;
        if !provider_is_source {
            assert_saved(&provider, &provider_entry, roots[2].path(), content).await;
        }
        assert!(
            matches!(
                provider
                    .execute(
                        Operation::QueryOutboundProgress(OutboundProgressInput {
                            entry_id: source.into(),
                            target_device_id: receiver_id
                        }),
                        CancellationToken::new()
                    )
                    .await
                    .unwrap(),
                OperationResult::OutboundProgress(None)
            ),
            "a provider must not acquire the sender's outbound ownership"
        );
        drop((sender_facade, provider_facade));
        for runtime in [&provider, &sender, &receiver] {
            runtime.shutdown(None).await.unwrap();
        }
    }
}
