use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc};
use tokio::task::{JoinError, JoinHandle};
use tracing::debug;
use uc_application::facade::{HostEvent, HostEventBus, TransferHostEvent};
use uc_core::file_transfer::{
    FileTransferCancellationReason, FileTransferDirection, OutboundProgressStatus,
};
use uc_infra::network::iroh::transfer_progress_adapter::InboundProgressEvent;

// 每次传输最多每秒发布五次进度，终态不受节流影响。
const TRANSLATOR_PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(200);

/// 把接收端推回的进度帧翻译成 `HostEvent::Transfer` 发给 emitter。
///
/// 每帧:
/// * 先发一条 `Progress { direction: Sending }`,前端用它更新 sender 端
///   transfer 进度条 + 文案。
/// * 终态(`Completed` / `Failed`)再补一条 `StatusChanged`,前端把
///   `entryStatusById[transfer_id]` 切到对应状态,UI 退出 transferring。
///
/// transfer_id 字段直接复用帧里的 sender 端 entry_id —— sender 本地
/// entry_id == transfer_id 是发送侧的协议约定(同接收侧约定对称)。
pub(super) struct OutboundProgressRuntime {
    commands: mpsc::UnboundedSender<OutboundProgressCommand>,
    task: JoinHandle<()>,
}

enum OutboundProgressCommand {
    Shutdown {
        reason: FileTransferCancellationReason,
    },
}

struct ActiveOutboundProgress {
    peer_id: String,
    bytes_transferred: u64,
    total_bytes: Option<u64>,
}

fn forward_outbound_progress(
    bus: &HostEventBus,
    last_progress_emit: &mut HashMap<String, Instant>,
    active: &mut HashMap<String, ActiveOutboundProgress>,
    event: InboundProgressEvent,
) {
    let terminal = match &event.status {
        OutboundProgressStatus::InProgress => None,
        OutboundProgressStatus::Completed => Some(("completed", None)),
        OutboundProgressStatus::Failed => {
            Some(("failed", Some("receiver fetch failed".to_string())))
        }
        OutboundProgressStatus::Cancelled { reason } => {
            Some(("cancelled", Some(reason.as_str().to_string())))
        }
    };

    // 终态绕过节流，宿主必须收到最终字节数与状态。
    let should_emit_progress = if terminal.is_some() {
        true
    } else {
        let now = Instant::now();
        match last_progress_emit.get(&event.transfer_id) {
            Some(previous) if now.duration_since(*previous) < TRANSLATOR_PROGRESS_MIN_INTERVAL => {
                false
            }
            _ => {
                last_progress_emit.insert(event.transfer_id.clone(), now);
                true
            }
        }
    };

    if should_emit_progress {
        bus.emit_or_warn(HostEvent::Transfer(TransferHostEvent::Progress {
            transfer_id: event.transfer_id.clone(),
            entry_id: Some(event.transfer_id.clone()),
            attempt_id: None,
            peer_id: event.from_device.as_str().to_string(),
            direction: FileTransferDirection::Sending,
            bytes_transferred: event.bytes_transferred,
            total_bytes: event.total_bytes,
        }));
    }

    if let Some((status, reason)) = terminal {
        // 先移除已结束传输，关闭时不能再次取消它。
        last_progress_emit.remove(&event.transfer_id);
        active.remove(&event.transfer_id);
        bus.emit_or_warn(HostEvent::Transfer(TransferHostEvent::StatusChanged {
            transfer_id: event.transfer_id.clone(),
            entry_id: Some(event.transfer_id),
            attempt_id: None,
            status: status.to_string(),
            reason,
        }));
    } else {
        active.insert(
            event.transfer_id,
            ActiveOutboundProgress {
                peer_id: event.from_device.as_str().to_owned(),
                bytes_transferred: event.bytes_transferred,
                total_bytes: event.total_bytes,
            },
        );
    }
}

