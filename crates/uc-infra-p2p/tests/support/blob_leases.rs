//! SDK故障探针复用通用租约，并可单独注入进度或停止故障。
#[path = "blob_leases/lease.rs"]
mod lease_owner;
pub use lease_owner::lease;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::Notify;
use uc_application::deps::{
    BlobExchangeError, BlobValidatedProgress, ConnectivityError, PeerReceiveStream, StreamLease,
};

pub struct Progress(pub Arc<Notify>);
impl BlobValidatedProgress for Progress {
    fn validated_through(&self, end: u64, _: u64) -> Result<(), BlobExchangeError> {
        if end > 0 {
            self.0.notify_one();
        }
        Ok(())
    }
}

struct RefusingStop(Box<dyn PeerReceiveStream>);
impl AsyncRead for RefusingStop {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}
impl PeerReceiveStream for RefusingStop {
    fn id(&self) -> u64 {
        self.0.id()
    }
    fn stop(&mut self, _: u64) -> Result<(), ConnectivityError> {
        Err(ConnectivityError::Stream(
            io::Error::from(io::ErrorKind::PermissionDenied).into(),
        ))
    }
}
pub fn refuse_receive_stop(mut stream: StreamLease) -> StreamLease {
    stream.receive = Box::new(RefusingStop(stream.receive));
    stream
}
