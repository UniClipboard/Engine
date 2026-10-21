//! 第七片阶段 0：固定 SDK 的失败返回不等于该次 store 工作已经退出。
//!
//! 高层缺口保留 SDK 原路径；替代证明调用唯一 Infra 交换实现，使用测试专属 tracing gate。
//! 成功表示复现了前置阻塞，不表示受管传输合同通过。

#[path = "support/blob_leases.rs"]
mod blob_leases;

use std::error::Error;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use iroh::endpoint::{RecvStream, VarInt};
use iroh::{protocol::Router, Endpoint, RelayMode};
use iroh_blobs::{
    api::remote::GetProgressItem, get::StreamPair, protocol::GetRequest, store::fs::FsStore,
    util::RecvStream as BlobReceiveStream, BlobsProtocol,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{field::Visit, Event, Subscriber};
use tracing_subscriber::{layer::Context, prelude::*, Layer};
use uc_application::deps::{BlobExchangeFailure, BlobExchangePort};
use uc_core::ports::blob::BlobDigest;
use uc_infra_p2p::network::iroh::blob_exchange::IrohBlobExchangeAdapter;

const DEADLINE: Duration = Duration::from_secs(10);

#[derive(Default)]
struct ImportGate {
    entered: Notify,
    released: Mutex<bool>,
    wake: Condvar,
    claimed: AtomicBool,
    timed_out: AtomicBool,
    export: AtomicBool,
    panic_export: AtomicBool,
}

impl ImportGate {
    fn release(&self) {
        *self.released.lock().expect("gate lock") = true;
        self.wake.notify_all();
    }
}

struct ReleaseOnDrop(Arc<ImportGate>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct ImportLayer(Arc<ImportGate>);
#[derive(Default)]
struct ImportMessage(bool, bool);
impl Visit for ImportMessage {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            // 只匹配固定 SDK 调用点，不保存或输出包含摘要的第三方消息。
            self.0 = format!("{value:?}").starts_with(if self.1 {
                "exporting bao:"
            } else {
                "importing bao:"
            });
        }
    }
}
impl<S: Subscriber> Layer<S> for ImportLayer {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() != "iroh_blobs::store::fs" {
            return;
        }
        let mut message = ImportMessage(false, self.0.export.load(Ordering::SeqCst));
        event.record(&mut message);
        if !message.0 || self.0.claimed.swap(true, Ordering::SeqCst) {
            return;
        }
        if self.0.panic_export.swap(false, Ordering::SeqCst) {
            panic!("synthetic SDK export worker failure");
        }
        let released = self.0.released.lock().expect("gate lock");
        self.0.entered.notify_one();
        let (_released, result) = self
            .0
            .wake
            .wait_timeout_while(released, DEADLINE, |released| !*released)
            .expect("gate wait");
        self.0.timed_out.store(result.timed_out(), Ordering::SeqCst);
    }
}

