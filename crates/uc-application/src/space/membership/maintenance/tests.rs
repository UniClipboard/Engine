use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::*;

#[derive(Clone)]
struct RecordingStep {
    name: &'static str,
    calls: Arc<Mutex<Vec<&'static str>>>,
    outcome: MembershipMaintenanceStepOutcome,
}

impl RecordingStep {
    fn record(&self) -> MembershipMaintenanceStepOutcome {
        self.calls.lock().unwrap().push(self.name);
        self.outcome
    }
}

#[async_trait]
impl RecoverSpaceAdmissionsPort for RecordingStep {
    async fn recover_space_admissions(
        &self,
        _trigger: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceStepOutcome {
        self.record()
    }
}

#[async_trait]
impl RecoverMembershipEffectsPort for RecordingStep {
    async fn recover_membership_effects(&self) -> MembershipMaintenanceStepOutcome {
        self.record()
    }
}

#[async_trait]
impl DeliverRestrictedMembershipPort for RecordingStep {
    async fn deliver_restricted_membership(&self) -> MembershipMaintenanceStepOutcome {
        self.record()
    }
}

#[async_trait]
impl SynchronizeMembershipMaintenancePort for RecordingStep {
    async fn periodic_synchronization_required(
        &self,
    ) -> Result<bool, MembershipMaintenanceStepOutcome> {
        Ok(true)
    }

    async fn synchronize_membership(
        &self,
        _trigger: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceStepOutcome {
        self.record()
    }
}

#[async_trait]
impl CleanupLegacyMembershipDataPort for RecordingStep {
    async fn cleanup_legacy_membership_data(&self) -> MembershipMaintenanceStepOutcome {
        self.record()
    }
}

struct NoopNetworkActivity;

impl MembershipNetworkActivityPort for NoopNetworkActivity {
    fn pause_network_work(&self) {}
    fn resume_network_work(&self) {}
}

struct BlockingAdmission {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

struct NonCooperativeAdmission {
    started: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl RecoverSpaceAdmissionsPort for NonCooperativeAdmission {
    async fn recover_space_admissions(
        &self,
        _trigger: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceStepOutcome {
        self.started.notify_one();
        std::future::pending().await
    }
}

#[async_trait]
impl RecoverSpaceAdmissionsPort for BlockingAdmission {
    async fn recover_space_admissions(
        &self,
        _trigger: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceStepOutcome {
        self.started.notify_one();
        self.release.notified().await;
        MembershipMaintenanceStepOutcome::Completed
    }
}

struct PausingNetworkActivity {
    pauses: AtomicUsize,
    release: Arc<tokio::sync::Notify>,
}

impl MembershipNetworkActivityPort for PausingNetworkActivity {
    fn pause_network_work(&self) {
        self.pauses.fetch_add(1, Ordering::SeqCst);
        self.release.notify_waiters();
    }

    fn resume_network_work(&self) {}
}

#[tokio::test]
async fn startup_runs_the_fixed_sequence_and_continues_after_deferred_work() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let step = |name, outcome| {
        Arc::new(RecordingStep {
            name,
            calls: Arc::clone(&calls),
            outcome,
        })
    };
    let maintain = MaintainSpaceMembershipUseCase::new(MaintainSpaceMembershipDeps {
        admissions: step("admissions", MembershipMaintenanceStepOutcome::Completed),
        effects: step("effects", MembershipMaintenanceStepOutcome::Deferred),
        restricted_delivery: step("restricted", MembershipMaintenanceStepOutcome::Completed),
        synchronization: step("synchronize", MembershipMaintenanceStepOutcome::Completed),
        cleanup: step("cleanup", MembershipMaintenanceStepOutcome::Completed),
    });

    let report = maintain
        .execute(MembershipMaintenanceTrigger::Startup)
        .await;

    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[
            "admissions",
            "effects",
            "restricted",
            "synchronize",
            "cleanup"
        ]
    );
    assert_eq!(report.completed_count, 4);
    assert_eq!(report.deferred_count, 1);
    assert_eq!(report.stable_failure_count, 0);
}

#[tokio::test]
async fn corrupt_step_stops_later_permission_expanding_work() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let step = |name, outcome| {
        Arc::new(RecordingStep {
            name,
            calls: Arc::clone(&calls),
            outcome,
        })
    };
    let maintain = MaintainSpaceMembershipUseCase::new(MaintainSpaceMembershipDeps {
        admissions: step("admissions", MembershipMaintenanceStepOutcome::Completed),
        effects: step("effects", MembershipMaintenanceStepOutcome::Corrupt),
        restricted_delivery: step("restricted", MembershipMaintenanceStepOutcome::Completed),
        synchronization: step("synchronize", MembershipMaintenanceStepOutcome::Completed),
        cleanup: step("cleanup", MembershipMaintenanceStepOutcome::Completed),
    });

