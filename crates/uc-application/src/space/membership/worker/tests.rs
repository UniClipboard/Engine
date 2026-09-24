use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{LedgerWork, MemberEffectPhase, PeerLink, DEPARTURE_WINDOW_MS};
use uc_core::ports::{ClockPort, ReachabilityState};

use crate::space::membership::query_device_trust::NoCurrentJoinStatus;
use crate::space::membership::testing::{
    EstablishedSpace, OwnerFixture, TestSigner, WorkerFixture, WorkerPorts,
};
use crate::space::membership::{
    DeviceTrustObservation, LoadDeviceTrustObservationsPort, MembershipEffectExecutionError,
    MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger, QueryDeviceTrustError,
    QueryDeviceTrustUseCase, RecoverMembershipEffectsPort, RemoveSpaceMemberUseCase,
    RestrictedMembershipDelivery, RestrictedMembershipDeliveryError, RunMembershipWorkPort,
};

struct OfflineObservations;

#[async_trait]
impl LoadDeviceTrustObservationsPort for OfflineObservations {
    async fn load(
        &self,
        device_ids: &[DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError> {
        Ok(device_ids
            .iter()
            .map(|device_id| DeviceTrustObservation {
                device_id: *device_id,
                display_name: None,
                reachability: ReachabilityState::Offline,
            })
            .collect())
    }
}

/// 移除动作不立即推进效果，效果全部留给执行器。
struct LeaveEffectsToWorker;

#[async_trait]
impl RecoverMembershipEffectsPort for LeaveEffectsToWorker {
    async fn recover_membership_effects(&self) -> MembershipMaintenanceStepOutcome {
        MembershipMaintenanceStepOutcome::Deferred
    }
}

struct Case {
    owner: OwnerFixture,
    worker: WorkerFixture,
}

fn device_b() -> DeviceId {
    DeviceId::new("device-b")
}

/// 本机 device-a 已签名移除 device-b，效果与通知都尚未执行。
async fn removed_peer() -> Case {
    let space = EstablishedSpace::new(&["device-a", "device-b"]);
    let owner = OwnerFixture::new(space.record("device-a", 1));
    let signer: TestSigner = space.signer("device-a");
    let query = Arc::new(QueryDeviceTrustUseCase::new_for_tests(
        owner.owner.clone(),
        Arc::new(OfflineObservations),
        Arc::new(NoCurrentJoinStatus),
    ));
    RemoveSpaceMemberUseCase::new(
        owner.owner.clone(),
        Arc::new(signer),
        query,
        Arc::new(LeaveEffectsToWorker),
    )
    .execute(&device_b())
    .await
    .unwrap();
    let worker = WorkerFixture::new(owner.owner.clone(), WorkerPorts::default());
    Case { owner, worker }
}

fn removal_notice_outstanding(case: &Case) -> bool {
    case.owner
        .records
        .ledger()
        .outstanding_work(case.owner.clock.now_ms())
        .unwrap()
        .iter()
        .any(|item| matches!(item.work, LedgerWork::DeliverRemovalNotice { .. }))
}

#[tokio::test]
async fn one_run_advances_every_effect_phase_in_order_and_delivers_the_removal_notice() {
    let case = removed_peer().await;

    let report = case
        .worker
        .worker
        .run_membership_work(&MembershipMaintenanceTrigger::StateChanged)
        .await;

    assert_eq!(
        case.worker.effects.steps(),
        vec!["member_facts", "security", "activation"]
    );
    let deliveries = case.worker.delivery.deliveries();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].0, device_b());
    assert!(matches!(
        deliveries[0].1,
        RestrictedMembershipDelivery::Event(_)
    ));
    // 通知送达即结束离开，不再等待窗口。
    let ledger = case.owner.records.ledger();
    assert_eq!(ledger.unfinished_effects().count(), 0);
    assert!(ledger.peer(&device_b()).is_none());
    assert_eq!(report.stable_failure_count, 0);
    assert_eq!(report.corrupt_count, 0);
}

#[tokio::test]
async fn a_deferred_effect_step_stays_at_its_phase_until_a_later_run() {
    let case = removed_peer().await;
    case.worker
        .effects
        .fail_next(MembershipEffectExecutionError::Deferred);

    let report = case
        .worker
        .worker
        .run_membership_work(&MembershipMaintenanceTrigger::StateChanged)
        .await;

    assert!(report.deferred_count >= 1);
    assert!(case.worker.effects.steps().is_empty());
    let effect_phase = case
        .owner
        .records
        .ledger()
        .unfinished_effects()
        .map(|effect| effect.phase())
        .next();
    assert_eq!(effect_phase, Some(MemberEffectPhase::Prepared));

    case.worker
        .worker
        .run_membership_work(&MembershipMaintenanceTrigger::Periodic)
        .await;

    assert_eq!(
        case.worker.effects.steps(),
        vec!["member_facts", "security", "activation"]
    );
    assert_eq!(case.owner.records.ledger().unfinished_effects().count(), 0);
}

