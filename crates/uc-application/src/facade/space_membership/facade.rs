use std::sync::Arc;
use tokio::sync::broadcast;
use uc_core::ports::ClockPort;
use uc_core::DeviceId;

use crate::space::decide_pending_membership_removal::{
    DecidePendingMembershipRemovalDeps, DecidePendingMembershipRemovalError,
    DecidePendingMembershipRemovalResult, DecidePendingMembershipRemovalUseCase,
};
use crate::space::initiate_space_member_removal::{
    InitiateSpaceMemberRemovalDeps, InitiateSpaceMemberRemovalError,
    InitiateSpaceMemberRemovalResult, InitiateSpaceMemberRemovalUseCase,
};
use crate::space::membership_history::MembershipHistoryStore;
use crate::space::query_space_membership_status::{
    QuerySpaceMembershipStatusDeps, QuerySpaceMembershipStatusError,
    QuerySpaceMembershipStatusUseCase, SpaceMembershipStatus,
};

pub struct SpaceMembershipFacade {
    admission_attempts: Arc<dyn crate::deps::AdmissionAttemptRepositoryPort>,
    query_space_membership_status: Arc<QuerySpaceMembershipStatusUseCase>,
    decide_pending_membership_removal:
        tokio::sync::RwLock<Option<Arc<DecidePendingMembershipRemovalUseCase>>>,
    initiate_space_member_removal:
        tokio::sync::RwLock<Option<Arc<InitiateSpaceMemberRemovalUseCase>>>,
    own_device: DeviceId,
    clock: Arc<dyn ClockPort>,
    active_event_task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    events: tokio::sync::broadcast::Sender<u64>,
}

impl SpaceMembershipFacade {
    pub fn new(
        admission_attempts: Arc<dyn crate::deps::AdmissionAttemptRepositoryPort>,
        own_device: DeviceId,
        clock: Arc<dyn ClockPort>,
    ) -> Arc<Self> {
        let (events, _) = broadcast::channel(64);
        let query_space_membership_status = Arc::new(QuerySpaceMembershipStatusUseCase::new(
            QuerySpaceMembershipStatusDeps {
                admission_attempts: Arc::clone(&admission_attempts),
                own_device: own_device.clone(),
                clock: Arc::clone(&clock),
            },
        ));
        Arc::new(Self {
            admission_attempts,
            query_space_membership_status,
            decide_pending_membership_removal: tokio::sync::RwLock::new(None),
            initiate_space_member_removal: tokio::sync::RwLock::new(None),
            own_device,
            clock,
            active_event_task: tokio::sync::Mutex::new(None),
            events,
        })
    }

    pub async fn attach_active(
        self: &Arc<Self>,
        active: Option<Arc<crate::space::assembly::SpaceModules>>,
    ) {
        let decide_pending_membership_removal = active.as_ref().map(|active| {
            let membership = active.membership_status_deps();
            Arc::new(DecidePendingMembershipRemovalUseCase::new(
                DecidePendingMembershipRemovalDeps {
                    membership_history: membership.membership_history,
                    state_repository: membership.state_repository,
                    member_signatures: membership.member_signatures,
                    own_device: self.own_device.clone(),
                    clock: Arc::clone(&self.clock),
                    state_write_lock: active.membership_state_write_lock(),
                    state_events: active.membership_state_events(),
                    recovery_requests: active.membership_recovery_requests(),
                    membership_status_query: Arc::clone(&self.query_space_membership_status),
                },
            ))
        });
        let initiate_space_member_removal = active.as_ref().map(|active| {
            let membership = active.membership_status_deps();
            Arc::new(InitiateSpaceMemberRemovalUseCase::new(
                InitiateSpaceMemberRemovalDeps {
                    membership_history: membership.membership_history,
                    state_repo: membership.state_repository,
                    member_signatures: membership.member_signatures,
                    own_device: self.own_device.clone(),
                    state_write_lock: active.membership_state_write_lock(),
                    state_events: active.membership_state_events(),
                    recovery_requests: active.membership_recovery_requests(),
                },
            ))
        });
        *self.decide_pending_membership_removal.write().await = decide_pending_membership_removal;
        *self.initiate_space_member_removal.write().await = initiate_space_member_removal;
        self.query_space_membership_status
            .replace_active_space(
                active
                    .as_ref()
                    .map(|active| active.membership_status_deps()),
            )
            .await;
        if let Some(task) = self.active_event_task.lock().await.take() {
            task.abort();
        }
        if let Some(active) = active {
            let active = active.workspace_membership();
            let mut changes = active.subscribe();
            let events = self.events.clone();
            let admission_attempts = Arc::clone(&self.admission_attempts);
            *self.active_event_task.lock().await = Some(tokio::spawn(async move {
                while let Ok(snapshot) = changes.recv().await {
                    let revision = admission_attempts
                        .profile_metadata()
                        .await
                        .map(|metadata| metadata.device_trust_revision.max(snapshot.revision))
                        .unwrap_or(snapshot.revision);
                    let _ = events.send(revision);
                }
            }));
        }
    }

    #[cfg(test)]
    pub(crate) async fn attach_workspace_membership_for_test(
        self: &Arc<Self>,
        active: Arc<crate::space::workspace_membership::WorkspaceMembership>,
    ) {
        self.query_space_membership_status
            .replace_active_space(Some(
                crate::space::query_space_membership_status::ActiveSpaceMembershipStatusDeps {
                    state_repository: Arc::clone(&active.deps.repository),
                    membership_history: Arc::new(MembershipHistoryStore::new(
                        Arc::clone(&active.deps.membership_history_repo),
                        Arc::clone(&active.deps.historical_membership_signatures),
                    )),
                    member_signatures: Arc::clone(&active.deps.member_signatures),
                    member_repo: Arc::clone(&active.deps.member_repo),
                    presence: Arc::clone(&active.deps.presence),
                },
            ))
            .await;
    }

    pub fn subscribe(&self) -> broadcast::Receiver<u64> {
        self.events.subscribe()
    }

    pub async fn query_space_membership_status(
        &self,
    ) -> Result<SpaceMembershipStatus, QuerySpaceMembershipStatusError> {
        self.query_space_membership_status.execute().await
    }

    pub async fn decide_pending_membership_removal(
        &self,
        removal_event_id: uc_core::membership::MembershipEventId,
        decision: uc_core::membership::RemovalDecision,
        confirm_self_removal: bool,
    ) -> Result<DecidePendingMembershipRemovalResult, DecidePendingMembershipRemovalError> {
        let use_case = self
            .decide_pending_membership_removal
            .read()
            .await
            .clone()
            .ok_or(DecidePendingMembershipRemovalError::Unavailable)?;
        use_case
            .execute(removal_event_id, decision, confirm_self_removal)
            .await
    }

    pub async fn initiate_space_member_removal(
        &self,
        target_device: &uc_core::DeviceId,
    ) -> Result<InitiateSpaceMemberRemovalResult, InitiateSpaceMemberRemovalError> {
        let use_case = self
            .initiate_space_member_removal
            .read()
            .await
            .clone()
            .ok_or(InitiateSpaceMemberRemovalError::Unavailable)?;
        use_case.execute(target_device).await
    }
}
