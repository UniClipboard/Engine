use super::{failure::combine, io, provide, receive};
use async_trait::async_trait;
use iroh_blobs::{api::TempTag, store::fs::FsStore, Hash, HashAndFormat};
use std::sync::Arc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    BlobExchangeError, BlobExchangeFailure, BlobExchangePort, BlobReceived, BlobRetainedContent,
    BlobStoreLifecyclePort, BlobValidatedProgress, StreamLease, StreamWorkLease,
};
use uc_core::ports::blob::BlobDigest;

pub struct IrohBlobExchangeAdapter {
    store: FsStore,
}
impl IrohBlobExchangeAdapter {
    pub fn new(store: FsStore) -> Self {
        Self { store }
    }
}
struct RetainedContent {
    _tag: TempTag,
    _work: StreamWorkLease,
}
impl BlobRetainedContent for RetainedContent {}

#[async_trait]
impl BlobExchangePort for IrohBlobExchangeAdapter {
    async fn receive(
        &self,
        mut stream: StreamLease,
        digest: BlobDigest,
        cancel: CancellationToken,
        progress: Arc<dyn BlobValidatedProgress>,
    ) -> Result<BlobReceived, BlobExchangeError> {
        let hash = Hash::from_bytes(*digest.as_bytes());
        // store 接受请求后不能丢弃准备 future；取消先停止两半流，再等待保护准备结算。
        let preparing = self.store.tags().temp_tag(HashAndFormat::raw(hash));
        tokio::pin!(preparing);
        let tag = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                let mut failure = BlobExchangeError::refused(BlobExchangeFailure::Cancelled);
                for result in [stream.send.reset(0), stream.receive.stop(0)] {
                    if let Err(source) = result {
                        failure = failure.with_settlement(BlobExchangeError::new(BlobExchangeFailure::Network, source));
                    }
                }
                if let Err(source) = preparing.await {
                    failure = failure.with_settlement(BlobExchangeError::new(BlobExchangeFailure::Store, source));
                }
                return Err(failure);
            }
            result = &mut preparing => match result {
                Ok(tag) => tag,
                Err(source) => {
                    let mut failure = BlobExchangeError::new(BlobExchangeFailure::Store, source);
                    for result in [stream.send.reset(0), stream.receive.stop(0)] {
                        if let Err(source) = result {
                            failure = failure.with_settlement(BlobExchangeError::new(BlobExchangeFailure::Network, source));
                        }
                    }
                    return Err(failure);
                }
            },
        };
        let (send, reader, control, work, completion) = io::split(stream);
        let received =
            receive::receive(&self.store, send, reader, control, hash, &cancel, progress).await;
        let size = combine(received, completion.finish())?;
        Ok(BlobReceived {
            digest,
            size,
            protection: Box::new(RetainedContent {
                _tag: tag,
                _work: work,
            }),
        })
    }

    async fn provide(
        &self,
        stream: StreamLease,
        request_deadline: Instant,
        cancel: CancellationToken,
    ) -> Result<(), BlobExchangeError> {
        let (send, reader, control, _work, completion) = io::split(stream);
        drop(control);
        let provided = provide::provide(&self.store, send, reader, request_deadline, &cancel).await;
        combine(provided, completion.finish())
    }
}

#[async_trait]
impl BlobStoreLifecyclePort for IrohBlobExchangeAdapter {
    async fn shutdown(&self) -> Result<(), BlobExchangeError> {
        self.store
            .shutdown()
            .await
            .map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Store, source))
    }
}
