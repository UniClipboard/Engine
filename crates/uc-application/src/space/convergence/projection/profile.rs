use super::super::*;
use crate::space::convergence::admission::complete_ack_frame;

impl ProfileWorkspaceConvergence {
    pub fn new(
        admission_attempts: Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort>,
        own_device: DeviceId,
        clock: Arc<dyn ClockPort>,
    ) -> Arc<Self> {
        let (events, _) = broadcast::channel(64);
        Arc::new(Self {
            admission: admission::DurableAdmissionProjection::new(Arc::clone(&admission_attempts)),
            admission_attempts,
            own_device,
            clock,
            active: tokio::sync::RwLock::new(None),
            active_event_task: tokio::sync::Mutex::new(None),
            events,
        })
    }

    pub async fn attach_active(self: &Arc<Self>, active: Option<Arc<WorkspaceConvergence>>) {
        *self.active.write().await = active.clone();
        if let Some(task) = self.active_event_task.lock().await.take() {
            task.abort();
        }
        if let Some(active) = active {
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

    pub fn subscribe(&self) -> broadcast::Receiver<u64> {
        self.events.subscribe()
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

    pub async fn prepare_reset_space(&self) -> Result<(), WorkspaceConvergenceError> {
        let metadata = self.admission.reset_join_projection_if_quiet().await?;
        let _ = self.events.send(metadata.device_trust_revision);
        Ok(())
    }

    pub async fn query_device_trust(
        &self,
    ) -> Result<DeviceTrustSnapshot, WorkspaceConvergenceError> {
        if let Some(active) = self.active.read().await.clone() {
            return match active.query_device_trust().await {
                Ok(snapshot) => Ok(snapshot),
                Err(WorkspaceConvergenceError::Locked)
                | Err(WorkspaceConvergenceError::Repository(
                    WorkspaceConvergenceRepositoryError::Locked,
                )) => Ok(self.unavailable_device_trust_snapshot()),
                Err(error) => Err(error),
            };
        }
        let metadata = self
            .admission_attempts
            .profile_metadata()
            .await
            .map_err(admission::map_repository_error)?;
        Ok(DeviceTrustSnapshot {
            revision: metadata.device_trust_revision,
            local_device_id: self.own_device.clone(),
            local_membership: DeviceMembership::Unavailable,
            current_change: None,
            current_join: self.admission.current_local_join().await?,
            pending_inbound_member: None,
            devices: Vec::new(),
            recovery: RecoveryAvailability::NotAvailableInThisVersion,
            allowed_actions: Vec::new(),
            blocked_reason: None,
            updated_at_ms: self.clock.now_ms(),
        })
    }

    fn unavailable_device_trust_snapshot(&self) -> DeviceTrustSnapshot {
        DeviceTrustSnapshot {
            revision: 0,
            local_device_id: self.own_device.clone(),
            local_membership: DeviceMembership::Unavailable,
            current_change: None,
            current_join: None,
            pending_inbound_member: None,
            devices: Vec::new(),
            recovery: RecoveryAvailability::NotAvailableInThisVersion,
            allowed_actions: Vec::new(),
            blocked_reason: Some(ActionUnavailableReason::EngineUnavailable),
            updated_at_ms: self.clock.now_ms(),
        }
    }
}
