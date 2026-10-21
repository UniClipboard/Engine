//! iroh-blobs backed implementation of [`BlobTransferPort`].
//!
//! Adapter 只处理已经加密好的密文字节:发布到本地 iroh-blobs store、生成
//! ticket、记录本地保留标签。获取及其停止统一由受管交换负责人执行，加解密与明文去重由
//! 上层 use case 和 sqlite `BlobReferenceRepositoryPort` 负责。

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::blob_exchange::without_request_span;
use async_trait::async_trait;
use bytes::Bytes;
use iroh::Endpoint;
use iroh_blobs::{
    api::blobs::{AddPathOptions, ImportMode},
    store::fs::FsStore,
    ticket::BlobTicket as NativeBlobTicket,
    BlobFormat, Hash, HashAndFormat,
};
use iroh_tickets::Ticket;
use tracing::instrument;

use uc_core::ports::blob::{BlobDigest, BlobError, BlobTicket, BlobTransferPort, TagReason};
use uc_observability_contract::diagnostics::connectivity::{
    LocalWorkObservation, LocalWorkOutcome, LocalWorkStep,
};
use uc_observability_contract::{log_fields::log_vocab_debug, uc_debug, uc_info};

pub const BLOBS_ALPN: &[u8] = iroh_blobs::ALPN;

/// 1h between GC sweeps (Phase D1 decision Q1=A).
///
/// The sweep itself is cheap (redb scan + free list of any blob whose
/// tags have all been released). Trade-off ratio: timely reclaim of
/// ex-clipboard-entry blobs vs idle IO; 1h gives prompt cleanup without
/// burning the disk on a daemon that never stops.
///
/// Phase F closed the auto-tag leak that previously made this GC
/// effectively a no-op for the publish path: `publish` / `publish_path`
/// now route through `AddProgress::with_named_tag(reason)` instead of
/// the default `with_tag` IntoFuture, so the only persistent tag a blob
/// carries is the caller-supplied business reason.
/// `BlobTransferPort::untag(reason)` therefore really drops the last
/// reference, and the next sweep within `BLOBS_GC_INTERVAL` reclaims
/// metadata + data atomically.
pub const BLOBS_GC_INTERVAL: Duration = Duration::from_secs(3600);

pub struct IrohBlobTransferAdapter {
    endpoint: Arc<Endpoint>,
    store: FsStore,
}

impl IrohBlobTransferAdapter {
    pub fn new(endpoint: Arc<Endpoint>, store: FsStore) -> Self {
        Self { endpoint, store }
    }

    fn native_hash(digest: &BlobDigest) -> Hash {
        Hash::from_bytes(*digest.as_bytes())
    }

    fn core_digest(hash: Hash) -> BlobDigest {
        BlobDigest::from_bytes(*hash.as_bytes())
    }

    fn parse_ticket(ticket: &BlobTicket) -> Result<NativeBlobTicket, BlobError> {
        NativeBlobTicket::decode_bytes(ticket.as_bytes()).map_err(BlobError::invalid_ticket_from)
    }

    fn tag_name(reason: &TagReason) -> String {
        match reason {
            TagReason::ClipboardEntry(entry_id) => {
                format!("uc-clipboard-entry:{}", entry_id.as_ref())
            }
        }
    }
}

/// Pick the `ImportMode` for `Blobs::add_path` based on the host platform.
///
/// See `publish_path` for the full rationale; in short: only Windows benefits
/// from `TryReference` (NTFS has no reflink, ReFS prefers external handle
/// over reflink anyway). All other platforms keep `Copy` so APFS / Btrfs /
/// XFS reflink fast paths fire.
fn preferred_import_mode() -> ImportMode {
    if cfg!(target_os = "windows") {
        ImportMode::TryReference
    } else {
        ImportMode::Copy
    }
}

