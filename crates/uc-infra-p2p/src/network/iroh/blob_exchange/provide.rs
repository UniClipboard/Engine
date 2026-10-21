use super::failure::{combine, network};
use super::io::{SdkReceive, SdkSend};
use super::response_size::expected_size;
use futures_util::StreamExt;
use iroh_blobs::protocol::Request;
use iroh_blobs::provider::{events::RequestTracker, StreamPair};
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::util::{AsyncReadRecvStream, SendStream};
use iroh_blobs::{api::blobs::BlobStatus, Hash, HashAndFormat};
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{BlobExchangeError, BlobExchangeFailure};

pub(super) async fn provide(
    store: &FsStore,
    send: SdkSend,
    reader: AsyncReadRecvStream<SdkReceive>,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<(), BlobExchangeError> {
    if Instant::now() >= deadline {
        return Err(BlobExchangeError::refused(
            BlobExchangeFailure::RequestExpired,
        ));
    }
    let mut pair = StreamPair::new(0, reader, send, Default::default());
    let request = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled)),
        result = timeout_at(deadline, pair.read_request()) => match result {
            Ok(result) => result.map_err(|source| BlobExchangeError::new(BlobExchangeFailure::InvalidRequest, source))?,
            Err(source) => return Err(BlobExchangeError::new(BlobExchangeFailure::RequestExpired, source)),
        },
    };
    if Instant::now() >= deadline {
        return Err(BlobExchangeError::refused(
            BlobExchangeFailure::RequestExpired,
        ));
    }
    let Request::Get(request) = request else {
        return Err(BlobExchangeError::refused(
            BlobExchangeFailure::InvalidRequest,
        ));
    };
    let Some((0, ranges)) = request.ranges.as_single() else {
        return Err(BlobExchangeError::refused(
            BlobExchangeFailure::InvalidRequest,
        ));
    };
    let mut writer = pair
        .into_writer(RequestTracker::NONE)
        .await
        .map_err(network)?
        .inner;
    let preparing = store.tags().temp_tag(HashAndFormat::raw(request.hash));
    tokio::pin!(preparing);
    let _tag = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            let stopped = writer.reset(0u32.into()).map_err(network);
            let settled = preparing.await.map(|_| ()).map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Store, source));
            return combine(combine(Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled)), stopped), settled);
        }
        result = &mut preparing => result.map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Store, source))?,
    };
    let BlobStatus::Complete { size } =
        checked_status(store, request.hash, &mut writer, cancel).await?
    else {
        // 原 SDK 用空 FIN 表达未提供数据；保留旧接收端可识别的 not-found 语义。
        finish(&mut writer, cancel).await?;
        return Err(BlobExchangeError::refused(
            BlobExchangeFailure::SourceUnavailable,
        ));
    };
    let expected = expected_size(size, ranges, cancel).await?;
    let bytes = store
        .blobs()
        .export_bao(request.hash, ranges.clone())
        .into_byte_stream();
    tokio::pin!(bytes);
    let mut failure = None;
    let mut sent = 0u64;
    loop {
        let item = tokio::select! {
            biased;
            _ = cancel.cancelled(), if failure.is_none() => {
                let error = BlobExchangeError::refused(BlobExchangeFailure::Cancelled);
                failure = Some(match writer.reset(0u32.into()) {
                    Ok(()) => error,
                    Err(source) => error.with_settlement(network(source)),
                });
                continue;
            }
            item = bytes.next() => item,
        };
        let Some(item) = item else { break };
        match item {
            Ok(bytes) if failure.is_none() => {
                let count = bytes.len() as u64;
                let result = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled)),
                    result = writer.send_bytes(bytes) => result.map_err(network),
                };
                match result {
                    Ok(()) => sent += count,
                    Err(error) => {
                        failure = Some(match writer.reset(0u32.into()) {
                            Ok(()) => error,
                            Err(source) => error.with_settlement(network(source)),
                        });
                    }
                }
            }
            Ok(_) => {}
            Err(source) => {
                let error = BlobExchangeError::new(BlobExchangeFailure::Store, source);
                failure = Some(match failure {
                    Some(previous) => previous.with_settlement(error),
                    None => error,
                });
            }
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    // SDK 字节流可能将通道异常映射为 EOF；完整长度和本地状态共同核验正常结束。
    let status = checked_status(store, request.hash, &mut writer, cancel).await?;
    match status {
        BlobStatus::Complete { size: after } if after == size && sent == expected => {}
        _ => {
            return Err(BlobExchangeError::refused(
                BlobExchangeFailure::IncompleteExport,
            ))
        }
    }
    finish(&mut writer, cancel).await
}

async fn checked_status(
    store: &FsStore,
    hash: Hash,
    writer: &mut SdkSend,
    cancel: &CancellationToken,
) -> Result<BlobStatus, BlobExchangeError> {
    let checking = store.blobs().status(hash);
    tokio::pin!(checking);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            let stopped = writer.reset(0u32.into()).map_err(network);
            let settled = checking.await.map(|_| ()).map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Store, source));
            combine(combine(Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled)), stopped), settled)
        }
        result = &mut checking => result.map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Store, source)),
    }
}

async fn finish(writer: &mut SdkSend, cancel: &CancellationToken) -> Result<(), BlobExchangeError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            let stopped = writer.reset(0u32.into()).map_err(network);
            combine(Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled)), stopped)
        }
        result = writer.sync() => result.map_err(network),
    }
}
