use std::sync::Arc;

use uc_core::membership::{
    MembershipHistoryExchangeEndpointPort, MembershipHistoryExchangeError,
    MembershipHistoryExchangePort, MembershipHistoryMessage,
};
use uc_core::ports::ClockPort;

use super::{
    CurrentSpaceMemberScopePort, HandleMembershipHistoryMessageUseCase, MembershipLedger,
    MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger,
    SynchronizeMembershipHistoryUseCase, SynchronizeMembershipMaintenancePort,
    WakeSpaceMembershipMaintenancePort,
};

/// 成员历史反熵的唯一应用层负责人；调用方不需要拼装收发、ACK 与重试步骤。
pub(crate) struct MembershipHistoryAntiEntropy {
    inbound: HandleMembershipHistoryMessageUseCase,
    outbound: SynchronizeMembershipHistoryUseCase,
}

impl MembershipHistoryAntiEntropy {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        current_scope: Arc<dyn CurrentSpaceMemberScopePort>,
        transport: Arc<dyn MembershipHistoryExchangePort>,
        clock: Arc<dyn ClockPort>,
        maintenance_wake: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    ) -> Self {
        Self {
            inbound: HandleMembershipHistoryMessageUseCase::new_with_wake(
                Arc::clone(&ledger),
                maintenance_wake,
            ),
            outbound: SynchronizeMembershipHistoryUseCase::new(
                ledger,
                current_scope,
                transport,
                clock,
            ),
        }
    }
}

#[async_trait::async_trait]
impl MembershipHistoryExchangeEndpointPort for MembershipHistoryAntiEntropy {
    async fn handle_membership_history_exchange(
        &self,
        source_device_id: &uc_core::ids::DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        self.inbound
            .handle_membership_history_exchange(source_device_id, message)
            .await
    }
}

#[async_trait::async_trait]
impl SynchronizeMembershipMaintenancePort for MembershipHistoryAntiEntropy {
    async fn periodic_synchronization_required(
        &self,
    ) -> Result<bool, MembershipMaintenanceStepOutcome> {
        self.outbound.periodic_synchronization_required().await
    }

    async fn synchronize_membership(
        &self,
        trigger: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceStepOutcome {
        self.outbound.synchronize_membership(trigger).await
    }
}