impl OutboundProgressRuntime {
    pub(super) fn spawn(
        mut rx: broadcast::Receiver<InboundProgressEvent>,
        bus: Arc<HostEventBus>,
    ) -> Self {
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            // 终态清除节流记录，避免已完成传输的状态持续积累。
            let mut last_progress_emit: HashMap<String, Instant> = HashMap::new();
            let mut active = HashMap::<String, ActiveOutboundProgress>::new();
            loop {
                tokio::select! {
                    biased;
                    command = command_rx.recv() => match command {
                        Some(OutboundProgressCommand::Shutdown { reason }) => {
                            // 固定停止时的接收位置；收尾期间新来的进度不能延长本次排空。
                            let mut remaining = rx.len();
                            while remaining > 0 {
                                match rx.try_recv() {
                                    Ok(event) => {
                                        remaining -= 1;
                                        forward_outbound_progress(&bus, &mut last_progress_emit, &mut active, event);
                                    }
                                    Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                                        remaining = remaining.saturating_sub(usize::try_from(skipped).unwrap_or(usize::MAX));
                                    }
                                    Err(_) => break,
                                }
                            }
                            for (transfer_id, progress) in active.drain() {
                                bus.emit_or_warn(HostEvent::Transfer(TransferHostEvent::Progress {
                                    entry_id: Some(transfer_id.clone()),
                                    transfer_id: transfer_id.clone(),
                                    attempt_id: None,
                                    peer_id: progress.peer_id,
                                    direction: FileTransferDirection::Sending,
                                    bytes_transferred: progress.bytes_transferred,
                                    total_bytes: progress.total_bytes,
                                }));
                                bus.emit_or_warn(HostEvent::Transfer(TransferHostEvent::StatusChanged {
                                    entry_id: Some(transfer_id.clone()),
                                    transfer_id,
                                    attempt_id: None,
                                    status: "cancelled".to_owned(),
                                    reason: Some(reason.as_str().to_owned()),
                                }));
                            }
                            return;
                        }
                        None => return,
                    },
                    received = rx.recv() => match received {
                    Ok(event) => forward_outbound_progress(&bus, &mut last_progress_emit, &mut active, event),
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!(
                            skipped = n,
                            "outbound progress translator: lagged; some frames skipped"
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }}
            }
        });
        Self { commands, task }
    }

    pub(super) async fn shutdown(
        self,
        reason: FileTransferCancellationReason,
    ) -> Result<(), JoinError> {
        // 事件源可能已经正常关闭；任务本身的 join 才是最终完成依据。
        let _ = self
            .commands
            .send(OutboundProgressCommand::Shutdown { reason });
        self.task.await
    }
}