#[tokio::test]
async fn an_undelivered_notice_is_retried_by_each_later_run_within_the_window() {
    let case = removed_peer().await;
    case.worker
        .delivery
        .respond_with(Err(RestrictedMembershipDeliveryError::Deferred));

    let report = case
        .worker
        .worker
        .run_membership_work(&MembershipMaintenanceTrigger::StateChanged)
        .await;

    // 同一次运行中只尝试一次；下一次唤醒安排在窗口结束时。
    assert!(report.deferred_count >= 1);
    assert_eq!(case.worker.delivery.deliveries().len(), 1);
    assert!(removal_notice_outstanding(&case));
    let window_end = match case.owner.records.ledger().peer(&device_b()) {
        Some(PeerLink::Departing(departing)) => departing.expires_at_ms(),
        _ => panic!("the removed device is no longer departing"),
    };
    assert!(case.owner.wake.deadlines().contains(&window_end));

    case.worker.delivery.respond_with(Ok(()));
    case.worker
        .worker
        .run_membership_work(&MembershipMaintenanceTrigger::Periodic)
        .await;

    assert_eq!(case.worker.delivery.deliveries().len(), 2);
    assert!(case.owner.records.ledger().peer(&device_b()).is_none());
}

#[tokio::test]
async fn a_rejected_notice_is_a_stable_failure_bounded_by_the_window() {
    let case = removed_peer().await;
    case.worker
        .delivery
        .respond_with(Err(RestrictedMembershipDeliveryError::Rejected));

    let report = case
        .worker
        .worker
        .run_membership_work(&MembershipMaintenanceTrigger::StateChanged)
        .await;

    assert_eq!(report.stable_failure_count, 1);
    assert!(matches!(
        case.owner.records.ledger().peer(&device_b()),
        Some(PeerLink::Departing(_))
    ));
}

#[tokio::test]
async fn the_departure_ends_exactly_when_its_window_elapses() {
    let case = removed_peer().await;
    case.worker
        .delivery
        .respond_with(Err(RestrictedMembershipDeliveryError::Deferred));
    let removed_at = case.owner.clock.now_ms();
    case.worker
        .worker
        .run_membership_work(&MembershipMaintenanceTrigger::StateChanged)
        .await;

    case.owner.clock.set(removed_at + DEPARTURE_WINDOW_MS - 1);
    case.worker
        .worker
        .run_membership_work(&MembershipMaintenanceTrigger::Periodic)
        .await;
    assert!(case.owner.records.ledger().peer(&device_b()).is_some());

    case.owner.clock.set(removed_at + DEPARTURE_WINDOW_MS);
    case.worker
        .worker
        .run_membership_work(&MembershipMaintenanceTrigger::Periodic)
        .await;
    assert!(case.owner.records.ledger().peer(&device_b()).is_none());
    // 离开结束后成员读模型不再保留该设备。
    let projection = case.owner.records.last_projection().unwrap();
    assert!(projection
        .members
        .iter()
        .all(|facts| facts.device_id != device_b()));
}

#[tokio::test]
async fn recovering_effects_reports_deferred_while_an_effect_remains() {
    let case = removed_peer().await;
    case.worker
        .effects
        .fail_next(MembershipEffectExecutionError::Deferred);

    let deferred = case.worker.worker.recover_membership_effects().await;
    let completed = case.worker.worker.recover_membership_effects().await;

    assert_eq!(deferred, MembershipMaintenanceStepOutcome::Deferred);
    assert_eq!(completed, MembershipMaintenanceStepOutcome::Completed);
    // 只推进效果，不执行网络待办。
    assert!(case.worker.delivery.deliveries().is_empty());
    assert!(removal_notice_outstanding(&case));
}

#[tokio::test]
async fn concurrent_effect_recovery_runs_each_phase_once() {
    let case = removed_peer().await;

    let (first, second) = tokio::join!(
        case.worker.worker.recover_membership_effects(),
        case.worker.worker.recover_membership_effects()
    );

    assert_eq!(first, MembershipMaintenanceStepOutcome::Completed);
    assert_eq!(second, MembershipMaintenanceStepOutcome::Completed);
    assert_eq!(
        case.worker.effects.steps(),
        vec!["member_facts", "security", "activation"]
    );
    assert_eq!(case.owner.records.ledger().unfinished_effects().count(), 0);
}
