//! 独占日志捕获验证真实维护调度；只增加观测，不改变被阻塞时的串行行为。
use std::fmt::Debug;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Notify;
use tracing::{
    field::{Field, Visit},
    Event, Subscriber,
};
use tracing_subscriber::{layer::Context, prelude::*, Layer};
use uc_observability_contract::diagnostics::connectivity::decode_local_record;

use super::tests::{NoopNetworkActivity, RecordingStep};
use super::*;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<Value>>>);
#[derive(Default)]
struct Fields {
    name: String,
    payload: String,
}
impl Visit for Fields {
    fn record_debug(&mut self, _: &Field, _: &dyn Debug) {}
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "event.name" => self.name = value.into(),
            "payload" => self.payload = value.into(),
            _ => {}
        }
    }
}
impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() != "uc.connectivity" {
            return;
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        let record = decode_local_record(
            &fields.name,
            &fields.payload,
            event.metadata().level().as_str(),
        )
        .expect("封闭事件必须可解码");
        self.0.lock().expect("capture").push(Value::Object(record));
    }
}
impl Capture {
    async fn wait_for(&self, name: &str) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if self
                    .0
                    .lock()
                    .expect("capture")
                    .iter()
                    .any(|r| r["event.name"] == name)
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("预期事件必须出现");
    }
}

struct BlockFirstUpdate {
    started: Arc<Notify>,
    release: Arc<Notify>,
    first: AtomicBool,
}
#[async_trait]
impl DeliverPendingGroupUpdatesPort for BlockFirstUpdate {
    async fn deliver_pending_group_updates(
        &self,
        _: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceStepOutcome {
        if self.first.swap(false, Ordering::SeqCst) {
            self.started.notify_one();
            self.release.notified().await;
        }
        MembershipMaintenanceStepOutcome::Deferred
    }
}

#[tokio::test]
#[ignore = "独占进程日志捕获，精确运行"]
async fn blocked_updates_explain_queue_wait_and_coalesced_wakes_without_changing_order() {
    let capture = Capture::default();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(capture.clone()))
        .expect("subscriber");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let step = |name| {
        Arc::new(RecordingStep {
            name,
            calls: calls.clone(),
            outcome: MembershipMaintenanceStepOutcome::Completed,
        })
    };
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let maintain = Arc::new(MaintainSpaceMembershipUseCase::new(
        MaintainSpaceMembershipDeps {
            admissions: step("admissions"),
            effects: step("effects"),
            conflicts: step("conflicts"),
            restricted_delivery: step("restricted"),
            synchronization: step("synchronization"),
            cleanup: step("cleanup"),
            group_update_delivery: Arc::new(BlockFirstUpdate {
                started: started.clone(),
                release: release.clone(),
                first: AtomicBool::new(true),
            }),
        },
    ));
    let (_presence, receiver) = tokio::sync::broadcast::channel(4);
    let runtime = SpaceMembershipMaintenanceRuntime::start(
        maintain,
        receiver,
        tokio::sync::broadcast::channel(1).1,
        Duration::from_secs(3600),
        Arc::new(NoopNetworkActivity),
    );
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .expect("进入更新等待");
    runtime.activity().request_state_changed().expect("wake");
    runtime
        .activity()
        .request_state_changed()
        .expect("duplicate wake");
    capture.wait_for("membership.maintenance.coalesced").await;
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(
        calls
            .lock()
            .expect("calls")
            .iter()
            .filter(|name| **name == "admissions")
            .count(),
        1,
        "补日志不改变原有等待顺序"
    );
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if capture
                .0
                .lock()
                .expect("capture")
                .iter()
                .filter(|r| r["event.name"] == "membership.maintenance.finished")
                .count()
                == 2
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("两轮完成");
    runtime.shutdown().await.unwrap();
    let events = capture.0.lock().expect("capture");
    let queued = events
        .iter()
        .find(|r| r["event.name"] == "membership.maintenance.queued")
        .expect("queued");
    let merged = events
        .iter()
        .find(|r| r["event.name"] == "membership.maintenance.coalesced")
        .expect("merged");
    assert_eq!(merged["coalesced_into_round"], queued["maintenance_round"]);
    assert!(!events
        .iter()
        .any(|r| r["event.name"] == "membership.maintenance.started"
            && r["maintenance_round"] == merged["maintenance_round"]));
    let began = events
        .iter()
        .find(|r| {
            r["event.name"] == "membership.maintenance.started"
                && r["maintenance_round"] == queued["maintenance_round"]
        })
        .expect("queued round ran");
    assert!(began["queue_wait_ms"].as_u64().expect("duration") >= 40);
    let update = events
        .iter()
        .find(|r| {
            r["event.name"] == "runtime.work.finished" && r["step"] == "maintenance_group_updates"
        })
        .expect("update timing");
    assert!(update["duration_ms"].as_u64().expect("duration") >= 40);
    assert_eq!(update["uc.outcome"], "deferred");
    assert_eq!(
        events
            .iter()
            .filter(|r| r["event.name"] == "membership.maintenance.finished")
            .count(),
        2
    );
}
