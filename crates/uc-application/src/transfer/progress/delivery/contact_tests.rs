use super::*;

struct Contacts {
    tx: mpsc::Sender<(bool, u64)>,
    gate: Arc<Semaphore>,
}
#[async_trait]
impl ProgressLiveRoundPort for Contacts {
    async fn report_contact(
        &self,
        _: &DeviceId,
        _: ProgressTaskDescriptor,
        sequence: u64,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        deadline.check(cancel)?;
        self.tx.try_send((true, sequence)).unwrap();
        self.gate.acquire().await.unwrap().forget();
        deadline.check(cancel)
    }
    async fn report_live(
        &self,
        _: &DeviceId,
        _: ProgressTaskDescriptor,
        value: ProgressActive,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        deadline.check(cancel)?;
        self.tx.try_send((false, value.sequence)).unwrap();
        self.gate.acquire().await.unwrap().forget();
        deadline.check(cancel)
    }
}
#[async_trait]
impl ProgressTerminalRoundPort for Contacts {
    async fn report_settled(
        &self,
        _: &DeviceId,
        _: ProgressTaskDescriptor,
        _: ProgressTerminal,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        deadline.check(cancel)?;
        self.tx.try_send((false, 0)).unwrap();
        Ok(())
    }
}
async fn next(rx: &mut mpsc::Receiver<(bool, u64)>) -> (bool, u64) {
    timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test]
async fn waiting_contact_uses_ten_seconds_without_overlap_or_renewal_by_duplicate_tracking() {
    let clock = Arc::new(Millis(AtomicU64::new(0)));
    let (tx, mut rx) = mpsc::channel(16);
    let gate = Arc::new(Semaphore::new(0));
    let rounds = Arc::new(Contacts {
        tx,
        gate: gate.clone(),
    });
    let (service, worker) = ProgressExchangeService::new(
        clock.clone(),
        authority(&[(1, 1)]),
        rounds.clone(),
        Arc::new(UnusedSnapshot),
        rounds,
    );
    assert_eq!(
        service
            .submit_activity(
                peer(1),
                service
                    .inner
                    .authority
                    .current_task(&peer(1), id(1))
                    .unwrap()
                    .descriptor,
                None
            )
            .unwrap(),
        ProgressSubmission::Retained
    );
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    clock.0.store(9000, Ordering::SeqCst);
    assert_eq!(
        service
            .submit_activity(
                peer(1),
                service
                    .inner
                    .authority
                    .current_task(&peer(1), id(1))
                    .unwrap()
                    .descriptor,
                None
            )
            .unwrap(),
        ProgressSubmission::Unchanged
    );
    clock.0.store(9999, Ordering::SeqCst);
    service.network_recovered(Some(peer(1)));
    assert!(timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    clock.0.store(10000, Ordering::SeqCst);
    service.network_recovered(Some(peer(1)));
    assert_eq!(next(&mut rx).await, (true, 1));
    clock.0.store(30000, Ordering::SeqCst);
    service.network_recovered(Some(peer(1)));
    assert!(timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    assert_eq!(service.inner.state.lock().unwrap().active.len(), 1);
    gate.add_permits(1);
    until(|| service.inner.state.lock().unwrap().active.is_empty()).await;
    clock.0.store(39999, Ordering::SeqCst);
    service.network_recovered(Some(peer(1)));
    assert!(timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    clock.0.store(40000, Ordering::SeqCst);
    service.network_recovered(Some(peer(1)));
    assert_eq!(next(&mut rx).await, (true, 2));
    service
        .submit_settled(
            peer(1),
            service
                .inner
                .authority
                .current_task(&peer(1), id(1))
                .unwrap()
                .descriptor,
            ProgressTerminal {
                item: value(0, 1).item,
                round: 0,
                sequence: 1,
                result_revision: 1,
                bytes: 0,
                total: None,
                outcome: ProgressTerminalOutcome::Completed,
            },
        )
        .unwrap();
    assert!(timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    gate.add_permits(1);
    assert_eq!(next(&mut rx).await, (false, 0));
    clock.0.store(100000, Ordering::SeqCst);
    service.network_recovered(Some(peer(1)));
    assert!(timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    stop.cancel();
    running.await.unwrap();
}
#[tokio::test]
async fn old_live_epoch_completion_cannot_remove_activity_submitted_after_clock_recovery() {
    let clock = Arc::new(Millis(AtomicU64::new(0)));
    let (tx, mut rx) = mpsc::channel(16);
    let gate = Arc::new(Semaphore::new(0));
    let rounds = Arc::new(Contacts {
        tx,
        gate: gate.clone(),
    });
    let (service, worker) = ProgressExchangeService::new(
        clock.clone(),
        authority(&[(1, 1)]),
        rounds.clone(),
        Arc::new(UnusedSnapshot),
        rounds,
    );
    service
        .submit_activity(
            peer(1),
            service
                .inner
                .authority
                .current_task(&peer(1), id(1))
                .unwrap()
                .descriptor,
            Some(value(0, 1)),
        )
        .unwrap();
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    assert_eq!(next(&mut rx).await, (false, 1));
    clock.0.store(1000, Ordering::SeqCst);
    service
        .submit_activity(
            peer(1),
            service
                .inner
                .authority
                .current_task(&peer(1), id(1))
                .unwrap()
                .descriptor,
            Some(value(0, 2)),
        )
        .unwrap();
    clock.0.store(500, Ordering::SeqCst);
    assert!(service
        .submit_activity(
            peer(1),
            service
                .inner
                .authority
                .current_task(&peer(1), id(1))
                .unwrap()
                .descriptor,
            Some(value(0, 3))
        )
        .is_err());
    service
        .submit_activity(
            peer(1),
            service
                .inner
                .authority
                .current_task(&peer(1), id(1))
                .unwrap()
                .descriptor,
            Some(value(0, 3)),
        )
        .unwrap();
    gate.add_permits(1);
    until(|| service.inner.state.lock().unwrap().active.is_empty()).await;
    clock.0.store(700, Ordering::SeqCst);
    service.network_recovered(Some(peer(1)));
    assert_eq!(next(&mut rx).await, (false, 3));
    gate.add_permits(1);
    until(|| service.inner.state.lock().unwrap().active.is_empty()).await;
    stop.cancel();
    running.await.unwrap();
}
