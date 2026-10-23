//! 受管Engine的真实大文件流；工件仅含计数，不含内容、身份或路径。
#![cfg(target_os = "linux")]
use super::receive_retry::member_id;
use super::{
    empty_engine_host, mount_engine_rendezvous, MemoryHostSecureStorage, StaticHostClipboard,
    ENGINE_TEST_LOCK,
};
use crate::{
    BlobResourceInput, CreateSpaceInput, Engine, EngineConfig, EngineEvent,
    EntryReceiveProgressInput, HistoryEntryInput, HostCapabilities, HostCapabilityError,
    HostCapabilityErrorCategory, HostClipboardSnapshot, HostDirectories, HostFileAccess,
    HostFileHandle, HostFileMetadata, JoinSpaceInput, Operation, OperationResult, SecretString,
    SendFilesInput, SettingsPatch, SyncSettingsPatch,
};
use anyhow::{anyhow, Context, Error as SourceError, Result};
use rayon::current_num_threads;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout};
use url::Url;
use wiremock::MockServer;

const FILE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_CHUNK: usize = 128 * 1024;

#[derive(Default)]
struct GeneratedFiles {
    max_returned: AtomicUsize,
    reads: AtomicUsize,
}
impl HostFileAccess for Arc<GeneratedFiles> {
    fn metadata(
        &self,
        _: &HostFileHandle,
    ) -> std::result::Result<HostFileMetadata, HostCapabilityError> {
        Ok(HostFileMetadata {
            display_name: "resource-fixture.bin".into(),
            size_bytes: FILE_BYTES,
            mime_type: Some("application/octet-stream".into()),
        })
    }
    fn read_chunk(
        &self,
        handle: &HostFileHandle,
        offset: u64,
        max_bytes: u32,
    ) -> std::result::Result<Vec<u8>, HostCapabilityError> {
        let pair = match handle.as_str() {
            "0" => [0x36, 0xa9],
            "1" => [0xc2, 0x5d],
            _ => return Err(unavailable()),
        };
        let len =
            (FILE_BYTES.saturating_sub(offset).min(u64::from(max_bytes)) as usize).min(MAX_CHUNK);
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.max_returned.fetch_max(len, Ordering::SeqCst);
        let pair = if offset % 2 == 0 {
            pair
        } else {
            [pair[1], pair[0]]
        };
        let mut bytes = pair.repeat(len.div_ceil(2));
        bytes.truncate(len);
        Ok(bytes)
    }
    fn write_chunk(
        &self,
        _: &HostFileHandle,
        _: u64,
        _: &[u8],
    ) -> std::result::Result<(), HostCapabilityError> {
        Err(unavailable())
    }
    fn finish_write(&self, _: &HostFileHandle) -> std::result::Result<(), HostCapabilityError> {
        Err(unavailable())
    }
}
fn unavailable() -> HostCapabilityError {
    HostCapabilityError::new(
        HostCapabilityErrorCategory::Unavailable,
        "fixture file unavailable",
    )
}

