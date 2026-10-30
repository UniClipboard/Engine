//! 在原下载负责人中触发真实重试分支，核对轮次展示与原超时分别推进。
use super::{Connection, Grant, ReceiveHalf, SendHalf};
use crate::connectivity::{
    AdmissionAttempt, AdmissionGrant, ConnectivityBackend, ConnectivityError,
    InboundIdentityReservation, PeerConnection, SessionConnectionAdmissionPort, StreamLease,
};
use crate::transfer::blob::test_support::{retained, RejectUnexpectedNetwork};
use crate::transfer::blob::transport::{
    BlobDownloadPreparation, BlobDownloadResources, BlobExchangeError, BlobExchangeFailure,
    BlobExchangePort, BlobReadStorePort, BlobReceived, BlobShutdownWait, BlobSource,
    BlobSourcePort, BlobTransport, BlobValidatedProgress,
};
use crate::transfer::blob::BlobProgressSink;
use crate::transfer::receive::DownloadSettlement;
use async_trait::async_trait;
use bytes::Bytes;
use std::io;
use std::path::Path;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::sync::{mpsc, watch, Semaphore};
use tokio::time::{advance, Duration, Instant};
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;
use uc_core::ports::blob::{BlobDigest, BlobTicket};

struct Backend(Arc<Connection>);
#[async_trait]
impl ConnectivityBackend for Backend {
    async fn connect(
        &self,
        _: DeviceId,
        _: Option<Vec<u8>>,
        _: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        Ok(self.0.clone())
    }
}
struct Authorized;
#[async_trait]
impl SessionConnectionAdmissionPort for Authorized {
    fn begin(&self, _: DeviceId) -> Box<dyn AdmissionAttempt> {
        Box::new(Authorized)
    }
    async fn adopt_inbound(
        &self,
        _: Arc<dyn PeerConnection>,
        _: &dyn InboundIdentityReservation,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        panic!("outbound fixture")
    }
}
#[async_trait]
impl AdmissionAttempt for Authorized {
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        Ok(())
    }
    async fn adopt(
        &self,
        _: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        Ok(Box::new(Grant(DeviceId::new("peer-1"))))
    }
}
struct Source;
#[async_trait]
impl BlobSourcePort for Source {
    fn inspect(&self, _: &BlobTicket) -> Result<BlobSource, BlobExchangeError> {
        Ok(BlobSource {
            source_key: [1; 32],
            digest: BlobDigest::from_bytes([1; 32]),
            address_hint: Vec::new(),
        })
    }
    async fn resolve_peer(&self, _: &[u8; 32]) -> Result<DeviceId, BlobExchangeError> {
        Ok(DeviceId::new("peer-1"))
    }
}
struct MissingStore;
#[async_trait]
impl BlobReadStorePort for MissingStore {
    async fn protect(&self, _: BlobDigest) -> Result<Option<BlobReceived>, BlobExchangeError> {
        Ok(None)
    }
    async fn read(&self, _: BlobDigest) -> Result<Bytes, BlobExchangeError> {
        panic!("download only")
    }
    async fn export(&self, _: BlobDigest, _: &Path) -> Result<u64, BlobExchangeError> {
        panic!("download only")
    }
}
#[derive(Default)]
struct Sink {
    rounds: Mutex<Vec<(u32, u64)>>,
    highwater: Mutex<Vec<u64>>,
    retries: Mutex<Vec<u32>>,
    obtained: Mutex<Vec<(u32, u64)>>,
}
#[async_trait]
impl BlobProgressSink for Sink {
    fn observe_round(&self, round: u32, bytes: u64, _: Option<u64>) {
        self.rounds.lock().unwrap().push((round, bytes));
    }
    fn observe_retrying(&self, round: u32) {
        self.retries.lock().unwrap().push(round);
    }
    fn observe_obtained(&self, round: u32, size: u64) {
        self.obtained.lock().unwrap().push((round, size));
    }
    async fn report(&self, bytes: u64, _: Option<u64>) {
        self.highwater.lock().unwrap().push(bytes);
    }
}
struct Exchange {
    calls: AtomicUsize,
    replayed: Semaphore,
    finish: Semaphore,
}
#[async_trait]
impl BlobExchangePort for Exchange {
    async fn receive(
        &self,
        _: StreamLease,
        digest: BlobDigest,
        cancel: CancellationToken,
        progress: Arc<dyn BlobValidatedProgress>,
    ) -> Result<BlobReceived, BlobExchangeError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            progress.validated_through(80, 100)?;
            return Err(BlobExchangeError::new(
                BlobExchangeFailure::TransientNetwork,
                io::Error::from(io::ErrorKind::ConnectionReset),
            ));
        }
        progress.validated_through(20, 100)?;
        self.replayed.add_permits(1);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled)),
            permit = self.finish.acquire() => permit.unwrap().forget(),
        }
        progress.validated_through(100, 100)?;
        let mut content = retained(digest);
        content.size = 100;
        Ok(content)
    }
    async fn provide(
        &self,
        _: StreamLease,
        _: Instant,
        _: CancellationToken,
    ) -> Result<(), BlobExchangeError> {
        panic!("outbound fixture")
    }
}

