//! 运行期周期普查：tokio 调度器与受管后台任务的数量，用于定位任务或队列的无界增长。
//!
//! 记录只含计数；不读取任何业务状态。
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::time::{interval, MissedTickBehavior};
use uc_core::TaskRegistry;
use uc_observability_contract::uc_info;

const CADENCE: Duration = Duration::from_secs(300);

/// 登记周期普查任务；任务只持有注册表的弱引用，不延长其生命周期。
pub(super) async fn spawn_runtime_census(tasks: &Arc<TaskRegistry>) {
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
