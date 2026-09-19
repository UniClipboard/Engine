use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::runtime::SpaceMembershipMaintenanceRuntimeError;
use super::*;

struct PanickingAdmission;

#[async_trait]
impl RecoverSpaceAdmissionsPort for PanickingAdmission {
    async fn recover_space_admissions(
        &self,
        _: &MembershipMaintenanceTrigger,
    ) -> AdmissionMaintenanceOutcome {
        panic!("PRIVATE_MEMBERSHIP_FAILURE");
    }
}

#[tokio::test]
async fn failed_round_is_retained_by_pause_resume_shutdown_and_application_report() {
    for finish_in_background in [false, true] {
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
                admissions: Arc::new(PanickingAdmission),
                effects: step("effects"),
                conflicts: step("conflicts"),
                group_update_delivery: step("group_updates"),
                restricted_delivery: step("restricted"),
                synchronization: step("synchronize"),
                cleanup: step("cleanup"),
            },
        ));
        let (_presence_tx, presence_rx) = tokio::sync::broadcast::channel(4);
        let runtime = SpaceMembershipMaintenanceRuntime::start(
            maintain,
            presence_rx,
            inactive_known_peer_contacts(),
            std::time::Duration::from_secs(3600),
            Arc::new(NoopNetworkActivity),
        );
        let activity = runtime.activity();
        if finish_in_background {
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while activity.request_state_changed().is_ok() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }
        let pause_error = activity.pause().await.unwrap_err();
        let SpaceMembershipMaintenanceRuntimeError::Task(source) = &pause_error else {
            panic!("missing original task failure");
        };
        assert!(source.is_panic());
        assert!(pause_error.source().is_some());
        assert!(!format!("{pause_error:?} {pause_error}").contains("PRIVATE"));
        let resume_error = activity.resume().await.unwrap_err();
        let SpaceMembershipMaintenanceRuntimeError::Task(resume_source) = resume_error else {
            panic!("resume lost task failure");
        };
        assert!(Arc::ptr_eq(source, &resume_source));
        let shutdown_error = runtime.shutdown().await.unwrap_err();
        let SpaceMembershipMaintenanceRuntimeError::Task(shutdown_source) = shutdown_error
            .downcast_ref::<SpaceMembershipMaintenanceRuntimeError>()
            .unwrap()
        else {
            panic!("shutdown lost task failure");
        };
        assert!(Arc::ptr_eq(source, shutdown_source));
        assert!(calls.lock().unwrap().is_empty());
    }
}

#[derive(Clone)]
pub(super) struct RecordingStep {
    pub(super) name: &'static str,
    pub(super) calls: Arc<Mutex<Vec<&'static str>>>,
    pub(super) outcome: MembershipMaintenanceStepOutcome,
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
    ) -> AdmissionMaintenanceOutcome {
        AdmissionMaintenanceOutcome::Continue(self.record())
    }
}

#[async_trait]
impl RecoverMembershipEffectsPort for RecordingStep {
    async fn recover_membership_effects(&self) -> MembershipMaintenanceStepOutcome {
        self.record()
    }
}