#[async_trait]
impl BlobTransferPort for IrohBlobTransferAdapter {
    #[instrument(skip_all, fields(bytes = ciphertext.len()))]
    async fn publish(&self, ciphertext: Bytes, reason: TagReason) -> Result<BlobDigest, BlobError> {
        // GH#487: 大 blob 的 add_bytes 包含 BLAKE3 + BAO outboard 编码 +
        // 写盘,冷启动 / 慢盘场景下整段都可能阻塞十几秒。这里独立计时,让
        // 上游 publish_blob 的 publish_ms 能与本层 add_bytes_ms 对齐核对。
        //
        // Phase F: 用 `with_named_tag(reason_name)` 替代 `add_bytes(...).
        // await` 默认走的 `with_tag` 路径。后者会自动给 blob 打一个
        // `auto-<timestamp>` 持久 tag(`AddProgress::IntoFuture` 的默认行为),
        // 我们没有任何渠道能在 untag 时一并清掉它 —— 结果就是即使业务侧
        // 调了 `untag(ClipboardEntry)`,blob 仍被这个孤儿 auto-tag 钉死,
        // GC(Phase D1)永远跑不掉它,cache 文件实际只能等下次 daemon 重启
        // 让 Phase E1 的启动 sweep 清理。改用 `with_named_tag` 后,publish
        // 时直接打业务 tag,untag 即真正释放,GC 1h 内就能回收。
        //
        // 内部行为(`api/blobs.rs:654-662`):
        //   `with_named_tag = temp_tag().await? → tags.set(name, haf) → drop(tt)`
        // TempTag 在 set 完成前一直保活,关闭了 admit ↔ tag 之间的 GC 窗口;
        // set 是覆盖式,同一 reason 重复 publish 等价于覆盖到同一 hash,
        // 语义上与 publish 自身的幂等性一致。
        let bytes = ciphertext.len() as u64;
        let started = Instant::now();
        let tag_name = Self::tag_name(&reason);
        let observation = LocalWorkObservation::begin(LocalWorkStep::BlobStorePublish);
        let publish_result = without_request_span(|| async {
            self.store
                .blobs()
                .add_bytes(ciphertext)
                .with_named_tag(tag_name.as_bytes())
                .await
        })
        .await;
        observation.finish(if publish_result.is_ok() {
            LocalWorkOutcome::Ok
        } else {
            LocalWorkOutcome::Error
        });
        let haf = publish_result.map_err(|e| BlobError::Internal(e.into()))?;
        uc_info!(
            bytes = bytes,
            add_bytes_ms = started.elapsed().as_millis() as u64,
            "iroh blob publish: add_bytes completed"
        );
        Ok(Self::core_digest(haf.hash))
    }

    #[instrument(skip_all)]
    async fn publish_path(
        &self,
        path: &std::path::Path,
        reason: TagReason,
    ) -> Result<BlobDigest, BlobError> {
        // GH#487 P1 streaming publish:走 iroh-blobs 的 add_path 入口,增量算
        // BAO outboard,整段过程内存峰值受 iroh chunk size 主导,与文件大小
        // 无关。旧路径(`tokio::fs::read → Bytes → add_bytes`)在 1GB 文件上
        // 需要 ~2GB 临时内存,且阻塞 outbound dispatch 主流程 ~11s。
        //
        // ImportMode 选择(平台条件):
        //   - 非 Windows(macOS APFS / Linux Btrfs / XFS-with-reflink):用
        //     `ImportMode::Copy` —— 内部 `reflink_or_copy_with_progress` 在
        //     CoW FS 上是 zero-copy reflink,免费;ext4 / 其他 fallback 真
        //     拷贝,这部分用户量少,先不优化。
        //   - Windows(NTFS 大头 / ReFS 少数):用 `ImportMode::TryReference`
        //     (iroh-blobs 0.97 `store/fs/import.rs:485-490`)—— 不进 store
        //     数据目录,直接 `OpenOptions::read(true).open(path)` 拿外部句柄
        //     算 outboard,store entry 状态为 External。NTFS 1GB 实测 ~21s
        //     真拷贝直接消失,只剩 read + BAO 的成本。
        //
        // 正确性窗口:TryReference 模式下,如果用户在 dispatch 后、所有对端
        // fetch 完成前**修改 / 移动**源文件 → outboard 与内容失配,接收端
        // BAO 校验失败。Windows 上 store 持有的 `OpenOptions::read` 句柄会
        // 在 share=full-access 下不阻止改动,但天然阻止文件被删除(file in
        // use)—— 覆盖了"不小心拖去回收站"这一最常见误操作。其他场景接受
        // 失败、由用户重新复制(对端报错而 sender 这一侧不感知,fallback
        // 到 Copy 重 import 的反向通知链路超出本次 step 范围)。
        // Phase F: 与 `publish` 同理,改走 `with_named_tag`,避免 add_path
        // 的 `IntoFuture` 默认 `with_tag` 路径产生孤儿 auto-tag。
        let started = Instant::now();
        let mode = preferred_import_mode();
        let tag_name = Self::tag_name(&reason);
        let haf = without_request_span(|| async {
            self.store
                .blobs()
                .add_path_with_opts(AddPathOptions {
                    path: path.to_owned(),
                    format: BlobFormat::Raw,
                    mode,
                })
                .with_named_tag(tag_name.as_bytes())
                .await
        })
        .await
        .map_err(|e| BlobError::Internal(e.into()))?;
        uc_info!(
            add_path_ms = started.elapsed().as_millis() as u64,
            mode = log_vocab_debug(&mode),
            "iroh blob publish: add_path completed (streaming)"
        );
        Ok(Self::core_digest(haf.hash))
    }

