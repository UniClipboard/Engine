//! 完整双 Engine 验收：真实 presence 协议失败期间，原 Blob 流必须继续完成。
use super::{
    empty_engine_host, mount_engine_rendezvous, MemoryHostSecureStorage, StaticHostClipboard,
    StreamFiles, ENGINE_TEST_LOCK, LARGE_BYTES, PASSPHRASE,
};
use crate::testing::receive_streams::{ReceiveStreams, ReleaseStreamsOnDrop};
use crate::{
    ClipboardRestoreMode, CreateSpaceInput, DeviceMembershipSummary, Engine, EngineConfig,
    EntryDeliveryStatusSummary, EntryReceiveProgressInput, HistoryEntryInput, HostCapabilities,
    HostClipboardSnapshot, HostDirectories, HostFileHandle, JoinSpaceInput, Operation,
    OperationResult, ReceiveItemKind, ReceiveProgressSummary, ResendEntryInput, ResendEntryOutcome,
    RestoreClipboardInput, SecretString, SendFilesInput, SendTextInput, SettingsPatch,
    SyncSettingsPatch,
};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{error::Elapsed, sleep, timeout, timeout_at, Instant};
use uc_infra_p2p::network::iroh::test_support::BlobNativeObservation;
use wiremock::MockServer;

fn micros(origin: Instant, at: Instant) -> u64 {
    at.saturating_duration_since(origin)
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn exchanges(streams: &ReceiveStreams, origin: Instant) -> Value {
    let rows: Vec<_> = streams.receives().iter().enumerate().map(|(index, call)| {
        let response = &call.response;
        let failure = *response.error_class.lock().unwrap();
        json!({
            "exchange_alias": index + 1,
            "entered_us": micros(origin, response.entered_at),
            "logical_deadline_us": micros(origin, response.logical_deadline),
            "response_timeout_ms": response.response_timeout.as_millis() as u64,
            "first_read_us": response.first_read_at.lock().unwrap().map(|at| micros(origin, at)),
            "held_before_first_read": response.held_before_first_read,
            "read_bytes": call.read.load(Ordering::SeqCst),
            "validated_bytes": call.validated.load(Ordering::SeqCst),
            "total_bytes": call.total.load(Ordering::SeqCst),
            "stop_called": call.stopped.load(Ordering::SeqCst),
            "stop_succeeded": *response.stop_succeeded.lock().unwrap(),
            "stop_us": response.stopped_at.lock().unwrap().map(|at| micros(origin, at)),
            "finished": call.finished.load(Ordering::SeqCst),
            "finished_us": response.finished_at.lock().unwrap().map(|at| micros(origin, at)),
            "succeeded": response.succeeded.load(Ordering::SeqCst),
            "error_class": failure,
            "elapsed_source": response.timeout_elapsed.load(Ordering::SeqCst),
            "additional_failures": response.additional_count.load(Ordering::SeqCst),
        })
    }).collect();
    json!(rows)
}

fn progress_state(state: &str) -> &'static str {
    match state {
        "pending" => "pending",
        "queued" => "queued",
        "receiving" => "receiving",
        "committing" => "committing",
        "completed" => "completed",
        "failed" => "failed",
        "cancelled" => "cancelled",
        "interrupted" => "interrupted",
        _ => "unknown",
    }
}

