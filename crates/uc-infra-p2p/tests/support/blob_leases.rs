//! SDK 工作结算探针的预连接 fixture；身份只匹配测试握手 key，不替代真实授权测试。
use async_trait::async_trait;
use futures_util::future::BoxFuture;
use iroh::endpoint::{Connection, RecvStream, SendStream, VarInt};
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::Notify;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    AdmissionAttempt, AdmissionGrant, BlobExchangeError, BlobValidatedProgress, ConnectionLease,
    ConnectionMode, ConnectionProtocol, ConnectivityBackend, ConnectivityError,
    ConnectivityManager, ConnectivityPolicy, InboundIdentityReservation, PeerConnection,
    PeerReceiveStream, PeerSendStream, SessionConnectionAdmissionPort, StreamLease, StreamPair,
};
use uc_core::ids::DeviceId;
use uc_core::ports::ConnectionChannel;

struct Native(Connection);
#[async_trait]
impl ConnectivityBackend for Native {
    async fn connect(
        &self,
        _: DeviceId,
        _address_hint: Option<Vec<u8>>,
        _: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        Ok(Arc::new(Native(self.0.clone())))
    }
}
#[async_trait]
impl PeerConnection for Native {
    fn id(&self) -> usize {
        self.0.stable_id()
    }
    fn authenticated_key(&self) -> [u8; 32] {
        *self.0.remote_id().as_bytes()
    }
    fn mode(&self) -> ConnectionMode {
        ConnectionMode::Multiplexed
    }
    fn protocol(&self) -> ConnectionProtocol {
        ConnectionProtocol::Blob
    }
    fn channel(&self) -> ConnectionChannel {
        ConnectionChannel::Unknown
    }
    fn is_closed(&self) -> bool {
        self.0.close_reason().is_some()
    }
    fn close(&self) {
        self.0.close(0u32.into(), b"probe closed");
    }
    async fn closed(&self) {
        self.0.closed().await;
    }
    async fn open_bi(&self) -> Result<StreamPair, ConnectivityError> {
        let (send, receive) = self
            .0
            .open_bi()
            .await
            .map_err(|source| ConnectivityError::Stream(source.into()))?;
        Ok((Box::new(Sending(send)), Box::new(Receiving(receive))))
    }
    async fn accept_bi(&self) -> Result<StreamPair, ConnectivityError> {
        let (send, receive) = self
            .0
            .accept_bi()
            .await
            .map_err(|source| ConnectivityError::Stream(source.into()))?;
        Ok((Box::new(Sending(send)), Box::new(Receiving(receive))))
    }
}
struct Sending(SendStream);
impl AsyncWrite for Sending {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.0), cx, data)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.0), cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.0), cx)
    }
}
impl PeerSendStream for Sending {
    fn id(&self) -> u64 {
        self.0.id().index()
    }
    fn reset(&mut self, code: u64) -> Result<(), ConnectivityError> {
        let code =
            VarInt::from_u64(code).map_err(|source| ConnectivityError::Stream(source.into()))?;
        self.0
            .reset(code)
            .map_err(|source| ConnectivityError::Stream(source.into()))
    }
    fn finish(&mut self) -> Result<(), ConnectivityError> {
        self.0
            .finish()
            .map_err(|source| ConnectivityError::Stream(source.into()))
    }
    fn settled(&mut self) -> BoxFuture<'_, Result<(), ConnectivityError>> {
        Box::pin(async {
            self.0
                .stopped()
                .await
                .map_err(|source| ConnectivityError::Stream(source.into()))?;
            Ok(())
        })
    }
}
struct Receiving(RecvStream);
impl AsyncRead for Receiving {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        AsyncRead::poll_read(Pin::new(&mut self.0), cx, buf)
    }
}
impl PeerReceiveStream for Receiving {
    fn id(&self) -> u64 {
        self.0.id().index()
    }
    fn stop(&mut self, code: u64) -> Result<(), ConnectivityError> {
        let code =
            VarInt::from_u64(code).map_err(|source| ConnectivityError::Stream(source.into()))?;
        self.0
            .stop(code)
            .map_err(|source| ConnectivityError::Stream(source.into()))
    }
}
#[derive(Clone)]
struct Identity([u8; 32]);
#[async_trait]
impl AdmissionGrant for Identity {
    fn peer(&self) -> DeviceId {
        DeviceId::new("sdk-work-probe")
    }
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        Ok(())
    }
}
#[async_trait]
impl AdmissionAttempt for Identity {
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        Ok(())
    }
    async fn adopt(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        if connection.authenticated_key() != self.0 {
            return Err(ConnectivityError::Denied);
        }
        Ok(Box::new(self.clone()))
    }
}
#[async_trait]
impl SessionConnectionAdmissionPort for Identity {
    fn begin(&self, _: DeviceId) -> Box<dyn AdmissionAttempt> {
        Box::new(self.clone())
    }
    async fn adopt_inbound(
        &self,
        connection: Arc<dyn PeerConnection>,
        reservation: &dyn InboundIdentityReservation,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        reservation.identified(self.peer())?;
        self.adopt(connection).await
    }
}
pub async fn lease(
    connection: Connection,
    inbound: bool,
) -> anyhow::Result<(ConnectivityManager, ConnectionLease)> {
    let identity = Identity(*connection.remote_id().as_bytes());
    let manager = ConnectivityManager::new(
        Arc::new(Native(connection.clone())),
        Arc::new(identity),
        ConnectivityPolicy::default(),
    );
    let lease = if inbound {
        manager.accept(Arc::new(Native(connection))).await?
    } else {
        manager
            .request(DeviceId::new("sdk-work-probe"))
            .connect(
                Instant::now() + Duration::from_secs(10),
                &CancellationToken::new(),
            )
            .await?
    };
    Ok((manager, lease))
}
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
