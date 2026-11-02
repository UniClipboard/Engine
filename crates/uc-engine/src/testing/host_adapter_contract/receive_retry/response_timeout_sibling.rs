//! 完整双 Engine 验收：原响应超时保留兄弟流，恢复后用未缓存内容验证连接复用。
use super::{
    empty_engine_host, mount_engine_rendezvous, MemoryHostSecureStorage, StaticHostClipboard,
    StreamFiles, ENGINE_TEST_LOCK, LARGE_BYTES, PASSPHRASE, TAIL_BYTES,
};
use crate::testing::receive_streams::{ReceiveStream, ReceiveStreams, ReleaseStreamsOnDrop};
use crate::{
    BlobResourceInput, CancelEntryReceiveInput, CreateSpaceInput, DeviceMembershipSummary, Engine,
    EngineConfig, EngineError, EntryDeliveryStatusSummary, EntryReceiveCancellationOutcome,
    EntryReceiveProgressInput, EntryReceiveRetryOutcome, HistoryEntryInput, HostCapabilities,
    HostClipboardRepresentation, HostClipboardSnapshot, HostDirectories, HostFileHandle,
    JoinSpaceInput, ListHistoryEntriesInput, ObserveClipboardChangeInput, Operation,
    OperationResult, ReceiveItemKind, ReceiveProgressSummary, ResendEntryInput, ResendEntryOutcome,
    RetryEntryReceiveInput, SecretString, SendFilesInput, SettingsPatch, SyncSettingsPatch,
};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::{error::Elapsed, sleep, timeout, timeout_at, Instant};
use uc_application::deps::BlobExchangeFailure;
use uc_core::ports::blob::BlobDigest;
use uc_infra_p2p::network::iroh::test_support::BlobNativeObservation;
use url::Url;
use wiremock::MockServer;

