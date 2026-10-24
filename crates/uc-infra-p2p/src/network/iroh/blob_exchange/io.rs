use bytes::Bytes;
use iroh::endpoint::VarInt;
use iroh_blobs::util::{AsyncReadRecvStream, AsyncReadRecvStreamExtra, SendStream};
use std::io;
use std::mem::take;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};
use uc_application::deps::{
    BlobExchangeError, BlobExchangeFailure, ConnectivityError, PeerReceiveStream, PeerSendStream,
    StreamLease, StreamWorkLease,
};

/// 两半流的兜底停止最多各产生一项失败；随本交换统一结算，不从 Drop 吞掉来源。
#[derive(Clone, Default)]
pub(super) struct IoCompletion {
    failures: Arc<Mutex<Vec<ConnectivityError>>>,
}
impl IoCompletion {
    fn record(&self, source: ConnectivityError) {
        self.failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(source);
    }
    pub fn finish(self) -> Result<(), BlobExchangeError> {
        let failures = take(&mut *self.failures.lock().unwrap_or_else(PoisonError::into_inner));
        let mut failures = failures.into_iter();
        let Some(primary) = failures.next() else {
            return Ok(());
        };
        let mut error = BlobExchangeError::new(BlobExchangeFailure::Settlement, primary);
        for additional in failures {
            error.additional.push(BlobExchangeError::new(
                BlobExchangeFailure::Settlement,
                additional,
            ));
        }
        Err(error)
    }
}

pub(super) struct SdkSend {
    stream: Box<dyn PeerSendStream>,
    finished: bool,
    completion: IoCompletion,
}
impl SendStream for SdkSend {
    async fn send_bytes(&mut self, bytes: Bytes) -> io::Result<()> {
        self.stream.write_all(&bytes).await
    }
    async fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.stream.write_all(bytes).await
    }
    async fn sync(&mut self) -> io::Result<()> {
        self.stream.flush().await?;
        self.stream.finish().map_err(io::Error::other)?;
        self.finished = true;
        Ok(())
    }
    fn reset(&mut self, code: VarInt) -> io::Result<()> {
        if self.finished {
            return Ok(());
        }
        self.stream
            .reset(u64::from(code))
            .map_err(io::Error::other)?;
        self.finished = true;
        Ok(())
    }
    async fn stopped(&mut self) -> io::Result<Option<VarInt>> {
        self.stream.settled().await.map_err(io::Error::other)?;
        Ok(None)
    }
    fn id(&self) -> u64 {
        self.stream.id()
    }
}
impl Drop for SdkSend {
    fn drop(&mut self) {
        if !self.finished {
            if let Err(source) = self.stream.reset(0) {
                self.completion.record(source);
            }
        }
    }
}

struct ReceiveState {
    stream: Box<dyn PeerReceiveStream>,
    stopped: bool,
    completion: IoCompletion,
}
impl Drop for ReceiveState {
    fn drop(&mut self) {
        if !self.stopped {
            if let Err(source) = self.stream.stop(0) {
                self.completion.record(source);
            }
        }
    }
}
#[derive(Clone)]
pub(super) struct ReceiveControl {
    state: Arc<Mutex<ReceiveState>>,
}
impl ReceiveControl {
    pub fn stop(&self, code: u64) -> Result<(), ConnectivityError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.stopped {
            state.stream.stop(code)?;
            state.stopped = true;
        }
        Ok(())
    }
}
pub(super) struct SdkReceive {
    control: ReceiveControl,
}
impl AsyncRead for SdkReceive {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut state = self
            .control
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let before = buf.filled().len();
        let result = Pin::new(&mut state.stream).poll_read(cx, buf);
        if matches!(result, Poll::Ready(Ok(()))) && buf.filled().len() == before {
            state.stopped = true;
        }
        result
    }
}
impl AsyncReadRecvStreamExtra for SdkReceive {
    fn inner(&mut self) -> &mut (impl AsyncRead + Unpin + Send) {
        self
    }
    fn stop(&mut self, code: VarInt) -> io::Result<()> {
        self.control.stop(u64::from(code)).map_err(io::Error::other)
    }
    fn id(&self) -> u64 {
        self.control
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .stream
            .id()
    }
}
pub(super) fn split(
    stream: StreamLease,
) -> (
    SdkSend,
    AsyncReadRecvStream<SdkReceive>,
    ReceiveControl,
    StreamWorkLease,
    IoCompletion,
) {
    let ((send, receive), work) = stream.into_parts();
    let completion = IoCompletion::default();
    let control = ReceiveControl {
        state: Arc::new(Mutex::new(ReceiveState {
            stream: receive,
            stopped: false,
            completion: completion.clone(),
        })),
    };
    (
        SdkSend {
            stream: send,
            finished: false,
            completion: completion.clone(),
        },
        AsyncReadRecvStream::new(SdkReceive {
            control: control.clone(),
        }),
        control,
        work,
        completion,
    )
}