#[derive(Clone, Copy, Default, Serialize)]
struct Resources {
    rss_kib: u64,
    peak_rss_kib: u64,
    fds: usize,
    threads: u64,
    runtime_tasks: usize,
}
impl Resources {
    fn read() -> Result<Self> {
        let status = std::fs::read_to_string("/proc/self/status")?;
        let field = |key: &str| -> Result<u64> {
            status
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .and_then(|value| value.split_whitespace().next())
                .context("resource field missing")?
                .parse()
                .context("resource field invalid")
        };
        Ok(Self {
            rss_kib: field("VmRSS:")?,
            peak_rss_kib: field("VmHWM:")?,
            fds: std::fs::read_dir("/proc/self/fd")?.count(),
            threads: field("Threads:")?,
            runtime_tasks: tokio::runtime::Handle::current()
                .metrics()
                .num_alive_tasks(),
        })
    }
    fn include(&mut self, value: Self) {
        self.rss_kib = self.rss_kib.max(value.rss_kib);
        self.peak_rss_kib = self.peak_rss_kib.max(value.peak_rss_kib);
        self.fds = self.fds.max(value.fds);
        self.threads = self.threads.max(value.threads);
        self.runtime_tasks = self.runtime_tasks.max(value.runtime_tasks);
    }
}
async fn saved_path(receiver: &Engine, entry: &str, root: &Path) -> Result<PathBuf> {
    let OperationResult::HistoryEntryResource(resource) = receiver
        .execute(Operation::GetHistoryEntryResource(HistoryEntryInput {
            entry_id: entry.into(),
        }))
        .await?
    else {
        return Err(anyhow!("file resource missing"));
    };
    assert!(
        (0..64 * 1024).contains(&resource.size_bytes),
        "只允许读取小型URI表示"
    );
    let bytes = match (resource.inline_data, resource.blob_id) {
        (Some(bytes), _) => bytes,
        (None, Some(blob_id)) => {
            let OperationResult::BlobRead(blob) = receiver
                .execute(Operation::ReadBlob(BlobResourceInput { blob_id }))
                .await?
            else {
                return Err(anyhow!("URI representation missing"));
            };
            blob.bytes
        }
        _ => return Err(anyhow!("URI representation absent")),
    };
    assert!(
        bytes.len() < 64 * 1024,
        "只读取小URI表示，不能把文件内容读入内存"
    );
    let text = std::str::from_utf8(&bytes)?;
    let uri = Url::parse(
        text.lines()
            .find(|line| !line.is_empty())
            .context("file URI absent")?,
    )?;
    let path = uri
        .to_file_path()
        .map_err(|_| anyhow!("not a local output"))?
        .canonicalize()?;
    assert!(path.starts_with(root.canonicalize()?));
    Ok(path)
}
fn verify_file(path: &Path, round: usize) -> Result<()> {
    let mut file = File::open(path)?;
    assert_eq!(file.metadata()?.len(), FILE_BYTES);
    // 独立的固定期望字节；不调用Host生成实现，也不读取整份大文件。
    let expected = match round {
        0 => [0x36, 0xa9].repeat(32 * 1024),
        _ => [0xc2, 0x5d].repeat(32 * 1024),
    };
    let mut bytes = [0; 64 * 1024];
    for offset in (0..FILE_BYTES).step_by(bytes.len()) {
        file.read_exact(&mut bytes)?;
        assert!(
            bytes.as_slice() == expected.as_slice(),
            "大文件字节不匹配，块偏移{offset}"
        );
    }
    assert_eq!(file.read(&mut bytes)?, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_engine_two_one_gib_files_stream_with_bounded_memory_and_settle_fd_and_tasks(
) -> Result<()> {
    run_managed_resources(true).await
}

// 同一真实配对接缝验证关闭后的 SDK 私有线程，不依赖大文件传输。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_engine_pair_shutdown_waits_for_sdk_runtimes() -> Result<()> {
    run_managed_resources(false).await
}

fn thread_groups() -> Result<BTreeMap<&'static str, usize>> {
    let mut groups = BTreeMap::new();
    for entry in std::fs::read_dir("/proc/self/task")? {
        let name = match std::fs::read_to_string(entry?.path().join("comm")) {
            Ok(name) => name,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    || error.raw_os_error() == Some(3) =>
            {
                continue
            }
            Err(error) => return Err(error.into()),
        };
        let kind = if name.starts_with("tokio") {
            "tokio"
        } else if name.starts_with("r2d2") {
            "sqlite_pool"
        } else if name.starts_with("iroh") {
            "iroh"
        } else if name.starts_with("tantivy") || name.starts_with("thrd-tantivy") {
            "search"
        } else if name.starts_with("rayon") {
            "rayon"
        } else {
            "other"
        };
        *groups.entry(kind).or_default() += 1;
    }
    Ok(groups)
}

