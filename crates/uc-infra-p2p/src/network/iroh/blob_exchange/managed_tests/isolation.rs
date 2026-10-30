//! 完整Application获取、Uni与实际SDK双流：取消A不得使B重试或重拨。
use super::{ExistingConnection, Members, NoDial};
use crate::network::iroh::admitted_connections::AdmittedConnections;
use crate::network::iroh::blob_exchange::{
    IrohBlobExchangeAdapter, IrohBlobHandler, IrohBlobSourceResolver,
};
use crate::network::iroh::blobs::{IrohBlobTransferAdapter, BLOBS_ALPN};
use crate::network::iroh::connectivity::{
    ConnectivityProfile, IrohConnectionAdmission, IrohPeerConnection,
};
use async_trait::async_trait;
use futures_util::task::AtomicWaker;
use futures_util::StreamExt;
use iroh::endpoint::{presets, QuicTransportConfig, VarInt};
use iroh::{protocol::Router, Endpoint, RelayMode};
use iroh_blobs::store::fs::FsStore;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::Notify;
use tokio::time::{sleep, timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    BlobDownloadResources, BlobExchangeError, BlobExchangePort, BlobReceived, BlobShutdownWait,
    BlobStoreLifecyclePort, BlobTransferDeps, BlobTransport, BlobValidatedProgress,
    ConnectivityError, KnownPeerIdentity, PeerReceiveStream, StreamLease,
};
use uc_application::facade::blob_transfer::{
    BlobTransferError, BlobTransferFacade, FetchBlobToPathCommand,
};
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::blob::{
    BlobDigest, BlobReferenceError, BlobReferenceRepositoryPort, BlobTransferPort, PlaintextHash,
    TagReason,
};
use uc_core::ports::security::{
    IdentityFingerprintFactoryPort, TransferCipherError, TransferCipherPort,
};
use uc_core::ports::ContentHashPort;
use uc_core::ContentHash;
use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;

