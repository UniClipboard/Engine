//! 阶段 0 的迁移前真实流式资源基线；只输出计数，不保存内容或原始路径。
#![cfg(target_os = "linux")]

use anyhow::{Context, Result};
use futures_util::StreamExt;
use iroh::endpoint::Connection;
use iroh::{
    address_lookup::memory::MemoryLookup,
    protocol::{AcceptError, ProtocolHandler, Router},
    Endpoint, RelayMode,
};
use iroh_blobs::{store::fs::FsStore, BlobsProtocol};
use serde_json::json;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::time::timeout;
use uc_core::ids::EntryId;
use uc_core::ports::blob::{BlobTransferPort, TagReason};
use uc_infra_p2p::network::iroh::IrohBlobTransferAdapter;

const FILE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Copy)]
struct Resources {
    rss_kib: u64,
    peak_rss_kib: u64,
    fds: usize,
    threads: u64,
    runtime_tasks: usize,
}
fn resources() -> Result<Resources> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    let field = |key: &str| -> Result<u64> {
        status
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .and_then(|value| value.split_whitespace().next())
            .context("resource field unavailable")?
            .parse()
            .context("parse resource count")
    };
    Ok(Resources {
        rss_kib: field("VmRSS:")?,
        peak_rss_kib: field("VmHWM:")?,
        fds: std::fs::read_dir("/proc/self/fd")?.count(),
        threads: field("Threads:")?,
        runtime_tasks: tokio::runtime::Handle::current()
            .metrics()
            .num_alive_tasks(),
    })
}
#[derive(Debug)]
struct ActiveConnection(Arc<AtomicUsize>);
impl Drop for ActiveConnection {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
#[derive(Clone, Debug)]
struct CountedProvider {
    protocol: BlobsProtocol,
    active: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}
impl ProtocolHandler for CountedProvider {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let count = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(count, Ordering::SeqCst);
        self.total.fetch_add(1, Ordering::SeqCst);
        let _active = ActiveConnection(self.active.clone());
        self.protocol.accept(connection).await
    }
    async fn shutdown(&self) {
        self.protocol.shutdown().await;
    }
}
async fn tag_count(store: &FsStore) -> Result<usize> {
    let tags = store.tags().list().await?;
    tokio::pin!(tags);
    let mut count = 0;
    while let Some(tag) = tags.next().await {
        tag?;
        count += 1;
    }
    Ok(count)
}
fn temp_files(path: &Path) -> Result<usize> {
    if !path.exists() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        count += if entry.file_type()?.is_dir() {
            temp_files(&entry.path())?
        } else {
            1
        };
    }
    Ok(count)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blob_streaming_one_gib_resource_baseline() -> Result<()> {
    let initial = resources()?;
    let root = tempfile::tempdir()?;
    let source_file = root.path().join("source.bin");
    File::create(&source_file)?.set_len(FILE_BYTES)?;
    let source_root = root.path().join("provider-store");
    let receive_root = root.path().join("receiver-store");
    let source_store = FsStore::load(&source_root).await?;
    let receive_store = FsStore::load(&receive_root).await?;
    let active = Arc::new(AtomicUsize::new(0));
    let total = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let source_endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?;
    let discovery = MemoryLookup::new();
    let receive_endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
        .relay_mode(RelayMode::Disabled)
        .address_lookup(discovery.clone())
        .bind()
        .await?;
    timeout(Duration::from_secs(5), async {
        while source_endpoint.addr().ip_addrs().next().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    discovery.add_endpoint_info(source_endpoint.addr());
    let router = Router::builder(source_endpoint.clone())
        .accept(
            iroh_blobs::ALPN,
            CountedProvider {
                protocol: BlobsProtocol::new(&source_store, None),
                active: active.clone(),
                total: total.clone(),
                peak: peak.clone(),
            },
        )
        .spawn();
    let publishing = IrohBlobTransferAdapter::new(Arc::new(source_endpoint), source_store.clone());
    let fetching =
        IrohBlobTransferAdapter::new(Arc::new(receive_endpoint.clone()), receive_store.clone());
    let prepared = resources()?;
    let source_reason = TagReason::ClipboardEntry(EntryId::from("baseline-source"));
    let receive_reason = TagReason::ClipboardEntry(EntryId::from("baseline-receiver"));
    let digest = timeout(
        Duration::from_secs(180),
        publishing.publish_path(&source_file, source_reason.clone()),
    )
    .await??;
    let ticket = publishing.issue_ticket(&digest).await?;
    let target = root.path().join("received.bin");
    let received = timeout(
        Duration::from_secs(180),
        fetching.fetch_to_path(&ticket, &target, None),
    )
    .await??;
    assert_eq!(received, digest);
    fetching.tag(&digest, receive_reason.clone()).await?;
    assert_eq!(std::fs::metadata(&target)?.len(), FILE_BYTES);
    // 独立于 SDK 的逐块精确比较，不把整个文件读入内存。
    let mut input = File::open(&target)?;
    let mut buffer = vec![0; 1024 * 1024];
    let mut read = 0u64;
    loop {
        let size = input.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        assert!(buffer[..size].iter().all(|byte| *byte == 0));
        read += size as u64;
    }
    assert_eq!(read, FILE_BYTES);
    drop(input);
    drop(buffer);
    let completed = resources()?;
    let source_tags = tag_count(&source_store).await?;
    let receiver_tags = tag_count(&receive_store).await?;
    assert_eq!(source_tags, 1);
    assert_eq!(receiver_tags, 1);
    publishing.untag(source_reason).await?;
    fetching.untag(receive_reason).await?;
    assert_eq!(tag_count(&source_store).await?, 0);
    assert_eq!(tag_count(&receive_store).await?, 0);
    source_store.wait_idle().await?;
    receive_store.wait_idle().await?;
    let temporary_files =
        temp_files(&source_root.join("temp"))? + temp_files(&receive_root.join("temp"))?;
    assert_eq!(temporary_files, 0);
    assert!(
        completed.peak_rss_kib.saturating_sub(initial.peak_rss_kib) * 1024 < FILE_BYTES / 2,
        "one GiB transfer must not allocate an entire-file buffer"
    );
    drop(publishing);
    drop(fetching);
    receive_endpoint.close().await;
    router.shutdown().await?;
    receive_store.shutdown().await?;
    drop(source_store);
    drop(receive_store);
    drop(router);
    timeout(Duration::from_secs(5), async {
        while active.load(Ordering::SeqCst) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let closed = resources()?;
    let evidence = json!({
        "bytes": FILE_BYTES, "exact_bytes": true, "alpn_unchanged": true, "relay_enabled": false,
        "rss_kib_before": initial.rss_kib, "rss_kib_prepared": prepared.rss_kib, "rss_kib_complete": completed.rss_kib,
        "peak_rss_kib": completed.peak_rss_kib, "rss_kib_closed": closed.rss_kib,
        "fds_before": initial.fds, "fds_prepared": prepared.fds, "fds_complete": completed.fds, "fds_closed": closed.fds,
        "threads_before": initial.threads, "threads_prepared": prepared.threads, "threads_complete": completed.threads, "threads_closed": closed.threads,
        "local_runtime_tasks_before": initial.runtime_tasks, "local_runtime_tasks_complete": completed.runtime_tasks, "local_runtime_tasks_closed": closed.runtime_tasks,
        "provider_connections_total": total.load(Ordering::SeqCst), "provider_connections_peak": peak.load(Ordering::SeqCst), "provider_connections_closed": active.load(Ordering::SeqCst),
        "provider_tags_complete": source_tags, "receiver_tags_complete": receiver_tags,
        "tags_released": true, "sdk_temporary_files_after_settlement": temporary_files,
    });
    if let Some(path) = std::env::var_os("UC_BLOB_RESOURCE_EVIDENCE") {
        std::fs::write(path, serde_json::to_vec_pretty(&evidence)?)?;
    }
    Ok(())
}