/// 第一次有效进度后注入具体连接重置，令 SDK 自己返回错误，而非 abort 外层任务。
struct FailReceive {
    inner: RecvStream,
    fail: Arc<AtomicBool>,
}
impl FailReceive {
    fn check(&self) -> io::Result<()> {
        if self.fail.load(Ordering::SeqCst) {
            Err(io::ErrorKind::ConnectionReset.into())
        } else {
            Ok(())
        }
    }
}
impl BlobReceiveStream for FailReceive {
    async fn recv_bytes(&mut self, len: usize) -> io::Result<Bytes> {
        self.check()?;
        BlobReceiveStream::recv_bytes(&mut self.inner, len).await
    }
    async fn recv_bytes_exact(&mut self, len: usize) -> io::Result<Bytes> {
        self.check()?;
        BlobReceiveStream::recv_bytes_exact(&mut self.inner, len).await
    }
    async fn recv_exact(&mut self, target: &mut [u8]) -> io::Result<()> {
        self.check()?;
        BlobReceiveStream::recv_exact(&mut self.inner, target).await
    }
    fn stop(&mut self, code: VarInt) -> io::Result<()> {
        BlobReceiveStream::stop(&mut self.inner, code)
    }
    fn id(&self) -> u64 {
        BlobReceiveStream::id(&self.inner)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sdk_get_error_returns_before_import_worker_finishes() -> anyhow::Result<()> {
    let gate = Arc::new(ImportGate::default());
    let _release = ReleaseOnDrop(gate.clone());
    // 独立探针二进制只有此测试；SDK 的 store 使用自己的运行时，需要全局测试 subscriber。
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(ImportLayer(gate.clone())),
    )?;
    let temp = tempfile::tempdir()?;
    let provider_store = FsStore::load(temp.path().join("provider")).await?;
    let receiver_store = FsStore::load(temp.path().join("receiver")).await?;
    let provider_endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?;
    let router = Router::builder(provider_endpoint)
        .accept(iroh_blobs::ALPN, BlobsProtocol::new(&provider_store, None))
        .spawn();
    let receiver = Endpoint::builder(iroh::endpoint::presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?;
    timeout(DEADLINE, async {
        while router.endpoint().addr().ip_addrs().next().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let payload: Vec<u8> = (0..1024 * 1024).map(|index| (index % 251) as u8).collect();
    let tag = provider_store.blobs().add_bytes(payload).await?;
    let connection = timeout(
        DEADLINE,
        receiver.connect(router.endpoint().addr(), iroh_blobs::ALPN),
    )
    .await??;
    let (send, receive) = connection.open_bi().await?;
    let fail = Arc::new(AtomicBool::new(false));
    let pair = StreamPair::new(
        connection.stable_id() as u64,
        FailReceive {
            inner: receive,
            fail: fail.clone(),
        },
        send,
    );
    let progress = receiver_store
        .remote()
        .execute_get(pair, GetRequest::blob(tag.hash))
        .stream();
    tokio::pin!(progress);
    let first = timeout(DEADLINE, progress.next()).await?;
    assert!(matches!(first, Some(GetProgressItem::Progress(_))));
    timeout(DEADLINE, gate.entered.notified()).await?;
    fail.store(true, Ordering::SeqCst);
    timeout(DEADLINE, async {
        loop {
            match progress.next().await {
                Some(GetProgressItem::Progress(_)) => {}
                Some(GetProgressItem::Error(_)) => break,
                other => panic!("expected SDK network failure, got {other:?}"),
            }
        }
    })
    .await?;
    // 网络 future 已自行返回失败；对应 import 仍停在 store 独立运行时内。
    assert!(!*gate.released.lock().expect("gate lock"));
    assert!(!gate.timed_out.load(Ordering::SeqCst));
    let idle = receiver_store.wait_idle();
    tokio::pin!(idle);
    assert!(timeout(Duration::from_millis(100), &mut idle)
        .await
        .is_err());

    gate.release();
    timeout(DEADLINE, idle).await??;
    assert!(!gate.timed_out.load(Ordering::SeqCst));
    assert!(connection.close_reason().is_none());
    // 冻结 SDK 的底层 FSM 可保留这一次 import 的完成权。
    *gate.released.lock().expect("gate lock") = false;
    gate.claimed.store(false, Ordering::SeqCst);
    let (managed_downloads, managed_connection) =
        blob_leases::lease(connection.clone(), false).await?;
    let stream = managed_connection
        .open_bi(Instant::now() + DEADLINE)
        .await?;
    let cancel = CancellationToken::new();
    let ready = Arc::new(Notify::new());
    let adapter = IrohBlobExchangeAdapter::new(receiver_store.clone());
    let stop = cancel.clone();
    let data_ready = ready.clone();
    let mut managed = tokio::spawn(async move {
        adapter
            .receive(
                stream,
                BlobDigest::from_bytes(*tag.hash.as_bytes()),
                stop,
                Arc::new(blob_leases::Progress(data_ready)),
            )
            .await
    });
    timeout(DEADLINE, gate.entered.notified()).await?;
    timeout(DEADLINE, ready.notified()).await?;
    cancel.cancel();
    assert!(timeout(Duration::from_millis(100), &mut managed)
        .await
        .is_err());

    // 同一连接上的另一项仍正常获取和读回，不被取消项或全局空闲等待阻塞。
    let sibling = b"independent-sibling";
    let sibling_tag = provider_store.blobs().add_bytes(sibling.as_slice()).await?;
    let (send, receive) = connection.open_bi().await?;
    timeout(
        DEADLINE,
        receiver_store.remote().execute_get(
            StreamPair::new(connection.stable_id() as u64, receive, send),
            GetRequest::blob(sibling_tag.hash),
        ),
    )
    .await??;
    assert_eq!(
        receiver_store
            .blobs()
            .get_bytes(sibling_tag.hash)
            .await?
            .as_ref(),
        sibling
    );
    assert!(!managed.is_finished());
    gate.release();
    assert!(timeout(DEADLINE, managed).await??.is_err());
    assert!(!gate.timed_out.load(Ordering::SeqCst));

    managed_downloads.shutdown().await?;
    receiver.close().await;
    router.shutdown().await?;
    receiver_store.shutdown().await?;
    prove_provider_settlement(&gate).await?;
    prove_export_worker_failure(&gate).await?;
    Ok(())
}

async fn prove_provider_settlement(gate: &Arc<ImportGate>) -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let provider_store = FsStore::load(temp.path().join("provider")).await?;
    let receiver_store = FsStore::load(temp.path().join("receiver")).await?;
    let server = Endpoint::builder(iroh::endpoint::presets::N0)
        .relay_mode(RelayMode::Disabled)
        .alpns(vec![iroh_blobs::ALPN.to_vec()])
        .bind()
        .await?;
    let client = Endpoint::builder(iroh::endpoint::presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?;
    timeout(DEADLINE, async {
        while server.addr().ip_addrs().next().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let (outgoing, incoming) =
        tokio::join!(client.connect(server.addr(), iroh_blobs::ALPN), async {
            server.accept().await.expect("incoming connection").await
        },);
    let outgoing = outgoing?;
    let incoming = incoming?;
    let (downloads, local) = blob_leases::lease(outgoing.clone(), false).await?;
    let (provides, remote) = blob_leases::lease(incoming.clone(), true).await?;
    let tag = provider_store
        .blobs()
        .add_bytes(vec![19; 1024 * 1024])
        .await?;
    *gate.released.lock().expect("gate lock") = false;
    gate.claimed.store(false, Ordering::SeqCst);
    gate.export.store(true, Ordering::SeqCst);
    let stream = local.open_bi(Instant::now() + DEADLINE).await?;
    let adapter = IrohBlobExchangeAdapter::new(receiver_store.clone());
    let get = tokio::spawn(async move {
        adapter
            .receive(
                stream,
                BlobDigest::from_bytes(*tag.hash.as_bytes()),
                CancellationToken::new(),
                Arc::new(blob_leases::Progress(Arc::new(Notify::new()))),
            )
            .await
    });
    let stream = remote.accept_bi().await?;
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let adapter = IrohBlobExchangeAdapter::new(provider_store.clone());
    let mut provide = tokio::spawn(async move {
        adapter
            .provide(stream, Instant::now() + DEADLINE, stop)
            .await
    });
    timeout(DEADLINE, gate.entered.notified()).await?;
    cancel.cancel();
    assert!(timeout(Duration::from_millis(100), &mut provide)
        .await
        .is_err());
    assert!(timeout(DEADLINE, get).await??.is_err());
    assert!(!provide.is_finished());

    // A 的 export 尚未结算；另一份 store 工作和同连接 B 仍完整读回。
    let bytes = b"provider-independent-sibling";
    let sibling = provider_store.blobs().add_bytes(bytes.as_slice()).await?;
    let stream = local.open_bi(Instant::now() + DEADLINE).await?;
    let adapter = IrohBlobExchangeAdapter::new(receiver_store.clone());
    let get_b = tokio::spawn(async move {
        adapter
            .receive(
                stream,
                BlobDigest::from_bytes(*sibling.hash.as_bytes()),
                CancellationToken::new(),
                Arc::new(blob_leases::Progress(Arc::new(Notify::new()))),
            )
            .await
    });
    let stream = remote.accept_bi().await?;
    timeout(
        DEADLINE,
        IrohBlobExchangeAdapter::new(provider_store.clone()).provide(
            stream,
            Instant::now() + DEADLINE,
            CancellationToken::new(),
        ),
    )
    .await??;
    let received = timeout(DEADLINE, get_b).await???;
    assert_eq!(received.size, bytes.len() as u64);
    drop(received);
    assert_eq!(
        receiver_store
            .blobs()
            .get_bytes(sibling.hash)
            .await?
            .as_ref(),
        bytes
    );
    assert!(!provide.is_finished());
    gate.release();
    assert!(timeout(DEADLINE, provide).await??.is_err());
    assert!(!gate.timed_out.load(Ordering::SeqCst));
    assert!(outgoing.close_reason().is_none());
    // 固定 SDK 主动获取这一角色也能读取替代提供端，包含合法空 blob。
    for payload in [Vec::new(), b"sdk-receiver-compatible".to_vec()] {
        let tag = provider_store.blobs().add_bytes(payload.clone()).await?;
        let (send, receive) = outgoing.open_bi().await?;
        let saved = receiver_store.clone();
        let get = tokio::spawn(async move {
            saved
                .remote()
                .execute_get(
                    StreamPair::new(0, receive, send),
                    GetRequest::blob(tag.hash),
                )
                .await
        });
        let stream = remote.accept_bi().await?;
        timeout(
            DEADLINE,
            IrohBlobExchangeAdapter::new(provider_store.clone()).provide(
                stream,
                Instant::now() + DEADLINE,
                CancellationToken::new(),
            ),
        )
        .await??;
        timeout(DEADLINE, get).await???;
        assert_eq!(
            receiver_store.blobs().get_bytes(tag.hash).await?.as_ref(),
            payload
        );
        // 同一内容已经在本地也能完成一次独立交换，空文件不会新增无主标签。
        let tags_before = tag_count(&receiver_store).await?;
        let stream = local.open_bi(Instant::now() + DEADLINE).await?;
        let adapter = IrohBlobExchangeAdapter::new(receiver_store.clone());
        let received = tokio::spawn(async move {
            adapter
                .receive(
                    stream,
                    BlobDigest::from_bytes(*tag.hash.as_bytes()),
                    CancellationToken::new(),
                    Arc::new(blob_leases::Progress(Arc::new(Notify::new()))),
                )
                .await
        });
        let stream = remote.accept_bi().await?;
        timeout(
            DEADLINE,
            IrohBlobExchangeAdapter::new(provider_store.clone()).provide(
                stream,
                Instant::now() + DEADLINE,
                CancellationToken::new(),
            ),
        )
        .await??;
        let received = timeout(DEADLINE, received).await???;
        assert_eq!(received.size, payload.len() as u64);
        drop(received);
        assert_eq!(tag_count(&receiver_store).await?, tags_before);
    }
    // 来源不存在沿原协议使用空 FIN，接收方获得明确不可用，而不是可重试的网络错误。
    let missing = iroh_blobs::Hash::new(b"synthetic-missing-source");
    let stream = local.open_bi(Instant::now() + DEADLINE).await?;
    let adapter = IrohBlobExchangeAdapter::new(receiver_store.clone());
    let receive = tokio::spawn(async move {
        adapter
            .receive(
                stream,
                BlobDigest::from_bytes(*missing.as_bytes()),
                CancellationToken::new(),
                Arc::new(blob_leases::Progress(Arc::new(Notify::new()))),
            )
            .await
    });
    let stream = remote.accept_bi().await?;
    let Err(error) = timeout(
        DEADLINE,
        IrohBlobExchangeAdapter::new(provider_store.clone()).provide(
            stream,
            Instant::now() + DEADLINE,
            CancellationToken::new(),
        ),
    )
    .await?
    else {
        panic!("missing source provided");
    };
    assert_eq!(error.kind, BlobExchangeFailure::SourceUnavailable);
    let Err(error) = timeout(DEADLINE, receive).await?? else {
        panic!("missing source received");
    };
    assert_eq!(error.kind, BlobExchangeFailure::SourceUnavailable);
    assert!(outgoing.close_reason().is_none());
    // 请求已送出但响应头仍未到达；Drop 兜底停止失败也必须保留，不能报告取消成功。
    let source_hash = provider_store
        .blobs()
        .add_bytes(b"header-stop-proof".as_slice())
        .await?;
    let stream = local.open_bi(Instant::now() + DEADLINE).await?;
    let stream = blob_leases::refuse_receive_stop(stream);
    let token = CancellationToken::new();
    let stop = token.clone();
    let adapter = IrohBlobExchangeAdapter::new(receiver_store.clone());
    let receiving = tokio::spawn(async move {
        adapter
            .receive(
                stream,
                BlobDigest::from_bytes(*source_hash.hash.as_bytes()),
                stop,
                Arc::new(blob_leases::Progress(Arc::new(Notify::new()))),
            )
            .await
    });
    let mut withheld = remote.accept_bi().await?;
    let mut request_started = [0; 1];
    withheld.receive.read_exact(&mut request_started).await?;
    token.cancel();
    let Err(error) = timeout(DEADLINE, receiving).await?? else {
        panic!("failed stop reported successful receive");
    };
    assert_eq!(error.kind, BlobExchangeFailure::Settlement);
    assert!(!error.additional.is_empty());
    let mut cause: Option<&(dyn Error + 'static)> = Some(&error.additional[0]);
    let mut preserved = false;
    while let Some(current) = cause {
        preserved |= current
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::PermissionDenied);
        cause = current.source();
    }
    assert!(preserved, "implicit stop source was lost");
    drop(withheld);
    // 不完整请求头也能只停止本交换；恰好到期拒绝，不因为已经有字节而续期。
    for expired in [false, true] {
        let mut incomplete = local.open_bi(Instant::now() + DEADLINE).await?;
        incomplete.send.write_all(&[0]).await?;
        let stream = remote.accept_bi().await?;
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let deadline = if expired {
            Instant::now()
        } else {
            Instant::now() + DEADLINE
        };
        let adapter = IrohBlobExchangeAdapter::new(provider_store.clone());
        let serving = tokio::spawn(async move { adapter.provide(stream, deadline, token).await });
        if !expired {
            cancel.cancel();
        }
        let Err(error) = timeout(DEADLINE, serving).await?? else {
            panic!("incomplete header accepted");
        };
        assert_eq!(
            error.kind,
            if expired {
                BlobExchangeFailure::RequestExpired
            } else {
                BlobExchangeFailure::Cancelled
            }
        );
        assert!(outgoing.close_reason().is_none());
        drop(incomplete);
    }
    downloads.shutdown().await?;
    provides.shutdown().await?;
    client.close().await;
    server.close().await;
    receiver_store.shutdown().await?;
    provider_store.shutdown().await?;
    Ok(())
}

async fn tag_count(store: &FsStore) -> anyhow::Result<usize> {
    let tags = store.tags().list().await?;
    tokio::pin!(tags);
    let mut count = 0;
    while let Some(tag) = tags.next().await {
        tag?;
        count += 1;
    }
    Ok(count)
}

async fn prove_export_worker_failure(gate: &Arc<ImportGate>) -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let source = FsStore::load(temp.path().join("source")).await?;
    let destination = FsStore::load(temp.path().join("destination")).await?;
    let server = Endpoint::builder(iroh::endpoint::presets::N0)
        .relay_mode(RelayMode::Disabled)
        .alpns(vec![iroh_blobs::ALPN.to_vec()])
        .bind()
        .await?;
    let client = Endpoint::builder(iroh::endpoint::presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?;
    timeout(DEADLINE, async {
        while server.addr().ip_addrs().next().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let (outgoing, incoming) =
        tokio::join!(client.connect(server.addr(), iroh_blobs::ALPN), async {
            server.accept().await.expect("incoming connection").await
        });
    let (downloads, local) = blob_leases::lease(outgoing?, false).await?;
    let (provides, remote) = blob_leases::lease(incoming?, true).await?;
    let tag = source.blobs().add_bytes(vec![31; 128 * 1024]).await?;
    gate.export.store(true, Ordering::SeqCst);
    gate.claimed.store(false, Ordering::SeqCst);
    gate.panic_export.store(true, Ordering::SeqCst);
    let stream = local.open_bi(Instant::now() + DEADLINE).await?;
    let adapter = IrohBlobExchangeAdapter::new(destination.clone());
    let receive = tokio::spawn(async move {
        adapter
            .receive(
                stream,
                BlobDigest::from_bytes(*tag.hash.as_bytes()),
                CancellationToken::new(),
                Arc::new(blob_leases::Progress(Arc::new(Notify::new()))),
            )
            .await
    });
    let stream = remote.accept_bi().await?;
    let result = timeout(
        DEADLINE,
        IrohBlobExchangeAdapter::new(source.clone()).provide(
            stream,
            Instant::now() + DEADLINE,
            CancellationToken::new(),
        ),
    )
    .await?;
    let Err(error) = result else {
        panic!("SDK worker failure reported as completed");
    };
    assert!(matches!(
        error.kind,
        BlobExchangeFailure::IncompleteExport | BlobExchangeFailure::Store
    ));
    assert!(timeout(DEADLINE, receive).await??.is_err());
    assert!(!gate.panic_export.load(Ordering::SeqCst));
    downloads.shutdown().await?;
    provides.shutdown().await?;
    client.close().await;
    server.close().await;
    destination.shutdown().await?;
    source.shutdown().await?;
    Ok(())
}