fn micros(origin: Instant, at: Instant) -> u64 {
    u64::try_from(at.saturating_duration_since(origin).as_micros()).unwrap_or(u64::MAX)
}
fn digest(byte: u8, size: usize) -> BlobDigest {
    BlobDigest::from_bytes(*blake3::hash(&vec![byte; size]).as_bytes())
}
fn calls(streams: &ReceiveStreams, digest: BlobDigest) -> Vec<Arc<ReceiveStream>> {
    streams
        .receives()
        .into_iter()
        .filter(|call| call.digest == digest)
        .collect()
}
fn held_body(call: &ReceiveStream) -> bool {
    call.paused.load(Ordering::SeqCst)
        && call.total.load(Ordering::SeqCst) == LARGE_BYTES as u64
        && call.validated.load(Ordering::SeqCst) >= 64 * 1024
        && call.validated.load(Ordering::SeqCst) < LARGE_BYTES as u64
        && call.read.load(Ordering::SeqCst) < LARGE_BYTES as u64
        && !call.finished.load(Ordering::SeqCst)
        && !call.stopped.load(Ordering::SeqCst)
}
fn native_sample(native: &BlobNativeObservation, origin: Instant) -> Value {
    json!({ "observed_us": micros(origin, Instant::now()), "counts": native.counts(),
        "closed_count": native.closed_count() })
}
async fn within(
    engine: &Engine,
    operation: Operation,
    deadline: Instant,
) -> Result<OperationResult> {
    let result = timeout_at(deadline, engine.execute(operation)).await??;
    ensure!(
        Instant::now() < deadline,
        "operation returned after original deadline"
    );
    Ok(result)
}
async fn query(engine: &Engine, entry: &str, deadline: Instant) -> Result<ReceiveProgressSummary> {
    let OperationResult::EntryReceiveProgress(Some(row)) = within(
        engine,
        Operation::QueryEntryReceiveProgress(EntryReceiveProgressInput {
            entry_id: entry.into(),
        }),
        deadline,
    )
    .await?
    else {
        bail!("exact receive progress missing")
    };
    ensure!(row.entry_id == entry, "exact progress entry changed");
    Ok(row)
}
fn same_receive(row: &ReceiveProgressSummary, expected: &ReceiveProgressSummary) -> bool {
    row.entry_id == expected.entry_id
        && row.attempt_id == expected.attempt_id
        && row.items_total == expected.items_total
        && row.items.len() == expected.items.len()
        && expected.items.iter().all(|item| {
            row.items
                .iter()
                .any(|current| current.item_id == item.item_id && current.kind == item.kind)
        })
}
fn state(state: &str) -> &'static str {
    match state {
        "pending" => "pending",
        "transferring" => "transferring",
        "receiving" => "receiving",
        "committing" => "committing",
        "cancelling" => "cancelling",
        "failing" => "failing",
        "completed" => "completed",
        "failed" => "failed",
        "cancelled" => "cancelled",
        _ => "unknown",
    }
}
fn observe(
    evidence: &mut Value,
    alias: &'static str,
    row: &ReceiveProgressSummary,
    expected: &ReceiveProgressSummary,
    origin: Instant,
) {
    let items: Vec<_> = row.items.iter().take(2).map(|item| json!({
        "kind": item.kind, "state": state(&item.state), "failure_reason": item.failure_reason,
        "known_item": expected.items.iter().any(|old| old.item_id == item.item_id),
        "item_alias": expected.items.iter().position(|old| old.item_id == item.item_id)
            .map(|index| format!("{alias}-item-{}", index + 1)),
    })).collect();
    let snapshot = json!({ "entry_alias": alias, "same_entry": row.entry_id == expected.entry_id,
        "same_attempt": row.attempt_id == expected.attempt_id, "same_items": same_receive(row, expected),
        "state": state(&row.state), "items": items, "item_count": row.items.len(),
        "items_total": row.items_total, "items_completed": row.items_completed,
        "total_bytes": row.total_bytes, "completed_bytes": row.completed_bytes });
    let changed = evidence["latest_progress"][alias]["snapshot"] != snapshot;
    let observed = json!({ "observed_us": micros(origin, Instant::now()), "snapshot": snapshot });
    evidence["latest_progress"][alias] = observed.clone();
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
            .push(observed);
    }
}
async fn discover(
    engine: &Engine,
    count: u32,
    deadline: Instant,
) -> Result<ReceiveProgressSummary> {
    loop {
        let OperationResult::EntryReceiveProgressList(rows) =
            within(engine, Operation::ListEntryReceiveProgress, deadline).await?
        else {
            bail!("active progress list missing")
        };
        let mut rows = rows.into_iter().filter(|row| {
            row.items
                .iter()
                .any(|item| item.kind == ReceiveItemKind::File)
        });
        if let Some(row) = rows.next() {
            ensure!(
                rows.next().is_none()
                    && row.items_total == count
                    && row.items.len() == count as usize,
                "unexpected active file intent"
            );
            ensure!(
                row.items
                    .iter()
                    .all(|item| item.kind == ReceiveItemKind::File),
                "unexpected item kind"
            );
            return Ok(row);
        }
        sleep(Duration::from_millis(10)).await;
    }
}
async fn discover_c(
    engine: &Engine,
    excluded: &[&str],
    deadline: Instant,
) -> Result<ReceiveProgressSummary> {
    // 小文件可在 List 查询前完成；从稳定历史取身份，再直接查询权威进度。
    loop {
        let OperationResult::HistoryEntries(entries) = within(
            engine,
            Operation::ListHistoryEntries(ListHistoryEntriesInput {
                limit: 32,
                offset: 0,
            }),
            deadline,
        )
        .await?
        else {
            bail!("stable history list missing")
        };
        ensure!(entries.len() < 32, "history discovery could be truncated");
        let mut candidates = Vec::new();
        for entry in entries {
            if excluded.contains(&entry.entry_id.as_str()) {
                continue;
            }
            let result = within(
                engine,
                Operation::QueryEntryReceiveProgress(EntryReceiveProgressInput {
                    entry_id: entry.entry_id,
                }),
                deadline,
            )
            .await?;
            let OperationResult::EntryReceiveProgress(row) = result else {
                bail!("history receive query missing")
            };
            if let Some(row) = row {
                if row.items_total == 1
                    && row.items.len() == 1
                    && row.items[0].kind == ReceiveItemKind::File
                {
                    candidates.push(row);
                }
            }
        }
        ensure!(candidates.len() <= 1, "ambiguous C file intent");
        if let Some(row) = candidates.pop() {
            return Ok(row);
        }
        sleep(Duration::from_millis(10)).await;
    }
}
async fn paused(
    streams: &ReceiveStreams,
    digest: BlobDigest,
    index: usize,
    deadline: Instant,
) -> Result<Arc<ReceiveStream>> {
    timeout_at(deadline, async {
        loop {
            let found = calls(streams, digest);
            ensure!(
                found.len() <= index + 1,
                "unexpected receive retry before gate"
            );
            if let Some(call) = found.get(index) {
                if call.paused.load(Ordering::SeqCst) {
                    return Ok(call.clone());
                }
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await?
}
async fn finished(call: &ReceiveStream, deadline: Instant) -> Result<()> {
    timeout_at(deadline, async {
        while !call.finished.load(Ordering::SeqCst) {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    ensure!(
        Instant::now() < deadline,
        "SDK finished after original deadline"
    );
    Ok(())
}
async fn completed(
    engine: &Engine,
    expected: &ReceiveProgressSummary,
    deadline: Instant,
    evidence: &mut Value,
    alias: &'static str,
    origin: Instant,
) -> Result<()> {
    loop {
        let row = query(engine, &expected.entry_id, deadline).await?;
        observe(evidence, alias, &row, expected, origin);
        ensure!(same_receive(&row, expected), "receive identity changed");
        if row.state == "completed" {
            ensure!(
                row.items_completed == row.items_total
                    && row.items.iter().all(|item| item.state == "completed"),
                "completed receive has unfinished items"
            );
            return Ok(());
        }
        ensure!(
            matches!(row.state.as_str(), "receiving" | "committing"),
            "receive did not complete"
        );
        sleep(Duration::from_millis(10)).await;
    }
}
async fn files(
    engine: &Engine,
    entry: &str,
    root: &Path,
    deadline: Instant,
) -> Result<BTreeMap<Vec<u8>, PathBuf>> {
    let OperationResult::HistoryEntryResource(resource) = within(
        engine,
        Operation::GetHistoryEntryResource(HistoryEntryInput {
            entry_id: entry.into(),
        }),
        deadline,
    )
    .await?
    else {
        bail!("file resource missing")
    };
    let bytes = match (resource.inline_data, resource.blob_id) {
        (Some(bytes), _) => bytes,
        (None, Some(blob_id)) => {
            let OperationResult::BlobRead(blob) = within(
                engine,
                Operation::ReadBlob(BlobResourceInput { blob_id }),
                deadline,
            )
            .await?
            else {
                bail!("file URI blob missing")
            };
            blob.bytes
        }
        _ => bail!("file URI bytes missing"),
    };
    let root = root
        .canonicalize()
        .context("canonicalize private receiver root")?;
    let mut saved = BTreeMap::new();
    for line in std::str::from_utf8(&bytes)
        .context("decode file URI list")?
        .lines()
        .filter(|line| !line.is_empty())
    {
        let uri = Url::parse(line).context("parse file URI")?;
        if uri.scheme() == "uniclip-missing" {
            continue;
        }
        ensure!(uri.scheme() == "file", "unexpected resource URI scheme");
        let path = uri
            .to_file_path()
            .ok()
            .context("invalid local file URI")?
            .canonicalize()
            .context("canonicalize receiver output")?;
        ensure!(
            path.starts_with(&root),
            "file output escaped private receiver root"
        );
        let data = std::fs::read(&path).context("read actual receiver file")?;
        ensure!(
            saved.insert(data, path).is_none(),
            "duplicate output content"
        );
        ensure!(
            Instant::now() < deadline,
            "readback exceeded original deadline"
        );
    }
    Ok(saved)
}
async fn verify_files(
    engine: &Engine,
    entry: &str,
    root: &Path,
    expected: &[(u8, usize)],
    deadline: Instant,
) -> Result<Value> {
    let saved = files(engine, entry, root, deadline).await?;
    ensure!(saved.len() == expected.len(), "wrong file output count");
    let mut rows = Vec::new();
    for (index, (byte, count)) in expected.iter().enumerate() {
        let expected = vec![*byte; *count];
        let (actual, _) = saved
            .get_key_value(&expected)
            .context("full original file bytes differ")?;
        rows.push(json!({ "file_alias": index + 1, "bytes": actual.len(),
            "expected_sha256": format!("{:x}", Sha256::digest(&expected)),
            "readback_sha256": format!("{:x}", Sha256::digest(actual)) }));
    }
    ensure!(
        Instant::now() < deadline,
        "hash verification exceeded original deadline"
    );
    Ok(json!(rows))
}
async fn delivered(
    engine: &Engine,
    entry: &str,
    target: &str,
    deadline: Instant,
    evidence: &mut Value,
    alias: &'static str,
    origin: Instant,
) -> Result<()> {
    loop {
        let OperationResult::EntryDelivery(row) = within(
            engine,
            Operation::QueryEntryDelivery(HistoryEntryInput {
                entry_id: entry.into(),
            }),
            deadline,
        )
        .await?
        else {
            bail!("original sender delivery missing")
        };
        ensure!(
            row.deliveries.len() == 1 && row.deliveries[0].target_device_id == target,
            "sender target changed"
        );
        let status = &row.deliveries[0].status;
        let kind = match status {
            EntryDeliveryStatusSummary::Pending => "pending",
            EntryDeliveryStatusSummary::Unreachable => "unreachable",
            EntryDeliveryStatusSummary::Delivered => "delivered",
            EntryDeliveryStatusSummary::Duplicate => "duplicate",
            EntryDeliveryStatusSummary::Superseded => "superseded",
            EntryDeliveryStatusSummary::Failed { .. } => "failed",
        };
        let reason = match status {
            EntryDeliveryStatusSummary::Failed { reason } => Some(reason),
            _ => None,
        };
        let snapshot = json!({ "entry_alias": alias, "target_alias": "receiver",
            "status": kind, "failure_reason": reason });
        let changed = evidence["last_sender"][alias]["snapshot"] != snapshot;
        let observed =
            json!({ "observed_us": micros(origin, Instant::now()), "snapshot": snapshot });
        evidence["last_sender"][alias] = observed.clone();
        if changed {
            if evidence["sender_changes"].as_array().unwrap().len() == 64 {
                evidence["sender_changes_truncated"] = json!(true);
                evidence["sender_changes"].as_array_mut().unwrap().remove(0);
            }
            evidence["sender_changes"]
                .as_array_mut()
                .unwrap()
                .push(observed);
        }
        match status {
            EntryDeliveryStatusSummary::Delivered => return Ok(()),
            // 既有发送负责人允许这两种状态在原期限内继续结算，只有 Delivered 才成功。
            EntryDeliveryStatusSummary::Pending | EntryDeliveryStatusSummary::Unreachable => {}
            _ => bail!("original sender did not deliver"),
        }
        sleep(Duration::from_millis(10)).await;
    }
}
async fn current(engine: &Engine, expected: &str, deadline: Instant) -> Result<()> {
    ensure!(
        matches!(within(engine, Operation::QueryActiveClipboard, deadline).await?,
        OperationResult::ActiveClipboard(Some(row)) if row.entry_id == expected),
        "Source current changed"
    );
    Ok(())
}
async fn member(engine: &Engine) -> Result<String> {
    timeout(Duration::from_secs(30), async {
        loop {
            match engine.execute(Operation::QueryDeviceGroupChoices).await {
                Ok(OperationResult::DeviceGroupChoices(choices))
                    if choices.device_trust.local_membership == DeviceMembershipSummary::Active
                        && choices.device_trust.devices.iter().any(|peer| {
                            !peer.is_local && peer.membership == DeviceMembershipSummary::Active
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
    .await?
}
fn resend(
    engine: &Arc<Engine>,
    entry: String,
    target: String,
) -> JoinHandle<Result<OperationResult, EngineError>> {
    let engine = engine.clone();
    tokio::spawn(async move {
        engine
            .execute(Operation::ResendEntry(ResendEntryInput {
                confirm_new_transfer: true,
                entry_id: entry,
                target_devices: vec![target],
            }))
            .await
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_header_timeout_preserves_manual_retry_sibling_and_reuses_healthy_pair(
) -> Result<()> {
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
    let a_prefix_digest = digest(0x55, TAIL_BYTES);
    let b_prefix_digest = digest(0x66, TAIL_BYTES);
    let a_digest = digest(0x11, LARGE_BYTES);
    let b_digest = digest(0x33, LARGE_BYTES);
    let c_digest = digest(0x77, TAIL_BYTES);
    let mut source = None;
    let mut receiver = None;
    let mut sending_b = None;
    let mut sending_a = None;
    let mut sending_c = None;
    let mut stage = "setup";
    let mut failure_reason = "setup_failed";
    let mut origin = Instant::now();
    let mut overlap_proved = false;
    let mut a_passed = false;
    let mut evidence = json!({ "case": "engine_response_timeout_manual_retry_sibling_reuse", "version": 1,
        "status": "failed", "latest_progress": {}, "progress_changes": [], "progress_changes_truncated": false,
        "last_sender": {}, "sender_changes": [], "sender_changes_truncated": false,
        "A_delivered": false, "C_delivered": false, "B1_completed": false });
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
                    observed_at_ms: 1,
                    representations: vec![HostClipboardRepresentation::Inline {
                        format: "text".into(),
                        mime_type: Some("text/plain".into()),
                        bytes: b"sibling-unrelated-current".to_vec(),
                    }],
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
                    sync_on_restore: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            })))
            .await?;
        let mut entries = Vec::new();
        for handles in [vec!["2", "0"], vec!["3", "1"], vec!["4"]] {
            let OperationResult::EntrySent(report) = source_engine
                .execute(Operation::SendFiles(SendFilesInput {
                    files: handles.into_iter().map(HostFileHandle::new).collect(),
                    target_devices: vec![],
                }))
                .await?
            else {
                bail!("file history staging failed")
            };
            entries.push(report.entry_id);
        }
        // 只在配对前观察无关文本；连接期间不 restore、不重新发布 Source current。
        ensure!(
            matches!(
                source_engine
                    .execute(Operation::ObserveClipboardChange(
                        ObserveClipboardChangeInput { dispatch: false }
                    ))
                    .await?,
                OperationResult::ClipboardChangeObserved { .. }
            ),
            "pre-pair text observation failed"
        );
        let OperationResult::ActiveClipboard(Some(active)) = source_engine
            .execute(Operation::QueryActiveClipboard)
            .await?
        else {
            bail!("pre-pair Source current missing")
        };
        let source_current = active.entry_id;
        ensure!(
            entries.iter().all(|entry| *entry != source_current),
            "file became Source current"
        );
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
        let receiver_id = member(&receiver_engine).await?;
        member(&source_engine).await?;
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
        let ready_by = Instant::now() + Duration::from_secs(30);
        loop {
            let OperationResult::PeerConnections(peers) =
                within(&source_engine, Operation::QueryPeerConnections, ready_by).await?
            else {
                bail!("peer readiness query missing")
            };
            if peers
                .iter()
                .any(|peer| peer.peer_id == receiver_id && peer.connected)
            {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        ensure!(
            receiving.receives().is_empty()
                && provider.provides.lock().unwrap().is_empty()
                && native.counts() == (0, 0),
            "file fetched before selected actions"
        );
        stage = "B0_real_cancel_settlement";
        failure_reason = "B0_preparation_failed";
        origin = Instant::now();
        let b_deadline = origin + Duration::from_secs(60);
        receiving.hold_digest_body(b_digest);
        sending_b = Some(resend(
            &source_engine,
            entries[1].clone(),
            receiver_id.clone(),
        ));
        let b_original = discover(&receiver_engine, 2, b_deadline).await?;
        let b0 = paused(&receiving, b_digest, 0, b_deadline).await?;
        ensure!(held_body(&b0), "B0 actual body gate not reached");
        let b_at_hold = query(&receiver_engine, &b_original.entry_id, b_deadline).await?;
        observe(&mut evidence, "B0", &b_at_hold, &b_original, origin);
        ensure!(
            same_receive(&b_at_hold, &b_original) && b_at_hold.items_completed == 1,
            "B0 prefix not completed before body hold"
        );
        let b_prefix = calls(&receiving, b_prefix_digest);
        ensure!(
            b_prefix.len() == 1
                && b_prefix[0].finished.load(Ordering::SeqCst)
                && b_prefix[0].response.succeeded.load(Ordering::SeqCst)
                && b_prefix[0].validated.load(Ordering::SeqCst) == TAIL_BYTES as u64
                && b_prefix[0].total.load(Ordering::SeqCst) == TAIL_BYTES as u64
                && !b_prefix[0].stopped.load(Ordering::SeqCst),
            "B0 prefix lacks actual SDK success"
        );
        evidence["B0_prefix"] = verify_files(
            &receiver_engine,
            &b_original.entry_id,
            receiver_root.path(),
            &[(0x66, TAIL_BYTES)],
            b_deadline,
        )
        .await?;
        ensure!(
            calls(&receiving, b_digest).len() == 1
                && native.counts() == (2, 2)
                && native.closed_count() == 0,
            "B0 did not use one open native pair"
        );
        let OperationResult::EntryReceiveCancellation(cancelled) = within(
            &receiver_engine,
            Operation::CancelEntryReceive(CancelEntryReceiveInput {
                entry_id: b_original.entry_id.clone(),
                attempt_id: b_original.attempt_id.clone(),
            }),
            b_deadline,
        )
        .await?
        else {
            bail!("B0 cancellation result missing")
        };
        evidence["B0_cancel_result"] = json!(cancelled);
        ensure!(
            matches!(
                cancelled,
                EntryReceiveCancellationOutcome::Cancelled
                    | EntryReceiveCancellationOutcome::CancellationIncomplete
            ),
            "B0 cancellation not accepted"
        );
        finished(&b0, b_deadline).await?;
        ensure!(
            b0.stopped.load(Ordering::SeqCst)
                && *b0.response.stop_succeeded.lock().unwrap() == Some(true)
                && b0
                    .failure
                    .lock()
                    .unwrap()
                    .is_some_and(|(kind, _)| kind == BlobExchangeFailure::Cancelled)
                && b0.response.additional_count.load(Ordering::SeqCst) == 0
                && !b0.response.succeeded.load(Ordering::SeqCst),
            "B0 did not really stop and settle"
        );
        let b_terminal = loop {
            let row = query(&receiver_engine, &b_original.entry_id, b_deadline).await?;
            observe(&mut evidence, "B0", &row, &b_original, origin);
            ensure!(same_receive(&row, &b_original), "B0 identity changed");
            if matches!(row.state.as_str(), "failed" | "cancelled") {
                break row;
            }
            ensure!(
                matches!(row.state.as_str(), "receiving" | "cancelling" | "failing"),
                "B0 unexpected state"
            );
            sleep(Duration::from_millis(10)).await;
        };
        ensure!(
            b_terminal.items_completed == 1
                && b_terminal
                    .items
                    .iter()
                    .filter(|item| item.state == "completed")
                    .count()
                    == 1,
            "B0 prefix completion changed"
        );
        let failed: Vec<_> = b_terminal
            .items
            .iter()
            .filter(|item| matches!(item.state.as_str(), "failed" | "cancelled"))
            .collect();
        ensure!(failed.len() == 1, "B0 large item not eligible");
        let selected_b0_item = failed[0].item_id.clone();
        let b_send_result = timeout_at(
            b_deadline,
            sending_b.as_mut().context("B0 sender task missing")?,
        )
        .await?
        .context("B0 sender task failed")?;
        ensure!(Instant::now() < b_deadline, "B0 sender returned late");
        evidence["B0_sender_returned"] = json!(true);
        evidence["B0_sender_operation_ok"] = json!(b_send_result.is_ok());
        sending_b.take();
        ensure!(
            b0.response
                .stopped_at
                .lock()
                .unwrap()
                .zip(*b0.response.finished_at.lock().unwrap())
                .is_some_and(|(stopped, finished)| stopped <= finished),
            "B0 stop/finish order invalid"
        );
        evidence["B0_prefix_after_cancel"] = verify_files(
            &receiver_engine,
            &b_original.entry_id,
            receiver_root.path(),
            &[(0x66, TAIL_BYTES)],
            b_deadline,
        )
        .await?;
        ensure!(
            native.counts() == (2, 2) && native.closed_count() == 0,
            "B0 cancellation closed the healthy pair"
        );
        current(&source_engine, &source_current, b_deadline).await?;
        evidence["B0_barrier_us"] = json!(micros(origin, Instant::now()));
        evidence["B0_native"] = native_sample(&native, origin);
        stage = "A_header_and_B1_manual_retry_overlap";
        failure_reason = "overlap_precondition_failed";
        receiving.hold_digest_header_once(a_digest);
        let action_at = Instant::now();
        let action_deadline = action_at + Duration::from_secs(60);
        evidence["A_action_us"] = json!(micros(origin, action_at));
        evidence["A_action_deadline_us"] = json!(micros(origin, action_deadline));
        sending_a = Some(resend(
            &source_engine,
            entries[0].clone(),
            receiver_id.clone(),
        ));
        let a_original = discover(&receiver_engine, 2, action_deadline).await?;
        ensure!(
            a_original.entry_id != b_original.entry_id,
            "A replaced B entry"
        );
        let a0 = paused(&receiving, a_digest, 0, action_deadline).await?;
        let a_deadline = action_deadline.min(a0.response.logical_deadline);
        evidence["A_original_logical_deadline_us"] =
            json!(micros(origin, a0.response.logical_deadline));
        evidence["A_effective_deadline_us"] = json!(micros(origin, a_deadline));
        ensure!(
            a0.response.held_before_first_read
                && a0.response.response_timeout == Duration::from_secs(30)
                && a0.read.load(Ordering::SeqCst) == 0
                && a0.validated.load(Ordering::SeqCst) == 0
                && !a0.finished.load(Ordering::SeqCst),
            "A header gate invalid"
        );
        let a_at_hold = query(&receiver_engine, &a_original.entry_id, a_deadline).await?;
        observe(&mut evidence, "A", &a_at_hold, &a_original, origin);
        ensure!(
            same_receive(&a_at_hold, &a_original) && a_at_hold.items_completed == 1,
            "A prefix not completed"
        );
        let a_prefix = calls(&receiving, a_prefix_digest);
        ensure!(
            a_prefix.len() == 1
                && a_prefix[0].finished.load(Ordering::SeqCst)
                && a_prefix[0].response.succeeded.load(Ordering::SeqCst)
                && a_prefix[0].validated.load(Ordering::SeqCst) == TAIL_BYTES as u64
                && a_prefix[0].total.load(Ordering::SeqCst) == TAIL_BYTES as u64
                && !a_prefix[0].stopped.load(Ordering::SeqCst),
            "A prefix lacks actual SDK success"
        );
        evidence["A_prefix"] = verify_files(
            &receiver_engine,
            &a_original.entry_id,
            receiver_root.path(),
            &[(0x55, TAIL_BYTES)],
            a_deadline,
        )
        .await?;
        let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted {
            attempt_id: b1_attempt,
        }) = within(
            &receiver_engine,
            Operation::RetryEntryReceive(RetryEntryReceiveInput {
                entry_id: b_original.entry_id.clone(),
                expected_attempt_id: b_original.attempt_id.clone(),
                item_ids: vec![selected_b0_item],
            }),
            a_deadline,
        )
        .await?
        else {
            bail!("single B1 retry was not Accepted")
        };
        ensure!(
            b1_attempt != b_original.attempt_id,
            "B1 did not change attempt"
        );
        let b1 = paused(&receiving, b_digest, 1, a_deadline).await?;
        let b1_expected = query(&receiver_engine, &b_original.entry_id, a_deadline).await?;
        observe(&mut evidence, "B1", &b1_expected, &b1_expected, origin);
        ensure!(
            b1_expected.entry_id == b_original.entry_id
                && b1_expected.attempt_id == b1_attempt
                && b1_expected.items_total == 2
                && b1_expected.items.len() == 2
                && b1_expected.items_completed == 1
                && b1_expected
                    .items
                    .iter()
                    .all(|item| item.kind == ReceiveItemKind::File
                        && b_original
                            .items
                            .iter()
                            .all(|old| old.item_id != item.item_id)),
            "B1 identity generation invalid"
        );
        ensure!(
            held_body(&b1)
                && a0.paused.load(Ordering::SeqCst)
                && !a0.finished.load(Ordering::SeqCst)
                && a0.validated.load(Ordering::SeqCst) == 0
                && a0.read.load(Ordering::SeqCst) == 0,
            "B1 did not overlap actual A header hold"
        );
        ensure!(
            b1.stream_id != a0.stream_id
                && b1.digest != a0.digest
                && native.counts() == (2, 2)
                && native.closed_count() == 0
                && calls(&receiving, a_digest).len() == 1
                && calls(&receiving, b_digest).len() == 2,
            "overlap not on the same open pair"
        );
        current(&source_engine, &source_current, a_deadline).await?;
        overlap_proved = true;
        evidence["overlap_distinct_digest_stream"] = json!(true);
        evidence["overlap_us"] = json!(micros(origin, Instant::now()));
        evidence["B1_identity"] = json!({ "same_entry_as_B0": true, "new_attempt": true,
            "new_item_ids": true, "selected_old_large_only": true, "accepted_once": true });
        evidence["overlap_native"] = native_sample(&native, origin);
        stage = "A_actual_timeout_B1_held_through_backoff";
        failure_reason = "timeout_or_sibling_isolation_failed";
        finished(&a0, a_deadline).await?;
        let a_finished_at = a0
            .response
            .finished_at
            .lock()
            .unwrap()
            .context("A actual finish time missing")?;
        ensure!(
            a0.response
                .stopped_at
                .lock()
                .unwrap()
                .zip(Some(a_finished_at))
                .is_some_and(|(stopped, finished)| stopped <= finished),
            "A stop/finish order invalid"
        );
        ensure!(
            a0.failure
                .lock()
                .unwrap()
                .is_some_and(|(kind, _)| kind == BlobExchangeFailure::ResponseTimedOut)
                && a0.response.timeout_elapsed.load(Ordering::SeqCst)
                && a0.stopped.load(Ordering::SeqCst)
                && *a0.response.stop_succeeded.lock().unwrap() == Some(true)
                && a0.response.additional_count.load(Ordering::SeqCst) == 0
                && a0.read.load(Ordering::SeqCst) == 0
                && a0.validated.load(Ordering::SeqCst) == 0
                && a_finished_at.duration_since(a0.response.entered_at) >= Duration::from_secs(30),
            "A did not actually timeout stop and complete"
        );
        let backoff_until = a_finished_at + Duration::from_millis(200);
        loop {
            ensure!(
                held_body(&b1)
                    && calls(&receiving, b_digest).len() == 2
                    && calls(&receiving, a_digest).len() == 1
                    && native.counts() == (2, 2)
                    && native.closed_count() == 0,
                "A timeout disturbed held B1 or bypassed old capacity"
            );
            ensure!(
                Instant::now() < a_deadline,
                "backoff exceeded original A deadline"
            );
            if Instant::now() >= backoff_until {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
        evidence["A_timeout_complete_us"] = json!(micros(origin, a_finished_at));
        evidence["backoff_observation_end_us"] = json!(micros(origin, Instant::now()));
        evidence["B1_untouched_through_backoff"] = json!(true);
        evidence["backoff_native"] = native_sample(&native, origin);
        stage = "B1_real_output_settlement_then_A_reacquisition";
        failure_reason = "B1_or_A_settlement_failed";
        b1.release();
        evidence["B1_release_us"] = json!(micros(origin, Instant::now()));
        finished(&b1, a_deadline).await?;
        ensure!(
            b1.response.succeeded.load(Ordering::SeqCst)
                && !b1.stopped.load(Ordering::SeqCst)
                && b1.validated.load(Ordering::SeqCst) == LARGE_BYTES as u64,
            "B1 real SDK did not succeed"
        );
        evidence["B1_SDK_finished_us"] = json!(micros(
            origin,
            b1.response
                .finished_at
                .lock()
                .unwrap()
                .context("B1 finish time missing")?
        ));
        completed(
            &receiver_engine,
            &b1_expected,
            a_deadline,
            &mut evidence,
            "B1",
            origin,
        )
        .await?;
        evidence["B1_readback"] = verify_files(
            &receiver_engine,
            &b1_expected.entry_id,
            receiver_root.path(),
            &[(0x66, TAIL_BYTES), (0x33, LARGE_BYTES)],
            a_deadline,
        )
        .await?;
        ensure!(
            calls(&receiving, b_prefix_digest).len() == 1 && calls(&receiving, b_digest).len() == 2,
            "B1 retried preserved prefix or repeated the large receive"
        );
        evidence["B1_completed"] = json!(true);
        evidence["B1_completed_us"] = json!(micros(origin, Instant::now()));
        evidence["B1_completed_native"] = native_sample(&native, origin);
        let a1 = timeout_at(a_deadline, async {
            loop {
                let found = calls(&receiving, a_digest);
                ensure!(found.len() <= 2, "A exceeded one recovery retry");
                if let Some(call) = found.get(1) {
                    return Ok::<_, anyhow::Error>(call.clone());
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await??;
        evidence["A_retry_entered_us"] = json!(micros(origin, a1.response.entered_at));
        evidence["A_retry_poll_native"] = native_sample(&native, origin);
        evidence["exact_local_owner_release_edge"] = json!("not_observed");
        ensure!(
            a1.digest == a0.digest
                && a1.response.logical_deadline == a0.response.logical_deadline
                && a1.response.entered_at >= backoff_until
                && b1
                    .response
                    .finished_at
                    .lock()
                    .unwrap()
                    .is_some_and(|at| a1.response.entered_at >= at)
                && !a1.response.held_before_first_read,
            "A retry changed content deadline or settled backoff"
        );
        finished(&a1, a_deadline).await?;
        ensure!(
            a1.response.succeeded.load(Ordering::SeqCst)
                && !a1.stopped.load(Ordering::SeqCst)
                && a1.validated.load(Ordering::SeqCst) == LARGE_BYTES as u64
                && a1.total.load(Ordering::SeqCst) == LARGE_BYTES as u64,
            "A retry lacks real SDK success"
        );
        stage = "A_original_item_readback_and_Delivered";
        failure_reason = "A_original_result_failed";
        completed(
            &receiver_engine,
            &a_original,
            a_deadline,
            &mut evidence,
            "A",
            origin,
        )
        .await?;
        evidence["A_readback"] = verify_files(
            &receiver_engine,
            &a_original.entry_id,
            receiver_root.path(),
            &[(0x55, TAIL_BYTES), (0x11, LARGE_BYTES)],
            a_deadline,
        )
        .await?;
        delivered(
            &source_engine,
            &entries[0],
            &receiver_id,
            a_deadline,
            &mut evidence,
            "A",
            origin,
        )
        .await?;
        let a_sent = timeout_at(
            a_deadline,
            sending_a.as_mut().context("A sender task missing")?,
        )
        .await?
        .context("A sender task failed")?
        .context("A original send failed")?;
        ensure!(
            matches!(
                a_sent,
                OperationResult::EntryResent(ResendEntryOutcome::Completed(_))
            ),
            "A original send did not complete"
        );
        sending_a.take();
        current(&source_engine, &source_current, a_deadline).await?;
        ensure!(
            Instant::now() < a_deadline
                && calls(&receiving, a_digest).len() == 2
                && calls(&receiving, b_digest).len() == 2
                && native.counts() == (4, 2)
                && native.closed_count() == 2,
            "A did not normally reacquire exactly one fresh open pair"
        );
        a_passed = true;
        evidence["A_delivered"] = json!(true);
        evidence["A_passed_us"] = json!(micros(origin, Instant::now()));
        evidence["A_original_identity_preserved"] = json!(true);
        evidence["A_terminal_native"] = native_sample(&native, origin);
        stage = "C_uncached_positive_healthy_reuse";
        failure_reason = "healthy_reuse_failed";
        ensure!(
            calls(&receiving, c_digest).is_empty(),
            "C was already fetched"
        );
        let reuse_native = native.counts();
        let reuse_closed = native.closed_count();
        let before_receives = receiving.receives().len();
        let before_provides = provider.provides.lock().unwrap().len();
        let c_started = Instant::now();
        let c_deadline = c_started + Duration::from_secs(30);
        evidence["C_action_us"] = json!(micros(origin, c_started));
        evidence["C_deadline_us"] = json!(micros(origin, c_deadline));
        sending_c = Some(resend(
            &source_engine,
            entries[2].clone(),
            receiver_id.clone(),
        ));
        let c_original = discover_c(
            &receiver_engine,
            &[&a_original.entry_id, &b_original.entry_id],
            c_deadline,
        )
        .await?;
        completed(
            &receiver_engine,
            &c_original,
            c_deadline,
            &mut evidence,
            "C",
            origin,
        )
        .await?;
        evidence["C_readback"] = verify_files(
            &receiver_engine,
            &c_original.entry_id,
            receiver_root.path(),
            &[(0x77, TAIL_BYTES)],
            c_deadline,
        )
        .await?;
        delivered(
            &source_engine,
            &entries[2],
            &receiver_id,
            c_deadline,
            &mut evidence,
            "C",
            origin,
        )
        .await?;
        let c_sent = timeout_at(
            c_deadline,
            sending_c.as_mut().context("C sender task missing")?,
        )
        .await?
        .context("C sender task failed")?
        .context("C send failed")?;
        ensure!(
            matches!(
                c_sent,
                OperationResult::EntryResent(ResendEntryOutcome::Completed(_))
            ),
            "C send did not complete"
        );
        sending_c.take();
        let c_calls = calls(&receiving, c_digest);
        ensure!(c_calls.len() == 1, "C lacks exactly one SDK receive");
        let c = &c_calls[0];
        ensure!(
            c.finished.load(Ordering::SeqCst)
                && c.response.succeeded.load(Ordering::SeqCst)
                && !c.stopped.load(Ordering::SeqCst)
                && c.validated.load(Ordering::SeqCst) == TAIL_BYTES as u64
                && c.total.load(Ordering::SeqCst) == TAIL_BYTES as u64
                && c.read.load(Ordering::SeqCst) > 0,
            "C is not a positive uncached SDK exchange"
        );
        ensure!(
            receiving.receives().len() == before_receives + 1
                && provider.provides.lock().unwrap().len() == before_provides + 1
                && receiving.receives().len() == 7
                && provider.provides.lock().unwrap().len() == 7
                && native.counts() == reuse_native
                && native.closed_count() == reuse_closed,
            "C did not reuse the recovered open pair"
        );
        current(&source_engine, &source_current, c_deadline).await?;
        ensure!(Instant::now() < c_deadline, "C completed late");
        evidence["C_delivered"] = json!(true);
        evidence["C_native"] = native_sample(&native, origin);
        evidence["source_current_unchanged"] = json!(true);
        Ok::<_, anyhow::Error>(())
    };
    // 看门狗只负责失败清理，不改变任何原动作的绝对预算。
    let outcome = timeout(Duration::from_secs(180), scenario).await;
    let passed = matches!(&outcome, Ok(Ok(())));
    evidence["failed_stage"] = if passed { Value::Null } else { json!(stage) };
    evidence["failure_reason"] = if passed {
        Value::Null
    } else if outcome.is_err() {
        json!("scenario_watchdog_elapsed")
    } else {
        json!(failure_reason)
    };
    evidence["verification_deadline_elapsed"] = json!(matches!(&outcome,
        Ok(Err(error)) if error.downcast_ref::<Elapsed>().is_some()));
    evidence["failure_domain"] = json!(if passed {
        "none"
    } else if a_passed {
        "healthy_reuse"
    } else if overlap_proved {
        "concurrent_timeout_or_recovery"
    } else {
        "precondition"
    });
    evidence["overlap_proved"] = json!(overlap_proved);
    evidence["A_passed_before_C"] = json!(a_passed);
    evidence["native_before_cleanup"] = native_sample(&native, origin);
    evidence["exchanges"] = exchange_evidence(&receiving, origin, a_digest, b_digest, c_digest);
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
    let b_joined = cleanup_task(sending_b).await;
    let a_joined = cleanup_task(sending_a).await;
    let c_joined = cleanup_task(sending_c).await;
    evidence["exchanges_after_cleanup"] =
        exchange_evidence(&receiving, origin, a_digest, b_digest, c_digest);
    evidence["provider_exchanges"] = json!(provider.provides.lock().unwrap().len());
    drop(receiver);
    drop(source);
    let receiver_removed = receiver_root.close().is_ok();
    let source_removed = source_root.close().is_ok();
    let live = native.counts().1;
    let cleanup = receiver_closed
        && source_closed
        && b_joined
        && a_joined
        && c_joined
        && receiver_removed
        && source_removed
        && live == 0;
    evidence["cleanup"] = json!({ "receiver_closed": receiver_closed, "source_closed": source_closed,
        "B0_sender_joined": b_joined, "A_sender_joined": a_joined, "C_sender_joined": c_joined,
        "receiver_directory_removed": receiver_removed, "source_directory_removed": source_removed,
        "native_live": live, "passed": cleanup });
    if passed && !cleanup {
        evidence["failure_reason"] = json!("cleanup_failed");
        evidence["failure_domain"] = json!("cleanup");
    }
    evidence["status"] = json!(if passed && cleanup {
        "passed"
    } else {
        "failed"
    });
    let path = std::env::var_os("UC_BLOB_SIBLING_E2E_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/response-sibling-e2e/result.json")
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

async fn cleanup_task(task: Option<JoinHandle<Result<OperationResult, EngineError>>>) -> bool {
    let Some(mut task) = task else {
        return true;
    };
    match timeout(Duration::from_secs(30), &mut task).await {
        Ok(Ok(_)) => true,
        Ok(Err(_)) => false,
        Err(_) => {
            task.abort();
            let _aborted = task.await;
            false
        }
    }
}

fn exchange_evidence(
    streams: &ReceiveStreams,
    origin: Instant,
    a: BlobDigest,
    b: BlobDigest,
    c: BlobDigest,
) -> Value {
    let all = streams.receives();
    let rows: Vec<_> = all.iter().take(64).enumerate().map(|(index, call)| {
        let response = &call.response;
        let alias = if call.digest == a { "A-large" } else if call.digest == b { "B-large" }
            else if call.digest == c { "C" } else if call.digest == digest(0x55, TAIL_BYTES) { "A-prefix" }
            else if call.digest == digest(0x66, TAIL_BYTES) { "B-prefix" } else { "unexpected-content" };
        json!({ "exchange_alias": index + 1, "content_alias": alias,
            "entered_us": micros(origin, response.entered_at), "logical_deadline_us": micros(origin, response.logical_deadline),
            "response_timeout_ms": response.response_timeout.as_millis() as u64,
            "header_held": response.held_before_first_read, "paused": call.paused.load(Ordering::SeqCst),
            "read_bytes": call.read.load(Ordering::SeqCst), "validated_bytes": call.validated.load(Ordering::SeqCst),
            "total_bytes": call.total.load(Ordering::SeqCst), "stopped": call.stopped.load(Ordering::SeqCst),
            "stop_succeeded": *response.stop_succeeded.lock().unwrap(),
            "stopped_us": response.stopped_at.lock().unwrap().map(|at| micros(origin, at)),
            "finished": call.finished.load(Ordering::SeqCst),
            "finished_us": response.finished_at.lock().unwrap().map(|at| micros(origin, at)),
            "succeeded": response.succeeded.load(Ordering::SeqCst), "error_class": *response.error_class.lock().unwrap(),
            "elapsed_source": response.timeout_elapsed.load(Ordering::SeqCst),
            "additional_failures": response.additional_count.load(Ordering::SeqCst) })
    }).collect();
    json!({ "count": all.len(), "truncated": all.len() > 64, "calls": rows })
}
