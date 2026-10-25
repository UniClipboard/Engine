//! 全局发送投影从原意图建立，再由真实已认证流登记和采用。
use super::*;
use uc_application::deps::{
    ProgressLiveRoundPort, ProgressProjectionAdmission, ProgressProjectionKey,
    ProgressProjectionRegistry,
};
use uc_core::file_transfer::{ProgressActive, ProgressFreshnessView, ProgressPhase};
use uc_sync_protocol::transfer_progress::v2::{
    ProducerDescriptor, RejectionReason, TaskScope, WireId,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_intent_authorizes_real_updates_and_terminal_in_the_global_registry(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        let fixture = Fixture::new(4, 4, TRANSFER_PROGRESS_ALPN).await?;
        let (registry, worker) = ProgressProjectionRegistry::new(Arc::new(SystemElapsedClock));
        let key = ProgressProjectionKey {
            peer: fixture.receiving.peer(),
            source: SOURCE,
        };
        let stop = CancellationToken::new();
        let running = tokio::spawn(worker.run(stop.clone()));
        let peer = fixture.sending.peer();
        let producer = producer(&fixture);
        let expected = Authority::default()
            .current_task(&peer, SOURCE)
            .unwrap()
            .descriptor;
        assert!(registry.snapshot(key).is_none());
        assert_eq!(
            registry.accept_intent(key, vec![item()])?,
            ProgressProjectionAdmission::Retained
        );
        let initial = registry.snapshot(key).unwrap();
        assert!(initial.bound.is_none());
        assert!(initial.items[0].1.is_none());
        assert_eq!(initial.freshness.unwrap(), ProgressFreshnessView::Waiting);
        let serve = async {
            for _ in 0..3 {
                TransferProgressInbound::serve(
                    fixture.receiving.accept_bi().await?,
                    &fixture.receiving,
                    1,
                    progress_budget(),
                    &stop,
                    &registry,
                )
                .await?;
                TransferProgressInbound::serve(
                    fixture.receiving.accept_bi().await?,
                    &fixture.receiving,
                    1,
                    progress_budget(),
                    &stop,
                    &registry,
                )
                .await?;
            }
            anyhow::Ok(())
        };
        let send = async {
            let active = ProgressActive {
                item: item(),
                round: 0,
                sequence: 1,
                bytes: 5,
                total: Some(10),
                phase: ProgressPhase::Transferring,
            };
            producer
                .report_live(&peer, expected, active, &progress_budget(), &stop)
                .await?;
            assert_eq!(
                registry.snapshot(key).unwrap().items[0].1,
                Some(ProgressItemState::Active(active))
            );
            producer
                .report_contact(&peer, expected, 1, &progress_budget(), &stop)
                .await?;
            assert_eq!(
                registry.snapshot(key).unwrap().items[0].1,
                Some(ProgressItemState::Active(active))
            );
            producer
                .report_settled(&peer, expected, terminal(10), &progress_budget(), &stop)
                .await?;
            assert_eq!(
                registry.snapshot(key).unwrap().freshness.unwrap(),
                ProgressFreshnessView::Settled
            );
            anyhow::Ok(())
        };
        let (sent, served) = tokio::join!(send, serve);
        sent?;
        served?;
        registry.remove(key);
        assert!(registry.snapshot(key).is_none());
        let rejected = async {
            TransferProgressInbound::serve(
                fixture.receiving.accept_bi().await?,
                &fixture.receiving,
                1,
                progress_budget(),
                &stop,
                &registry,
            )
            .await
            .map_err(anyhow::Error::from)
        };
        let (registered, rejected) =
            tokio::join!(producer.register_current(&peer, SOURCE, &stop), rejected);
        assert!(registered.is_err());
        assert!(matches!(
            rejected
                .as_ref()
                .err()
                .and_then(|source| source.downcast_ref::<ProgressReceiveError>()),
            Some(ProgressReceiveError::UnknownTask)
        ));
        assert!(registry.snapshots().is_empty());
        stop.cancel();
        running.await?;
        fixture.close().await?;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delayed_real_challenge_cannot_repopulate_a_replaced_or_closed_intent() -> anyhow::Result<()>
{
    timeout(Duration::from_secs(30), async {
        for closing in [false, true] {
            let fixture = Fixture::new(4, 4, TRANSFER_PROGRESS_ALPN).await?;
            let (registry, worker) = ProgressProjectionRegistry::new(Arc::new(SystemElapsedClock));
            let key = ProgressProjectionKey {
                peer: fixture.receiving.peer(),
                source: SOURCE,
            };
            registry.accept_intent(key, vec![item()])?;
            let stop = CancellationToken::new();
            let mut running = tokio::spawn(worker.run(stop.clone()));
            let mut client = fixture.sending.open_bi(Instant::now() + BUDGET).await?;
            write_frame(
                &mut client.send,
                &Envelope {
                    trace_context: None,
                    message: Message::Register {
                        source_entry: WireId(SOURCE.bytes()),
                    },
                },
            )
            .await?;
            let accepted = fixture.receiving.accept_bi().await?;
            let connection = fixture.receiving.clone();
            let owner = registry.clone();
            let server_stop = stop.clone();
            let serving = tokio::spawn(async move {
                TransferProgressInbound::serve(
                    accepted,
                    &connection,
                    1,
                    progress_budget(),
                    &server_stop,
                    &owner,
                )
                .await
            });
            let Message::Challenge { nonce } = read_frame(&mut client.receive).await?.message
            else {
                bail!("expected challenge");
            };
            if closing {
                registry.close();
                assert!(timeout(Duration::from_millis(30), &mut running)
                    .await
                    .is_err());
            } else {
                registry.remove(key);
                registry.accept_intent(key, vec![item()])?;
                assert!(registry.snapshot(key).unwrap().bound.is_none());
            }
            write_frame(
                &mut client.send,
                &Envelope {
                    trace_context: None,
                    message: Message::CurrentDescriptor {
                        challenge: nonce,
                        descriptor: ProducerDescriptor {
                            producer: WireId([2; 16]),
                            owner_revision: 1,
                            scope: TaskScope {
                                source_entry: WireId(SOURCE.bytes()),
                                attempt: WireId(ATTEMPT.bytes()),
                            },
                            item_count: 1,
                        },
                    },
                },
            )
            .await?;
            client.send.finish()?;
            assert!(matches!(
                serving.await?,
                Err(ProgressReceiveError::UnknownTask)
            ));
            assert!(matches!(
                read_frame(&mut client.receive).await?.message,
                Message::Rejected {
                    challenge,
                    reason: RejectionReason::UnknownTask,
                } if challenge == nonce
            ));
            if closing {
                timeout(Duration::from_secs(2), &mut running).await??;
                assert!(registry.snapshot(key).is_none());
            } else {
                assert!(registry.snapshot(key).unwrap().bound.is_none());
                stop.cancel();
                running.await?;
            }
            drop(client);
            fixture.close().await?;
        }
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}
