use std::sync::Arc;

use tokio::sync::broadcast;
use uc_core::ports::ClockPort;
use uc_core::DeviceId;

use super::durable::{self as admission, complete_ack_frame};
use super::{CurrentJoinStatus, PendingJoinerCompleteAck, ProfileSpaceAdmission};
use crate::space::query_space_membership_status::{
    QuerySpaceMembershipStatusDeps, QuerySpaceMembershipStatusError,
    QuerySpaceMembershipStatusUseCase, SpaceMembershipStatus,
};
use crate::space::workspace_membership::WorkspaceConvergenceError;

impl ProfileSpaceAdmission {
    pub fn new(
        admission_attempts: Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort>,
        own_device: DeviceId,
        clock: Arc<dyn ClockPort>,
    ) -> Arc<Self> {
        let (events, _) = broadcast::channel(64);
        let query_space_membership_status =
            QuerySpaceMembershipStatusUseCase::new(QuerySpaceMembershipStatusDeps {
                admission_attempts: Arc::clone(&admission_attempts),
                own_device,
                clock,
            });
        Arc::new(Self {
            admission: admission::DurableAdmissionProjection::new(Arc::clone(&admission_attempts)),
            admission_attempts,
            query_space_membership_status,
            active_event_task: tokio::sync::Mutex::new(None),
            events,
        })
    }

    pub async fn attach_active(
        self: &Arc<Self>,
        active: Option<Arc<crate::space::assembly::SpaceModules>>,
    ) {
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
                    historical_signatures: Arc::clone(
                        &active.deps.historical_membership_signatures,
                    ),
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

    pub async fn current_join(
        &self,
    ) -> Result<Option<CurrentJoinStatus>, WorkspaceConvergenceError> {
        self.admission.current_local_join().await
    }

    pub async fn pending_joiner_complete_ack(
        &self,
    ) -> Result<Option<PendingJoinerCompleteAck>, WorkspaceConvergenceError> {
        use sha2::Digest as _;
        use uc_core::membership::{AdmissionIdentityBindingV1, AdmissionTerminalResultV1};

        let Some(projection) = self
            .admission_attempts
            .project_current_local_join()
            .await
            .map_err(admission::map_repository_error)?
        else {
            return Ok(None);
        };
        if projection.terminal_result != Some(AdmissionTerminalResultV1::Active) {
            return Ok(None);
        }
        let terminal = self
            .admission_attempts
            .load_terminal(projection.attempt_id)
            .await
            .map_err(admission::map_repository_error)?
            .ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "active local join terminal is missing".to_owned(),
                )
            })?;
        let binding = AdmissionIdentityBindingV1::decode(
            terminal.identity_binding.as_deref().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "active local join identity is missing".to_owned(),
                )
            })?,
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let completion_digest: [u8; 32] = sha2::Sha256::digest(&terminal.replay_result).into();
        let acknowledgment = terminal
            .acknowledgment_rebuild
            .iter()
            .find(|record| record.payload_digest == completion_digest)
            .ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "active local join completion acknowledgment is missing".to_owned(),
                )
            })?;
        let payload = postcard::to_stdvec(acknowledgment)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        Ok(Some(PendingJoinerCompleteAck {
            sponsor_device_id: binding.sponsor_device_id,
            frame: complete_ack_frame(projection.attempt_id, acknowledgment.message_id, payload),
        }))
    }

    pub async fn cancel_join_space(
        &self,
        join_id: [u8; 16],
    ) -> Result<CurrentJoinStatus, WorkspaceConvergenceError> {
        let result = self.admission.cancel_local_join(join_id).await?;
        let revision = self
            .admission_attempts
            .profile_metadata()
            .await
            .map_err(admission::map_repository_error)?
            .device_trust_revision;
        let _ = self.events.send(revision);
        Ok(result)
    }
}
