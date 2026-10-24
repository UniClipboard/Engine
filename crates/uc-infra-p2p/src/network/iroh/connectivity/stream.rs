use futures_util::future::BoxFuture;
use iroh::endpoint::{RecvStream, SendStream, VarInt};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use uc_application::deps::{ConnectivityError, PeerReceiveStream, PeerSendStream, StreamPair};

struct SendHalf {
    native: SendStream,
    finished: bool,
}
struct ReceiveHalf(RecvStream);
pub(super) fn wrap(send: SendStream, receive: RecvStream) -> StreamPair {
    (wrap_send(send), wrap_receive(receive))
}
pub(super) fn wrap_send(send: SendStream) -> Box<dyn PeerSendStream> {
    Box::new(SendHalf {
        native: send,
        finished: false,
    })
}
pub(super) fn wrap_receive(receive: RecvStream) -> Box<dyn PeerReceiveStream> {
    Box::new(ReceiveHalf(receive))
}
impl AsyncWrite for SendHalf {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.native), cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.native), cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.native), cx)
    }
}
impl PeerSendStream for SendHalf {
    fn id(&self) -> u64 {
        self.native.id().index()
    }
    fn reset(&mut self, code: u64) -> Result<(), ConnectivityError> {
        let code =
            VarInt::from_u64(code).map_err(|source| ConnectivityError::Stream(Box::new(source)))?;
        self.native
            .reset(code)
            .map_err(|source| ConnectivityError::Stream(Box::new(source)))?;
        self.finished = true;
        Ok(())
    }
    fn settled(&mut self) -> BoxFuture<'_, Result<(), ConnectivityError>> {
        Box::pin(async {
            self.native
                .stopped()
                .await
                .map_err(|source| ConnectivityError::Stream(Box::new(source)))?;
            Ok(())
        })
    }
    fn finish(&mut self) -> Result<(), ConnectivityError> {
        self.native
            .finish()
            .map_err(|error| ConnectivityError::Stream(Box::new(error)))?;
        self.finished = true;
        Ok(())
    }
}
impl Drop for SendHalf {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.native.reset(0u32.into());
        }
    }
}
impl AsyncRead for ReceiveHalf {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        AsyncRead::poll_read(Pin::new(&mut self.0), cx, buf)
    }
}
impl PeerReceiveStream for ReceiveHalf {
    fn id(&self) -> u64 {
        self.0.id().index()
    }
    fn stop(&mut self, code: u64) -> Result<(), ConnectivityError> {
        let code =
            VarInt::from_u64(code).map_err(|source| ConnectivityError::Stream(Box::new(source)))?;
        self.0
            .stop(code)
            .map_err(|source| ConnectivityError::Stream(Box::new(source)))?;
        Ok(())
    }
}
impl Drop for ReceiveHalf {
    fn drop(&mut self) {
        let _ = self.0.stop(0u32.into());
    }
}

#[cfg(test)]
#[path = "stream_blob_probe.rs"]
mod blob_probe;

#[cfg(test)]
#[path = "stream_progress_probe.rs"]
mod progress_probe;
