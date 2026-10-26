//! 完整 facade 动作经生产装配发送真实 B 消息，引用实际 C 提供的内容。
mod ownership;
mod quota;
use super::ProductionRuntime;
use crate::assembly::host::{derive_app_paths, profile_key_recovery_store};
use crate::engine::event_stream::event_channel;
use crate::engine::{EngineRuntime, StartupProgress};
use crate::testing::{empty_engine_host, mount_engine_rendezvous};
use crate::{
    BlobResourceInput, CreateSpaceInput, DeviceMembershipSummary, EngineConfig, EngineEvent,
    EntryReceiveProgressInput, EventStream, HistoryEntryInput, JoinSpaceInput, Operation,
    OperationResult, ReceiveItemFailureReason, ReceiveProgressSummary, RemoveMemberInput,
    SecretString, SettingsPatch, SyncSettingsPatch,
};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::tempdir;
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{BlobExchangeError, ConnectivityError};
use uc_application::facade::{
    AppFacade, BlobTransferError, FetchBlobToPathCommand, PublishBlobPathCommand,
    PublishBlobResult, V3BlobRef,
};
use uc_core::clipboard::{
    ClipboardChangeOrigin, ObservedClipboardRepresentation, SystemClipboardSnapshot,
};
use uc_core::ids::{FormatId, RepresentationId};
use uc_core::MimeType;
use url::Url;
use wiremock::MockServer;

const PASSPHRASE: &str = "third-party blob source fixture";

async fn start(root: &Path, server: &MockServer) -> (ProductionRuntime, EventStream) {
    let host = empty_engine_host(root);
    let config = EngineConfig::new("1.2.3")
        .with_rendezvous_base_url(server.uri())
        .with_test_relay_fallback(false);
    let paths = derive_app_paths(host.directories());
    let recovery = profile_key_recovery_store(&config, &paths, &host);
    let (events, stream) = event_channel(256);
    let (progress, _) = StartupProgress::channel();
    let runtime = ProductionRuntime::start(
        config,
        host,
        paths,
        events,
        Arc::clone(&progress.store),
        recovery,
    )
    .await
    .unwrap();
    (runtime, stream)
}

async fn join(sponsor: &ProductionRuntime, joiner: &ProductionRuntime) {
    let OperationResult::InvitationIssued {
        invitation_code, ..
    } = sponsor
        .execute(Operation::IssueInvitation, CancellationToken::new())
        .await
        .unwrap()
    else {
        panic!("invitation expected")
    };
    joiner
        .execute(
            Operation::JoinSpace(JoinSpaceInput {
                invitation_code,
                device_name: Some("blob-source-fixture".into()),
                passphrase: SecretString::new(PASSPHRASE),
                preserve_unreadable_history: false,
            }),
            CancellationToken::new(),
        )
        .await
        .unwrap();
}

