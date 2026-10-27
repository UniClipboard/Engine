//! 同一终态在真实连接重建后仍受原保留期限控制，丢ACK不创造新任务。
use super::super::super::progress_probe::UnusedLive;
use super::*;
use uc_application::deps::{
    ProgressClaimRefusal, ProgressExchangeDeadline, ProgressExchangeService, ProgressSourceTask,
    ProgressSubmission, ProgressTaskAuthorityPort, ProgressTaskDescriptor,
    ProgressTerminalRoundPort, TransferProgressSnapshotReader,
};
use uc_core::file_transfer::ProgressItem;

struct SettledSource;
impl ProgressTaskAuthorityPort for SettledSource {
    fn current_task(&self, _: &DeviceId, source: ProgressIdentity) -> Option<ProgressSourceTask> {
        (source == SOURCE).then(|| ProgressSourceTask {
            descriptor: descriptor(),
            items: Arc::from([item()]),
            notification_closed: Default::default(),
        })
    }
    fn claim_terminal(
        &self,
        peer: &DeviceId,
        expected: ProgressTaskDescriptor,
        actual: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal> {
        assert_eq!(actual, item());
        assert!(expected == descriptor());
        Ok(self.current_task(peer, SOURCE).unwrap())
    }
}
struct Started {
    connection: usize,
    at: u64,
    deadline: ProgressExchangeDeadline,
}
struct ObservedRounds {
    producer: TransferProgressProducer,
    manager: ConnectivityManager,
    clock: Arc<Clock>,
    started: mpsc::Sender<Started>,
    finished: mpsc::Sender<bool>,
}
#[async_trait]
impl ProgressTerminalRoundPort for ObservedRounds {
    async fn report_settled(
        &self,
        peer: &DeviceId,
        expected: ProgressTaskDescriptor,
        actual: ProgressTerminal,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        assert!(expected == descriptor());
        assert_eq!(actual, result());
        let lease = self
            .manager
            .request(*peer)
            .connect(deadline.instant(), cancel)
            .await?;
        self.started
            .send(Started {
                connection: lease.connection_id(),
                at: self.clock.0.load(Ordering::SeqCst),
                deadline: deadline.clone(),
            })
            .await
            .unwrap();
        let result = self
            .producer
            .report_settled(peer, expected, actual, deadline, cancel)
            .await;
        self.finished.send(result.is_ok()).await.unwrap();
        result
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_reconnection_keeps_original_terminal_retention_and_does_not_replay_expired_result(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        let context = ProjectionContext::new();
        let mut pair = Fixture::new(false, context.registry.clone()).await?;
        pair.adoption.lose_ack.store(true, Ordering::SeqCst);
        let clock = pair.clock.clone();
        let authority = Arc::new(SettledSource);
        let producer = TransferProgressProducer::new(
            pair.outbound.clone(),
            Arc::new(TransferProgressWire),
            authority.clone(),
            Arc::new(Facts { peer: pair.peer }),
            clock.clone(),
        );
        let snapshot = Arc::new(TransferProgressSnapshotReader::new(
            pair.outbound.clone(),
            Arc::new(TransferProgressWire),
        ));
        let (started, mut starts) = mpsc::channel(8);
        let (finished, mut finishes) = mpsc::channel(8);
        let rounds = Arc::new(ObservedRounds {
            producer,
            manager: pair.outbound.clone(),
            clock: clock.clone(),
            started,
            finished,
        });
        let (service, worker) = ProgressExchangeService::new(
            clock.clone(),
            authority,
            rounds,
            snapshot,
            Arc::new(UnusedLive),
        );
        let stop = CancellationToken::new();
        assert_eq!(
            service.submit_settled(pair.peer, descriptor(), result())?,
            ProgressSubmission::Retained
        );
        let running = tokio::spawn(worker.run(stop.clone()));
        let first = timeout(BUDGET, starts.recv()).await?.unwrap();
        assert_eq!(first.at, 0);
        assert!(!timeout(BUDGET, finishes.recv()).await?.unwrap());
        assert_eq!(pair.adoption.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            pair.registry
                .snapshot(pair.key)
                .unwrap()
                .summary
                .counts
                .completed,
            1
        );
        let old = pair
            .outbound
            .request(pair.peer)
            .connect(Instant::now() + BUDGET, &stop)
            .await?;
        assert_eq!(old.connection_id(), first.connection);
        old.close();
        old.closed().await;
        drop(old);
        timeout(BUDGET, pair.done.recv()).await?.unwrap();
        // 同一producer/attempt/result只做连接重建，未重新提交终态或创建发送意图。
        pair.adoption.paused.store(true, Ordering::SeqCst);
        clock.0.store(595, Ordering::SeqCst);
        for _ in 0..20 {
            service.network_recovered(Some(pair.peer));
        }
        let second = timeout(BUDGET, starts.recv()).await?.unwrap();
        assert_eq!(second.at, 595);
        assert_ne!(first.connection, second.connection);
        pair.adoption.entered.acquire().await?.forget();
        assert_eq!(pair.adoption.calls.load(Ordering::SeqCst), 2);
        clock.0.store(599, Ordering::SeqCst);
        assert!(second.deadline.check(&stop).is_ok());
        clock.0.store(600, Ordering::SeqCst);
        service.network_recovered(Some(pair.peer));
        assert!(second.deadline.check(&stop).is_err());
        assert!(!timeout(BUDGET, finishes.recv()).await?.unwrap());
        pair.adoption.release.add_permits(1);
        for _ in 0..20 {
            service.network_recovered(Some(pair.peer));
        }
        assert!(timeout(Duration::from_millis(150), starts.recv())
            .await
            .is_err());
        assert_eq!(pair.adoption.calls.load(Ordering::SeqCst), 2);
        stop.cancel();
        timeout(BUDGET, running).await??;
        pair.close().await?;
        context.close().await
    })
    .await?
}