#[cfg(test)]
mod outbound_progress_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use uc_application::facade::{EmitError, HostEventEmitterPort};
    use uc_core::ids::DeviceId;

    struct RefillingEmitter {
        events: broadcast::Sender<InboundProgressEvent>,
        count: AtomicUsize,
    }

    impl HostEventEmitterPort for RefillingEmitter {
        fn emit(&self, event: HostEvent) -> Result<(), EmitError> {
            if matches!(
                event,
                HostEvent::Transfer(TransferHostEvent::Progress { .. })
            ) {
                let count = self.count.fetch_add(1, Ordering::SeqCst);
                // 限制故障实现的循环次数，让旧行为也能结束并由断言明确失败。
                if count < 32 {
                    let _ = self.events.send(progress(
                        &format!("late-{count}"),
                        OutboundProgressStatus::InProgress,
                    ));
                }
            }
            Ok(())
        }
    }

    fn progress(transfer: &str, status: OutboundProgressStatus) -> InboundProgressEvent {
        InboundProgressEvent {
            transfer_id: transfer.to_owned(),
            from_device: DeviceId::new("peer-test"),
            bytes_transferred: 1,
            total_bytes: Some(2),
            status,
        }
    }

    #[tokio::test]
    async fn shutdown_does_not_drain_notifications_produced_during_cleanup() {
        let (events, _) = broadcast::channel(4);
        let bus = Arc::new(HostEventBus::new());
        let refill = Arc::new(RefillingEmitter {
            events: events.clone(),
            count: AtomicUsize::new(0),
        });
        bus.register(
            "refill",
            Arc::clone(&refill) as Arc<dyn HostEventEmitterPort>,
        );
        let runtime = OutboundProgressRuntime::spawn(events.subscribe(), bus);
        events
            .send(progress("initial", OutboundProgressStatus::InProgress))
            .unwrap();
        runtime
            .shutdown(FileTransferCancellationReason::ConnectivityRecovery)
            .await
            .unwrap();
        assert_eq!(
            refill.count.load(Ordering::SeqCst),
            2,
            "只发布原进度及其取消前最终进度，不消费新到的传输"
        );
    }

    #[tokio::test]
    async fn shutdown_keeps_buffered_terminal_events_after_receiver_lag() {
        let (events, _) = broadcast::channel(2);
        let bus = Arc::new(HostEventBus::new());
        let recorder = Arc::new(Recorder::default());
        bus.register(
            "record",
            Arc::clone(&recorder) as Arc<dyn HostEventEmitterPort>,
        );
        let runtime = OutboundProgressRuntime::spawn(events.subscribe(), bus);
        events
            .send(progress("a", OutboundProgressStatus::InProgress))
            .unwrap();
        events
            .send(progress("b", OutboundProgressStatus::InProgress))
            .unwrap();
        events
            .send(progress("a", OutboundProgressStatus::Completed))
            .unwrap();
        runtime
            .shutdown(FileTransferCancellationReason::ConnectivityRecovery)
            .await
            .unwrap();
        let events = recorder.0.lock().unwrap();
        assert!(events.iter().any(|event| matches!(event,
            HostEvent::Transfer(TransferHostEvent::StatusChanged { transfer_id, status, .. }) if transfer_id == "a" && status == "completed"
        )));
        assert!(events.iter().any(|event| matches!(event,
            HostEvent::Transfer(TransferHostEvent::StatusChanged { transfer_id, status, .. }) if transfer_id == "b" && status == "cancelled"
        )));
    }

    #[tokio::test]
    async fn outbound_progress_shutdown_preserves_worker_failure() {
        let (commands, _receiver) = mpsc::unbounded_channel();
        let runtime = OutboundProgressRuntime {
            commands,
            task: tokio::spawn(async { panic!("PRIVATE_PROGRESS_FAILURE") }),
        };
        assert!(runtime
            .shutdown(FileTransferCancellationReason::Unknown)
            .await
            .unwrap_err()
            .is_panic());
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<HostEvent>>);

    impl HostEventEmitterPort for Recorder {
        fn emit(&self, event: HostEvent) -> Result<(), EmitError> {
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event);
            Ok(())
        }
    }

    #[tokio::test]
    async fn network_recovery_finishes_each_active_outbound_transfer_once() {
        let (events, _) = broadcast::channel(4);
        let bus = Arc::new(HostEventBus::new());
        let recorder = Arc::new(Recorder::default());
        bus.register(
            "test",
            Arc::clone(&recorder) as Arc<dyn HostEventEmitterPort>,
        );
        let runtime = OutboundProgressRuntime::spawn(events.subscribe(), bus);

        events
            .send(InboundProgressEvent {
                from_device: DeviceId::new("peer-a"),
                transfer_id: "transfer-a".to_owned(),
                bytes_transferred: 12,
                total_bytes: Some(20),
                status: OutboundProgressStatus::InProgress,
            })
            .unwrap_or_else(|error| panic!("send progress: {error}"));
        events
            .send(InboundProgressEvent {
                from_device: DeviceId::new("peer-a"),
                transfer_id: "transfer-a".to_owned(),
                bytes_transferred: 12,
                total_bytes: Some(20),
                status: OutboundProgressStatus::InProgress,
            })
            .unwrap_or_else(|error| panic!("send progress: {error}"));
        tokio::task::yield_now().await;

        runtime
            .shutdown(FileTransferCancellationReason::ConnectivityRecovery)
            .await
            .unwrap();

        let events = recorder
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let terminals = events.iter().filter(|event| matches!(event,
            HostEvent::Transfer(TransferHostEvent::StatusChanged { transfer_id, status, reason, .. })
            if transfer_id == "transfer-a" && status == "cancelled" && reason.as_deref() == Some("connectivity_recovery")
        )).count();
        assert_eq!(terminals, 1);
    }

    #[tokio::test]
    async fn network_recovery_does_not_repeat_an_existing_outbound_terminal() {
        let (events, _) = broadcast::channel(4);
        let bus = Arc::new(HostEventBus::new());
        let recorder = Arc::new(Recorder::default());
        bus.register(
            "test",
            Arc::clone(&recorder) as Arc<dyn HostEventEmitterPort>,
        );
        let runtime = OutboundProgressRuntime::spawn(events.subscribe(), bus);

        events
            .send(InboundProgressEvent {
                from_device: DeviceId::new("peer-a"),
                transfer_id: "transfer-a".to_owned(),
                bytes_transferred: 20,
                total_bytes: Some(20),
                status: OutboundProgressStatus::Completed,
            })
            .unwrap_or_else(|error| panic!("send terminal: {error}"));
        runtime
            .shutdown(FileTransferCancellationReason::ConnectivityRecovery)
            .await
            .unwrap();

        let events = recorder
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let terminals = events.iter().filter(|event| matches!(event,
            HostEvent::Transfer(TransferHostEvent::StatusChanged { transfer_id, .. }) if transfer_id == "transfer-a"
        )).count();
        assert_eq!(terminals, 1);
    }
}