async fn membership(runtime: &ProductionRuntime, count: usize, role: &str) -> String {
    let mut last = (DeviceMembershipSummary::Unknown, 0_usize, 0_usize);
    timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(OperationResult::DeviceGroupChoices(choices)) = runtime
                .execute(Operation::QueryDeviceGroupChoices, CancellationToken::new())
                .await
            {
                last = (
                    choices.device_trust.local_membership,
                    choices
                        .device_trust
                        .devices
                        .iter()
                        .filter(|d| d.membership == DeviceMembershipSummary::Active)
                        .count(),
                    choices
                        .device_trust
                        .devices
                        .iter()
                        .filter(|d| d.membership == DeviceMembershipSummary::Removed)
                        .count(),
                );
                if choices.device_trust.local_membership == DeviceMembershipSummary::Active
                    && choices
                        .device_trust
                        .devices
                        .iter()
                        .filter(|device| device.membership == DeviceMembershipSummary::Active)
                        .count()
                        == count
                {
                    return choices.device_trust.local_device_id;
                }
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{role} membership must converge to {count}; last: {last:?}"))
}

async fn dispatch(sender: &AppFacade, receiver_id: &str, blob: &PublishBlobResult, size: usize) {
    let snapshot = SystemClipboardSnapshot {
        ts_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
            .try_into()
            .unwrap(),
        representations: vec![ObservedClipboardRepresentation::new(
            RepresentationId::new(),
            FormatId::from("text/uri-list"),
            Some(MimeType("text/uri-list".into())),
            b"file:///third-party-source.bin\r\n".to_vec(),
        )],
        file_content_digests: vec![*blob.plaintext_hash.as_bytes()],
        file_set_v1_component: None,
    };
    let outcome = sender
        .dispatch_clipboard_snapshot_with_blob_refs(
            snapshot,
            vec![V3BlobRef {
                ticket: blob.ticket.clone(),
                entry_id: blob.entry_id.clone(),
                filename: Some("third-party-source.bin".into()),
                mime: Some("application/octet-stream".into()),
                size_bytes: size as u64,
                representation_index: None,
            }],
            ClipboardChangeOrigin::LocalCapture,
        )
        .await
        .unwrap();
    assert!(
        outcome
            .per_target
            .iter()
            .any(|target| target.device_id.as_str() == receiver_id && target.outcome.is_ok())
            || outcome
                .pending_targets
                .iter()
                .any(|target| target.as_str() == receiver_id),
        "receiver result: accepted={}, duplicate={}, offline={}, errored={}, pending={}, \
         receiver_settled={:?}, receiver_pending={}",
        outcome.total_accepted,
        outcome.total_duplicate,
        outcome.total_offline,
        outcome.total_errored,
        outcome.total_pending,
        outcome
            .per_target
            .iter()
            .find(|target| target.device_id.as_str() == receiver_id)
            .map(|target| target.outcome.is_ok()),
        outcome
            .pending_targets
            .iter()
            .any(|target| target.as_str() == receiver_id)
    );
}

async fn pending(events: &mut EventStream, sender_id: &str) -> String {
    timeout(Duration::from_secs(20), async {
        while let Some(event) = events.next().await {
            if let EngineEvent::IncomingPending(pending) = event {
                assert!(
                    pending.from_device == sender_id,
                    "message author must be the real sender"
                );
                assert!(pending.attempt_id.is_some());
                return pending.entry_id;
            }
        }
        panic!("receive event stream closed")
    })
    .await
    .expect("the actual sender must create a pending receive")
}

async fn settled(
    runtime: &ProductionRuntime,
    entry_id: &str,
    state: &str,
) -> ReceiveProgressSummary {
    let mut last = None;
    timeout(Duration::from_secs(20), async {
        loop {
            let OperationResult::EntryReceiveProgress(row) = runtime
                .execute(
                    Operation::QueryEntryReceiveProgress(EntryReceiveProgressInput {
                        entry_id: entry_id.into(),
                    }),
                    CancellationToken::new(),
                )
                .await
                .unwrap()
            else {
                panic!("receive progress expected")
            };
            if let Some(row) = row {
                if row.state == state {
                    return row;
                }
                last = Some((
                    row.state,
                    row.items_completed,
                    row.items
                        .iter()
                        .map(|item| item.failure_reason)
                        .collect::<Vec<_>>(),
                ));
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("original receive must settle as {state}; last: {last:?}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sender_cannot_authorize_a_revoked_ticket_provider_even_when_cached() {
    let server = MockServer::start().await;
    mount_engine_rendezvous(&server).await;
    let roots = [tempdir().unwrap(), tempdir().unwrap(), tempdir().unwrap()];
    let (receiver, mut receiver_events) = start(roots[0].path(), &server).await;
    let (sender, _sender_events) = start(roots[1].path(), &server).await;
    let (provider, _provider_events) = start(roots[2].path(), &server).await;
    receiver
        .execute(
            Operation::CreateSpace(CreateSpaceInput {
                device_name: Some("blob-receiver-fixture".into()),
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
    assert!(sender_id != provider_id, "sender and provider must differ");
    assert!(
        receiver_id != provider_id,
        "receiver and provider must differ"
    );
    receiver
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
    // 仅取得既有完整能力的句柄；不读取或改写接收流程内部状态。
    let provider_facade = provider.current_facade().await.unwrap();
    let sender_facade = sender.current_facade().await.unwrap();
    let contents = [
        b"authorized third-party file".as_slice(),
        b"revoked third-party file".as_slice(),
    ];
    let source_paths = [
        roots[2].path().join("first.bin"),
        roots[2].path().join("second.bin"),
    ];
    for (path, bytes) in source_paths.iter().zip(contents) {
        std::fs::write(path, bytes).unwrap();
    }
    let first = provider_facade
        .publish_blob_path(PublishBlobPathCommand {
            path: source_paths[0].clone(),
            entry_id: None,
        })
        .await
        .unwrap();
    let second = provider_facade
        .publish_blob_path(PublishBlobPathCommand {
            path: source_paths[1].clone(),
            entry_id: None,
        })
        .await
        .unwrap();
    dispatch(&sender_facade, &receiver_id, &first, contents[0].len()).await;
    let entry_id = pending(&mut receiver_events, &sender_id).await;
    let received = settled(&receiver, &entry_id, "completed").await;
    assert_eq!(received.items_completed, 1);
    let OperationResult::HistoryEntryResource(resource) = receiver
        .execute(
            Operation::GetHistoryEntryResource(HistoryEntryInput {
                entry_id: received.entry_id.clone(),
            }),
            CancellationToken::new(),
        )
        .await
        .unwrap()
    else {
        panic!("saved file resource expected")
    };
    let uri_bytes = match (resource.inline_data, resource.blob_id) {
        (Some(bytes), _) => bytes,
        (None, Some(blob_id)) => {
            let OperationResult::BlobRead(blob) = receiver
                .execute(
                    Operation::ReadBlob(BlobResourceInput { blob_id }),
                    CancellationToken::new(),
                )
                .await
                .unwrap()
            else {
                panic!("saved file URI expected")
            };
            blob.bytes
        }
        _ => panic!("saved file URI missing"),
    };
    let uri = std::str::from_utf8(&uri_bytes).unwrap().trim();
    let file = Url::parse(uri)
        .unwrap()
        .to_file_path()
        .unwrap()
        .canonicalize()
        .unwrap();
    assert!(file.starts_with(roots[0].path().canonicalize().unwrap()));
    assert!(
        std::fs::read(&file).unwrap() == contents[0],
        "received file bytes must match"
    );
    receiver
        .execute(
            Operation::RemoveMember(RemoveMemberInput {
                device_id: provider_id,
            }),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        membership(&receiver, 2, "receiver").await == receiver_id,
        "receiver identity must remain stable"
    );
    let OperationResult::DeviceGroupChoices(current) = receiver
        .execute(Operation::QueryDeviceGroupChoices, CancellationToken::new())
        .await
        .unwrap()
    else {
        panic!("current receiver membership expected")
    };
    assert!(current
        .device_trust
        .devices
        .iter()
        .any(|device| device.device_id == sender_id
            && device.membership == DeviceMembershipSummary::Active));
    // 只以接收端的当前权威权限决定能否获取；不依赖 B 先收到 C 的撤销通知。

    let receiver_facade = receiver.current_facade().await.unwrap();
    // 首项已实际下载，旧 ticket 的内容仍在本机；缓存不能代替当前提供端授权。
    let error = receiver_facade
        .fetch_blob_to_path(FetchBlobToPathCommand {
            cancellation: CancellationToken::new(),
            ticket: first.ticket,
            entry_id: first.entry_id,
            transfer_context: None,
            target_path: roots[0].path().join("revoked-cache.bin").into(),
        })
        .await
        .err()
        .expect("cached content must not bypass provider authorization");
    assert!(!roots[0].path().join("revoked-cache.bin").exists());
    let BlobTransferError::Fetch(source) = error else {
        panic!("fetch authorization error expected")
    };
    assert!(
        source.chain().any(|cause| matches!(
            cause.downcast_ref::<ConnectivityError>(),
            Some(ConnectivityError::Denied)
        )),
        "cached fetch must fail at current admission; exchange: {:?}",
        source
            .chain()
            .filter_map(|cause| cause
                .downcast_ref::<BlobExchangeError>()
                .map(|error| error.kind))
            .collect::<Vec<_>>()
    );
    dispatch(&sender_facade, &receiver_id, &second, contents[1].len()).await;
    let denied_entry = pending(&mut receiver_events, &sender_id).await;
    assert!(
        denied_entry != entry_id,
        "the rejected message must own a distinct receive"
    );
    let denied = settled(&receiver, &denied_entry, "failed").await;
    assert_eq!(denied.items_completed, 0);
    assert_eq!(denied.items.len(), 1);
    assert_eq!(
        denied.items[0].failure_reason,
        Some(ReceiveItemFailureReason::AccessDenied)
    );
    assert!(
        std::fs::read(file).unwrap() == contents[0],
        "revocation must preserve the saved file"
    );
    drop((provider_facade, sender_facade, receiver_facade));
    for runtime in [&provider, &sender, &receiver] {
        runtime.shutdown(None).await.unwrap();
    }
}