fn observe_progress(
    evidence: &mut Value,
    row: &ReceiveProgressSummary,
    original: &ReceiveProgressSummary,
    origin: Instant,
) {
    // 只保存固定状态和身份相等结果，不保存原始身份或错误正文。
    let items: Vec<_> = row.items.iter().take(2).map(|item| json!({
        "item_alias": "item-1",
        "same_item": original.items.first().is_some_and(|first| item.item_id == first.item_id),
        "kind": item.kind,
        "state": progress_state(&item.state),
        "failure_reason": item.failure_reason,
    })).collect();
    let snapshot = json!({
        "entry_alias": "entry-1", "attempt_alias": "attempt-1",
        "same_entry": row.entry_id == original.entry_id,
        "same_attempt": row.attempt_id == original.attempt_id,
        "state": progress_state(&row.state),
        "items_total": row.items_total, "items_completed": row.items_completed,
        "item_count": row.items.len(), "items": items,
        "total_bytes": row.total_bytes, "completed_bytes": row.completed_bytes,
    });
    let changed = evidence["last_progress"]["snapshot"] != snapshot;
    let observation =
        json!({ "observed_us": micros(origin, Instant::now()), "snapshot": snapshot });
    evidence["last_progress"] = observation.clone();
    if changed {
        if evidence["progress_changes"].as_array().unwrap().len() == 64 {
            evidence["progress_changes_truncated"] = json!(true);
            evidence["progress_changes"]
                .as_array_mut()
                .unwrap()
                .remove(0);
        }
        evidence["progress_changes"]
            .as_array_mut()
            .unwrap()
            .push(observation);
    }
}

fn observe_peer(evidence: &mut Value, snapshot: Value, origin: Instant) {
    let changed = evidence["last_peer_query"]["snapshot"] != snapshot;
    let observation =
        json!({ "observed_us": micros(origin, Instant::now()), "snapshot": snapshot });
    evidence["last_peer_query"] = observation.clone();
    if changed {
        if evidence["peer_changes"].as_array().unwrap().len() == 64 {
            evidence["peer_changes_truncated"] = json!(true);
            evidence["peer_changes"].as_array_mut().unwrap().remove(0);
        }
        evidence["peer_changes"]
            .as_array_mut()
            .unwrap()
            .push(observation);
    }
}

