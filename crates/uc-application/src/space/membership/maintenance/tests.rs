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
                work: step("work"),
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
        AdmissionMaintenanceOutcome::new(SpaceWorkMode::Active, self.record())
    }
}

#[async_trait]
impl RunMembershipWorkPort for RecordingStep {
    async fn run_membership_work(
        &self,
        _trigger: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceReport {
        report_of(self.record())
    }
}

fn report_of(outcome: MembershipMaintenanceStepOutcome) -> MembershipMaintenanceReport {
    let mut report = MembershipMaintenanceReport::default();
    match outcome {
        MembershipMaintenanceStepOutcome::Completed => report.completed_count = 1,
        MembershipMaintenanceStepOutcome::Deferred => report.deferred_count = 1,
        MembershipMaintenanceStepOutcome::StableFailure => report.stable_failure_count = 1,
        MembershipMaintenanceStepOutcome::Corrupt => report.corrupt_count = 1,
    }
    report
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

struct DeferredPairingAdmission {
    calls: Arc<Mutex<Vec<&'static str>>>,
}

struct PairingThenActiveAdmission {
    calls: Arc<Mutex<Vec<&'static str>>>,
    pairing: std::sync::atomic::AtomicBool,
}

struct BlockingPairingAdmission {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

struct SharedWorkPermit {
    lock: Arc<tokio::sync::Mutex<()>>,
}

#[async_trait]
impl AcquireSpaceWorkPermitPort for SharedWorkPermit {
    async fn acquire_space_work_permit(&self) -> Result<SpaceWorkPermit, QuerySpaceWorkModeError> {
        let guard = Arc::clone(&self.lock).lock_owned().await;
        Ok(SpaceWorkPermit::guarded(SpaceWorkMode::Active, guard))
    }
}

#[async_trait]
impl RecoverSpaceAdmissionsPort for BlockingPairingAdmission {
    async fn recover_space_admissions(
        &self,
        _trigger: &MembershipMaintenanceTrigger,
    ) -> AdmissionMaintenanceOutcome {
        self.started.notify_one();
        self.release.notified().await;
        AdmissionMaintenanceOutcome::new(
            SpaceWorkMode::Pairing,
            MembershipMaintenanceStepOutcome::Deferred,
        )
    }
}

#[async_trait]
impl RecoverSpaceAdmissionsPort for YieldingAdmission {
    async fn recover_space_admissions(
        &self,
        _trigger: &MembershipMaintenanceTrigger,
    ) -> AdmissionMaintenanceOutcome {
        self.calls.lock().unwrap().push("admissions");
        AdmissionMaintenanceOutcome::new(
            SpaceWorkMode::Pairing,
            MembershipMaintenanceStepOutcome::Completed,
        )
    }
}

#[async_trait]
impl RecoverSpaceAdmissionsPort for DeferredPairingAdmission {
    async fn recover_space_admissions(
        &self,
        _trigger: &MembershipMaintenanceTrigger,
    ) -> AdmissionMaintenanceOutcome {
        self.calls.lock().unwrap().push("admissions");
        AdmissionMaintenanceOutcome::new(
            SpaceWorkMode::Pairing,
            MembershipMaintenanceStepOutcome::Deferred,
        )
    }
}

#[async_trait]
impl RecoverSpaceAdmissionsPort for PairingThenActiveAdmission {
    async fn recover_space_admissions(
        &self,
        _trigger: &MembershipMaintenanceTrigger,
    ) -> AdmissionMaintenanceOutcome {
        self.calls.lock().unwrap().push("admissions");
        let mode = if self.pairing.swap(false, Ordering::SeqCst) {
            SpaceWorkMode::Pairing
        } else {
            SpaceWorkMode::Active
        };
        AdmissionMaintenanceOutcome::new(mode, MembershipMaintenanceStepOutcome::Completed)
    }
}

struct BlockingFirstRecordingAdmission {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    calls: Arc<Mutex<Vec<&'static str>>>,
    first: std::sync::atomic::AtomicBool,
}

struct BlockingFirstChangeWork {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    triggers: Arc<Mutex<Vec<MembershipMaintenanceTrigger>>>,
    first_change: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl RunMembershipWorkPort for BlockingFirstChangeWork {
    async fn run_membership_work(
        &self,
        trigger: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceReport {
        self.triggers.lock().unwrap().push(trigger.clone());
        if matches!(trigger, MembershipMaintenanceTrigger::StateChanged)
            && self.first_change.swap(false, Ordering::SeqCst)
        {
            self.started.notify_one();
            self.release.notified().await;
        }
        report_of(MembershipMaintenanceStepOutcome::Completed)
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
        AdmissionMaintenanceOutcome::new(
            SpaceWorkMode::Active,
            MembershipMaintenanceStepOutcome::Completed,
        )
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
        AdmissionMaintenanceOutcome::new(
            SpaceWorkMode::Active,
            MembershipMaintenanceStepOutcome::Completed,
        )
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
async fn startup_runs_admissions_then_all_due_membership_work() {
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
        work: step("work", MembershipMaintenanceStepOutcome::Deferred),
    });

    let report = maintain
        .execute(MembershipMaintenanceTrigger::Startup)
        .await;

    assert_eq!(calls.lock().unwrap().as_slice(), &["admissions", "work"]);
    assert_eq!(report.completed_count, 1);
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
        work: step("work"),
    });

    let report = maintain
        .execute(MembershipMaintenanceTrigger::StateChanged)
        .await;

    assert_eq!(calls.lock().unwrap().as_slice(), &["admissions"]);
    assert_eq!(report.completed_count, 1);
    assert_eq!(report.deferred_count, 0);
}

#[tokio::test]
async fn unfinished_pairing_with_transient_network_failure_excludes_ordinary_maintenance() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let step = |name| {
        Arc::new(RecordingStep {
            name,
            calls: Arc::clone(&calls),
            outcome: MembershipMaintenanceStepOutcome::Completed,
        })
    };
    let maintain = MaintainSpaceMembershipUseCase::new(MaintainSpaceMembershipDeps {
        admissions: Arc::new(DeferredPairingAdmission {
            calls: Arc::clone(&calls),
        }),
        work: step("work"),
    });

    let report = maintain
        .execute(MembershipMaintenanceTrigger::StateChanged)
        .await;

    assert_eq!(calls.lock().unwrap().as_slice(), &["admissions"]);
    assert_eq!(report.deferred_count, 1);
    assert_eq!(report.completed_count, 0);
}

#[tokio::test]
async fn local_admission_actions_can_interrupt_pairing_recovery_before_ordinary_work() {
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
    let work_lock = Arc::new(tokio::sync::Mutex::new(()));
    let maintain = Arc::new(MaintainSpaceMembershipUseCase::new_coordinated(
        MaintainSpaceMembershipDeps {
            admissions: Arc::new(BlockingPairingAdmission {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
            }),
            work: step("work"),
        },
        Arc::new(SharedWorkPermit {
            lock: Arc::clone(&work_lock),
        }),
    ));
    let round = tokio::spawn(async move {
        maintain
            .execute(MembershipMaintenanceTrigger::StateChanged)
            .await
    });
    started.notified().await;

    let local_action = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        Arc::clone(&work_lock).lock_owned(),
    )
    .await
    .expect("local admission action must not wait for pairing network recovery");
    release.notify_one();
    let report = round.await.unwrap();
    drop(local_action);

    assert_eq!(report.deferred_count, 1);
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn ordinary_maintenance_resumes_after_pairing_reaches_its_terminal_state() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let step = |name| {
        Arc::new(RecordingStep {
            name,
            calls: Arc::clone(&calls),
            outcome: MembershipMaintenanceStepOutcome::Completed,
        })
    };
    let maintain = MaintainSpaceMembershipUseCase::new(MaintainSpaceMembershipDeps {
        admissions: Arc::new(PairingThenActiveAdmission {
            calls: Arc::clone(&calls),
            pairing: std::sync::atomic::AtomicBool::new(true),
        }),
        work: step("work"),
    });

    let pairing = maintain
        .execute(MembershipMaintenanceTrigger::StateChanged)
        .await;
    assert_eq!(calls.lock().unwrap().as_slice(), &["admissions"]);
    assert_eq!(pairing.completed_count, 1);

    let active = maintain
        .execute(MembershipMaintenanceTrigger::StateChanged)
        .await;
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &["admissions", "admissions", "work"]
    );
    assert_eq!(active.completed_count, 2);
}