#[tokio::test(start_paused = true)]
async fn actual_retry_replays_current_round_without_refreshing_original_stall_deadline() {
    for expire in [false, true] {
        let (send, streams) = mpsc::channel(2);
        for _ in 0..2 {
            let reset = Arc::new(Semaphore::new(0));
            send.try_send((
                Box::new(SendHalf(reset.clone())) as _,
                Box::new(ReceiveHalf(reset)) as _,
            ))
            .unwrap();
        }
        let connection = Arc::new(Connection {
            id: 1,
            peer: 1,
            streams: tokio::sync::Mutex::new(streams),
            closed: watch::channel(false).0,
        });
        let exchange = Arc::new(Exchange {
            calls: AtomicUsize::new(0),
            replayed: Semaphore::new(0),
            finish: Semaphore::new(0),
        });
        let transport = BlobTransport::new(
            Arc::new(Backend(connection)),
            Arc::new(Authorized),
            exchange.clone(),
            Arc::new(RejectUnexpectedNetwork),
        );
        let downloads = Arc::new(BlobDownloadResources {
            transport: transport.clone(),
            source: Arc::new(Source),
            store: Arc::new(MissingStore),
        });
        let sink = Arc::new(Sink::default());
        let progress: Arc<dyn BlobProgressSink> = sink.clone();
        let cancel = CancellationToken::new();
        let prepared = downloads
            .prepare(&BlobTicket::from_bytes(vec![1]), &cancel)
            .unwrap();
        let start = Instant::now();
        let task = tokio::spawn(async move {
            downloads
                .obtain(
                    prepared,
                    &cancel,
                    BlobDownloadPreparation {
                        work: Box::pin(async { Ok(Some(progress)) }),
                        stop: Arc::new(Mutex::new(None)),
                    },
                    DownloadSettlement::default(),
                )
                .await
        });
        exchange.replayed.acquire().await.unwrap().forget();
        assert_eq!(*sink.rounds.lock().unwrap(), [(0, 80), (1, 0), (1, 20)]);
        assert!(sink
            .highwater
            .lock()
            .unwrap()
            .iter()
            .all(|value| *value == 80));
        assert_eq!(*sink.retries.lock().unwrap(), [0, 1]);
        if expire {
            // 重传20字节未越过原80字节高水位；期限仍从第一次有效数据起算。
            advance((start + Duration::from_secs(300)).saturating_duration_since(Instant::now()))
                .await;
            let error = match task.await.unwrap() {
                Err(error) => error,
                Ok(_) => panic!("replay prolonged the original budget"),
            };
            assert_eq!(error.kind, BlobExchangeFailure::RequestExpired);
            assert!(sink.obtained.lock().unwrap().is_empty());
            assert_eq!(*sink.rounds.lock().unwrap(), [(0, 80), (1, 0), (1, 20)]);
        } else {
            exchange.finish.add_permits(1);
            drop(task.await.unwrap().unwrap());
            assert_eq!(*sink.obtained.lock().unwrap(), [(1, 100)]);
            assert_eq!(
                *sink.rounds.lock().unwrap(),
                [(0, 80), (1, 0), (1, 20), (1, 100)]
            );
        }
        assert_eq!(exchange.calls.load(Ordering::SeqCst), 2);
        transport
            .shutdown(BlobShutdownWait::Complete)
            .await
            .unwrap();
    }
}
