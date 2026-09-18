use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use thiserror::Error;
use tokio::sync::{
    broadcast,
    mpsc::{self, UnboundedReceiver},
};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use super::{resurface_entry, ActiveClipboardConvergedEvent, ActiveClipboardFacade};
use crate::clipboard::sync::active_state::peer_online_resync_worker::PeerOnlineResyncWorker;
use crate::clipboard::sync::active_state::restore_broadcast_worker::RestoreBroadcastWorker;
use crate::clipboard::write::RestoreBroadcastRequest;
use crate::runtime_lifecycle::LifecycleError;

impl ActiveClipboardFacade {
    /// 启动并持有当前剪贴板的全部后台工作，装配方只通过返回值管理其生命周期。
    pub fn start_background_workers(self: &Arc<Self>) -> ActiveClipboardLifecycle {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let mut workers = JoinSet::new();
        let cancel = CancellationToken::new();

        // 先订阅再调度接收循环，避免历史置顶遗漏首次收敛通知。
        let resurface_rx = self.inbound_uc.subscribe_converged();
        let resurface_facade = Arc::clone(self);
        let resurface_cancel = cancel.child_token();
        workers.spawn(async move {
            resurface_facade
                .run_resurface_worker(resurface_rx, resurface_cancel)
                .await;
            "resurface"
        });

        let inbound_uc = Arc::clone(&self.inbound_uc);
        let inbound_cancel = cancel.child_token();
        workers.spawn(async move {
            inbound_uc.run(inbound_cancel).await;
            "inbound"
        });

        let resync = self.peer_online_resync_worker();
        let resync_cancel = cancel.child_token();
        workers.spawn(async move {
            resync.run(resync_cancel).await;
            "peer_online_resync"
        });

        let restore_facade = Arc::clone(self);
        let restore_worker_starter: RestoreWorkerStarter = Arc::new(move |rx, cancel| {
            let worker = restore_facade.restore_broadcast_worker(rx);
            Box::pin(async move { worker.run(cancel).await })
        });

        ActiveClipboardLifecycle::start(
            workers,
            restore_worker_starter,
            commands,
            command_rx,
            cancel,
        )
    }

    fn peer_online_resync_worker(&self) -> PeerOnlineResyncWorker {
        PeerOnlineResyncWorker::new(
            Arc::clone(&self.peer_reachability),
            Arc::clone(&self.load_register),
            self.reconstructor.clone(),
            Arc::clone(&self.dispatch),
            Arc::clone(&self.peer_scope),
            Arc::clone(&self.member_repo),
        )
    }

    fn restore_broadcast_worker(
        &self,
        rx: UnboundedReceiver<RestoreBroadcastRequest>,
    ) -> RestoreBroadcastWorker {
        RestoreBroadcastWorker::new(
            rx,
            Arc::clone(&self.settings),
            Arc::clone(&self.dispatch),
            Arc::clone(&self.peer_addr_repo),
            Arc::clone(&self.peer_scope),
            Arc::clone(&self.peer_reachability),
            Arc::clone(&self.member_repo),
        )
    }