#[tokio::test]
async fn corrupt_admission_recovery_does_not_block_ordinary_membership_work() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let step = |name, outcome| {
        Arc::new(RecordingStep {
            name,
            calls: Arc::clone(&calls),
            outcome,
        })
    };
    let maintain = MaintainSpaceMembershipUseCase::new(MaintainSpaceMembershipDeps {
        admissions: step("admissions", MembershipMaintenanceStepOutcome::Corrupt),
        work: step("work", MembershipMaintenanceStepOutcome::Completed),
    });

    let report = maintain
        .execute(MembershipMaintenanceTrigger::StateChanged)
        .await;

    assert_eq!(calls.lock().unwrap().as_slice(), &["admissions", "work"]);
    assert_eq!(report.completed_count, 1);
    assert_eq!(report.corrupt_count, 1);
}

#[tokio::test]
async fn state_change_runs_complete_ordinary_maintenance() {
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
        work: step("work"),
    });

    let report = maintain
        .execute(MembershipMaintenanceTrigger::StateChanged)
        .await;

    assert_eq!(calls.lock().unwrap().as_slice(), &["admissions", "work"]);
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
        work: step("work"),
    });

    let report = maintain
        .execute(MembershipMaintenanceTrigger::Periodic)
        .await;

    assert_eq!(calls.lock().unwrap().as_slice(), &["admissions", "work"]);
    assert_eq!(report.completed_count, 2);
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
async fn known_peer_contact_wakes_one_complete_maintenance_round() {
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
            work: step("work"),
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
    wait_for_call_count(&calls, 2).await;

    let _ = known_peer_contact_tx.send(KnownPeerContact {
        device_id: uc_core::ids::DeviceId::new("device-b"),
    });

    wait_for_call_count(&calls, 4).await;
    assert_eq!(
        &calls.lock().unwrap().as_slice()[2..],
        &["admissions", "work"]
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn peer_contacts_submit_one_generic_change_wake_and_coalesce_while_running() {
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
            work: Arc::new(BlockingFirstChangeWork {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                triggers: Arc::clone(&triggers),
                first_change: std::sync::atomic::AtomicBool::new(true),
            }),
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
    wait_for_call_count(&calls, 1).await;
    for _ in 0..100 {
        if !triggers.lock().unwrap().is_empty() {
            break;
        }
        tokio::task::yield_now().await;
    }

    let activity = runtime.activity();
    activity.request_state_changed().unwrap();
    started.notified().await;

    let device_b = uc_core::ids::DeviceId::new("device-b");
    let device_c = uc_core::ids::DeviceId::new("device-c");
    for device_id in [device_b.clone(), device_b.clone(), device_c.clone()] {
        let _ = known_peer_contact_tx.send(KnownPeerContact { device_id });
    }
    release.notify_one();

    for _ in 0..100 {
        if triggers.lock().unwrap().len() >= 3 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        &triggers.lock().unwrap().as_slice()[1..],
        &[
            MembershipMaintenanceTrigger::StateChanged,
            MembershipMaintenanceTrigger::StateChanged,
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
            work: step("work"),
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
    wait_for_call_count(&calls, 2).await;

    activity.pause().await.unwrap();
    let _ = peer_reachability_tx.send(uc_core::ports::PeerReachabilityChanged {
        device_id: uc_core::ids::DeviceId::new("device-b"),
        state: uc_core::ports::ReachabilityState::Online,
        at: chrono::Utc::now(),
    });
    tokio::task::yield_now().await;
    assert_eq!(calls.lock().unwrap().len(), 2);

    activity.resume().await.unwrap();
    wait_for_call_count(&calls, 4).await;
    let _ = peer_reachability_tx.send(uc_core::ports::PeerReachabilityChanged {
        device_id: uc_core::ids::DeviceId::new("device-b"),
        state: uc_core::ports::ReachabilityState::Online,
        at: chrono::Utc::now(),
    });
    wait_for_call_count(&calls, 6).await;

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
            work: step("work"),
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
    assert_eq!(calls.lock().unwrap().as_slice(), &["work"]);
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
            work: step("work"),
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
    wait_for_call_count(&calls, 2).await;
    calls.lock().unwrap().clear();

    runtime.activity().schedule_at(301_000, 1_000);
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_millis(299_999)).await;
    tokio::task::yield_now().await;
    assert!(calls.lock().unwrap().is_empty());

    tokio::time::advance(std::time::Duration::from_millis(1)).await;
    wait_for_call_count(&calls, 2).await;
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
            work: step("work"),
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
    wait_for_call_count(&calls, 2).await;
    calls.lock().unwrap().clear();

    runtime.activity().schedule_at(2_000, 1_000);
    runtime.activity().schedule_at(4_000, 1_000);
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_millis(1_000)).await;

    wait_for_call_count(&calls, 2).await;
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
            work: step("work"),
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
    assert_eq!(calls.lock().unwrap().len(), 1);
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
                work: step("work"),
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
        assert_eq!(calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn online_events_for_different_peers_coalesce_into_one_change_wake() {
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
            work: step("work"),
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

    wait_for_call_count(&calls, 4).await;

    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &["admissions", "work", "admissions", "work"]
    );
    runtime.shutdown().await.unwrap();
}
