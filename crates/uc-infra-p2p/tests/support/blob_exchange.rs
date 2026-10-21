//! 阶段 0 替代原型：网络结束后仍由同一次交换等待本地工作。

use std::num::NonZeroU64;
use std::sync::Arc;

use anyhow::{bail, Context};
use futures_util::StreamExt;
use iroh_blobs::{
    get::{
        fsm::{AtConnected, BlobContentNext, ConnectedNext, EndBlobNext},
        StreamPair,
    },
    protocol::{GetRequest, Request},
    provider::{events::RequestTracker, StreamPair as ProviderStreamPair},
    store::fs::FsStore,
    util::{RecvStream, SendStream},
    Hash,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

pub async fn get<R: RecvStream, W: SendStream>(
    store: FsStore,
    pair: StreamPair<R, W>,
    hash: Hash,
    cancel: CancellationToken,
    data: Arc<Notify>,
) -> anyhow::Result<()> {
    let connected = AtConnected::new(
        pair.t0,
        pair.recv,
        pair.send,
        GetRequest::blob(hash),
        Default::default(),
    );
    let ConnectedNext::StartRoot(root) = connected.next().await? else {
        bail!("raw probe requires a root response");
    };
    let (mut content, size) = root.next().next().await?;
    let Some(size) = NonZeroU64::new(size) else {
        content.drain().await?;
        if hash != Hash::EMPTY {
            bail!("invalid empty response");
        }
        store.blobs().add_bytes(Vec::new()).await?;
        return Ok(());
    };
    // 取得已接受的 import 完成权以前不能丢弃准备 future。
    let handle = store.blobs().import_bao(hash, size, 8).await?;
    let tx = handle.tx;
    let network = {
        let read = async move {
            loop {
                match content.next().await {
                    BlobContentNext::More((next, item)) => {
                        tx.send(item?).await?;
                        if next.stats().payload_bytes_read > 0 {
                            data.notify_one();
                        }
                        content = next;
                    }
                    BlobContentNext::Done(end) => {
                        let EndBlobNext::Closing(closing) = end.next() else {
                            bail!("unexpected child response");
                        };
                        closing.next().await?;
                        return anyhow::Ok(());
                    }
                }
            }
        };
        tokio::pin!(read);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(anyhow::anyhow!("probe exchange cancelled")),
            result = &mut read => result,
        }
    };
    // read 离开作用域已关闭 tx；错误和取消均等待这一份 import，而非整个 store。
    handle
        .rx
        .await
        .context("probe import reply")?
        .context("probe import settlement")?;
    network
}

pub async fn provide<R: RecvStream, W: SendStream>(
    store: FsStore,
    mut pair: ProviderStreamPair<R, W>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let Request::Get(request) = pair.read_request().await? else {
        bail!("probe supports raw get only");
    };
    let Some((0, ranges)) = request.ranges.as_single() else {
        bail!("probe supports raw root ranges only");
    };
    // 本原型沿用当前生产配置，不启用 SDK 自身的 provider 进度追踪。
    let mut writer = pair.into_writer(RequestTracker::NONE).await?.inner;
    let bytes = store
        .blobs()
        .export_bao(request.hash, ranges.clone())
        .into_byte_stream();
    tokio::pin!(bytes);
    let mut failure = None;
    loop {
        let item = tokio::select! {
            biased;
            _ = cancel.cancelled(), if failure.is_none() => {
                let stopped = std::io::Error::from(std::io::ErrorKind::Interrupted);
                failure = Some(match writer.reset(0u32.into()) {
                    Ok(()) => anyhow::Error::new(stopped),
                    Err(reset) => anyhow::Error::new(stopped).context(reset),
                });
                continue;
            }
            item = bytes.next() => item,
        };
        let Some(item) = item else { break };
        match item {
            Ok(bytes) if failure.is_none() => {
                let result = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => Err(std::io::Error::from(std::io::ErrorKind::Interrupted)),
                    result = writer.send_bytes(bytes) => result,
                };
                if let Err(error) = result {
                    failure = Some(match writer.reset(0u32.into()) {
                        Ok(()) => anyhow::Error::new(error),
                        Err(reset) => anyhow::Error::new(error).context(reset),
                    });
                }
            }
            Ok(_) => {}
            Err(error) => {
                failure = Some(match failure {
                    Some(previous) => previous.context(error),
                    None => error.into(),
                });
            }
        }
    }
    // 取消后不再发送，但排空同一份有界 export，直至 SDK 关闭结果通道。
    if let Some(error) = failure {
        return Err(error);
    }
    writer.sync().await?;
    Ok(())
}
