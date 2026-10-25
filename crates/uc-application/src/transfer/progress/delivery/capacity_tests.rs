//! 同一监督器的中间值淘汰不能占用终态保留，实际worker继续遵守优先级与收尾许可。
use super::{authority, id, peer, until, value, Millis, UnusedSnapshot};
use crate::transfer::progress::{
    ProgressExchangeDeadline, ProgressExchangeService, ProgressLiveRoundPort, ProgressReceiveError,
    ProgressSubmission, ProgressTaskDescriptor, ProgressTerminalRoundPort,
};
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};
use tokio::time::{timeout, Duration};
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{ProgressActive, ProgressTerminal, ProgressTerminalOutcome};
use uc_core::ids::DeviceId;

struct TerminalFirst {
    started: mpsc::Sender<(DeviceId, ProgressTerminal)>,
    finish: Arc<Semaphore>,
}
#[async_trait]
impl ProgressTerminalRoundPort for TerminalFirst {
    async fn report_settled(
        &self,
        peer: &DeviceId,
        _: ProgressTaskDescriptor,
        result: ProgressTerminal,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        deadline.check(cancel)?;
        self.started.send((*peer, result)).await.unwrap();
        // 即使封口也要等真实收尾，不以任务取消提前释放本轮许可。
        self.finish.acquire().await.unwrap().forget();
        Ok(())
    }
}
#[async_trait]
impl ProgressLiveRoundPort for TerminalFirst {
    async fn report_contact(
        &self,
        _: &DeviceId,
        _: ProgressTaskDescriptor,
        _: u64,
        _: &ProgressExchangeDeadline,
        _: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        panic!("logical time has not reached the contact interval");
    }
    async fn report_live(
        &self,
        _: &DeviceId,
        _: ProgressTaskDescriptor,
        _: ProgressActive,
        _: &ProgressExchangeDeadline,
        _: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        panic!("pending terminal results must precede ordinary intermediate values");
    }
}
fn settled(index: u16) -> ProgressTerminal {
    ProgressTerminal {
        item: value(index, 1).item,
        round: 0,
        sequence: 1,
        result_revision: 1,
        bytes: 2,
        total: Some(2),
        outcome: ProgressTerminalOutcome::Completed,
    }
}
#[tokio::test]
async fn saturated_intermediate_cache_cannot_evict_terminal_results_or_bypass_priority_and_close() {
    let clock = Arc::new(Millis(AtomicU64::new(0)));
    let finish = Arc::new(Semaphore::new(0));
    let (started, mut calls) = mpsc::channel(8);
    let round = Arc::new(TerminalFirst {
        started,
        finish: finish.clone(),
    });
    let (service, worker) = ProgressExchangeService::new(
        clock.clone(),
        authority(&[(1, 1024), (2, 1024), (3, 1024), (4, 1024)]),
        round.clone(),
        Arc::new(UnusedSnapshot),
        round,
    );
    for p in 1..=4 {
        for item in 0..64 {
            assert_eq!(
                service
                    .submit_live(peer(p), id(p), id(99), value(item, 1))
                    .unwrap(),
                ProgressSubmission::Retained
            );
        }
        for item in 512..576 {
            assert_eq!(
                service
                    .submit_settled(peer(p), id(p), id(99), settled(item))
                    .unwrap(),
                ProgressSubmission::Retained
            );
        }
    }
    let retained = {
        let state = service.inner.state.lock().unwrap();
        assert_eq!(state.live.len(), 256);
        assert_eq!(state.entries.len(), 256);
        assert_eq!(state.ledger.len(), 256);
        state
            .entries
            .iter()
            .map(|(key, entry)| (*key, entry.result, entry.generated))
            .collect::<Vec<_>>()
    };
    // 用同一原计划的新项淘汰全部旧中间值；独立终态的结果和生成时刻不能改变。
    clock.0.store(1000, Ordering::SeqCst);
    for p in 1..=4 {
        for item in 64..128 {
            assert_eq!(
                service
                    .submit_live(peer(p), id(p), id(99), value(item, 1))
                    .unwrap(),
                ProgressSubmission::Retained
            );
        }
        assert_eq!(
            service
                .submit_live(peer(p), id(p), id(99), value(0, 1))
                .unwrap(),
            ProgressSubmission::Unchanged
        );
        for sequence in 2..=100 {
            service
                .submit_live(peer(p), id(p), id(99), value(127, sequence))
                .unwrap();
        }
    }
    {
        let state = service.inner.state.lock().unwrap();
        assert_eq!(state.live.len(), 256);
        assert_eq!(state.entries.len(), 256);
        assert_eq!(state.ledger.len(), 256);
        for p in 1..=4 {
            assert_eq!(
                state.live.keys().filter(|key| key.peer == peer(p)).count(),
                64
            );
        }
        for (key, result, generated) in retained {
            let current = state.entries.get(&key).unwrap();
            assert_eq!(current.result, result);
            assert_eq!(current.generated, generated);
        }
    }
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    let mut peers = HashSet::new();
    for _ in 0..4 {
        let (peer, result) = timeout(Duration::from_secs(2), calls.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(peers.insert(peer));
        assert_eq!(result, settled(result.item.index));
        assert!((512..576).contains(&result.item.index));
    }
    {
        let state = service.inner.state.lock().unwrap();
        assert_eq!(state.active.len(), 4);
        assert_eq!(state.queue.active(), 4);
        assert_eq!(state.live.len(), 256);
        assert_eq!(state.entries.len(), 256);
    }
    assert!(timeout(Duration::from_millis(30), calls.recv())
        .await
        .is_err());
    stop.cancel();
    until(|| service.inner.state.lock().unwrap().closed).await;
    assert!(!running.is_finished());
    assert_eq!(service.inner.state.lock().unwrap().active.len(), 4);
    finish.add_permits(4);
    timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
    let state = service.inner.state.lock().unwrap();
    assert!(state.active.is_empty() && state.entries.is_empty() && state.live.is_empty());
    assert_eq!(state.queue.active(), 0);
}
