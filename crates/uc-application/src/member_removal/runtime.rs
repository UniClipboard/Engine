use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::warn;

use uc_core::ports::{PresenceEvent, ReachabilityState};

use crate::facade::MemberRemovalView;

use super::RemovalCoordinator;

// 意图和恢复资料已经落盘后，第一次连接可能早于对端恢复完成。保持短而固定的
// 重试间隔，让新建、重连和进程恢复都能及时继续，同时由协调器的已确认记录避免
// 对已完成工作重复发送。
const RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// Owns member-removal convergence for one process lifetime.
pub struct MemberRemovalRuntime {
    task: JoinHandle<()>,
}

impl MemberRemovalRuntime {
    pub fn start(
        coordinator: Arc<RemovalCoordinator>,
        mut presence_events: broadcast::Receiver<PresenceEvent>,
        state_events: broadcast::Sender<MemberRemovalView>,
    ) -> Self {
        let wake = coordinator.wake();
        let task = tokio::spawn(async move {
            let mut retry = tokio::time::interval(RETRY_INTERVAL);
            retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut last_published = None;

            loop {
                let should_advance = tokio::select! {
                    _ = retry.tick() => true,
                    _ = wake.notified() => true,
                    event = presence_events.recv() => match event {
                        Ok(event) => event.state == ReachabilityState::Online,
                        Err(broadcast::error::RecvError::Lagged(_)) => true,
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                };
                if !should_advance {
                    continue;
                }

                let now_ms = chrono::Utc::now().timestamp_millis();
                if coordinator.reconcile(now_ms).await.is_err() {
                    warn!(
                        failure = "reconcile_deferred",
                        "member removal convergence remains deferred"
                    );
                    continue;
                }
                match coordinator.query(now_ms).await {
                    Ok(summary) => {
                        let view = MemberRemovalView::from_summary(summary);
                        if last_published.as_ref() != Some(&view) {
                            let _ = state_events.send(view.clone());
                            last_published = Some(view);
                        }
                    }
                    Err(_) => {
                        warn!(
                            failure = "state_refresh_failed",
                            "member removal state refresh failed"
                        );
                    }
                }
            }
        });
        Self { task }
    }

    pub async fn shutdown(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}