    #[instrument(skip_all)]
    async fn issue_ticket(&self, digest: &BlobDigest) -> Result<BlobTicket, BlobError> {
        if !self.has(digest).await? {
            return Err(BlobError::NotFound);
        }

        let ticket = NativeBlobTicket::new(
            self.endpoint.addr(),
            Self::native_hash(digest),
            BlobFormat::Raw,
        );
        Ok(BlobTicket::from_bytes(ticket.encode_bytes()))
    }

    #[instrument(skip_all)]
    async fn has(&self, digest: &BlobDigest) -> Result<bool, BlobError> {
        let hash = Self::native_hash(digest);
        let observed = without_request_span(|| self.store.blobs().observe(hash))
            .await
            .map_err(|e| BlobError::Internal(e.into()))?;
        Ok(observed.is_complete())
    }

    #[instrument(skip_all)]
    async fn tag(&self, digest: &BlobDigest, reason: TagReason) -> Result<(), BlobError> {
        let name = Self::tag_name(&reason);
        without_request_span(|| {
            self.store.tags().set(
                name.as_bytes(),
                HashAndFormat::raw(Self::native_hash(digest)),
            )
        })
        .await
        .map_err(|e| BlobError::Internal(e.into()))
    }

    #[instrument(skip_all)]
    async fn untag(&self, reason: TagReason) -> Result<(), BlobError> {
        let name = Self::tag_name(&reason);
        let removed = without_request_span(|| self.store.tags().delete(name.as_bytes()))
            .await
            .map_err(|e| BlobError::Internal(e.into()))?;
        uc_debug!(removed = removed, "blob tag removed");
        Ok(())
    }

    fn digest_of(&self, ticket: &BlobTicket) -> Result<BlobDigest, BlobError> {
        let native = Self::parse_ticket(ticket)?;
        Ok(Self::core_digest(native.hash()))
    }
}

