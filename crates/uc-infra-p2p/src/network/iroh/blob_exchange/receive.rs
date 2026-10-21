use super::failure::{combine, content, header, network, request_write};
use super::io::{ReceiveControl, SdkReceive, SdkSend};
use iroh_blobs::get::fsm::{AtConnected, BlobContentNext, ConnectedNext, EndBlobNext};
use iroh_blobs::protocol::GetRequest;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::util::AsyncReadRecvStream;
use iroh_blobs::Hash;
use std::num::NonZeroU64;
use std::sync::Arc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{BlobExchangeError, BlobExchangeFailure, BlobValidatedProgress};

pub(super) async fn receive(
    store: &FsStore,
    send: SdkSend,
    reader: AsyncReadRecvStream<SdkReceive>,
    control: ReceiveControl,
    hash: Hash,
    cancel: &CancellationToken,
    progress: Arc<dyn BlobValidatedProgress>,
) -> Result<u64, BlobExchangeError> {
    let prepare_header = async {
        let connected = AtConnected::new(
            Instant::now(),
            reader,
            send,
            GetRequest::blob(hash),
            Default::default(),
        );
        let ConnectedNext::StartRoot(root) = connected.next().await.map_err(request_write)? else {
            return Err(BlobExchangeError::refused(
                BlobExchangeFailure::InvalidRequest,
            ));
        };
        root.next().next().await.map_err(header)
    };
    let (mut data, size) = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled)),
        result = prepare_header => result?,
    };
    let Some(nonzero) = NonZeroU64::new(size) else {
        let finish = async {
            let end = data.drain().await.map_err(content)?;
            let EndBlobNext::Closing(closing) = end.next() else {
                return Err(BlobExchangeError::refused(
                    BlobExchangeFailure::InvalidRequest,
                ));
            };
            closing.next().await.map_err(network)?;
            if hash != Hash::EMPTY {
                return Err(BlobExchangeError::refused(BlobExchangeFailure::Integrity));
            }
            Ok(())
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled)),
            result = finish => result?,
        }
        // SDK 将 EMPTY 定义为始终完整；不调用 add_bytes 创建无主永久标签。
        return if cancel.is_cancelled() {
            Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled))
        } else {
            Ok(0)
        };
    };
    let preparing = store.blobs().import_bao(hash, nonzero, 8);
    tokio::pin!(preparing);
    let mut early_stop = None;
    let prepared = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            let cancelled = BlobExchangeError::refused(BlobExchangeFailure::Cancelled);
            let stopped = control.stop(0).map_err(network);
            early_stop = Some(match stopped { Ok(()) => cancelled, Err(error) => cancelled.with_settlement(error) });
            preparing.await
        }
        result = &mut preparing => result,
    };
    let handle = match prepared {
        Ok(handle) => handle,
        Err(source) => {
            let error = BlobExchangeError::new(BlobExchangeFailure::Store, source);
            return Err(match early_stop {
                Some(stop) => stop.with_settlement(error),
                None => error,
            });
        }
    };
    let tx = handle.tx;
    let result = if let Some(error) = early_stop {
        drop(tx);
        Err(error)
    } else {
        let reading = async move {
            loop {
                match data.next().await {
                    BlobContentNext::More((next, item)) => {
                        let item = item.map_err(content)?;
                        let end = next.stats().payload_bytes_read;
                        // 固定 SDK 只在验证成功的 Leaf 增加 payload 计数，Parent 不计入。
                        progress.validated_through(end, size)?;
                        tx.send(item).await.map_err(|source| {
                            BlobExchangeError::new(BlobExchangeFailure::Store, source)
                        })?;
                        data = next;
                    }
                    BlobContentNext::Done(end) => {
                        let EndBlobNext::Closing(closing) = end.next() else {
                            return Err(BlobExchangeError::refused(
                                BlobExchangeFailure::InvalidRequest,
                            ));
                        };
                        closing.next().await.map_err(network)?;
                        return Ok(());
                    }
                }
            }
        };
        tokio::pin!(reading);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled)),
            result = &mut reading => result,
        }
    };
    let result = if result.is_err() {
        combine(result, control.stop(0).map_err(network))
    } else {
        result
    };
    // reading 已退出并关闭 tx；网络错误和取消都保留这次 import 的完成 receiver。
    let settled = match handle.rx.await {
        Ok(result) => {
            result.map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Store, source))
        }
        Err(source) => Err(BlobExchangeError::new(
            BlobExchangeFailure::Settlement,
            source,
        )),
    };
    combine(result, settled)?;
    if cancel.is_cancelled() {
        return Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled));
    }
    Ok(size)
}