    async fn run_resurface_worker(
        self: Arc<Self>,
        mut rx: broadcast::Receiver<ActiveClipboardConvergedEvent>,
        cancel: CancellationToken,
    ) {
        loop {
            let event = tokio::select! {
                biased;
                _ = cancel.cancelled() => return,
                event = rx.recv() => event,
            };
            match event {
                Ok(event) => {
                    resurface_entry(
                        self.touch_entry.as_ref(),
                        &self.host_event_emitter,
                        self.resurface_clock.as_ref(),
                        &event.entry_id,
                    )
                    .await;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    debug!(
                        missed = n,
                        "resurface worker lagged; some entries may not resurface immediately"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

/// 持有当前剪贴板工作组并协调其完整退出。
pub struct ActiveClipboardLifecycle {
    commands: mpsc::UnboundedSender<ActiveClipboardLifecycleCommand>,
    restore_broadcast_attached: AtomicBool,
    supervisor: Option<JoinHandle<Result<(), LifecycleError>>>,
    cancel: CancellationToken,
}

impl ActiveClipboardLifecycle {
    fn start(
        workers: JoinSet<&'static str>,
        restore_worker_starter: RestoreWorkerStarter,
        commands: mpsc::UnboundedSender<ActiveClipboardLifecycleCommand>,
        command_rx: mpsc::UnboundedReceiver<ActiveClipboardLifecycleCommand>,
        cancel: CancellationToken,
    ) -> Self {
        let supervisor_cancel = cancel.clone();
        let supervisor = tokio::spawn(async move {
            ActiveClipboardWorkerSupervisor::new(
                restore_worker_starter,
                command_rx,
                supervisor_cancel,
            )
            .run(workers)
            .await
        });

        Self {
            commands,
            restore_broadcast_attached: AtomicBool::new(false),
            supervisor: Some(supervisor),
            cancel,
        }
    }

    /// 初次装配后接入恢复广播来源，每个生命周期只允许接入一次。
    pub fn attach_restore_broadcast(
        &self,
        rx: UnboundedReceiver<RestoreBroadcastRequest>,
    ) -> Result<(), ActiveClipboardLifecycleError> {
        if self.cancel.is_cancelled() {
            return Err(ActiveClipboardLifecycleError::Stopped);
        }
        if self
            .restore_broadcast_attached
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ActiveClipboardLifecycleError::RestoreBroadcastAlreadyAttached);
        }

        if self
            .commands
            .send(ActiveClipboardLifecycleCommand::AttachRestoreBroadcast { rx })
            .is_err()
        {
            self.restore_broadcast_attached
                .store(false, Ordering::Release);
            return Err(ActiveClipboardLifecycleError::Stopped);
        }

        Ok(())
    }

    /// 停止通知不丢弃当前动作，实际 join 是唯一的完成依据。
    pub async fn shutdown(mut self) -> Result<(), LifecycleError> {
        self.cancel.cancel();
        if let Some(supervisor) = self.supervisor.take() {
            return supervisor.await.map_err(|source| LifecycleError {
                primary: source.into(),
                additional: Vec::new(),
            })?;
        }
        Ok(())
    }
}

impl Drop for ActiveClipboardLifecycle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// 装配方可见的生命周期命令错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ActiveClipboardLifecycleError {
    #[error("active clipboard background workers have stopped")]
    Stopped,
    #[error("the restore broadcast source is already attached")]
    RestoreBroadcastAlreadyAttached,
}

#[derive(Debug, Error)]
#[error("required active clipboard worker stopped unexpectedly: {worker}")]
struct RequiredActiveClipboardWorkerStopped {
    worker: &'static str,
}

type RestoreWorkerFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type RestoreWorkerStarter = Arc<
    dyn Fn(UnboundedReceiver<RestoreBroadcastRequest>, CancellationToken) -> RestoreWorkerFuture
        + Send
        + Sync,
>;

enum ActiveClipboardLifecycleCommand {
    AttachRestoreBroadcast {
        rx: UnboundedReceiver<RestoreBroadcastRequest>,
    },
}

struct ActiveClipboardWorkerSupervisor {
    restore_worker_starter: RestoreWorkerStarter,
    commands: mpsc::UnboundedReceiver<ActiveClipboardLifecycleCommand>,
    cancel: CancellationToken,
}

impl ActiveClipboardWorkerSupervisor {
    fn new(
        restore_worker_starter: RestoreWorkerStarter,
        commands: mpsc::UnboundedReceiver<ActiveClipboardLifecycleCommand>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            restore_worker_starter,
            commands,
            cancel,
        }
    }

    async fn run(mut self, mut workers: JoinSet<&'static str>) -> Result<(), LifecycleError> {
        let mut errors = Vec::new();
        loop {
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => break,
                command = self.commands.recv() => match command {
                    Some(ActiveClipboardLifecycleCommand::AttachRestoreBroadcast { rx }) => {
                        if self.cancel.is_cancelled() { continue; }
                        let starter = Arc::clone(&self.restore_worker_starter);
                        let cancel = self.cancel.child_token();
                        workers.spawn(async move {
                            if !cancel.is_cancelled() {
                                (starter)(rx, cancel).await;
                            }
                            "restore_broadcast"
                        });
                    }
                    None => {
                        self.cancel.cancel();
                        break;
                    }
                },
                joined = workers.join_next(), if !workers.is_empty() => {
                    match joined {
                        Some(Ok(worker)) => {
                            errors.push(RequiredActiveClipboardWorkerStopped { worker }.into());
                            self.cancel.cancel();
                        }
                        Some(Err(source)) => {
                            errors.push(source.into());
                            self.cancel.cancel();
                        }
                        None => {}
                    }
                }
            }
        }
        while let Some(joined) = workers.join_next().await {
            if let Err(source) = joined {
                errors.push(source.into());
            }
        }
        LifecycleError::from_errors(errors)
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::Duration;

    use tokio::sync::mpsc;
    use tokio::sync::Notify;
    use tokio::task::JoinSet;
    use tokio_util::sync::CancellationToken;

    use super::{
        ActiveClipboardLifecycle, ActiveClipboardLifecycleError,
        RequiredActiveClipboardWorkerStopped, RestoreWorkerStarter,
    };
    use std::sync::Barrier;

    struct StopProbe(Arc<AtomicBool>);

    impl Drop for StopProbe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    fn blocking_lifecycle(
        panic_after: bool,
        factory_panics: bool,
    ) -> (
        ActiveClipboardLifecycle,
        Arc<Notify>,
        Arc<Barrier>,
        Arc<AtomicBool>,
        CancellationToken,
    ) {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Barrier::new(2));
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_started = Arc::clone(&started);
        let worker_release = Arc::clone(&release);
        let worker_stopped = Arc::clone(&stopped);
        let mut workers = JoinSet::new();
        workers.spawn(async move {
            tokio::task::spawn_blocking(move || {
                let _probe = StopProbe(worker_stopped);
                worker_started.notify_one();
                worker_release.wait();
            })
            .await
            .unwrap();
            assert!(!panic_after, "PRIVATE_WORKER_FAILURE");
            "held"
        });
        let starter: RestoreWorkerStarter = Arc::new(move |_rx, cancel| {
            assert!(!factory_panics, "PRIVATE_FACTORY_FAILURE");
            Box::pin(async move { cancel.cancelled().await })
        });
        let cancel = CancellationToken::new();
        let (commands, receiver) = mpsc::unbounded_channel();
        let lifecycle =
            ActiveClipboardLifecycle::start(workers, starter, commands, receiver, cancel.clone());
        (lifecycle, started, release, stopped, cancel)
    }

    #[tokio::test]
    async fn worker_failures_wait_for_disk_cleanup_and_keep_all_sources() {
        let (lifecycle, started, release, stopped, cancel) = blocking_lifecycle(true, true);
        started.notified().await;
        let (_sender, receiver) = mpsc::unbounded_channel();
        lifecycle.attach_restore_broadcast(receiver).unwrap();
        cancel.cancelled().await;
        let (_sender, receiver) = mpsc::unbounded_channel();
        assert_eq!(
            lifecycle.attach_restore_broadcast(receiver),
            Err(ActiveClipboardLifecycleError::Stopped)
        );
        let mut closing = tokio::spawn(lifecycle.shutdown());
        let early = tokio::time::timeout(Duration::from_millis(10), &mut closing).await;
        let remained_pending = early.is_err();
        let still_held = !stopped.load(Ordering::SeqCst);
        release.wait();
        let error = match early {
            Ok(result) => result,
            Err(_) => closing.await,
        }
        .unwrap()
        .unwrap_err();
        assert!(remained_pending && still_held);
        assert!(stopped.load(Ordering::SeqCst));
        assert_eq!(error.additional.len(), 1);
        assert!(error
            .primary
            .downcast_ref::<tokio::task::JoinError>()
            .unwrap()
            .is_panic());
        assert!(error.additional[0]
            .downcast_ref::<tokio::task::JoinError>()
            .unwrap()
            .is_panic());
        assert!(!format!("{error:?} {error}").contains("PRIVATE"));
    }

    #[tokio::test]
    async fn cancelled_shutdown_waiter_keeps_the_supervisor_and_disk_work_alive() {
        let (lifecycle, started, release, stopped, cancel) = blocking_lifecycle(false, false);
        let supervisor = lifecycle.supervisor.as_ref().unwrap().abort_handle();
        started.notified().await;
        let caller = tokio::spawn(lifecycle.shutdown());
        cancel.cancelled().await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        let remained_owned = !supervisor.is_finished() && !stopped.load(Ordering::SeqCst);
        release.wait();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !supervisor.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(remained_owned);
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn dropping_lifecycle_notifies_stop_without_aborting_the_disk_action() {
        let (lifecycle, started, release, stopped, cancel) = blocking_lifecycle(false, false);
        let supervisor = lifecycle.supervisor.as_ref().unwrap().abort_handle();
        started.notified().await;
        drop(lifecycle);
        let notified = cancel.is_cancelled();
        tokio::task::yield_now().await;
        let remained_owned = !supervisor.is_finished() && !stopped.load(Ordering::SeqCst);
        release.wait();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !supervisor.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(notified && remained_owned);
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn queued_restore_source_is_not_started_after_shutdown_acceptance() {
        let workers = JoinSet::new();
        let started = Arc::new(AtomicBool::new(false));
        let factory_started = Arc::clone(&started);
        let starter: RestoreWorkerStarter = Arc::new(move |_rx, _cancel| {
            factory_started.store(true, Ordering::SeqCst);
            Box::pin(async {})
        });
        let (commands, receiver) = mpsc::unbounded_channel();
        let lifecycle = ActiveClipboardLifecycle::start(
            workers,
            starter,
            commands,
            receiver,
            CancellationToken::new(),
        );
        let (_sender, receiver) = mpsc::unbounded_channel();
        lifecycle.attach_restore_broadcast(receiver).unwrap();
        lifecycle.shutdown().await.unwrap();
        assert!(!started.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn required_worker_early_return_stops_the_group_and_is_reported() {
        let release = Arc::new(Notify::new());
        let worker_release = Arc::clone(&release);
        let mut workers = JoinSet::new();
        workers.spawn(async move {
            worker_release.notified().await;
            "inbound"
        });
        let starter: RestoreWorkerStarter =
            Arc::new(move |_rx, cancel| Box::pin(async move { cancel.cancelled().await }));
        let cancel = CancellationToken::new();
        let (commands, receiver) = mpsc::unbounded_channel();
        let lifecycle =
            ActiveClipboardLifecycle::start(workers, starter, commands, receiver, cancel.clone());

        release.notify_one();
        tokio::time::timeout(Duration::from_secs(1), cancel.cancelled())
            .await
            .unwrap();
        let error = lifecycle.shutdown().await.unwrap_err();
        let stopped = error
            .primary
            .downcast_ref::<RequiredActiveClipboardWorkerStopped>()
            .unwrap();
        assert_eq!(stopped.worker, "inbound");
    }

    #[tokio::test]
    async fn lifecycle_starts_workers_attaches_restore_once_and_joins_on_shutdown() {
        let initial_started = Arc::new(AtomicBool::new(false));
        let initial_stopped = Arc::new(AtomicBool::new(false));
        let restore_started = Arc::new(AtomicBool::new(false));
        let restore_stopped = Arc::new(AtomicBool::new(false));
        let mut workers = JoinSet::new();
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.child_token();
        let initial_started_for_task = Arc::clone(&initial_started);
        let initial_stopped_for_task = Arc::clone(&initial_stopped);
        workers.spawn(async move {
            initial_started_for_task.store(true, Ordering::SeqCst);
            let _probe = StopProbe(initial_stopped_for_task);
            worker_cancel.cancelled().await;
            "inbound"
        });

        let restore_started_for_factory = Arc::clone(&restore_started);
        let restore_stopped_for_factory = Arc::clone(&restore_stopped);
        let restore_worker_starter: RestoreWorkerStarter = Arc::new(move |_rx, cancel| {
            restore_started_for_factory.store(true, Ordering::SeqCst);
            let stopped = Arc::clone(&restore_stopped_for_factory);
            Box::pin(async move {
                let _probe = StopProbe(stopped);
                cancel.cancelled().await;
            })
        });
        let (commands, command_rx) = mpsc::unbounded_channel();
        let lifecycle = ActiveClipboardLifecycle::start(
            workers,
            restore_worker_starter,
            commands,
            command_rx,
            cancel,
        );

        wait_for(&initial_started, "lifecycle should start initial workers").await;

        let (_restore_tx, restore_rx) = mpsc::unbounded_channel();
        assert_eq!(lifecycle.attach_restore_broadcast(restore_rx), Ok(()));
        wait_for(
            &restore_started,
            "lifecycle should start a late restore worker",
        )
        .await;

        let (_duplicate_tx, duplicate_rx) = mpsc::unbounded_channel();
        assert_eq!(
            lifecycle.attach_restore_broadcast(duplicate_rx),
            Err(ActiveClipboardLifecycleError::RestoreBroadcastAlreadyAttached)
        );

        tokio::time::timeout(Duration::from_secs(1), lifecycle.shutdown())
            .await
            .unwrap()
            .unwrap();
        assert!(initial_stopped.load(Ordering::SeqCst));
        assert!(restore_stopped.load(Ordering::SeqCst));
    }

    async fn wait_for(flag: &AtomicBool, message: &str) {
        let completed = tokio::time::timeout(Duration::from_secs(1), async {
            while !flag.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(completed.is_ok(), "{message}");
    }
}