#[async_trait]
impl RecoverMembershipConflictsPort for RecordingStep {
    async fn recover_membership_conflicts(&self) -> MembershipMaintenanceStepOutcome {
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
impl DeliverPendingGroupUpdatesPort for RecordingStep {
    async fn deliver_pending_group_updates(
        &self,
        _: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceStepOutcome {
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
impl ReconcileMembershipProjectionPort for RecordingStep {
    async fn reconcile_membership_projection(&self) -> MembershipMaintenanceStepOutcome {
        self.record()
    }
}

pub(super) struct NoopNetworkActivity;

impl MembershipNetworkActivityPort for NoopNetworkActivity {
    fn pause_network_work(&self) {}
    fn resume_network_work(&self) {}
}

struct BlockingAdmission {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

struct YieldingAdmission {
    calls: Arc<Mutex<Vec<&'static str>>>,
}

#[async_trait]
impl RecoverSpaceAdmissionsPort for YieldingAdmission {
    async fn recover_space_admissions(
        &self,
        _trigger: &MembershipMaintenanceTrigger,
    ) -> AdmissionMaintenanceOutcome {
        self.calls.lock().unwrap().push("admissions");
        AdmissionMaintenanceOutcome::Yield(MembershipMaintenanceStepOutcome::Completed)
    }
}

struct BlockingFirstRecordingAdmission {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    calls: Arc<Mutex<Vec<&'static str>>>,
    first: std::sync::atomic::AtomicBool,
}

struct BlockingFirstContactSynchronization {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    triggers: Arc<Mutex<Vec<MembershipMaintenanceTrigger>>>,
    first_contact: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl SynchronizeMembershipMaintenancePort for BlockingFirstContactSynchronization {
    async fn periodic_synchronization_required(
        &self,
    ) -> Result<bool, MembershipMaintenanceStepOutcome> {
        Ok(true)
    }

    async fn synchronize_membership(
        &self,
        trigger: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceStepOutcome {
        self.triggers.lock().unwrap().push(trigger.clone());
        if matches!(trigger, MembershipMaintenanceTrigger::PeerContact(_))
            && self.first_contact.swap(false, Ordering::SeqCst)
        {
            self.started.notify_one();
            self.release.notified().await;
        }
        MembershipMaintenanceStepOutcome::Completed
    }
}

#[async_trait]
impl RecoverSpaceAdmissionsPort for BlockingFirstRecordingAdmission {
    async fn recover_space_admissions(
        &self,
        _trigger: &MembershipMaintenanceTrigger,
    ) -> AdmissionMaintenanceOutcome {
        self.calls.lock().unwrap().push("admissions");
        if self.first.swap(false, Ordering::SeqCst) {
            self.started.notify_one();
            self.release.notified().await;
        }
        AdmissionMaintenanceOutcome::Continue(MembershipMaintenanceStepOutcome::Completed)
    }
}

#[async_trait]
impl RecoverSpaceAdmissionsPort for BlockingAdmission {
    async fn recover_space_admissions(
        &self,
        _trigger: &MembershipMaintenanceTrigger,
    ) -> AdmissionMaintenanceOutcome {
        self.started.notify_one();
        self.release.notified().await;
        AdmissionMaintenanceOutcome::Continue(MembershipMaintenanceStepOutcome::Completed)
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
        conflicts: step("conflicts", MembershipMaintenanceStepOutcome::Completed),
        group_update_delivery: step("group_updates", MembershipMaintenanceStepOutcome::Completed),
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
            "restricted",
            "effects",
            "conflicts",
            "group_updates",
            "synchronize",
            "cleanup"
        ]
    );
    assert_eq!(report.completed_count, 6);
    assert_eq!(report.deferred_count, 1);
    assert_eq!(report.stable_failure_count, 0);
}

#[tokio::test]
async fn session_transition_stops_the_current_maintenance_round_after_admission() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let step = |name| {
        Arc::new(RecordingStep {
            name,
            calls: Arc::clone(&calls),
            outcome: MembershipMaintenanceStepOutcome::Completed,
        })
    };
    let maintain = MaintainSpaceMembershipUseCase::new(MaintainSpaceMembershipDeps {
        admissions: Arc::new(YieldingAdmission {
            calls: Arc::clone(&calls),
        }),
        effects: step("effects"),
        conflicts: step("conflicts"),
        group_update_delivery: step("group_updates"),
        restricted_delivery: step("restricted"),
        synchronization: step("synchronize"),
        cleanup: step("cleanup"),
    });

    let report = maintain
        .execute(MembershipMaintenanceTrigger::StateChanged)
        .await;

    assert_eq!(calls.lock().unwrap().as_slice(), &["admissions"]);
    assert_eq!(report.completed_count, 1);
    assert_eq!(report.deferred_count, 0);
}

#[tokio::test]
async fn deferred_projection_is_revisited_by_periodic_maintenance() {
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
        effects: step("effects", MembershipMaintenanceStepOutcome::Completed),
        conflicts: step("conflicts", MembershipMaintenanceStepOutcome::Completed),
        group_update_delivery: step("group_updates", MembershipMaintenanceStepOutcome::Completed),
        restricted_delivery: step("restricted", MembershipMaintenanceStepOutcome::Completed),
        synchronization: step("synchronize", MembershipMaintenanceStepOutcome::Completed),
        cleanup: step("projection", MembershipMaintenanceStepOutcome::Deferred),
    });
    assert_eq!(
        maintain
            .execute(MembershipMaintenanceTrigger::Startup)
            .await
            .deferred_count,
        1
    );
    let retry = maintain
        .execute(MembershipMaintenanceTrigger::Periodic)
        .await;
    assert_eq!(
        retry.deferred_count, 1,
        "暂时失败的成员资料维护必须在定期恢复中再次执行"
    );
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
        conflicts: step("conflicts", MembershipMaintenanceStepOutcome::Completed),
        group_update_delivery: step("group_updates", MembershipMaintenanceStepOutcome::Completed),
        restricted_delivery: step("restricted", MembershipMaintenanceStepOutcome::Completed),
        synchronization: step("synchronize", MembershipMaintenanceStepOutcome::Completed),
        cleanup: step("cleanup", MembershipMaintenanceStepOutcome::Completed),
    });

    let report = maintain
        .execute(MembershipMaintenanceTrigger::StateChanged)
        .await;

    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &["admissions", "restricted", "effects"]
    );
    assert_eq!(report.completed_count, 2);
    assert_eq!(report.corrupt_count, 1);
}

#[tokio::test]
async fn peer_online_runs_targeted_network_work_and_local_projection() {
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
        conflicts: step("conflicts"),
        group_update_delivery: step("group_updates"),
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
        &[
            "admissions",
            "conflicts",
            "group_updates",
            "restricted",
            "synchronize",
            "cleanup"
        ]
    );
    assert_eq!(report.completed_count, 6);
}

