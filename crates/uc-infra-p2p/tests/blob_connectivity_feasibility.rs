//! 第七片阶段 0：固定 SDK 的失败返回不等于该次 store 工作已经退出。
//!
//! 探针仅使用 SDK 公开接口和测试专属 tracing gate，不替换生产路径。
//! 成功表示复现了前置阻塞，不表示受管传输合同通过。

#[path = "support/blob_exchange.rs"]
mod blob_exchange;

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
use tokio::sync::Notify;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{field::Visit, Event, Subscriber};
use tracing_subscriber::{layer::Context, prelude::*, Layer};

const DEADLINE: Duration = Duration::from_secs(10);

#[derive(Default)]
struct ImportGate {
    entered: Notify,
    released: Mutex<bool>,
    wake: Condvar,
    claimed: AtomicBool,
    timed_out: AtomicBool,
    export: AtomicBool,
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
    let (send, receive) = connection.open_bi().await?;
    let cancel = CancellationToken::new();
    let ready = Arc::new(Notify::new());
    let mut managed = tokio::spawn(blob_exchange::get(
        receiver_store.clone(),
        StreamPair::new(connection.stable_id() as u64, receive, send),
        tag.hash,
        cancel.clone(),
        ready.clone(),
    ));
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

    receiver.close().await;
    router.shutdown().await?;
    receiver_store.shutdown().await?;
    prove_provider_settlement(&gate).await?;
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
    let tag = provider_store
        .blobs()
        .add_bytes(vec![19; 1024 * 1024])
        .await?;
    *gate.released.lock().expect("gate lock") = false;
    gate.claimed.store(false, Ordering::SeqCst);
    gate.export.store(true, Ordering::SeqCst);
    let (send, receive) = outgoing.open_bi().await?;
    let get = tokio::spawn(blob_exchange::get(
        receiver_store.clone(),
        StreamPair::new(outgoing.stable_id() as u64, receive, send),
        tag.hash,
        CancellationToken::new(),
        Arc::new(Notify::new()),
    ));
    let (send, receive) = incoming.accept_bi().await?;
    let cancel = CancellationToken::new();
    let mut provide = tokio::spawn(blob_exchange::provide(
        provider_store.clone(),
        iroh_blobs::provider::StreamPair::new(
            incoming.stable_id() as u64,
            receive,
            send,
            Default::default(),
        ),
        cancel.clone(),
    ));
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
    let (send, receive) = outgoing.open_bi().await?;
    let get_b = tokio::spawn(blob_exchange::get(
        receiver_store.clone(),
        StreamPair::new(outgoing.stable_id() as u64, receive, send),
        sibling.hash,
        CancellationToken::new(),
        Arc::new(Notify::new()),
    ));
    let (send, receive) = incoming.accept_bi().await?;
    timeout(
        DEADLINE,
        blob_exchange::provide(
            provider_store.clone(),
            iroh_blobs::provider::StreamPair::new(
                incoming.stable_id() as u64,
                receive,
                send,
                Default::default(),
            ),
            CancellationToken::new(),
        ),
    )
    .await??;
    timeout(DEADLINE, get_b).await???;
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
        let (send, receive) = incoming.accept_bi().await?;
        timeout(
            DEADLINE,
            blob_exchange::provide(
                provider_store.clone(),
                iroh_blobs::provider::StreamPair::new(
                    incoming.stable_id() as u64,
                    receive,
                    send,
                    Default::default(),
                ),
                CancellationToken::new(),
            ),
        )
        .await??;
        timeout(DEADLINE, get).await???;
        assert_eq!(
            receiver_store.blobs().get_bytes(tag.hash).await?.as_ref(),
            payload
        );
    }
    client.close().await;
    server.close().await;
    receiver_store.shutdown().await?;
    provider_store.shutdown().await?;
    Ok(())
}