async fn run_managed_resources(transfer_files: bool) -> Result<()> {
    let _guard = ENGINE_TEST_LOCK.lock().await;
    let before_shared_cpu_pool = Resources::read()?;
    // 关闭断点的真实堆栈已确认：未命名 Rayon worker 会继承创建线程的名称。
    // 只预热进程级共享池，保留冷计数；不得预先创建 Engine 或 SDK Store。
    let shared_cpu_pool_threads = current_num_threads();
    let server = MockServer::start().await;
    mount_engine_rendezvous(&server).await;
    let initial = Resources::read()?;
    let (stop, mut stopped) = oneshot::channel();
    let sampler = tokio::spawn(async move {
        let mut peak = Resources::read()?;
        loop {
            tokio::select! { _ = &mut stopped => return Ok::<_, anyhow::Error>(peak), _ = sleep(Duration::from_millis(50)) => peak.include(Resources::read()?) }
        }
    });
    let source_root = tempfile::tempdir()?;
    let receiver_root = tempfile::tempdir()?;
    let files = Arc::new(GeneratedFiles::default());
    let host = HostCapabilities::new(
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
        Box::new(files.clone()),
    );
    let config = EngineConfig::new("1.2.3")
        .with_rendezvous_base_url(server.uri())
        .with_test_relay_fallback(false);
    let (source, _source_events) = Engine::start(config.clone(), host).await?;
    let (receiver, mut events) =
        Engine::start(config, empty_engine_host(receiver_root.path())).await?;
    source
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("Source".into()),
            passphrase: SecretString::new("resource fixture"),
            passphrase_confirmation: SecretString::new("resource fixture"),
        }))
        .await?;
    let OperationResult::InvitationIssued {
        invitation_code, ..
    } = source.execute(Operation::IssueInvitation).await?
    else {
        return Err(anyhow!("invitation absent"));
    };
    receiver
        .execute(Operation::JoinSpace(JoinSpaceInput {
            invitation_code,
            device_name: Some("Receiver".into()),
            passphrase: SecretString::new("resource fixture"),
            preserve_unreadable_history: false,
        }))
        .await?;
    let receiver_id = member_id(&receiver).await;
    member_id(&source).await;
    for (engine, automatic) in [(&source, true), (&receiver, false)] {
        engine
            .execute(Operation::UpdateSettings(Box::new(SettingsPatch {
                sync: Some(SyncSettingsPatch {
                    auto_sync_enabled: Some(automatic),
                    sync_on_restore: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            })))
            .await?;
    }
    let prepared = Resources::read()?;
    let mut rounds = Vec::new();
    let outcome: Result<()> = timeout(Duration::from_secs(480), async {
        for round in 0..if transfer_files { 2 } else { 0 } {
            let sent = source
                .execute(Operation::SendFiles(SendFilesInput {
                    files: vec![HostFileHandle::new(round.to_string())],
                    target_devices: vec![receiver_id.clone()],
                }))
                .await?;
            assert!(matches!(sent, OperationResult::EntrySent(_)));
            let entry = timeout(Duration::from_secs(180), async {
                loop {
                    match events.next().await {
                        Some(EngineEvent::IncomingPending(pending)) => {
                            break Ok::<_, anyhow::Error>(pending.entry_id)
                        }
                        Some(_) => {}
                        None => break Err(anyhow!("receive event stream closed")),
                    }
                }
            })
            .await??;
            timeout(Duration::from_secs(180), async {
                loop {
                    if let OperationResult::EntryReceiveProgress(Some(progress)) = receiver
                        .execute(Operation::QueryEntryReceiveProgress(
                            EntryReceiveProgressInput {
                                entry_id: entry.clone(),
                            },
                        ))
                        .await?
                    {
                        if progress.state == "completed" {
                            assert_eq!(progress.items_completed, 1);
                            break Ok::<_, anyhow::Error>(());
                        }
                        assert!(
                            !matches!(progress.state.as_str(), "failed" | "cancelled"),
                            "大文件接收失败：{progress:?}"
                        );
                    }
                    sleep(Duration::from_millis(100)).await;
                }
            })
            .await??;
            let path = saved_path(&receiver, &entry, receiver_root.path()).await?;
            tokio::task::spawn_blocking(move || verify_file(&path, round)).await??;
            rounds.push(Resources::read()?);
        }
        Ok(())
    })
    .await
    .unwrap_or_else(|source| Err(SourceError::new(source).context("large file scenario deadline")));
    receiver.shutdown_until_complete().await?;
    source.shutdown_until_complete().await?;
    drop((receiver, source));
    // 关闭后的框架驱动允许一次短交接；不把长期泄漏当作基线增长放宽。
    let settled = timeout(Duration::from_secs(20), async {
        loop {
            let value = Resources::read()?;
            if value.fds <= initial.fds + 8 && value.runtime_tasks <= initial.runtime_tasks + 4 {
                break Ok::<_, anyhow::Error>(value);
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    let closed = Resources::read()?;
    // 实测框架空闲退出接近30秒，预留调度余量；资源上限与SDK独立退出期限不放宽。
    let idle_started = Instant::now();
    let idle_settled = timeout(Duration::from_secs(45), async {
        loop {
            let value = Resources::read()?;
            if value.threads <= initial.threads
                && value.runtime_tasks <= initial.runtime_tasks + 1
                && value.fds <= initial.fds + 8
            {
                break Ok::<_, anyhow::Error>(value);
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    let idle_ms = idle_started.elapsed().as_millis();
    let closed_after_idle = Resources::read()?;
    let _ = stop.send(());
    let peaks = sampler.await??;
    let evidence = serde_json::json!({ "bytes_per_file": FILE_BYTES, "completed_rounds": rounds.len(), "exact_bytes": transfer_files && outcome.is_ok(), "source_max_chunk_bytes": files.max_returned.load(Ordering::SeqCst), "source_reads": files.reads.load(Ordering::SeqCst), "before_shared_cpu_pool": before_shared_cpu_pool, "shared_cpu_pool_threads": shared_cpu_pool_threads, "initial": initial, "prepared": prepared, "rounds": rounds, "peaks": peaks, "closed": closed, "tags_verified": false, "quota_verified": false, "native_blob_connections_verified": false, "sampler_included_in_closed_tasks": true, "file_transfer_enabled": transfer_files, "thread_groups_after_idle": thread_groups()?, "thread_return_to_initial_verified": matches!(&idle_settled, Ok(Ok(_))), "tasks_without_sampler_return_to_initial_verified": closed_after_idle.runtime_tasks <= initial.runtime_tasks + 1, "closed_after_idle": closed_after_idle, "idle_settlement_ms": idle_ms });
    let evidence_path = std::env::var_os("UC_MANAGED_BLOB_RESOURCE_EVIDENCE")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("UC_TEST_ARTIFACTS_DIR")
                .map(|root| PathBuf::from(root).join("blob-resources/managed-engine.json"))
        });
    if let Some(path) = evidence_path {
        assert!(
            path.is_absolute(),
            "工件必须使用驱动提供的绝对路径，避免crate工作目录歧义"
        );
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(&evidence)?)?;
    }
    outcome?;
    assert_eq!(rounds.len(), if transfer_files { 2 } else { 0 });
    assert!(
        matches!(settled, Ok(Ok(_))),
        "关闭后进程FD或任务未回到原预算，详见计数工件"
    );
    assert!(
        peaks.peak_rss_kib.saturating_sub(prepared.peak_rss_kib) * 1024 < FILE_BYTES / 2,
        "大文件路径不能出现整文件内存增长"
    );
    assert!(
        matches!(idle_settled, Ok(Ok(_))),
        "空闲保留期限结束后线程或任务未回初值，详见计数工件"
    );
    if transfer_files {
        assert!(rounds[1].runtime_tasks <= rounds[0].runtime_tasks + 4);
        assert!(rounds[1].fds <= rounds[0].fds + 4);
        assert!(rounds[1].threads <= rounds[0].threads + 2);
        assert!(rounds[1].rss_kib <= rounds[0].rss_kib + 64 * 1024);
    }
    assert!(files.max_returned.load(Ordering::SeqCst) <= MAX_CHUNK);
    Ok(())
}