#[tokio::test]
async fn peer_contact_only_confirms_contacted_membership_and_local_projection() {
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
        conflicts: step("conflicts"),
        group_update_delivery: step("group_updates"),
        restricted_delivery: step("restricted"),
        synchronization: step("synchronize"),
        cleanup: step("cleanup"),
    });

    let report = maintain
        .execute(MembershipMaintenanceTrigger::PeerContact(
            uc_core::ids::DeviceId::new("device-b"),
        ))
        .await;

    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &["synchronize", "cleanup"]
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
        conflicts: step("conflicts"),
        group_update_delivery: step("group_updates"),
        restricted_delivery: step("restricted"),
        synchronization: step("synchronize"),
        cleanup: step("cleanup"),
    });

    let report = maintain
        .execute(MembershipMaintenanceTrigger::Periodic)
        .await;

    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[
            "admissions",
            "restricted",
            "effects",
            "conflicts",
            "group_updates",
            "synchronize",
            "cleanup"
        ]
    );
    assert_eq!(report.completed_count, 7);
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

fn inactive_known_peer_contacts() -> tokio::sync::broadcast::Receiver<KnownPeerContact> {
    tokio::sync::broadcast::channel(1).1
}

#[tokio::test]
async fn known_peer_contact_wakes_targeted_membership_confirmation() {
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
            conflicts: step("conflicts"),
            group_update_delivery: step("group_updates"),
            restricted_delivery: step("restricted"),
            synchronization: step("synchronize"),
            cleanup: step("cleanup"),
        },
    ));
    let (_peer_reachability_tx, peer_reachability_rx) = tokio::sync::broadcast::channel(4);
    let (known_peer_contact_tx, known_peer_contact_rx) = tokio::sync::broadcast::channel(4);
    let runtime = SpaceMembershipMaintenanceRuntime::start(
        maintain,
        peer_reachability_rx,
        known_peer_contact_rx,
        std::time::Duration::from_secs(3600),
        Arc::new(NoopNetworkActivity),
    );
    wait_for_call_count(&calls, 7).await;

    let _ = known_peer_contact_tx.send(KnownPeerContact {
        device_id: uc_core::ids::DeviceId::new("device-b"),
    });

    wait_for_call_count(&calls, 9).await;
    assert_eq!(
        &calls.lock().unwrap().as_slice()[7..],
        &["synchronize", "cleanup"]
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn repeated_known_peer_contacts_are_coalesced_and_run_serially() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let step = |name| {
        Arc::new(RecordingStep {
            name,
            calls: Arc::clone(&calls),
            outcome: MembershipMaintenanceStepOutcome::Completed,
        })
    };
    let triggers = Arc::new(Mutex::new(Vec::new()));
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let maintain = Arc::new(MaintainSpaceMembershipUseCase::new(
        MaintainSpaceMembershipDeps {
            admissions: step("admissions"),
            effects: step("effects"),
            conflicts: step("conflicts"),
            group_update_delivery: step("group_updates"),
            restricted_delivery: step("restricted"),
            synchronization: Arc::new(BlockingFirstContactSynchronization {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                triggers: Arc::clone(&triggers),
                first_contact: std::sync::atomic::AtomicBool::new(true),
            }),
            cleanup: step("cleanup"),
        },
    ));
    let (_peer_reachability_tx, peer_reachability_rx) = tokio::sync::broadcast::channel(4);
    let (known_peer_contact_tx, known_peer_contact_rx) = tokio::sync::broadcast::channel(8);
    let runtime = SpaceMembershipMaintenanceRuntime::start(
        maintain,
        peer_reachability_rx,
        known_peer_contact_rx,
        std::time::Duration::from_secs(3600),
        Arc::new(NoopNetworkActivity),
    );
    wait_for_call_count(&calls, 6).await;

    let device_b = uc_core::ids::DeviceId::new("device-b");
    let device_c = uc_core::ids::DeviceId::new("device-c");
    let _ = known_peer_contact_tx.send(KnownPeerContact {
        device_id: device_b.clone(),
    });
    started.notified().await;
    for device_id in [device_b.clone(), device_b.clone(), device_c.clone()] {
        let _ = known_peer_contact_tx.send(KnownPeerContact { device_id });
    }
    release.notify_one();

    for _ in 0..100 {
        if triggers.lock().unwrap().len() >= 4 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        &triggers.lock().unwrap().as_slice()[1..],
        &[
            MembershipMaintenanceTrigger::PeerContact(device_b.clone()),
            MembershipMaintenanceTrigger::PeerContact(device_b),
            MembershipMaintenanceTrigger::PeerContact(device_c),
        ]
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn runtime_pause_resume_peer_reachability_and_shutdown_share_one_lifecycle() {
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
            conflicts: step("conflicts"),
            group_update_delivery: step("group_updates"),
            restricted_delivery: step("restricted"),
            synchronization: step("synchronize"),
            cleanup: step("cleanup"),
        },
    ));
    let (peer_reachability_tx, peer_reachability_rx) = tokio::sync::broadcast::channel(8);
    let runtime = SpaceMembershipMaintenanceRuntime::start(
        maintain,
        peer_reachability_rx,
        inactive_known_peer_contacts(),
        std::time::Duration::from_secs(3600),
        Arc::new(NoopNetworkActivity),
    );
    let activity = runtime.activity();
    wait_for_call_count(&calls, 7).await;

    activity.pause().await.unwrap();
    let _ = peer_reachability_tx.send(uc_core::ports::PeerReachabilityChanged {
        device_id: uc_core::ids::DeviceId::new("device-b"),
        state: uc_core::ports::ReachabilityState::Online,
        at: chrono::Utc::now(),
    });
    tokio::task::yield_now().await;
    assert_eq!(calls.lock().unwrap().len(), 7);

    activity.resume().await.unwrap();
    wait_for_call_count(&calls, 14).await;
    let _ = peer_reachability_tx.send(uc_core::ports::PeerReachabilityChanged {
        device_id: uc_core::ids::DeviceId::new("device-b"),
        state: uc_core::ports::ReachabilityState::Online,
        at: chrono::Utc::now(),
    });
    wait_for_call_count(&calls, 19).await;

    runtime.shutdown().await.unwrap();
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
            conflicts: step("conflicts"),
            group_update_delivery: step("group_updates"),
            restricted_delivery: step("restricted"),
            synchronization: step("synchronize"),
            cleanup: step("cleanup"),
        },
    ));
    let network = Arc::new(PausingNetworkActivity {
        pauses: AtomicUsize::new(0),
        release,
    });
    let (_peer_reachability_tx, peer_reachability_rx) = tokio::sync::broadcast::channel(4);
    let runtime = SpaceMembershipMaintenanceRuntime::start(
        maintain,
        peer_reachability_rx,
        inactive_known_peer_contacts(),
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
        &[
            "restricted",
            "effects",
            "conflicts",
            "group_updates",
            "synchronize",
            "cleanup"
        ]
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn admission_deadline_wakes_maintenance_at_the_exact_boundary() {
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
            conflicts: step("conflicts"),
            group_update_delivery: step("group_updates"),
            restricted_delivery: step("restricted"),
            synchronization: step("synchronize"),
            cleanup: step("cleanup"),
        },
    ));
    let (_peer_reachability_tx, peer_reachability_rx) = tokio::sync::broadcast::channel(4);
    let runtime = SpaceMembershipMaintenanceRuntime::start(
        maintain,
        peer_reachability_rx,
        inactive_known_peer_contacts(),
        std::time::Duration::from_secs(3600),
        Arc::new(NoopNetworkActivity),
    );
    wait_for_call_count(&calls, 7).await;
    calls.lock().unwrap().clear();

    runtime.activity().schedule_at(301_000, 1_000);
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_millis(299_999)).await;
    tokio::task::yield_now().await;
    assert!(calls.lock().unwrap().is_empty());

    tokio::time::advance(std::time::Duration::from_millis(1)).await;
    wait_for_call_count(&calls, 7).await;
    assert_eq!(calls.lock().unwrap().first(), Some(&"admissions"));
    runtime.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_later_admission_deadline_cannot_postpone_the_nearest_wake() {
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
            conflicts: step("conflicts"),
            group_update_delivery: step("group_updates"),
            restricted_delivery: step("restricted"),
            synchronization: step("synchronize"),
            cleanup: step("cleanup"),
        },
    ));
    let (_peer_reachability_tx, peer_reachability_rx) = tokio::sync::broadcast::channel(4);
    let runtime = SpaceMembershipMaintenanceRuntime::start(
        maintain,
        peer_reachability_rx,
        inactive_known_peer_contacts(),
        std::time::Duration::from_secs(3600),
        Arc::new(NoopNetworkActivity),
    );
    wait_for_call_count(&calls, 7).await;
    calls.lock().unwrap().clear();

    runtime.activity().schedule_at(2_000, 1_000);
    runtime.activity().schedule_at(4_000, 1_000);
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_millis(1_000)).await;

    wait_for_call_count(&calls, 7).await;
    assert_eq!(calls.lock().unwrap().first(), Some(&"admissions"));
    runtime.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn shutdown_waits_beyond_the_old_timeout_until_the_active_round_finishes() {
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
            conflicts: step("conflicts"),
            group_update_delivery: step("group_updates"),
            restricted_delivery: step("restricted"),
            synchronization: step("synchronize"),
            cleanup: step("cleanup"),
        },
    ));
    let (_peer_reachability_tx, peer_reachability_rx) = tokio::sync::broadcast::channel(4);
    let runtime = SpaceMembershipMaintenanceRuntime::start(
        maintain,
        peer_reachability_rx,
        inactive_known_peer_contacts(),
        std::time::Duration::from_secs(3600),
        Arc::new(NoopNetworkActivity),
    );
    started.notified().await;

    let shutdown = tokio::spawn(runtime.shutdown());
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(5)).await;
    tokio::task::yield_now().await;

    assert!(!shutdown.is_finished());
    assert!(calls.lock().unwrap().is_empty());
    release.notify_one();
    shutdown.await.unwrap().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 6);
}

