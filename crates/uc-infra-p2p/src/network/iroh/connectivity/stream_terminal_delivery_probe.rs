//! 同一原生绑定中丢ACK、重复、冲突和乱序不能重复发布或回退已采用事实。
use super::super::super::profile_probe::ProfilePair;
use super::super::UnusedSnapshotProvider;
use super::{item, progress_budget, projection, serve, terminal, AdoptedAck, Authority, SOURCE};
use crate::network::iroh::transfer_progress_v2::TransferProgressWire;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast::error::TryRecvError, Notify, Semaphore};
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ProgressProjectionEvent, ProgressReceiveError, ProgressTerminalNotice, ProgressUpdateNotice,
    ProgressWirePort, TransferProgressProducer,
};
use uc_core::file_transfer::{
    ProgressActive, ProgressFailureReason, ProgressFreshnessView, ProgressItemState, ProgressPhase,
    ProgressTerminal, ProgressTerminalOutcome,
};
use uc_infra_local::SystemElapsedClock;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_native_ack_and_event_replay_exact_result_without_duplicate_changes_or_terminal_rollback(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        for lose_event in [false, true] {
            let pair = ProfilePair::new().await?;
            let (registry, key, _worker) = projection(&pair)?;
            let stop = CancellationToken::new();
            let producer = TransferProgressProducer::new(
                pair.outbound.clone(),
                Arc::new(TransferProgressWire),
                Arc::new(Authority::default()),
                Arc::new(UnusedSnapshotProvider),
                Arc::new(SystemElapsedClock),
            );
            let (registered, served) = tokio::join!(
                producer.register_current(&pair.peer, SOURCE, &stop),
                serve(&pair, &registry)
            );
            served?;
            let bound = registered?;
            let connection = pair
                .outbound
                .request(pair.peer)
                .connect(Instant::now() + Duration::from_secs(5), &stop)
                .await?;
            let mut events = Some(registry.subscribe());
            let update = ProgressUpdateNotice {
                binding: bound.binding,
                source: SOURCE,
                attempt: bound.descriptor.attempt,
                revision: bound.descriptor.revision,
                update: ProgressActive {
                    item: item(),
                    round: 1,
                    sequence: 2,
                    bytes: 5,
                    total: Some(10),
                    phase: ProgressPhase::Transferring,
                },
            };
            let initial = registry.snapshot(key).unwrap().version;
            for changed in [true, false] {
                let budget = progress_budget();
                let mut stream = connection.open_bi(budget.instant()).await?;
                let (sent, served) = tokio::join!(
                    TransferProgressWire.update(&mut stream, &connection, update, &budget, &stop),
                    serve(&pair, &registry)
                );
                sent?;
                served?;
                assert_eq!(registry.snapshot(key).unwrap().version, initial + 1);
                if changed {
                    assert_eq!(
                        events.as_mut().unwrap().try_recv()?,
                        ProgressProjectionEvent::Changed(key)
                    );
                }
                assert!(matches!(
                    events.as_mut().unwrap().try_recv(),
                    Err(TryRecvError::Empty)
                ));
            }
            for stale in [
                ProgressActive {
                    sequence: 1,
                    ..update.update
                },
                ProgressActive {
                    round: 0,
                    sequence: u64::MAX,
                    ..update.update
                },
            ] {
                let budget = progress_budget();
                let mut stream = connection.open_bi(budget.instant()).await?;
                let (_sent, served) = tokio::join!(
                    TransferProgressWire.update(
                        &mut stream,
                        &connection,
                        ProgressUpdateNotice {
                            update: stale,
                            ..update
                        },
                        &budget,
                        &stop
                    ),
                    serve(&pair, &registry)
                );
                assert!(matches!(served, Err(ProgressReceiveError::Stale)));
                assert_eq!(registry.snapshot(key).unwrap().version, initial + 1);
                assert!(matches!(
                    events.as_mut().unwrap().try_recv(),
                    Err(TryRecvError::Empty)
                ));
            }
            let notice = ProgressTerminalNotice {
                binding: bound.binding,
                source: SOURCE,
                attempt: bound.descriptor.attempt,
                revision: bound.descriptor.revision,
                result: ProgressTerminal {
                    round: 1,
                    ..terminal(10)
                },
            };
            let owner = AdoptedAck {
                registry: registry.clone(),
                entered: Notify::new(),
                release: Semaphore::new(0),
            };
            // 首次采用后销毁真实入站流，尚未编码的ACK随流丢失；不会制造一份伪ACK。
            if lose_event {
                drop(events.take());
            }
            let budget = progress_budget();
            let mut stream = connection.open_bi(budget.instant()).await?;
            let lose_ack = async {
                tokio::select! {
                    result = serve(&pair, &owner) => panic!("ACK屏障提前结束: {result:?}"),
                    () = owner.entered.notified() => {}
                }
                assert_eq!(
                    registry.snapshot(key).unwrap().items[0].1,
                    Some(ProgressItemState::Terminal(notice.result))
                );
            };
            let (sent, ()) = tokio::join!(
                TransferProgressWire.terminal(&mut stream, &connection, notice, &budget, &stop),
                lose_ack
            );
            assert!(sent.is_err(), "已采用不等于发送端已获得ACK");
            drop(stream);
            if lose_event {
                events = Some(registry.subscribe());
            } else {
                assert_eq!(
                    events.as_mut().unwrap().try_recv()?,
                    ProgressProjectionEvent::Changed(key)
                );
            }
            let settled = registry.snapshot(key).unwrap();
            assert_eq!(settled.version, initial + 2);
            assert_eq!(
                settled.freshness.as_ref().unwrap().to_owned(),
                ProgressFreshnessView::Settled
            );
            // 同一binding重放，不能借重新登记合法增长的版本掩盖重复发布。
            for _ in 0..3 {
                let budget = progress_budget();
                let mut stream = connection.open_bi(budget.instant()).await?;
                let (sent, served) = tokio::join!(
                    TransferProgressWire.terminal(&mut stream, &connection, notice, &budget, &stop),
                    serve(&pair, &registry)
                );
                served?;
                assert_eq!(sent?, notice);
                assert_eq!(registry.snapshot(key).unwrap().version, settled.version);
                assert!(matches!(
                    events.as_mut().unwrap().try_recv(),
                    Err(TryRecvError::Empty)
                ));
            }
            for result in [
                ProgressTerminal {
                    bytes: 9,
                    ..notice.result
                },
                ProgressTerminal {
                    result_revision: 2,
                    ..notice.result
                },
                ProgressTerminal {
                    round: 2,
                    ..notice.result
                },
                ProgressTerminal {
                    sequence: 999,
                    ..notice.result
                },
                ProgressTerminal {
                    outcome: ProgressTerminalOutcome::Failed(
                        ProgressFailureReason::StorageUnavailable,
                    ),
                    ..notice.result
                },
            ] {
                let budget = progress_budget();
                let mut stream = connection.open_bi(budget.instant()).await?;
                let (sent, served) = tokio::join!(
                    TransferProgressWire.terminal(
                        &mut stream,
                        &connection,
                        ProgressTerminalNotice { result, ..notice },
                        &budget,
                        &stop
                    ),
                    serve(&pair, &registry)
                );
                assert!(matches!(sent, Err(ProgressReceiveError::InvalidPlan)));
                assert!(matches!(served, Err(ProgressReceiveError::Conflict)));
                let view = registry.snapshot(key).unwrap();
                assert_eq!(view.items, settled.items);
                assert_eq!(view.summary, settled.summary);
                assert_eq!(view.version, settled.version);
                assert!(matches!(
                    events.as_mut().unwrap().try_recv(),
                    Err(TryRecvError::Empty)
                ));
            }
            let budget = progress_budget();
            let mut stream = connection.open_bi(budget.instant()).await?;
            let (_sent, served) = tokio::join!(
                TransferProgressWire.update(
                    &mut stream,
                    &connection,
                    ProgressUpdateNotice {
                        update: ProgressActive {
                            round: 999,
                            sequence: 999,
                            bytes: 10,
                            ..update.update
                        },
                        ..update
                    },
                    &budget,
                    &stop
                ),
                serve(&pair, &registry)
            );
            assert!(matches!(served, Err(ProgressReceiveError::Stale)));
            let view = registry.snapshot(key).unwrap();
            assert_eq!(view.items, settled.items);
            assert_eq!(view.version, settled.version);
            assert_eq!(view.freshness.unwrap(), ProgressFreshnessView::Settled);
            assert!(matches!(
                events.as_mut().unwrap().try_recv(),
                Err(TryRecvError::Empty)
            ));
            drop(stream);
            drop(connection);
            pair.close().await?;
        }
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}