const WAIT: Duration = Duration::from_secs(10);
#[derive(Default)]
struct Gate {
    entered: Notify,
    claimed: AtomicBool,
    released: AtomicBool,
    waker: AtomicWaker,
    validated: AtomicU64,
}
impl Gate {
    fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        self.waker.wake();
    }
}
struct ReleaseGates([Arc<Gate>; 2]);
impl Drop for ReleaseGates {
    fn drop(&mut self) {
        for gate in &self.0 {
            gate.release();
        }
    }
}
struct GatedRead {
    inner: Box<dyn PeerReceiveStream>,
    read: u64,
    gate: Arc<Gate>,
}
impl AsyncRead for GatedRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.read >= 64 * 1024 && !self.gate.released.load(Ordering::SeqCst) {
            self.gate.waker.register(context.waker());
            if !self.gate.claimed.swap(true, Ordering::SeqCst) {
                self.gate.entered.notify_one();
            }
            if !self.gate.released.load(Ordering::SeqCst) {
                return Poll::Pending;
            }
        }
        let before = buffer.filled().len();
        let result = Pin::new(self.inner.as_mut()).poll_read(context, buffer);
        self.read += (buffer.filled().len() - before) as u64;
        result
    }
}
impl PeerReceiveStream for GatedRead {
    fn id(&self) -> u64 {
        self.inner.id()
    }
    fn stop(&mut self, code: u64) -> Result<(), ConnectivityError> {
        self.inner.stop(code)
    }
}
struct Validated {
    inner: Arc<dyn BlobValidatedProgress>,
    gate: Arc<Gate>,
}
impl BlobValidatedProgress for Validated {
    fn validated_through(&self, end: u64, total: u64) -> Result<(), BlobExchangeError> {
        self.inner.validated_through(end, total)?;
        self.gate.validated.store(end, Ordering::SeqCst);
        Ok(())
    }
}
struct ObservedExchange {
    inner: IrohBlobExchangeAdapter,
    gates: Mutex<VecDeque<Arc<Gate>>>,
    receives: Mutex<Vec<(BlobDigest, u64)>>,
    provides: AtomicUsize,
    provided: AtomicUsize,
}
impl ObservedExchange {
    fn new(store: FsStore) -> Self {
        Self {
            inner: IrohBlobExchangeAdapter::new(store),
            gates: Default::default(),
            receives: Mutex::new(Vec::new()),
            provides: AtomicUsize::new(0),
            provided: AtomicUsize::new(0),
        }
    }
}
#[async_trait]
impl BlobExchangePort for ObservedExchange {
    fn connection_failure(&self, source: ConnectivityError) -> BlobExchangeError {
        self.inner.connection_failure(source)
    }
    async fn receive(
        &self,
        mut stream: StreamLease,
        digest: BlobDigest,
        cancel: CancellationToken,
        progress: Arc<dyn BlobValidatedProgress>,
    ) -> Result<BlobReceived, BlobExchangeError> {
        {
            let mut calls = self.receives.lock().unwrap();
            assert_eq!(stream.send.id(), stream.receive.id());
            calls.push((digest, stream.send.id()));
        }
        let gate = self.gates.lock().unwrap().pop_front();
        if let Some(gate) = gate {
            stream.receive = Box::new(GatedRead {
                inner: stream.receive,
                read: 0,
                gate: gate.clone(),
            });
            self.inner
                .receive(
                    stream,
                    digest,
                    cancel,
                    Arc::new(Validated {
                        inner: progress,
                        gate: gate.clone(),
                    }),
                )
                .await
        } else {
            self.inner.receive(stream, digest, cancel, progress).await
        }
    }
    async fn provide(
        &self,
        stream: StreamLease,
        deadline: Instant,
        cancel: CancellationToken,
    ) -> Result<(), BlobExchangeError> {
        self.provides.fetch_add(1, Ordering::SeqCst);
        let result = self.inner.provide(stream, deadline, cancel).await;
        self.provided.fetch_add(1, Ordering::SeqCst);
        result
    }
}
#[async_trait]
impl BlobStoreLifecyclePort for ObservedExchange {
    async fn shutdown(&self) -> Result<(), BlobExchangeError> {
        self.inner.shutdown().await
    }
}
#[derive(Default)]
struct References(Mutex<HashMap<PlaintextHash, BlobDigest>>);
#[async_trait]
impl BlobReferenceRepositoryPort for References {
    async fn find_by_plaintext_hash(
        &self,
        hash: &PlaintextHash,
    ) -> Result<Option<BlobDigest>, BlobReferenceError> {
        Ok(self.0.lock().unwrap().get(hash).copied())
    }
    async fn save(
        &self,
        hash: PlaintextHash,
        digest: BlobDigest,
    ) -> Result<(), BlobReferenceError> {
        self.0.lock().unwrap().insert(hash, digest);
        Ok(())
    }
    async fn forget(&self, hash: &PlaintextHash) -> Result<(), BlobReferenceError> {
        self.0.lock().unwrap().remove(hash);
        Ok(())
    }
}
struct RejectMemory;
impl ContentHashPort for RejectMemory {
    fn hash_bytes(&self, _: &[u8]) -> anyhow::Result<ContentHash> {
        panic!("file export must not use the memory path")
    }
}
#[async_trait]
impl TransferCipherPort for RejectMemory {
    async fn encrypt(&self, _: &[u8]) -> Result<Vec<u8>, TransferCipherError> {
        panic!("raw file must not enter memory encryption")
    }
    async fn decrypt(&self, _: &[u8]) -> Result<Vec<u8>, TransferCipherError> {
        panic!("raw file must not enter memory decryption")
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn managed_download_cancel_preserves_active_sibling_without_retry() -> anyhow::Result<()> {
    prove(false, 4 * 1024 * 1024, 16).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn managed_download_cancel_preserves_larger_active_sibling_without_retry(
) -> anyhow::Result<()> {
    prove(false, 32 * 1024 * 1024, 3).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn managed_download_same_content_cancel_preserves_other_request_without_retry(
) -> anyhow::Result<()> {
    prove(true, 4 * 1024 * 1024, 8).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn managed_download_large_stream_settles_sdk_tags_and_connection() -> anyhow::Result<()> {
    prove(false, 1024 * 1024 * 1024, 2).await
}

fn write_fixture(path: &Path, byte: u8, size: usize) -> anyhow::Result<()> {
    let mut file = std::fs::File::create(path)?;
    let chunk = [byte; 64 * 1024];
    let mut remaining = size;
    while remaining > 0 {
        let written = remaining.min(chunk.len());
        file.write_all(&chunk[..written])?;
        remaining -= written;
    }
    Ok(())
}

fn verify_fixture(path: &Path, byte: u8, size: usize) -> anyhow::Result<()> {
    let mut file = std::fs::File::open(path)?;
    assert_eq!(file.metadata()?.len(), size as u64);
    let mut chunk = [0; 64 * 1024];
    let mut remaining = size;
    while remaining > 0 {
        let read = remaining.min(chunk.len());
        file.read_exact(&mut chunk[..read])?;
        assert!(
            chunk[..read].iter().all(|actual| *actual == byte),
            "实际文件内容不匹配"
        );
        remaining -= read;
    }
    assert_eq!(file.read(&mut chunk)?, 0);
    Ok(())
}

async fn prove(same_content: bool, size: usize, rounds: usize) -> anyhow::Result<()> {
    let server = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?;
    let client = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .transport_config(
            QuicTransportConfig::builder()
                .stream_receive_window(VarInt::from_u32(64 * 1024))
                .build(),
        )
        .bind()
        .await?;
    let root = tempfile::tempdir()?;
    let source = FsStore::load(root.path().join("source")).await?;
    let destination = FsStore::load(root.path().join("destination")).await?;
    let requester = Arc::new(Members {
        identities: vec![KnownPeerIdentity {
            device_id: DeviceId::new("requester"),
            identity_fingerprint: Sha256IdentityFingerprintFactory
                .from_public_key(client.id().as_bytes())?,
        }],
        admitted: AtomicBool::new(true),
    });
    let provider = Arc::new(ObservedExchange::new(source.clone()));
    let serving = BlobTransport::new(
        Arc::new(NoDial),
        Arc::new(IrohConnectionAdmission::new(
            requester.clone(),
            requester,
            Arc::new(Sha256IdentityFingerprintFactory),
            AdmittedConnections::default(),
            ConnectivityProfile::Blob,
        )),
        provider.clone(),
        provider.clone(),
    );
    let router = Router::builder(server.clone())
        .accept(BLOBS_ALPN, IrohBlobHandler::new(serving.clone()))
        .spawn();
    let source_tags_before = source.tags().list_temp_tags().await?.count().await;
    let destination_tags_before = destination.tags().list_temp_tags().await?.count().await;
    assert_eq!((source_tags_before, destination_tags_before), (0, 0));
    let publishing = IrohBlobTransferAdapter::new(Arc::new(server.clone()), source.clone());
    let native = client.connect(server.addr(), BLOBS_ALPN).await?;
    let native_lifetime = native.weak_handle();
    let backend = Arc::new(ExistingConnection {
        connection: IrohPeerConnection::wrap(native.clone()),
        calls: AtomicUsize::new(0),
    });
    let peers = Arc::new(Members {
        identities: vec![KnownPeerIdentity {
            device_id: DeviceId::new("provider"),
            identity_fingerprint: Sha256IdentityFingerprintFactory
                .from_public_key(server.id().as_bytes())?,
        }],
        admitted: AtomicBool::new(true),
    });
    let receiving = Arc::new(ObservedExchange::new(destination.clone()));
    let downloading = BlobTransport::new(
        backend.clone(),
        Arc::new(IrohConnectionAdmission::new(
            peers.clone(),
            peers.clone(),
            Arc::new(Sha256IdentityFingerprintFactory),
            AdmittedConnections::default(),
            ConnectivityProfile::Blob,
        )),
        receiving.clone(),
        receiving.clone(),
    );
    let local = Arc::new(IrohBlobTransferAdapter::new(
        Arc::new(client.clone()),
        destination.clone(),
    ));
    let references = Arc::new(References::default());
    let facade = Arc::new(BlobTransferFacade::new(BlobTransferDeps {
        downloads: Arc::new(BlobDownloadResources {
            transport: downloading.clone(),
            source: Arc::new(IrohBlobSourceResolver::new(
                peers,
                Arc::new(Sha256IdentityFingerprintFactory),
            )),
            store: Arc::new(IrohBlobExchangeAdapter::new(destination.clone())),
        }),
        hash: Arc::new(RejectMemory),
        blob_transfer: local,
        blob_reference: references.clone(),
        transfer_cipher: Arc::new(RejectMemory),
        host_event_emitter: None,
        file_transfer: None,
    }));
    let mut resource_rounds = Vec::new();
    for round in 0..rounds {
        let offset = round * 2;
        let gates = [Arc::new(Gate::default()), Arc::new(Gate::default())];
        let _release = ReleaseGates(gates.clone());
        {
            let mut planned = receiving.gates.lock().unwrap();
            assert!(planned.is_empty());
            planned.extend(gates.clone());
        }
        let a_input = root.path().join(format!("a-input-{round}"));
        let b_input = root.path().join(format!("b-input-{round}"));
        let a_byte = 41 + round as u8;
        let b_byte = (if same_content { 41 } else { 71 }) + round as u8;
        let generated_a = a_input.clone();
        let generated_b = b_input.clone();
        tokio::task::spawn_blocking(move || {
            write_fixture(&generated_a, a_byte, size)?;
            write_fixture(&generated_b, b_byte, size)
        })
        .await??;
        let first = publishing
            .publish_path(&a_input, TagReason::ClipboardEntry(EntryId::new()))
            .await?;
        let second = publishing
            .publish_path(&b_input, TagReason::ClipboardEntry(EntryId::new()))
            .await?;
        let a_ticket = publishing.issue_ticket(&first).await?;
        let b_ticket = publishing.issue_ticket(&second).await?;
        let a_cancel = CancellationToken::new();
        let a_path = root.path().join(format!("a-output-{round}"));
        let b_path: PathBuf = root.path().join(format!("b-output-{round}"));
        let a = {
            let facade = facade.clone();
            let cancel = a_cancel.clone();
            let path = a_path.clone();
            tokio::spawn(async move {
                facade
                    .fetch_blob_to_path(FetchBlobToPathCommand {
                        cancellation: cancel,
                        ticket: a_ticket,
                        entry_id: EntryId::new(),
                        target_path: path.into(),
                        transfer_context: None,
                    })
                    .await
            })
        };
        timeout(WAIT, gates[0].entered.notified()).await?;
        assert!(gates[0].validated.load(Ordering::SeqCst) > 0);
        let b = {
            let facade = facade.clone();
            let path = b_path.clone();
            tokio::spawn(async move {
                facade
                    .fetch_blob_to_path(FetchBlobToPathCommand {
                        cancellation: CancellationToken::new(),
                        ticket: b_ticket,
                        entry_id: EntryId::new(),
                        target_path: path.into(),
                        transfer_context: None,
                    })
                    .await
            })
        };
        timeout(WAIT, async {
            while receiving.receives.lock().unwrap().len() < offset + 2
                || provider.provides.load(Ordering::SeqCst) < offset + 2
            {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        if !same_content {
            timeout(WAIT, gates[1].entered.notified()).await?;
            let validated = gates[1].validated.load(Ordering::SeqCst);
            assert!(
                validated > 0 && validated < size as u64,
                "B must have verified real partial data before A is cancelled"
            );
        }
        assert!(!b.is_finished());
        let source_tags_active = source.tags().list_temp_tags().await?.count().await;
        let destination_tags_active = destination.tags().list_temp_tags().await?.count().await;
        assert!(
            destination_tags_active > destination_tags_before,
            "真实暂停的下载必须持有SDK临时保护，不能只在关闭后观察空列表"
        );
        assert!(native_lifetime.upgrade().is_some());
        a_cancel.cancel();
        let cancelled = timeout(WAIT, a).await??;
        assert!(
            matches!(cancelled, Err(BlobTransferError::Cancelled)),
            "A must actually settle cancellation"
        );
        assert!(!b.is_finished(), "A cancellation must not finish paused B");
        assert!(
            native.close_reason().is_none(),
            "A cancellation must not close the shared native connection"
        );
        if same_content {
            timeout(WAIT, gates[1].entered.notified()).await?;
        }
        gates[1].release();
        let completion_wait = if size >= 1024 * 1024 * 1024 {
            Duration::from_secs(300)
        } else {
            WAIT
        };
        let completed = timeout(completion_wait, b).await???;
        assert_eq!(completed.bytes_written, size as u64);
        tokio::task::spawn_blocking(move || verify_fixture(&b_path, b_byte, size)).await??;
        assert!(!a_path.exists(), "cancelled A must not export output");
        let calls = receiving.receives.lock().unwrap().clone();
        assert_eq!(
            calls.len(),
            offset + 2,
            "complete Application download owner must not retry B"
        );
        assert_eq!(calls[offset].0, first);
        assert_eq!(calls[offset + 1].0, second);
        assert_ne!(
            calls[offset].1,
            calls[offset + 1].1,
            "two requests must use independent native streams"
        );
        assert_eq!(
            provider.provides.load(Ordering::SeqCst),
            offset + 2,
            "provider must see exactly two physical exchanges"
        );
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            1,
            "B must not redial after A cancellation"
        );
        assert!(native.close_reason().is_none());
        assert_eq!(
            references.0.lock().unwrap().get(&completed.plaintext_hash),
            Some(&second)
        );

        assert_eq!(
            calls
                .iter()
                .map(|call| call.1)
                .collect::<HashSet<_>>()
                .len(),
            calls.len(),
            "all requests must use unique streams on the same connection"
        );
        // 先确认上一轮提供端SDK导出实际结束，再复用同一连接，避免把合法过载误记为取消隔离失败。
        timeout(WAIT, async {
            while provider.provided.load(Ordering::SeqCst) < offset + 2 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        // SDK保护释放经原actor交接；反复读取实际列表，不用包装层调用数冒充tag计数。
        let (source_tags_settled, destination_tags_settled) = timeout(WAIT, async {
            loop {
                let source_count = source.tags().list_temp_tags().await?.count().await;
                let destination_count = destination.tags().list_temp_tags().await?.count().await;
                if source_count == source_tags_before
                    && destination_count == destination_tags_before
                {
                    break Ok::<_, anyhow::Error>((source_count, destination_count));
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        resource_rounds.push(serde_json::json!({
            "source_temp_tags_active": source_tags_active,
            "destination_temp_tags_active": destination_tags_active,
            "source_temp_tags_settled": source_tags_settled,
            "destination_temp_tags_settled": destination_tags_settled,
            "native_connections": usize::from(native_lifetime.upgrade().is_some()),
            "dial_calls": backend.calls.load(Ordering::SeqCst),
            "receive_calls": receiving.receives.lock().unwrap().len(),
            "provide_calls": provider.provides.load(Ordering::SeqCst),
        }));
    }
    timeout(WAIT, downloading.shutdown(BlobShutdownWait::Complete)).await??;
    timeout(WAIT, serving.shutdown(BlobShutdownWait::Complete)).await??;
    assert!(
        native.close_reason().is_some(),
        "完整关闭必须实际关闭原生连接"
    );
    router.shutdown().await?;
    client.close().await;
    drop((
        facade,
        downloading,
        serving,
        backend,
        native,
        router,
        client,
        server,
    ));
    timeout(WAIT, async {
        while native_lifetime.upgrade().is_some() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    if let Some(root) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        let directory = PathBuf::from(root).join("blob-resources");
        assert!(directory.is_absolute(), "资源工件须由驱动提供绝对目录");
        std::fs::create_dir_all(&directory)?;
        let evidence = serde_json::json!({
            "same_content": same_content, "bytes_per_file": size,
            "completed_rounds": rounds, "rounds": resource_rounds,
            "source_temp_tags_initial": source_tags_before,
            "destination_temp_tags_initial": destination_tags_before,
            "native_connections_after_shutdown": usize::from(native_lifetime.upgrade().is_some()),
            "tags_verified": true, "native_connection_release_verified": true,
            "full_engine_resources_verified": false, "quota_counters_verified": false,
        });
        std::fs::write(
            directory.join(format!(
                "managed-isolation-{same_content}-{size}-{rounds}.json"
            )),
            serde_json::to_vec_pretty(&evidence)?,
        )?;
    }
    Ok(())
}