async fn ready_member(engine: &Engine) -> Result<String> {
    timeout(Duration::from_secs(30), async {
        loop {
            match engine.execute(Operation::QueryDeviceGroupChoices).await {
                Ok(OperationResult::DeviceGroupChoices(choices))
                    if choices.device_trust.local_membership == DeviceMembershipSummary::Active
                        && choices.device_trust.devices.iter().any(|device| {
                            !device.is_local && device.membership == DeviceMembershipSummary::Active
                        }) =>
                {
                    return Ok(choices.device_trust.local_device_id)
                }
                Ok(OperationResult::DeviceGroupChoices(_)) => {}
                Err(error) if error.code() == 1103 || error.is_retryable() => {}
                _ => bail!("membership readiness failed"),
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("membership readiness deadline")?
}

async fn file_progress(engine: &Engine) -> Result<ReceiveProgressSummary> {
    loop {
        let OperationResult::EntryReceiveProgressList(rows) =
            engine.execute(Operation::ListEntryReceiveProgress).await?
        else {
            bail!("file progress list missing")
        };
        let mut files = rows.into_iter().filter(|row| {
            row.items
                .iter()
                .any(|item| item.kind == ReceiveItemKind::File)
        });
        if let Some(row) = files.next() {
            ensure!(files.next().is_none(), "unexpected second file intent");
            ensure!(
                row.items_total == 1 && row.items.len() == 1,
                "expected one file item"
            );
            return Ok(row);
        }
        sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_presence_failure_preserves_the_original_blob_before_deadline() -> Result<()> {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let native = BlobNativeObservation::start();
    let source_root = tempfile::tempdir()?;
    let receiver_root = tempfile::tempdir()?;
    let provider = ReceiveStreams::install(source_root.path(), false);
    let receiving = ReceiveStreams::install(receiver_root.path(), false);
    let _release = ReleaseStreamsOnDrop(receiving.clone());
    let _release_provider = ReleaseStreamsOnDrop(provider.clone());
    let server = MockServer::builder().start().await;
    mount_engine_rendezvous(&server).await;
    let mut source = None;
    let mut receiver = None;
    let mut sending = None;
    let mut refreshing = None;
    let mut valid_injector = false;
    let mut stage = "setup";
    let mut failure_reason = "scenario_failed";
    let mut action_at = Instant::now();
    let mut evidence = json!({
        "case": "engine_presence_blob_lifetime", "version": 4, "status": "failed",
        "peer_changes": [], "peer_changes_truncated": false, "last_peer_query": null,
        "progress_changes": [], "progress_changes_truncated": false,
        "last_progress": null, "last_progress_query": "not_run",
        "item_alias": "item-1", "attempt_alias": "attempt-1",
        "receiver_entry_alias": "entry-1", "sender_entry_alias": "entry-2",
        "receiver_readback": false, "sender_terminal": "not_observed",
        "sender_changes": [], "sender_changes_truncated": false, "sender_last": null,
        "same_item": false, "same_attempt": false, "same_entry": false,
    });
    let scenario = async {
        let source_host = HostCapabilities::new(
            HostDirectories::new(
                source_root.path().join("private"),
                source_root.path().join("cache"),
                source_root.path().join("temporary"),
                source_root.path().join("logs"),
            ),
            Box::new(MemoryHostSecureStorage::default()),
            Box::new(StaticHostClipboard {
                snapshot: HostClipboardSnapshot {
                    observed_at_ms: 0,
                    representations: Vec::new(),
                },
            }),
            Box::new(StreamFiles),
        );
        let config = EngineConfig::new("1.2.3")
            .with_rendezvous_base_url(server.uri())
            .with_test_relay_fallback(false);
        let (engine, _events) = Engine::start(config.clone(), source_host).await?;
        let source_engine = Arc::new(engine);
        source = Some(source_engine.clone());
        source_engine
            .execute(Operation::CreateSpace(CreateSpaceInput {
                device_name: Some("Source".into()),
                passphrase: SecretString::new(PASSPHRASE),
                passphrase_confirmation: SecretString::new(PASSPHRASE),
            }))
            .await?;
        source_engine
            .execute(Operation::UpdateSettings(Box::new(SettingsPatch {
                sync: Some(SyncSettingsPatch {
                    auto_sync_enabled: Some(true),
                    sync_on_restore: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            })))
            .await?;
        let OperationResult::EntrySent(file) = source_engine
            .execute(Operation::SendFiles(SendFilesInput {
                files: vec![HostFileHandle::new("0")],
                target_devices: Vec::new(),
            }))
            .await?
        else {
            bail!("file staging failed")
        };
        // 配对只同步当前文本；冷文件仅由下方唯一一次 ResendEntry 发起。
        let OperationResult::EntrySent(current) = source_engine
            .execute(Operation::SendText(SendTextInput {
                text: "presence-blob-current".into(),
                target_devices: Vec::new(),
            }))
            .await?
        else {
            bail!("current text staging failed")
        };
        source_engine
            .execute(Operation::RestoreClipboard(RestoreClipboardInput {
                entry_id: current.entry_id,
                mode: ClipboardRestoreMode::Standard,
            }))
            .await?;
        let (engine, _events) =
            Engine::start(config, empty_engine_host(receiver_root.path())).await?;
        let receiver_engine = Arc::new(engine);
        receiver = Some(receiver_engine.clone());
        let OperationResult::InvitationIssued {
            invitation_code, ..
        } = source_engine.execute(Operation::IssueInvitation).await?
        else {
            bail!("invitation missing")
        };
        receiver_engine
            .execute(Operation::JoinSpace(JoinSpaceInput {
                invitation_code,
                device_name: Some("Receiver".into()),
                passphrase: SecretString::new(PASSPHRASE),
                preserve_unreadable_history: false,
            }))
            .await?;
        let receiver_id = ready_member(&receiver_engine).await?;
        let source_id = ready_member(&source_engine).await?;
        receiver_engine
            .execute(Operation::UpdateSettings(Box::new(SettingsPatch {
                sync: Some(SyncSettingsPatch {
                    auto_sync_enabled: Some(true),
                    sync_on_restore: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            })))
            .await?;
        timeout(Duration::from_secs(30), async {
            loop {
                let OperationResult::PeerConnections(peers) = source_engine
                    .execute(Operation::QueryPeerConnections)
                    .await?
                else {
                    bail!("peer connection query missing")
                };
                if peers
                    .iter()
                    .any(|peer| peer.peer_id == receiver_id && peer.connected)
                {
                    return Ok::<_, anyhow::Error>(());
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("peer connection readiness deadline")??;
        ensure!(
            receiving.receives().is_empty(),
            "file fetched before the selected action"
        );
        let native_before = native.counts();
        stage = "hold_validated_blob_body";
        receiving.arm();
        action_at = Instant::now();
        let action_deadline = action_at + Duration::from_secs(60);
        let sender = source_engine.clone();
        let original_entry = file.entry_id.clone();
        let target = receiver_id.clone();
        sending = Some(tokio::spawn(async move {
            sender
                .execute(Operation::ResendEntry(ResendEntryInput {
                    confirm_new_transfer: true,
                    entry_id: original_entry,
                    target_devices: vec![target],
                }))
                .await
        }));
        let original = timeout_at(action_deadline, file_progress(&receiver_engine)).await??;
        ensure!(
            original.state == "receiving",
            "file did not enter the original receive attempt"
        );
        observe_progress(&mut evidence, &original, &original, action_at);
        let item_id = original.items[0].item_id.clone();
        let first = timeout_at(action_deadline, async {
            loop {
                if let Some(call) = receiving.receives().first() {
                    if call.paused.load(Ordering::SeqCst) {
                        return call.clone();
                    }
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        let logical_deadline = first.response.logical_deadline;
        let deadline = logical_deadline.min(action_deadline);
        ensure!(
            Instant::now() < deadline,
            "body hold exceeded original deadline"
        );
        ensure!(
            first.total.load(Ordering::SeqCst) == LARGE_BYTES as u64
                && first.validated.load(Ordering::SeqCst) >= 64 * 1024
                && first.validated.load(Ordering::SeqCst) < LARGE_BYTES as u64
                && first.read.load(Ordering::SeqCst) < LARGE_BYTES as u64
                && !first.finished.load(Ordering::SeqCst),
            "body was not held after actual SDK validation"
        );
        ensure!(
            receiving.receives().len() == 1
                && provider.provides.lock().unwrap().len() == 1
                && native.counts().0 == native_before.0 + 2
                && native.counts().1 == 2
                && native.closed_count() == 0,
            "initial Blob native pair was not open"
        );
        let native_first = native.counts();
        evidence["native_before"] = json!(native_before);
        evidence["native_first"] = json!(native_first);
        evidence["body_held_us"] = json!(micros(action_at, Instant::now()));
        evidence["held_validated_bytes"] = json!(first.validated.load(Ordering::SeqCst));
        evidence["held_read_bytes"] = json!(first.read.load(Ordering::SeqCst));
        evidence["native_closed_at_hold"] = json!(native.closed_count());
        let OperationResult::PeerConnections(peers) = timeout_at(
            deadline,
            receiver_engine.execute(Operation::QueryPeerConnections),
        )
        .await??
        else {
            bail!("initial receiver peer query missing")
        };
        ensure!(
            Instant::now() < deadline,
            "initial peer query returned late"
        );
        let mut paired = peers.iter().filter(|peer| peer.peer_id == source_id);
        ensure!(
            paired
                .next()
                .is_some_and(|peer| peer.is_paired && peer.connected)
                && paired.next().is_none(),
            "receiver did not observe initial paired Online"
        );
        observe_peer(
            &mut evidence,
            json!({ "peer_alias": "peer-A", "state": "row",
            "paired": true, "connected": true }),
            action_at,
        );
        evidence["online_before_arm_us"] = json!(micros(action_at, Instant::now()));
        stage = "real_presence_failure_and_stable_offline";
        failure_reason = "invalid_injector";
        let armed_at = Instant::now();
        provider.presence.arm(deadline);
        receiving.presence.arm(deadline);
        evidence["armed_us"] = json!(micros(action_at, armed_at));
        let refresher = receiver_engine.clone();
        refreshing = Some(tokio::spawn(async move {
            refresher.execute(Operation::RefreshPeerConnections).await
        }));
        // stable 查询与完整 refresh 并发，不能把 refresh 的返回值当作 Offline。
        let offline_at = timeout_at(deadline, async {
            loop {
                let query_started_at = Instant::now();
                let result = receiver_engine
                    .execute(Operation::QueryPeerConnections)
                    .await?;
                ensure!(Instant::now() < deadline, "presence query returned late");
                let OperationResult::PeerConnections(peers) = result else {
                    bail!("stable peer query result missing")
                };
                let mut matching = peers.iter().filter(|peer| peer.peer_id == source_id);
                let peer = matching.next();
                ensure!(matching.next().is_none(), "duplicate stable peer row");
                let snapshot = match peer {
                    Some(peer) => json!({ "peer_alias": "peer-A", "state": "row",
                        "paired": peer.is_paired, "connected": peer.connected }),
                    None => json!({ "peer_alias": "peer-A", "state": "missing" }),
                };
                observe_peer(&mut evidence, snapshot, action_at);
                evidence["native_during_injection"] = json!({
                    "counts": native.counts(), "closed_count": native.closed_count(),
                    "observed_us": micros(action_at, Instant::now()),
                });
                if peer.is_some_and(|peer| peer.is_paired && !peer.connected) {
                    if let Some((write_at, read_at)) = provider
                        .presence
                        .correlated_failure(&receiving.presence, armed_at)
                    {
                        if query_started_at < write_at || query_started_at < read_at {
                            sleep(Duration::from_millis(10)).await;
                            continue;
                        }
                        let observed_at = Instant::now();
                        evidence["wire_failure"] = json!({ "same_challenge": true,
                            "provider_alias": "peer-A", "reader_alias": "peer-B",
                            "write_failed_us": micros(action_at, write_at),
                            "read_failed_us": micros(action_at, read_at) });
                        return Ok::<_, anyhow::Error>(observed_at);
                    }
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        ensure!(
            first.paused.load(Ordering::SeqCst)
                && first.validated.load(Ordering::SeqCst) >= 64 * 1024
                && first.validated.load(Ordering::SeqCst) < LARGE_BYTES as u64
                && first.read.load(Ordering::SeqCst) < LARGE_BYTES as u64,
            "body not held through actual Offline"
        );
        valid_injector = true;
        evidence["offline_us"] = json!(micros(action_at, offline_at));
        evidence["body_held_at_offline"] = json!(true);
        evidence["native_at_offline"] = json!({ "counts": native.counts(),
            "closed_count": native.closed_count() });
        failure_reason = "blob_native_closed_after_valid_presence_failure";
        ensure!(
            native.closed_count() == 0,
            "original Blob native pair closed"
        );
        failure_reason = "blob_replaced_after_valid_presence_failure";
        ensure!(
            native.counts() == native_first
                && receiving.receives().len() == 1
                && provider.provides.lock().unwrap().len() == 1,
            "presence failure replaced the original Blob"
        );
        provider.presence.disarm();
        receiving.presence.disarm();
        evidence["disarmed_us"] = json!(micros(action_at, Instant::now()));
        first.release();
        evidence["body_released_us"] = json!(micros(action_at, Instant::now()));
        failure_reason = "original_blob_stream_failed_after_presence_offline";
        timeout_at(deadline, async {
            while !first.finished.load(Ordering::SeqCst) {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        evidence["native_after_stream"] = json!({ "counts": native.counts(),
            "closed_count": native.closed_count() });
        if native.closed_count() != 0 {
            failure_reason = "blob_native_closed_after_valid_presence_failure";
            bail!("original Blob native pair closed after release");
        }
        ensure!(
            first.response.succeeded.load(Ordering::SeqCst)
                && !first.stopped.load(Ordering::SeqCst)
                && first.validated.load(Ordering::SeqCst) == LARGE_BYTES as u64,
            "original SDK stream failed after real presence failure"
        );
        stage = "original_receiver_readback_and_sender_delivery";
        let completed = timeout_at(deadline, async {
            loop {
                failure_reason = "progress_query_failed";
                evidence["last_progress_query"] = json!("pending");
                let queried = receiver_engine
                    .execute(Operation::QueryEntryReceiveProgress(
                        EntryReceiveProgressInput {
                            entry_id: original.entry_id.clone(),
                        },
                    ))
                    .await;
                let queried = match queried {
                    Ok(result) => result,
                    Err(source) => {
                        evidence["last_progress_query"] = json!("operation_failed");
                        return Err(source.into());
                    }
                };
                failure_reason = "progress_query_returned_late";
                ensure!(Instant::now() < deadline, "progress query returned late");
                let row = match queried {
                    OperationResult::EntryReceiveProgress(Some(row)) => {
                        evidence["last_progress_query"] = json!("row");
                        row
                    }
                    OperationResult::EntryReceiveProgress(None) => {
                        failure_reason = "original_progress_missing";
                        evidence["last_progress_query"] = json!("missing");
                        bail!("original receive progress missing");
                    }
                    _ => {
                        failure_reason = "unexpected_progress_result";
                        evidence["last_progress_query"] = json!("unexpected_result");
                        bail!("original receive progress result missing");
                    }
                };
                observe_progress(&mut evidence, &row, &original, action_at);
                failure_reason = "original_progress_identity_changed";
                ensure!(
                    row.entry_id == original.entry_id
                        && row.attempt_id == original.attempt_id
                        && row.items_total == 1
                        && row.items.len() == 1
                        && row.items[0].kind == ReceiveItemKind::File
                        && row.items[0].item_id == item_id,
                    "recovery replaced the original file intent"
                );
                if row.state == "completed" {
                    return Ok::<_, anyhow::Error>(row);
                }
                failure_reason = "original_receive_unexpected_state";
                ensure!(
                    matches!(row.state.as_str(), "receiving" | "committing"),
                    "original receive failed"
                );
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        failure_reason = "completed_item_count_mismatch";
        ensure!(
            completed.items_completed == 1,
            "original file item not completed"
        );
        evidence["same_entry"] = json!(true);
        evidence["same_attempt"] = json!(true);
        evidence["same_item"] = json!(true);
        failure_reason = "original_file_readback_failed";
        let OperationResult::EntryFileRead(readback) = timeout_at(
            deadline,
            receiver_engine.execute(Operation::ReadEntryFile(HistoryEntryInput {
                entry_id: original.entry_id.clone(),
            })),
        )
        .await??
        else {
            bail!("original receiver file unavailable")
        };
        ensure!(
            Instant::now() < deadline,
            "original file readback returned late"
        );
        let expected = vec![0x11; LARGE_BYTES];
        ensure!(
            readback.bytes == expected,
            "original receiver file bytes differ"
        );
        evidence["file"] = json!({
            "expected_bytes": expected.len(), "readback_bytes": readback.bytes.len(),
            "expected_sha256": format!("{:x}", Sha256::digest(&expected)),
            "readback_sha256": format!("{:x}", Sha256::digest(&readback.bytes)),
        });
        evidence["receiver_readback"] = json!(true);
        evidence["receiver_readback_entry_alias"] = json!("entry-1");
        evidence["readback_us"] = json!(micros(action_at, Instant::now()));
        failure_reason = "original_sender_delivery_failed";
        timeout_at(deadline, async {
            loop {
                let OperationResult::EntryDelivery(delivery) = source_engine
                    .execute(Operation::QueryEntryDelivery(HistoryEntryInput {
                        entry_id: file.entry_id.clone(),
                    }))
                    .await?
                else {
                    bail!("original sender delivery missing")
                };
                ensure!(
                    Instant::now() < deadline,
                    "sender query returned after original deadline"
                );
                ensure!(
                    delivery.deliveries.len() == 1,
                    "unexpected sender target count"
                );
                let target = &delivery.deliveries[0];
                ensure!(
                    target.target_device_id == receiver_id,
                    "sender target changed"
                );
                let kind = match &target.status {
                    EntryDeliveryStatusSummary::Pending => "pending",
                    EntryDeliveryStatusSummary::Delivered => "delivered",
                    EntryDeliveryStatusSummary::Duplicate => "duplicate",
                    EntryDeliveryStatusSummary::Unreachable => "unreachable",
                    EntryDeliveryStatusSummary::Superseded => "superseded",
                    EntryDeliveryStatusSummary::Failed { .. } => "failed",
                };
                let snapshot = json!({ "observed_us": micros(action_at, Instant::now()),
                    "status": serde_json::to_value(&target.status)? });
                let changed = evidence["sender_last"]["status"] != snapshot["status"];
                evidence["sender_last"] = snapshot.clone();
                evidence["sender_terminal"] = json!(kind);
                if changed {
                    let changes = evidence["sender_changes"].as_array_mut().unwrap();
                    if changes.len() < 64 {
                        changes.push(snapshot);
                    } else {
                        evidence["sender_changes_truncated"] = json!(true);
                    }
                }
                match &target.status {
                    EntryDeliveryStatusSummary::Delivered
                    | EntryDeliveryStatusSummary::Duplicate => return Ok::<_, anyhow::Error>(()),
                    // 既有恢复负责人可以在原截止时间内结算同一投递记录。
                    EntryDeliveryStatusSummary::Pending
                    | EntryDeliveryStatusSummary::Unreachable => {}
                    _ => bail!("original sender did not deliver"),
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        evidence["sender_settled_us"] = json!(micros(action_at, Instant::now()));
        failure_reason = "original_send_operation_failed";
        let sent = timeout_at(
            deadline,
            sending.as_mut().context("original send task missing")?,
        )
        .await
        .context("original send deadline")?
        .context("original send task failed")?
        .context("original send operation failed")?;
        ensure!(
            matches!(
                sent,
                OperationResult::EntryResent(ResendEntryOutcome::Completed(_))
            ),
            "original send did not return"
        );
        sending.take();
        ensure!(Instant::now() < deadline, "original send returned late");
        failure_reason = "original_stream_or_native_replaced";
        let calls = receiving.receives();
        ensure!(
            calls.len() == 1 && Arc::ptr_eq(&calls[0], &first),
            "original receive stream was replaced"
        );
        ensure!(
            calls[0].digest == first.digest
                && first.total.load(Ordering::SeqCst) == LARGE_BYTES as u64
                && first.response.logical_deadline == logical_deadline,
            "original file content or logical budget changed"
        );
        ensure!(
            provider.provides.lock().unwrap().len() == 1,
            "provider exchange was retried"
        );
        // 完整读回和投递结算之后、显式 shutdown 之前检查，排除关闭连接读完缓冲的假成功。
        let native_after = native.counts();
        let native_closed_after = native.closed_count();
        evidence["native_after"] = json!(native_after);
        evidence["native_closed_after"] = json!(native_closed_after);
        evidence["native_terminal"] = json!({ "counts": native_after,
            "closed_count": native_closed_after,
            "observed_us": micros(action_at, Instant::now()),
            "after_readback_and_settlement": true, "before_shutdown": true,
            "receive_count": calls.len(),
            "provide_count": provider.provides.lock().unwrap().len() });
        failure_reason = "blob_native_closed_after_valid_presence_failure";
        ensure!(
            native_closed_after == 0,
            "original Blob native pair closed before shutdown"
        );
        failure_reason = "original_stream_or_native_replaced";
        ensure!(
            native_after == native_first,
            "original Blob native pair did not survive"
        );
        ensure!(Instant::now() < deadline, "original action finished late");
        evidence["same_digest"] = json!(true);
        evidence["original_stream_success"] = json!(true);
        evidence["no_retry"] = json!(true);
        Ok::<_, anyhow::Error>(())
    };
    // 外层看门狗仅触发失败清理；不会延长原文件动作的30/60秒预算。
    let outcome = timeout(Duration::from_secs(180), scenario).await;
    let passed = matches!(&outcome, Ok(Ok(())));
    evidence["failed_stage"] = if passed { Value::Null } else { json!(stage) };
    evidence["valid_injector"] = json!(valid_injector);
    evidence["failure_domain"] = json!(if passed {
        "none"
    } else if valid_injector {
        "product_or_completion"
    } else if stage == "real_presence_failure_and_stable_offline" {
        "invalid_injector"
    } else {
        "setup_or_body_precondition"
    });
    evidence["presence_A"] = provider.presence.snapshot(action_at);
    evidence["presence_B"] = receiving.presence.snapshot(action_at);
    evidence["native_before_cleanup"] = json!({ "counts": native.counts(),
        "closed_count": native.closed_count() });
    let classified_failure = match &outcome {
        Err(_) => Some("scenario_watchdog_elapsed"),
        Ok(Err(_)) if !valid_injector && stage == "real_presence_failure_and_stable_offline" => {
            Some("invalid_injector")
        }
        Ok(Err(error)) if error.downcast_ref::<Elapsed>().is_some() => Some(if stage == "setup" {
            "setup_deadline_elapsed"
        } else {
            "original_deadline_elapsed"
        }),
        Ok(Err(_)) => Some(failure_reason),
        Ok(Ok(())) => None,
    };
    evidence["failure_reason"] = json!(classified_failure);
    evidence["exchanges"] = exchanges(&receiving, action_at);
    receiving.presence.disarm();
    provider.presence.disarm();
    receiving.release_all();
    provider.release_all();
    let receiver_closed = match &receiver {
        Some(engine) => matches!(
            timeout(Duration::from_secs(30), engine.shutdown_until_complete()).await,
            Ok(Ok(()))
        ),
        None => true,
    };
    let source_closed = match &source {
        Some(engine) => matches!(
            timeout(Duration::from_secs(30), engine.shutdown_until_complete()).await,
            Ok(Ok(()))
        ),
        None => true,
    };
    let sender_joined = if let Some(mut task) = sending {
        match timeout(Duration::from_secs(30), &mut task).await {
            Ok(Ok(_)) => true,
            Ok(Err(_)) => false,
            Err(_) => {
                task.abort();
                let _aborted = task.await;
                false
            }
        }
    } else {
        true
    };
    let refresh_joined = if let Some(mut task) = refreshing {
        match timeout(Duration::from_secs(30), &mut task).await {
            Ok(Ok(_)) => true,
            Ok(Err(_)) => false,
            Err(_) => {
                task.abort();
                let _aborted = task.await;
                false
            }
        }
    } else {
        true
    };
    evidence["presence_A_after_cleanup"] = provider.presence.snapshot(action_at);
    evidence["presence_B_after_cleanup"] = receiving.presence.snapshot(action_at);
    evidence["exchanges_after_cleanup"] = exchanges(&receiving, action_at);
    evidence["provider_exchange_count"] = json!(provider.provides.lock().unwrap().len());
    drop(receiver);
    drop(source);
    let receiver_directory_removed = receiver_root.close().is_ok();
    let source_directory_removed = source_root.close().is_ok();
    let closed = native.counts();
    let cleanup = receiver_closed
        && source_closed
        && sender_joined
        && refresh_joined
        && closed.1 == 0
        && receiver_directory_removed
        && source_directory_removed;
    evidence["cleanup"] = json!({ "receiver_closed": receiver_closed, "source_closed": source_closed,
        "sender_task_joined": sender_joined, "refresh_task_joined": refresh_joined,
        "native_live": closed.1,
        "receiver_directory_removed": receiver_directory_removed,
        "source_directory_removed": source_directory_removed, "passed": cleanup });
    if passed && !cleanup {
        evidence["failure_reason"] = json!("cleanup_failed");
        evidence["failure_domain"] = json!("cleanup");
    }
    evidence["status"] = json!(if passed && cleanup {
        "passed"
    } else {
        "failed"
    });
    let path = std::env::var_os("UC_PRESENCE_BLOB_E2E_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/presence-blob-e2e/result.json")
        });
    ensure!(path.is_absolute(), "artifact path must be absolute");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(&evidence)?;
    std::fs::write(&path, &bytes)?;
    std::fs::write(
        path.with_extension("sha256"),
        format!("{:x}\n", Sha256::digest(&bytes)),
    )?;
    outcome.context("complete Engine scenario watchdog")??;
    ensure!(cleanup, "complete Engine cleanup failed");
    Ok(())
}
