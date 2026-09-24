//! 成员待办执行器测试台：可记录的效果、投递与历史交换替身。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    MembershipHistoryExchangeError, MembershipHistoryExchangePort, MembershipHistoryMessage,
    UnfinishedMemberEffect,
};

use crate::space::membership::{
    ActivateMembershipEffectPort, ApplyMembershipMemberFactsPort, ApplyMembershipSecurityPort,
    DeliverPendingGroupUpdatesPort, MembershipEffectExecutionError,
    MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger, MembershipOwner,
    MembershipWorker, MembershipWorkerDeps, RecoverMembershipConflictsPort,
    RefreshVerifiedPeerAddressPort, RestrictedMembershipDelivery,
    RestrictedMembershipDeliveryError, RestrictedMembershipDeliveryPort,
};

/// 按阶段记录并成功完成的成员效果能力。
#[derive(Default)]
pub(crate) struct RecordingEffects {
    steps: Mutex<Vec<(&'static str, uc_core::membership::MembershipEventId)>>,
    failure: Mutex<Option<MembershipEffectExecutionError>>,
}

impl RecordingEffects {
    pub(crate) fn steps(&self) -> Vec<&'static str> {
        self.steps
            .lock()
            .unwrap()
            .iter()
            .map(|(step, _)| *step)
            .collect()
    }

    /// 下一步效果执行以给定错误失败。
    pub(crate) fn fail_next(&self, error: MembershipEffectExecutionError) {
        *self.failure.lock().unwrap() = Some(error);
    }

    fn record(
        &self,
        step: &'static str,
        effect: &UnfinishedMemberEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        if let Some(error) = self.failure.lock().unwrap().take() {
            return Err(error);
        }
        self.steps.lock().unwrap().push((step, effect.event_id()));
        Ok(())
    }
}

#[async_trait]
impl ApplyMembershipMemberFactsPort for RecordingEffects {
    async fn apply_member_facts(
        &self,
        effect: &UnfinishedMemberEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        self.record("member_facts", effect)
    }
}

#[async_trait]
impl ApplyMembershipSecurityPort for RecordingEffects {
    async fn apply_membership_security(
        &self,
        effect: &UnfinishedMemberEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        self.record("security", effect)
    }
}

#[async_trait]
impl ActivateMembershipEffectPort for RecordingEffects {
    async fn activate_membership_effect(
        &self,
        effect: &UnfinishedMemberEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        self.record("activation", effect)
    }
}

/// 受限投递替身：记录每次投递并返回预设结果。
pub(crate) struct RecordingDelivery {
    deliveries: Mutex<Vec<(DeviceId, RestrictedMembershipDelivery)>>,
    result: Mutex<Result<(), RestrictedMembershipDeliveryError>>,
}

impl Default for RecordingDelivery {
    fn default() -> Self {
        Self {
            deliveries: Mutex::new(Vec::new()),
            result: Mutex::new(Ok(())),
        }
    }
}

impl RecordingDelivery {
    pub(crate) fn deliveries(&self) -> Vec<(DeviceId, RestrictedMembershipDelivery)> {
        self.deliveries.lock().unwrap().clone()
    }

    pub(crate) fn respond_with(&self, result: Result<(), RestrictedMembershipDeliveryError>) {
        *self.result.lock().unwrap() = result;
    }
}

#[async_trait]
impl RestrictedMembershipDeliveryPort for RecordingDelivery {
    async fn deliver_restricted_membership(
        &self,
        peer: &DeviceId,
        delivery: &RestrictedMembershipDelivery,
    ) -> Result<(), RestrictedMembershipDeliveryError> {
        self.deliveries
            .lock()
            .unwrap()
            .push((*peer, delivery.clone()));
        *self.result.lock().unwrap()
    }
}

/// 所有对端都离线的历史交换。
pub(crate) struct OfflineHistoryExchange;

#[async_trait]
impl MembershipHistoryExchangePort for OfflineHistoryExchange {
    async fn exchange_membership_history(
        &self,
        _peer: &DeviceId,
        _message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        Err(MembershipHistoryExchangeError::Offline)
    }
}

pub(crate) struct NoopAddressRefresh;

#[async_trait]
impl RefreshVerifiedPeerAddressPort for NoopAddressRefresh {
    async fn refresh_verified_peer_address(&self, _peer: &DeviceId) {}
}

pub(crate) struct CompletedStep;

#[async_trait]
impl RecoverMembershipConflictsPort for CompletedStep {
    async fn recover_membership_conflicts(&self) -> MembershipMaintenanceStepOutcome {
        MembershipMaintenanceStepOutcome::Completed
    }
}

#[async_trait]
impl DeliverPendingGroupUpdatesPort for CompletedStep {
    async fn deliver_pending_group_updates(
        &self,
        _trigger: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceStepOutcome {
        MembershipMaintenanceStepOutcome::Completed
    }
}

/// 执行器及其可观察协作者。
pub(crate) struct WorkerFixture {
    pub(crate) worker: Arc<MembershipWorker>,
    pub(crate) effects: Arc<RecordingEffects>,
    pub(crate) delivery: Arc<RecordingDelivery>,
}

pub(crate) struct WorkerPorts {
    pub(crate) history: Arc<dyn MembershipHistoryExchangePort>,
    pub(crate) conflicts: Arc<dyn RecoverMembershipConflictsPort>,
    pub(crate) group_updates: Arc<dyn DeliverPendingGroupUpdatesPort>,
}

impl Default for WorkerPorts {
    fn default() -> Self {
        Self {
            history: Arc::new(OfflineHistoryExchange),
            conflicts: Arc::new(CompletedStep),
            group_updates: Arc::new(CompletedStep),
        }
    }
}

impl WorkerFixture {
    pub(crate) fn new(owner: Arc<MembershipOwner>, ports: WorkerPorts) -> Self {
        let effects = Arc::new(RecordingEffects::default());
        let delivery = Arc::new(RecordingDelivery::default());
        let worker = Arc::new(MembershipWorker::new(
            owner,
            MembershipWorkerDeps {
                member_facts: effects.clone(),
                security: effects.clone(),
                activation: effects.clone(),
                restricted_delivery: delivery.clone(),
                history_transport: ports.history,
                address_refresh: Arc::new(NoopAddressRefresh),
                conflicts: ports.conflicts,
                group_updates: ports.group_updates,
            },
        ));
        Self {
            worker,
            effects,
            delivery,
        }
    }
}