    let report = maintain
        .execute(MembershipMaintenanceTrigger::StateChanged)
        .await;

    assert_eq!(calls.lock().unwrap().as_slice(), &["admissions", "effects"]);
    assert_eq!(report.completed_count, 1);
    assert_eq!(report.corrupt_count, 1);
}

#[tokio::test]
async fn peer_online_runs_only_exact_peer_capabilities() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let step = |name| {
        Arc::new(RecordingStep {
            name,
            calls: Arc::clone(&calls),
            outcome: MembershipMaintenanceStepOutcome::Completed,
        })
    };
    let maintain = MaintainSpaceMembershipUseCase::new(MaintainSpaceMembershipDeps {
        admissions: step("admissions"),
        effects: step("effects"),
        restricted_delivery: step("restricted"),
        synchronization: step("synchronize"),
        cleanup: step("cleanup"),
    });

    let report = maintain
        .execute(MembershipMaintenanceTrigger::PeerOnline(
            uc_core::ids::DeviceId::new("device-b"),
        ))
        .await;

    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &["restricted", "synchronize"]
    );
    assert_eq!(report.completed_count, 2);
}

#[tokio::test]
async fn periodic_retries_history_when_synchronization_is_still_required() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let step = |name| {
        Arc::new(RecordingStep {
            name,
            calls: Arc::clone(&calls),
            outcome: MembershipMaintenanceStepOutcome::Completed,
        })
    };
    let maintain = MaintainSpaceMembershipUseCase::new(MaintainSpaceMembershipDeps {
        admissions: step("admissions"),
        effects: step("effects"),
        restricted_delivery: step("restricted"),
        synchronization: step("synchronize"),
        cleanup: step("cleanup"),
    });

    let report = maintain
        .execute(MembershipMaintenanceTrigger::Periodic)
        .await;

    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &["admissions", "effects", "restricted", "synchronize"]
    );
    assert_eq!(report.completed_count, 4);
}

