//! 运行期事件投影与周期普查：把底层变化转成公开事件，并记录任务/队列计数。

use super::*;
use crate::EngineEvent;
#[cfg(feature = "lan-compat")]
use crate::MobileLanSettingsChanged;
#[cfg(feature = "lan-compat")]
use crate::MobileSyncSettingsUpdateSummary;
use crate::RePairingScope;
use crate::RefreshReason;

pub(crate) fn re_pairing_scope_for_setup_state(
    state: &uc_application::facade::SetupStateView,
) -> Option<RePairingScope> {
    state
        .re_pairing_required
        .then_some(RePairingScope::AllDevices)
}

pub(crate) fn network_recovery_summary(
    event: NetworkRecoveryEvent,
) -> NetworkRecoveryStatusSummary {
    match event {
        NetworkRecoveryEvent::Started => NetworkRecoveryStatusSummary {
            phase: NetworkRecoveryPhaseSummary::Recovering,
            retryable: false,
            next_retry_in_ms: None,
        },
        NetworkRecoveryEvent::RetryScheduled { delay } => NetworkRecoveryStatusSummary {
            phase: NetworkRecoveryPhaseSummary::RetryScheduled,
            retryable: true,
            next_retry_in_ms: Some(delay.as_millis().min(u128::from(u64::MAX)) as u64),
        },
        NetworkRecoveryEvent::Succeeded => NetworkRecoveryStatusSummary {
            phase: NetworkRecoveryPhaseSummary::Idle,
            retryable: false,
            next_retry_in_ms: None,
        },
        NetworkRecoveryEvent::Failed { retryable } => NetworkRecoveryStatusSummary {
            phase: NetworkRecoveryPhaseSummary::Failed,
            retryable,
            next_retry_in_ms: None,
        },
    }
}

pub(crate) async fn spawn_network_recovery_events(
    mut changes: tokio::sync::broadcast::Receiver<NetworkRecoveryEvent>,
    tasks: &Arc<TaskRegistry>,
    events: EventSender,
) {
    let _ = tasks
        .spawn(move |cancel| async move {
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return,
                    change = changes.recv() => match change {
                        Ok(change) => events.send(EngineEvent::NetworkRecoveryChanged(network_recovery_summary(change))),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => events.send(EngineEvent::RefreshRequired {
                            reason: RefreshReason::ConsumerLagged,
                        }),
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        })
        .await;
}

#[cfg(feature = "lan-compat")]
pub(crate) fn engine_event_for_mobile_settings_update(
    settings: &MobileSyncSettingsUpdateSummary,
) -> EngineEvent {
    EngineEvent::MobileLanSettingsChanged(MobileLanSettingsChanged {
        enabled: settings.enabled,
        lan_listen_enabled: settings.lan_listen_enabled,
        lan_port: settings.lan_port,
    })
}

pub(crate) async fn spawn_space_transition_watcher(
    supervisor: Arc<SessionSupervisor>,
    mut changes: tokio::sync::watch::Receiver<()>,
    tasks: &Arc<TaskRegistry>,
    events: EventSender,
) {
    let _ = tasks
        .spawn(move |cancel| async move {
            let mut check_required = true;
            loop {
                if !check_required {
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return,
                        changed = changes.changed() => {
                            if changed.is_err() {
                                return;
                            }
                            check_required = true;
                        }
                    }
                    continue;
                }
                check_required = false;
                let transition = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return,
                    result = supervisor.transition_pending_session() => result,
                };
                match transition {
                    Ok(Some(revision)) => {
                        events.send(EngineEvent::DeviceTrustChanged { revision });
                        events.send(EngineEvent::RefreshRequired {
                            reason: RefreshReason::StateInvalidated,
                        });
                    }
                    Ok(None) => {}
                    Err(error) => {
                        uc_warn!(
                            error_code = error.code(),
                            retryable = error.is_retryable(),
                            "runtime Space transition attempt failed"
                        );
                        if error.is_retryable() {
                            tokio::select! {
                                _ = cancel.cancelled() => return,
                                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                                    check_required = true;
                                }
                                changed = changes.changed() => {
                                    if changed.is_err() {
                                        return;
                                    }
                                    check_required = true;
                                }
                            }
                        }
                    }
                }
            }
        })
        .await;
}

use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::time::{interval, MissedTickBehavior};
use uc_core::TaskRegistry;
use uc_observability_contract::uc_info;

const CADENCE: Duration = Duration::from_secs(300);

/// 登记周期普查任务；任务只持有注册表的弱引用，不延长其生命周期。
pub(crate) async fn spawn_runtime_census(tasks: &Arc<TaskRegistry>) {
    let registry = Arc::downgrade(tasks);
    let _ = tasks
        .spawn(move |cancel| async move {
            let mut ticker = interval(CADENCE);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            // 第一次 tick 立即完成；首条记录在一个周期之后，避开启动瞬间。
            ticker.tick().await;
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return,
                    _ = ticker.tick() => emit(&registry).await,
                }
            }
        })
        .await;
}

async fn emit(registry: &Weak<TaskRegistry>) {
    let metrics = Handle::current().metrics();
    let registered_tasks = match registry.upgrade() {
        Some(registry) => Some(registry.task_count().await),
        None => None,
    };
    uc_info!(
        tokio_workers = metrics.num_workers(),
        tokio_alive_tasks = metrics.num_alive_tasks(),
        tokio_queue_depth = metrics.global_queue_depth(),
        registered_tasks = registered_tasks,
        "engine runtime census"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn census_task_stops_with_the_registry_and_holds_no_strong_reference() {
        let registry = Arc::new(TaskRegistry::new());
        spawn_runtime_census(&registry).await;
        assert_eq!(registry.task_count().await, 1);
        assert_eq!(Arc::strong_count(&registry), 1);
        tokio::time::advance(CADENCE * 2).await;
        let report = registry.shutdown(Duration::from_secs(1)).await;
        assert_eq!(report.timed_out_count, 0);
        assert!(report.into_result().is_ok());
    }
}