#[tokio::test]
async fn dropping_the_owner_or_shutdown_waiter_keeps_the_round_owned_until_completion() {
    for drop_waiter in [false, true] {
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
                conflicts: step("conflicts"),
                group_update_delivery: step("group_updates"),
                restricted_delivery: step("restricted"),
                synchronization: step("synchronize"),
                cleanup: step("cleanup"),
            },
        ));
        let (_presence_tx, presence_rx) = tokio::sync::broadcast::channel(4);
        let runtime = SpaceMembershipMaintenanceRuntime::start(
            maintain,
            presence_rx,
            inactive_known_peer_contacts(),
            std::time::Duration::from_secs(3600),
            Arc::new(NoopNetworkActivity),
        );
        let activity = runtime.activity();
        started.notified().await;
        if drop_waiter {
            let waiter = tokio::spawn(runtime.shutdown());
            tokio::task::yield_now().await;
            waiter.abort();
            assert!(waiter.await.unwrap_err().is_cancelled());
        } else {
            drop(runtime);
        }
        let confirmation = tokio::spawn(async move { activity.pause().await });
        tokio::task::yield_now().await;
        let finished_before_release = confirmation.is_finished();
        release.notify_one();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), confirmation)
            .await
            .unwrap()
            .unwrap();
        assert!(!finished_before_release);
        assert!(matches!(
            result,
            Err(SpaceMembershipMaintenanceRuntimeError::Closed)
        ));
        assert_eq!(calls.lock().unwrap().len(), 6);
    }
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
            admissions: Arc::new(BlockingFirstRecordingAdmission {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                calls: Arc::clone(&calls),
                first: std::sync::atomic::AtomicBool::new(true),
            }),
            effects: step("effects"),
            conflicts: step("conflicts"),
            group_update_delivery: step("group_updates"),
            restricted_delivery: step("restricted"),
            synchronization: step("synchronize"),
            cleanup: step("cleanup"),
        },
    ));
    let (peer_reachability_tx, peer_reachability_rx) = tokio::sync::broadcast::channel(4);
    let runtime = SpaceMembershipMaintenanceRuntime::start(
        maintain,
        peer_reachability_rx,
        inactive_known_peer_contacts(),
        std::time::Duration::from_secs(3600),
        Arc::new(NoopNetworkActivity),
    );
    started.notified().await;
    for device in ["device-b", "device-c"] {
        let _ = peer_reachability_tx.send(uc_core::ports::PeerReachabilityChanged {
            device_id: uc_core::ids::DeviceId::new(device),
            state: uc_core::ports::ReachabilityState::Online,
            at: chrono::Utc::now(),
        });
    }
    release.notify_one();

    wait_for_call_count(&calls, 19).await;

    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[
            "admissions",
            "restricted",
            "effects",
            "conflicts",
            "group_updates",
            "synchronize",
            "cleanup",
            "admissions",
            "conflicts",
            "group_updates",
            "restricted",
            "synchronize",
            "cleanup",
            "admissions",
            "conflicts",
            "group_updates",
            "restricted",
            "synchronize",
            "cleanup",
        ]
    );
    runtime.shutdown().await.unwrap();
}