async fn wait_for_call_count(calls: &Arc<Mutex<Vec<&'static str>>>, expected: usize) {
    for _ in 0..100 {
        if calls.lock().unwrap().len() >= expected {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("maintenance call count did not reach {expected}");
}

#[tokio::test]
async fn runtime_pause_resume_presence_and_shutdown_share_one_lifecycle() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let step = |name| {
        Arc::new(RecordingStep {
            name,
            calls: Arc::clone(&calls),
            outcome: MembershipMaintenanceStepOutcome::Completed,
        })
    };
    let maintain = Arc::new(MaintainSpaceMembershipUseCase::new(
        MaintainSpaceMembershipDeps {
            admissions: step("admissions"),
            effects: step("effects"),
            restricted_delivery: step("restricted"),
            synchronization: step("synchronize"),
            cleanup: step("cleanup"),
        },
    ));
    let (presence_tx, presence_rx) = tokio::sync::broadcast::channel(8);
    let runtime = SpaceMembershipRuntime::start(
        maintain,
        presence_rx,
        std::time::Duration::from_secs(3600),
        Arc::new(NoopNetworkActivity),
    );
    let activity = runtime.activity();
    wait_for_call_count(&calls, 5).await;

    activity.pause().await.unwrap();
    let _ = presence_tx.send(uc_core::ports::PresenceEvent {
        device_id: uc_core::ids::DeviceId::new("device-b"),
        state: uc_core::ports::ReachabilityState::Online,
        at: chrono::Utc::now(),
    });
    tokio::task::yield_now().await;
    assert_eq!(calls.lock().unwrap().len(), 5);

    activity.resume().await.unwrap();
    wait_for_call_count(&calls, 10).await;
    let _ = presence_tx.send(uc_core::ports::PresenceEvent {
        device_id: uc_core::ids::DeviceId::new("device-b"),
        state: uc_core::ports::ReachabilityState::Online,
        at: chrono::Utc::now(),
    });
    wait_for_call_count(&calls, 12).await;

    runtime.shutdown().await;
}

#[tokio::test]
async fn pause_cancels_network_work_and_waits_for_the_current_commit_boundary() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let step = |name| {
        Arc::new(RecordingStep {
            name,
            calls: Arc::clone(&calls),
            outcome: MembershipMaintenanceStepOutcome::Completed,
        })
    };
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let maintain = Arc::new(MaintainSpaceMembershipUseCase::new(
        MaintainSpaceMembershipDeps {
            admissions: Arc::new(BlockingAdmission {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
            }),
            effects: step("effects"),
            restricted_delivery: step("restricted"),
            synchronization: step("synchronize"),
            cleanup: step("cleanup"),
        },
    ));
    let network = Arc::new(PausingNetworkActivity {
        pauses: AtomicUsize::new(0),
        release,
    });
    let (_presence_tx, presence_rx) = tokio::sync::broadcast::channel(4);
    let runtime = SpaceMembershipRuntime::start(
        maintain,
        presence_rx,
        std::time::Duration::from_secs(3600),
        network.clone(),
    );
    started.notified().await;

    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        runtime.activity().pause(),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(network.pauses.load(Ordering::SeqCst), 1);
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &["effects", "restricted", "synchronize", "cleanup"]
    );
    runtime.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn shutdown_uses_one_five_second_budget_without_aborting_the_active_round() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let step = |name| {
        Arc::new(RecordingStep {
            name,
            calls: Arc::clone(&calls),
            outcome: MembershipMaintenanceStepOutcome::Completed,
        })
    };
    let started = Arc::new(tokio::sync::Notify::new());
    let maintain = Arc::new(MaintainSpaceMembershipUseCase::new(
        MaintainSpaceMembershipDeps {
            admissions: Arc::new(NonCooperativeAdmission {
                started: Arc::clone(&started),
            }),
            effects: step("effects"),
            restricted_delivery: step("restricted"),
            synchronization: step("synchronize"),
            cleanup: step("cleanup"),
        },
    ));
    let (_presence_tx, presence_rx) = tokio::sync::broadcast::channel(4);
    let runtime = SpaceMembershipRuntime::start(
        maintain,
        presence_rx,
        std::time::Duration::from_secs(3600),
        Arc::new(NoopNetworkActivity),
    );
    started.notified().await;

    let shutdown = tokio::spawn(runtime.shutdown());
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(5)).await;
    tokio::task::yield_now().await;

    assert!(shutdown.is_finished());
    shutdown.await.unwrap();
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn online_events_for_different_peers_are_not_overwritten_during_a_round() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let step = |name| {
        Arc::new(RecordingStep {
            name,
            calls: Arc::clone(&calls),
            outcome: MembershipMaintenanceStepOutcome::Completed,
        })
    };
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let maintain = Arc::new(MaintainSpaceMembershipUseCase::new(
        MaintainSpaceMembershipDeps {
            admissions: Arc::new(BlockingAdmission {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
            }),
            effects: step("effects"),
            restricted_delivery: step("restricted"),
            synchronization: step("synchronize"),
            cleanup: step("cleanup"),
        },
    ));
    let (presence_tx, presence_rx) = tokio::sync::broadcast::channel(4);
    let runtime = SpaceMembershipRuntime::start(
        maintain,
        presence_rx,
        std::time::Duration::from_secs(3600),
        Arc::new(NoopNetworkActivity),
    );
    started.notified().await;
    for device in ["device-b", "device-c"] {
        let _ = presence_tx.send(uc_core::ports::PresenceEvent {
            device_id: uc_core::ids::DeviceId::new(device),
            state: uc_core::ports::ReachabilityState::Online,
            at: chrono::Utc::now(),
        });
    }
    release.notify_one();

    wait_for_call_count(&calls, 8).await;

    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[
            "effects",
            "restricted",
            "synchronize",
            "cleanup",
            "restricted",
            "synchronize",
            "restricted",
            "synchronize",
        ]
    );
    runtime.shutdown().await;
}