#[cfg(test)]
#[path = "../../../tests/support/blob_leases/lease.rs"]
mod lease_fixture;

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    use crate::network::iroh::blob_exchange::IrohBlobExchangeAdapter;
    use iroh::{protocol::Router, RelayMode};
    use tempfile::{tempdir, TempDir};
    use tokio::time::Instant as TokioInstant;
    use tokio_util::sync::CancellationToken;
    use tracing::instrument::WithSubscriber;
    use uc_application::deps::{
        BlobExchangeError, BlobExchangePort, BlobReadStorePort, BlobValidatedProgress,
    };
    use uc_core::ids::EntryId;

    #[derive(Clone, Default)]
    struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

    impl CapturedWriter {
        fn dump(&self) -> String {
            String::from_utf8(self.0.lock().expect("lock captured logs").clone())
                .expect("captured logs should be UTF-8")
        }
    }

    impl Write for CapturedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("lock captured logs")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedWriter {
        type Writer = CapturedWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    struct Fixture {
        adapter: IrohBlobTransferAdapter,
        exchange: IrohBlobExchangeAdapter,
        router: Router,
        store: FsStore,
        _tempdir: TempDir,
    }

    impl Fixture {
        async fn bind() -> anyhow::Result<Self> {
            // 与生产节点一致：SDK长期驱动不持有测试启动subscriber的span。
            without_request_span(|| async {
                let tempdir = tempdir()?;
                let store = FsStore::load(store_path(&tempdir)).await?;
                let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
                    .relay_mode(RelayMode::Disabled)
                    .bind()
                    .await?;
                let protocol = iroh_blobs::BlobsProtocol::new(&store, None);
                let router = Router::builder(endpoint.clone())
                    .accept(BLOBS_ALPN, protocol)
                    .spawn();
                let endpoint = Arc::new(endpoint);
                let adapter = IrohBlobTransferAdapter::new(endpoint, store.clone());

                Ok(Self {
                    adapter,
                    exchange: IrohBlobExchangeAdapter::new(store.clone()),
                    router,
                    store,
                    _tempdir: tempdir,
                })
            })
            .await
        }

        async fn wait_for_direct_addr(&self) -> anyhow::Result<()> {
            for _ in 0..100 {
                if self.router.endpoint().addr().ip_addrs().next().is_some() {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            anyhow::bail!("iroh endpoint never published a loopback direct address")
        }

        async fn shutdown(self) -> anyhow::Result<()> {
            self.router.shutdown().await?;
            Ok(())
        }
    }

    fn store_path(tempdir: &TempDir) -> PathBuf {
        tempdir.path().join("iroh-blobs")
    }

    fn unknown_digest() -> BlobDigest {
        BlobDigest::from_bytes([0x7f; 32])
    }

    /// Phase F 之后 `publish` / `publish_path` 都强制要求 caller 传一个
    /// `TagReason`(原子打 named tag,避免 auto-tag leak)。绝大多数测试只
    /// 关心 publish 自身的语义,不在意打到哪个 reason 上,所以用一个针对
    /// 测试名字的固定 EntryId 即可。同一测试内多次 publish 会共用一个
    /// reason,语义上等价于"同一个业务实体反复刷新到同一个 hash",对测试
    /// 断言无影响。
    fn dummy_reason(name: &str) -> TagReason {
        TagReason::ClipboardEntry(EntryId::from_str(name))
    }

    #[derive(Default)]
    struct CountingProgressSink {
        reports: AtomicUsize,
    }

    impl BlobValidatedProgress for CountingProgressSink {
        fn validated_through(&self, _: u64, _: u64) -> Result<(), BlobExchangeError> {
            self.reports.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn publish_same_bytes_returns_stable_digest() -> anyhow::Result<()> {
        let fixture = Fixture::bind().await?;
        let payload = Bytes::from_static(b"slice3-t4-stable");
        let reason = dummy_reason("publish-stable");

        let first = fixture
            .adapter
            .publish(payload.clone(), reason.clone())
            .await?;
        let second = fixture.adapter.publish(payload, reason).await?;

        assert_eq!(first, second);
        fixture.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn local_export_writes_cached_ciphertext_to_target() -> anyhow::Result<()> {
        // GH#487：本地已有密文通过专属存储能力直接导出，
        // 不建连，也不把完整文件加载到内存。
        let fixture = Fixture::bind().await?;
        fixture.wait_for_direct_addr().await?;
        let payload = b"gh-487-fetch-to-path-local-hit".to_vec();
        let digest = fixture
            .adapter
            .publish(
                Bytes::from(payload.clone()),
                dummy_reason("fetch-local-hit"),
            )
            .await?;
        let ticket = fixture.adapter.issue_ticket(&digest).await?;
        assert_eq!(fixture.adapter.digest_of(&ticket)?, digest);

        let dir = tempdir()?;
        let target = dir.path().join("out.bin");
        let content = fixture
            .exchange
            .protect(digest)
            .await?
            .expect("protected local ciphertext");
        let returned = fixture.exchange.export(digest, &target).await?;
        drop(content);

        assert_eq!(returned, payload.len() as u64);
        let written = std::fs::read(&target)?;
        assert_eq!(written, payload);
        fixture.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn local_export_try_reference_writes_bytes_above_inline_limit() -> anyhow::Result<()> {
        // GH#487 receive-side TryReference: payload must exceed iroh-blobs'
        // default 16 KiB inline threshold so the store actually holds an
        // owned data file (the only case TryReference's `fs::rename` branch
        // can fire). With a small payload the store keeps the bytes inline
        // and `export` falls through to a write-from-memory path, which
        // would mask any rename-vs-copy regression.
        let fixture = Fixture::bind().await?;
        fixture.wait_for_direct_addr().await?;
        let payload = vec![0x5au8; 64 * 1024];
        let digest = fixture
            .adapter
            .publish(Bytes::from(payload.clone()), dummy_reason("fetch-try-ref"))
            .await?;
        let ticket = fixture.adapter.issue_ticket(&digest).await?;
        assert_eq!(fixture.adapter.digest_of(&ticket)?, digest);

        let dir = tempdir()?;
        let target = dir.path().join("big.bin");
        let content = fixture
            .exchange
            .protect(digest)
            .await?
            .expect("protected local ciphertext");
        let returned = fixture.exchange.export(digest, &target).await?;
        drop(content);

        assert_eq!(returned, payload.len() as u64);
        let written = std::fs::read(&target)?;
        assert_eq!(written.len(), payload.len());
        assert_eq!(written, payload);
        fixture.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn local_export_keeps_blob_observable_after_export() -> anyhow::Result<()> {
        // GH#487 receive-side TryReference contract regression: after export
        // the iroh store transitions the entry to External(target_path) and
        // drops its owned data file. `has(digest)` must still report
        // complete and `issue_ticket` must still succeed — otherwise tag
        // bookkeeping (TagReason::ClipboardEntry) and any "show me the
        // blobs we have" diagnostics would silently break.
        let fixture = Fixture::bind().await?;
        fixture.wait_for_direct_addr().await?;
        let payload = vec![0xa5u8; 64 * 1024];
        let digest = fixture
            .adapter
            .publish(
                Bytes::from(payload.clone()),
                dummy_reason("fetch-after-export"),
            )
            .await?;
        let ticket = fixture.adapter.issue_ticket(&digest).await?;
        assert_eq!(fixture.adapter.digest_of(&ticket)?, digest);

        let dir = tempdir()?;
        let target = dir.path().join("exported.bin");
        let content = fixture
            .exchange
            .protect(digest)
            .await?
            .expect("protected local ciphertext");
        fixture.exchange.export(digest, &target).await?;
        drop(content);

        assert!(
            fixture.adapter.has(&digest).await?,
            "has() must still report complete for an externally referenced entry"
        );
        let _reissued = fixture
            .adapter
            .issue_ticket(&digest)
            .await
            .expect("issue_ticket must succeed for an externally referenced entry");

        // Tag/untag must still work — clipboard_sync uses these to pin
        // blobs to a ClipboardEntry and clean up on entry deletion.
        let reason = TagReason::ClipboardEntry(EntryId::from_str("entry-after-export"));
        fixture.adapter.tag(&digest, reason.clone()).await?;
        fixture.adapter.untag(reason).await?;

        fixture.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn publish_path_returns_same_digest_as_publish_bytes() -> anyhow::Result<()> {
        // GH#487 P1: streaming path 必须与全内存 publish 产出一致的
        // content-addressed digest,否则发送端 / 接收端协议上的 ticket /
        // dedup 会全部断裂。
        let fixture = Fixture::bind().await?;
        let payload = b"gh-487-streaming-publish-path-payload-hello".to_vec();

        let dir = tempdir()?;
        let path = dir.path().join("payload.bin");
        std::fs::write(&path, &payload)?;

        let bytes_digest = fixture
            .adapter
            .publish(
                Bytes::from(payload.clone()),
                dummy_reason("publish-eq-bytes"),
            )
            .await?;
        let path_digest = fixture
            .adapter
            .publish_path(&path, dummy_reason("publish-eq-path"))
            .await?;

        assert_eq!(bytes_digest, path_digest);
        fixture.shutdown().await?;
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn publish_path_logs_do_not_include_source_path() -> anyhow::Result<()> {
        // 启动与请求使用不同registry，显式覆盖SDK关闭时误用启动span的回归。
        let fixture = Fixture::bind()
            .with_subscriber(tracing::Dispatch::new(
                tracing_subscriber::Registry::default(),
            ))
            .await?;
        let dir = tempdir()?;
        let secret_name = "customer-payroll-private.bin";
        let path = dir.path().join(secret_name);
        std::fs::write(&path, b"private file payload")?;

        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_ansi(false)
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);

        fixture
            .adapter
            .publish_path(&path, dummy_reason("publish-path-log-redaction"))
            .await?;

        let logs = writer.dump();
        assert!(
            logs.contains("iroh blob publish: add_path completed"),
            "实际adapter完成记录必须被捕获"
        );
        assert!(
            !logs.contains(secret_name),
            "source path leaked into adapter logs: {logs}"
        );
        fixture.shutdown().await?;
        Ok(())
    }

    #[test]
    fn preferred_import_mode_is_try_reference_on_windows_copy_elsewhere() {
        // GH#487 Step 2 platform contract: TryReference is the default on
        // Windows (NTFS / ReFS) to skip the ~21s NTFS stream-copy fallback;
        // everywhere else stays on Copy so the existing reflink fast paths
        // (APFS / Btrfs / XFS reflink) keep firing untouched.
        let mode = preferred_import_mode();
        if cfg!(target_os = "windows") {
            assert!(matches!(mode, ImportMode::TryReference));
        } else {
            assert!(matches!(mode, ImportMode::Copy));
        }
    }

    #[tokio::test]
    async fn publish_path_try_reference_yields_same_digest_as_copy() -> anyhow::Result<()> {
        // GH#487 Step 2: switching ImportMode must not change the
        // content-addressed digest. The store's own dedup invariant
        // promises this (BAO is computed off the file content, not the
        // import strategy), but a regression here would silently break
        // every ticket the sender mints once the platform-conditional
        // mode select kicks in. Payload is 64 KiB — above iroh-blobs'
        // default 16 KiB inline threshold — so both branches actually
        // hit the file-import path that differs between modes.
        let fixture = Fixture::bind().await?;
        let payload = vec![0xc3u8; 64 * 1024];

        let dir = tempdir()?;
        let path_copy = dir.path().join("copy.bin");
        let path_ref = dir.path().join("ref.bin");
        std::fs::write(&path_copy, &payload)?;
        std::fs::write(&path_ref, &payload)?;

        let copy_tag = fixture
            .store
            .blobs()
            .add_path_with_opts(AddPathOptions {
                path: path_copy.clone(),
                format: BlobFormat::Raw,
                mode: ImportMode::Copy,
            })
            .await?;
        let ref_tag = fixture
            .store
            .blobs()
            .add_path_with_opts(AddPathOptions {
                path: path_ref.clone(),
                format: BlobFormat::Raw,
                mode: ImportMode::TryReference,
            })
            .await?;

        assert_eq!(copy_tag.hash, ref_tag.hash);
        fixture.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn publish_path_try_reference_serves_correct_bytes_to_local_fetch() -> anyhow::Result<()>
    {
        // GH#487 Step 2 end-to-end contract: when the store entry is an
        // External(path) reference rather than an owned data file, fetching
        // the blob (here through the local-hit fast path) must still hand
        // back the exact original bytes. This is what catches "we kept a
        // reference but the BAO outboard ended up wrong" regressions.
        let fixture = Fixture::bind().await?;
        fixture.wait_for_direct_addr().await?;
        let payload = vec![0x91u8; 64 * 1024];

        let dir = tempdir()?;
        let source = dir.path().join("source.bin");
        std::fs::write(&source, &payload)?;

        let tag_info = fixture
            .store
            .blobs()
            .add_path_with_opts(AddPathOptions {
                path: source.clone(),
                format: BlobFormat::Raw,
                mode: ImportMode::TryReference,
            })
            .await?;
        let digest = IrohBlobTransferAdapter::core_digest(tag_info.hash);
        let ticket = fixture.adapter.issue_ticket(&digest).await?;
        assert_eq!(fixture.adapter.digest_of(&ticket)?, digest);

        let target = dir.path().join("out.bin");
        let content = fixture
            .exchange
            .protect(digest)
            .await?
            .expect("protected local ciphertext");
        let returned = fixture.exchange.export(digest, &target).await?;
        drop(content);

        assert_eq!(returned, payload.len() as u64);
        let fetched = std::fs::read(&target)?;
        assert_eq!(fetched, payload);
        fixture.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn has_reports_present_and_missing_blobs() -> anyhow::Result<()> {
        let fixture = Fixture::bind().await?;

        let digest = fixture
            .adapter
            .publish(
                Bytes::from_static(b"slice3-t4-has"),
                dummy_reason("has-test"),
            )
            .await?;

        assert!(fixture.adapter.has(&digest).await?);
        assert!(!fixture.adapter.has(&unknown_digest()).await?);
        fixture.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn issue_ticket_and_digest_of_round_trip() -> anyhow::Result<()> {
        let fixture = Fixture::bind().await?;
        fixture.wait_for_direct_addr().await?;
        let digest = fixture
            .adapter
            .publish(
                Bytes::from_static(b"slice3-t4-ticket"),
                dummy_reason("ticket-roundtrip"),
            )
            .await?;

        let ticket = fixture.adapter.issue_ticket(&digest).await?;
        assert_eq!(fixture.adapter.digest_of(&ticket)?, digest);

        assert_eq!(fixture.adapter.digest_of(&ticket)?, digest);
        assert_eq!(
            BlobTicket::from_bytes(ticket.as_bytes().to_vec()).as_bytes(),
            ticket.as_bytes()
        );
        fixture.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn digest_of_invalid_ticket_returns_invalid_ticket() -> anyhow::Result<()> {
        let fixture = Fixture::bind().await?;
        let ticket = BlobTicket::from_bytes(vec![1, 2, 3, 4, 5]);

        let err = fixture
            .adapter
            .digest_of(&ticket)
            .expect_err("corrupt ticket must fail");

        assert!(matches!(err, BlobError::InvalidTicket { .. }));
        fixture.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn issue_ticket_for_missing_digest_returns_not_found() -> anyhow::Result<()> {
        let fixture = Fixture::bind().await?;

        let err = fixture
            .adapter
            .issue_ticket(&unknown_digest())
            .await
            .expect_err("missing digest must not mint a ticket");

        assert!(matches!(err, BlobError::NotFound));
        fixture.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn fetch_self_ticket_returns_original_bytes() -> anyhow::Result<()> {
        let fixture = Fixture::bind().await?;
        fixture.wait_for_direct_addr().await?;
        let payload = Bytes::from_static(b"slice3-t5-self-fetch");
        let digest = fixture
            .adapter
            .publish(payload.clone(), dummy_reason("self-fetch"))
            .await?;
        let ticket = fixture.adapter.issue_ticket(&digest).await?;
        assert_eq!(fixture.adapter.digest_of(&ticket)?, digest);

        let fetched = fixture
            .exchange
            .read(fixture.adapter.digest_of(&ticket)?)
            .await?;

        assert_eq!(fetched, payload);
        fixture.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn fetch_remote_ticket_returns_original_bytes() -> anyhow::Result<()> {
        let provider = Fixture::bind().await?;
        let receiver = Fixture::bind().await?;
        provider.wait_for_direct_addr().await?;
        receiver.wait_for_direct_addr().await?;
        let payload = Bytes::from_static(b"slice3-t5-remote-fetch");
        let digest = provider
            .adapter
            .publish(payload.clone(), dummy_reason("remote-fetch"))
            .await?;
        let ticket = provider.adapter.issue_ticket(&digest).await?;

        let native = NativeBlobTicket::decode_bytes(ticket.as_bytes())?;
        let connection = receiver
            .adapter
            .endpoint
            .connect(native.addr().clone(), BLOBS_ALPN)
            .await?;
        let (manager, lease) = lease_fixture::lease(connection, false).await?;
        let stream = lease
            .open_bi(TokioInstant::now() + Duration::from_secs(5))
            .await?;
        let content = receiver
            .exchange
            .receive(
                stream,
                digest,
                CancellationToken::new(),
                Arc::new(CountingProgressSink::default()),
            )
            .await?;
        let fetched = receiver.exchange.read(content.digest).await?;
        drop(content);
        drop(lease);
        manager.shutdown().await?;

        assert_eq!(fetched, payload);
        receiver.shutdown().await?;
        provider.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn cached_protection_and_managed_missing_member_preserve_directory_bytes(
    ) -> anyhow::Result<()> {
        let provider = Fixture::bind().await?;
        let receiver = Fixture::bind().await?;
        provider.wait_for_direct_addr().await?;
        receiver.wait_for_direct_addr().await?;

        let cached_payload = vec![0x31; 64 * 1024];
        let missing_payload = vec![0x72; 64 * 1024];
        let cached_digest = provider
            .adapter
            .publish(
                Bytes::from(cached_payload.clone()),
                dummy_reason("resend-provider-cached"),
            )
            .await?;
        let missing_digest = provider
            .adapter
            .publish(
                Bytes::from(missing_payload.clone()),
                dummy_reason("resend-provider-missing"),
            )
            .await?;
        let missing_ticket = provider.adapter.issue_ticket(&missing_digest).await?;

        let receiver_cached_digest = receiver
            .adapter
            .publish(
                Bytes::from(cached_payload.clone()),
                dummy_reason("resend-receiver-preloaded"),
            )
            .await?;
        assert_eq!(receiver_cached_digest, cached_digest);

        let target_dir = tempdir()?;
        let cached_target = target_dir.path().join("cached-member.bin");
        let missing_target = target_dir.path().join("missing-member.bin");
        let cached_content = receiver
            .exchange
            .protect(cached_digest)
            .await?
            .expect("cached member");
        assert!(receiver.exchange.protect(missing_digest).await?.is_none());
        receiver
            .exchange
            .export(cached_digest, &cached_target)
            .await?;
        drop(cached_content);
        let missing_progress = Arc::new(CountingProgressSink::default());
        let native = NativeBlobTicket::decode_bytes(missing_ticket.as_bytes())?;
        let connection = receiver
            .adapter
            .endpoint
            .connect(native.addr().clone(), BLOBS_ALPN)
            .await?;
        let (manager, lease) = lease_fixture::lease(connection, false).await?;
        let stream = lease
            .open_bi(TokioInstant::now() + Duration::from_secs(5))
            .await?;
        let content = receiver
            .exchange
            .receive(
                stream,
                missing_digest,
                CancellationToken::new(),
                missing_progress.clone(),
            )
            .await?;
        receiver
            .exchange
            .export(content.digest, &missing_target)
            .await?;
        drop(content);
        drop(lease);
        manager.shutdown().await?;

        assert_eq!(std::fs::read(cached_target)?, cached_payload);
        assert_eq!(std::fs::read(missing_target)?, missing_payload);
        assert!(missing_progress.reports.load(Ordering::SeqCst) > 0);

        receiver.shutdown().await?;
        provider.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn tag_then_untag_is_idempotent() -> anyhow::Result<()> {
        let fixture = Fixture::bind().await?;
        // Phase F 之后 publish 自带 named tag,这里再调一次 tag(reason) 是
        // "已存在同名 tag 上 set 同一个 hash",iroh-blobs 视为 overwrite,所以
        // tag/untag 的幂等契约依然成立 —— 这正是这个测试要锁定的语义。
        let digest = fixture
            .adapter
            .publish(
                Bytes::from_static(b"slice3-t6-tag"),
                dummy_reason("tag-publish"),
            )
            .await?;
        let reason = TagReason::ClipboardEntry(EntryId::from_str("entry-a"));

        fixture.adapter.tag(&digest, reason.clone()).await?;
        fixture.adapter.untag(reason.clone()).await?;
        fixture.adapter.untag(reason).await?;

        fixture.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn multiple_tag_reasons_are_independent() -> anyhow::Result<()> {
        let fixture = Fixture::bind().await?;
        let digest = fixture
            .adapter
            .publish(
                Bytes::from_static(b"slice3-t6-multi-tag"),
                dummy_reason("multi-tag-publish"),
            )
            .await?;
        let first = TagReason::ClipboardEntry(EntryId::from_str("entry-a"));
        let second = TagReason::ClipboardEntry(EntryId::from_str("entry-b"));

        fixture.adapter.tag(&digest, first.clone()).await?;
        fixture.adapter.tag(&digest, second.clone()).await?;
        fixture.adapter.untag(first.clone()).await?;

        let second_tag = IrohBlobTransferAdapter::tag_name(&second);
        assert!(fixture
            .store
            .tags()
            .get(second_tag.as_bytes())
            .await?
            .is_some());

        fixture.adapter.untag(second).await?;
        fixture.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn untag_keeps_blob_observable_for_subsequent_local_fetch() -> anyhow::Result<()> {
        // Phase B 方案 X 的端到端回归。原 panic
        // (`Poisoned storage should not be used` from `bao_file.rs:410`)
        // 触发条件是 iroh-blobs metadata 还在、物理 data 文件被外部
        // unlink —— 旧版 `delete_entry` 走的正是这条路径(直接 remove_file
        // cache 而 metadata 留在 redb)。后续的 ObserveRequest 一进
        // BaoFileStorage 就把 entry 标 Poisoned,任何对该 hash 的操作
        // 都 panic。
        //
        // 修复后 `delete_entry` 改走 untag,不主动 unlink cache、不主动
        // 删 metadata,GC 在 1h 内一致地回收 metadata + data。这个测试
        // 复现"用户删 entry 后立即点旧历史"的场景:
        //   1. publish + tag (业务侧持有声明)
        //   2. untag (模拟 DeleteClipboardEntryUseCase)
        //   3. has + issue_ticket + 本地export（GUI立即点旧历史）
        //
        // 本地export内部读bao outboard，是原panic真正触发的代码
        // 路径 —— 跑通即证明我们没有把 store 弄成 Poisoned 状态。
        //
        // 64 KiB 大于 iroh-blobs 默认 16 KiB inline 阈值,确保实际写出
        // owned data 文件而非 inline,触达 bao_file 路径(否则小 payload
        // 会走 inline 短路,绕过 panic 触发点,失去回归意义)。
        let fixture = Fixture::bind().await?;
        fixture.wait_for_direct_addr().await?;
        let payload = vec![0xefu8; 64 * 1024];
        let reason = TagReason::ClipboardEntry(EntryId::from_str("entry-deleted"));
        // Phase F: publish 已原子打业务 tag,无需再单独 tag()。这同时是
        // Phase F 后语义对齐的关键 —— 1h GC interval 内 untag 后 store 立即
        // **完全无 tag 保护**(没有 auto-tag 兜底了),仍要求 has /
        // issue_ticket / 本地export在内存路径(尚未触发 GC)上不进入
        // Poisoned 状态。如果未来有人在 publish/untag 路径里偷偷物理 unlink
        // data 文件,这个测试同样会抓到。
        let digest = fixture
            .adapter
            .publish(Bytes::from(payload.clone()), reason.clone())
            .await?;

        // 模拟 DeleteClipboardEntryUseCase:只释放业务声明,不动 store。
        fixture.adapter.untag(reason).await?;

        // GUI 立即重新点旧历史 —— 关键回归点。
        assert!(
            fixture.adapter.has(&digest).await?,
            "untag must not remove metadata: blob should still be observable"
        );
        let ticket = fixture.adapter.issue_ticket(&digest).await?;
        assert_eq!(fixture.adapter.digest_of(&ticket)?, digest);

        // 本地export走完整的bao outboard读取路径，
        // 是原 panic 真正触发的代码点。
        let dir = tempdir()?;
        let target = dir.path().join("after_untag.bin");
        let content = fixture
            .exchange
            .protect(digest)
            .await?
            .expect("protected local ciphertext");
        let returned = fixture.exchange.export(digest, &target).await?;
        drop(content);
        assert_eq!(returned, payload.len() as u64);
        let written = std::fs::read(&target)?;
        assert_eq!(
            written, payload,
            "fetched bytes must match the original payload"
        );

        fixture.shutdown().await?;
        Ok(())
    }
}
