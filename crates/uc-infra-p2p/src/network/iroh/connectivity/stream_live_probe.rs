//! 最新中间值经真实登记与Update采用；终态绕过中间值节流并永久封口。
use super::*;
use tokio::sync::mpsc;
use uc_application::deps::{
    ProgressExchangeService, ProgressLiveRoundPort, ProgressSubmission,
    TransferProgressSnapshotReader,
};
use uc_core::file_transfer::{ProgressActive, ProgressFreshnessView, ProgressPhase};

struct Millis(AtomicU64);
impl ElapsedClockPort for Millis {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        Ok(Duration::from_millis(self.0.load(Ordering::SeqCst)))
    }
}
fn active(sequence: u64) -> ProgressActive {
    ProgressActive {
        item: item(),
        round: 0,
        sequence,
        bytes: sequence.min(10),
        total: Some(10),
        phase: if sequence < 10 {
            ProgressPhase::Transferring
        } else {
            ProgressPhase::Processing
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_live_updates_coalesce_on_real_streams_and_terminal_seals_projection(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        let fixture = Fixture::new(4, 4, TRANSFER_PROGRESS_ALPN).await?;
        let owner = owner(&fixture)?;
        let authority = Arc::new(Authority::default());
        let clock = Arc::new(Millis(AtomicU64::new(0)));
        let wire = Arc::new(TransferProgressWire);
        let producer = Arc::new(TransferProgressProducer::new(
            fixture.outbound.clone(),
            wire.clone(),
            authority.clone(),
            clock.clone(),
        ));
        let snapshot = Arc::new(TransferProgressSnapshotReader::new(
            fixture.outbound.clone(),
            wire,
        ));
        let (service, worker) = ProgressExchangeService::new(
            clock.clone(),
            authority,
            producer.clone(),
            snapshot,
            producer,
        );
        let stop = CancellationToken::new();
        let running = tokio::spawn(worker.run(stop.clone()));
        let peer = fixture.sending.peer();
        let (tx, mut rx) = mpsc::channel(3);
        let serve = async {
            for _ in 0..3 {
                serve_registration(&fixture, &owner, &stop).await?;
                TransferProgressInbound::serve(
                    fixture.receiving.accept_bi().await?,
                    &fixture.receiving,
                    1,
                    progress_budget(),
                    &stop,
                    &owner,
                )
                .await?;
                tx.send(owner.snapshot().items[0].1).await?;
            }
            anyhow::Ok(())
        };
        let send = async {
            assert_eq!(
                service.submit_live(peer, SOURCE, ATTEMPT, active(1))?,
                ProgressSubmission::Retained
            );
            assert_eq!(
                rx.recv().await.unwrap(),
                Some(ProgressItemState::Active(active(1)))
            );
            for seq in 2..=100 {
                service.submit_live(peer, SOURCE, ATTEMPT, active(seq))?;
            }
            clock.0.store(199, Ordering::SeqCst);
            service.network_recovered(Some(peer));
            assert!(timeout(Duration::from_millis(60), rx.recv()).await.is_err());
            clock.0.store(400, Ordering::SeqCst);
            service.network_recovered(Some(peer));
            assert_eq!(
                rx.recv().await.unwrap(),
                Some(ProgressItemState::Active(active(100)))
            );
            // 同一时点直接提交真实已结算结果，不等下一份中间值资格。
            assert_eq!(
                service.submit_settled(peer, SOURCE, ATTEMPT, terminal(10))?,
                ProgressSubmission::Retained
            );
            assert_eq!(
                rx.recv().await.unwrap(),
                Some(ProgressItemState::Terminal(terminal(10)))
            );
            assert_eq!(
                service.submit_live(peer, SOURCE, ATTEMPT, active(101))?,
                ProgressSubmission::Refused(ProgressClaimRefusal::AlreadyObserved)
            );
            anyhow::Ok(())
        };
        let (sent, served) = tokio::join!(send, serve);
        stop.cancel();
        running.await?;
        sent?;
        served?;
        // 即使绕过发送侧资格，真实采用负责人也拒绝终态后的迟到Update。
        let sealed = owner.snapshot();
        let bound = sealed.bound.unwrap();
        assert!(matches!(
            owner
                .adopt_update(
                    &fixture.receiving.peer(),
                    1,
                    ProgressUpdateNotice {
                        binding: bound.binding,
                        source: SOURCE,
                        attempt: ATTEMPT,
                        revision: bound.descriptor.revision,
                        update: active(102),
                    }
                )
                .await,
            Err(ProgressReceiveError::Stale)
        ));
        assert_eq!(owner.snapshot().version, sealed.version);
        assert_eq!(owner.snapshot().items, sealed.items);
        fixture.close().await?;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_task_contact_uses_real_streams_and_old_sequence_cannot_renew_display(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        let fixture = Fixture::new(4, 4, TRANSFER_PROGRESS_ALPN).await?;
        let clock = Arc::new(Millis(AtomicU64::new(0)));
        let owner = TransferProgressReceiver::new(
            fixture.receiving.peer(),
            SOURCE,
            vec![item()],
            clock.clone(),
        )?;
        let authority = Arc::new(Authority::default());
        let peer = fixture.sending.peer();
        let expected = authority.current_task(&peer, SOURCE).unwrap().descriptor;
        let wire = Arc::new(TransferProgressWire);
        let producer = Arc::new(TransferProgressProducer::new(
            fixture.outbound.clone(),
            wire.clone(),
            authority.clone(),
            clock.clone(),
        ));
        let (service, worker) = ProgressExchangeService::new(
            clock.clone(),
            authority,
            producer.clone(),
            Arc::new(TransferProgressSnapshotReader::new(
                fixture.outbound.clone(),
                wire,
            )),
            producer.clone(),
        );
        service.track_current(peer, SOURCE, ATTEMPT)?;
        let stop = CancellationToken::new();
        let running = tokio::spawn(worker.run(stop.clone()));
        let serve = async {
            serve_registration(&fixture, &owner, &stop).await?;
            TransferProgressInbound::serve(
                fixture.receiving.accept_bi().await?,
                &fixture.receiving,
                1,
                progress_budget(),
                &stop,
                &owner,
            )
            .await?;
            anyhow::Ok(())
        };
        clock.0.store(10000, Ordering::SeqCst);
        service.network_recovered(Some(peer));
        let served = serve.await;
        stop.cancel();
        running.await?;
        served?;
        assert_eq!(
            owner.snapshot().freshness.unwrap(),
            ProgressFreshnessView::Live
        );
        assert!(owner.snapshot().items[0].1.is_none());
        clock.0.store(70000, Ordering::SeqCst);
        assert_eq!(
            owner.snapshot().freshness.unwrap(),
            ProgressFreshnessView::Unconfirmed
        );
        let bound = owner.snapshot().bound.unwrap();
        owner
            .adopt_contact(
                &fixture.receiving.peer(),
                1,
                ProgressContactNotice {
                    binding: bound.binding,
                    source: SOURCE,
                    attempt: ATTEMPT,
                    revision: expected.revision,
                    sequence: 1,
                },
            )
            .await?;
        assert_eq!(
            owner.snapshot().freshness.unwrap(),
            ProgressFreshnessView::Unconfirmed
        );
        let cancel = CancellationToken::new();
        let deadline = progress_budget();
        let serve = async {
            serve_registration(&fixture, &owner, &cancel).await?;
            TransferProgressInbound::serve(
                fixture.receiving.accept_bi().await?,
                &fixture.receiving,
                1,
                progress_budget(),
                &cancel,
                &owner,
            )
            .await?;
            anyhow::Ok(())
        };
        let (sent, served) = tokio::join!(
            producer.report_contact(&peer, expected, 2, &deadline, &cancel),
            serve
        );
        sent?;
        served?;
        assert_eq!(
            owner.snapshot().freshness.unwrap(),
            ProgressFreshnessView::Live
        );
        assert!(owner.snapshot().items[0].1.is_none());
        let bound = owner.snapshot().bound.unwrap();
        owner
            .adopt_terminal(
                &fixture.receiving.peer(),
                1,
                ProgressTerminalNotice {
                    binding: bound.binding,
                    source: SOURCE,
                    attempt: ATTEMPT,
                    revision: expected.revision,
                    result: terminal(0),
                },
            )
            .await?;
        assert_eq!(
            owner.snapshot().freshness.unwrap(),
            ProgressFreshnessView::Settled
        );
        assert!(matches!(
            owner
                .adopt_contact(
                    &fixture.receiving.peer(),
                    1,
                    ProgressContactNotice {
                        binding: bound.binding,
                        source: SOURCE,
                        attempt: ATTEMPT,
                        revision: expected.revision,
                        sequence: 3
                    }
                )
                .await,
            Err(ProgressReceiveError::Stale)
        ));
        fixture.close().await?;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}
