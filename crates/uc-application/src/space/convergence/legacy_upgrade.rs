use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{debug, debug_span, info, instrument, warn, Instrument};
use uc_core::membership::{
    decide_legacy_upgrade, CurrentWorkspacePeerScopePort, CurrentWorkspacePeerScopeSource,
    LegacyProtectionCommand, LegacyProtectionPort, LegacyProtectionResult, LegacyRequestInspection,
    LegacyUpgradeAction, LegacyUpgradeDescriptor, LegacyUpgradeDispatchError,
    LegacyUpgradeDispatchPort, LegacyUpgradeEndpointPort, LegacyUpgradeError, LegacyUpgradeRequest,
    LegacyUpgradeResponse, LegacyUpgradeResponseKind, MemberRepositoryPort,
};
use uc_core::ports::{DeviceIdentityPort, PresenceEvent, ReachabilityState};

use super::WorkspaceConvergence;

const DISCOVERY_GRACE: Duration = Duration::from_secs(15);
const DISCOVERY_RETRY_DELAY: Duration = Duration::from_secs(5);
const STEADY_RETRY_DELAY: Duration = Duration::from_secs(30);

pub struct AutomaticLegacyUpgradeDeps {
    pub member_repo: Arc<dyn MemberRepositoryPort>,
    pub device_identity: Arc<dyn DeviceIdentityPort>,
    pub protection: Arc<dyn LegacyProtectionPort>,
    pub dispatch: Arc<dyn LegacyUpgradeDispatchPort>,
}

pub struct AutomaticLegacyUpgrade {
    deps: AutomaticLegacyUpgradeDeps,
    convergence: Option<Arc<WorkspaceConvergence>>,
    peer_scope: Option<Arc<dyn CurrentWorkspacePeerScopePort>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyUpgradePassState {
    Waiting,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LegacyUpgradePassOutcome {
    state: LegacyUpgradePassState,
    changed: bool,
}

impl LegacyUpgradePassOutcome {
    const fn waiting() -> Self {
        Self {
            state: LegacyUpgradePassState::Waiting,
            changed: false,
        }
    }

    const fn ready(changed: bool) -> Self {
        Self {
            state: LegacyUpgradePassState::Ready,
            changed,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LegacyDiscoveryPhase {
    Discovering,
    Complete,
}

impl AutomaticLegacyUpgrade {
    pub fn new(deps: AutomaticLegacyUpgradeDeps) -> Self {
        Self {
            deps,
            convergence: None,
            peer_scope: None,
        }
    }

    pub fn with_convergence(mut self, convergence: Arc<WorkspaceConvergence>) -> Self {
        self.peer_scope = Some(Arc::clone(&convergence) as Arc<dyn CurrentWorkspacePeerScopePort>);
        self.convergence = Some(convergence);
        self
    }

    #[cfg(test)]
    fn with_peer_scope(mut self, peer_scope: Arc<dyn CurrentWorkspacePeerScopePort>) -> Self {
        self.peer_scope = Some(peer_scope);
        self
    }

    async fn initialize_current_history(&self) -> Result<(), LegacyUpgradeError> {
        let Some(convergence) = &self.convergence else {
            return Ok(());
        };
        convergence
            .initialize_upgraded_legacy_space()
            .await
            .map(|_| ())
            .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))
    }

    #[instrument(name = "legacy_upgrade.run_once", level = "info", skip_all)]
    async fn reconcile_once(
        &self,
        discovery_phase: LegacyDiscoveryPhase,
    ) -> Result<LegacyUpgradePassOutcome, LegacyUpgradeError> {
        let local_device_id = self.deps.device_identity.current_device_id();
        let scope = self
            .peer_scope
            .as_ref()
            .ok_or_else(|| LegacyUpgradeError::Internal("current peer scope is absent".into()))?
            .snapshot()
            .await
            .map_err(|_| {
                LegacyUpgradeError::Internal("current peer scope is unavailable".into())
            })?;
        if scope.source != CurrentWorkspacePeerScopeSource::Legacy {
            return Ok(LegacyUpgradePassOutcome::ready(false));
        }
        let members = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
        let mut member_ids = scope.peer_device_ids.clone();
        member_ids.push(local_device_id);
        let mut successful_exchanges = 0usize;
        let mut should_create_local_group = false;

        for member in members
            .iter()
            .filter(|member| scope.peer_device_ids.contains(&member.device_id))
        {
            let request = self
                .deps
                .protection
                .begin_attempt(&local_device_id, &member.device_id)
                .await?;
            let local_descriptor = request.descriptor().clone();
            let response = match self
                .deps
                .dispatch
                .exchange_legacy_upgrade(&member.device_id, &request)
                .await
            {
                Ok(response) => response,
                Err(
                    LegacyUpgradeDispatchError::Offline | LegacyUpgradeDispatchError::Transport,
                ) => {
                    debug!(device_id = %member.device_id, "legacy upgrade peer is not reachable yet");
                    continue;
                }
                Err(LegacyUpgradeDispatchError::Rejected) => {
                    successful_exchanges += 1;
                    warn!(device_id = %member.device_id, "legacy upgrade peer rejected the request");
                    continue;
                }
            };
            successful_exchanges += 1;
            let action = decide_legacy_upgrade(
                &local_device_id,
                &local_descriptor,
                &member.device_id,
                &response.descriptor,
            );
            match response.kind {
                LegacyUpgradeResponseKind::Admission(admission)
                    if action == LegacyUpgradeAction::JoinRemote
                        && response.descriptor.protection_group_id()
                            == Some(&admission.protection_group_id) =>
                {
                    self.deps
                        .protection
                        .execute(LegacyProtectionCommand::JoinGroup {
                            peer: member.device_id,
                            admission,
                        })
                        .await?;
                    self.initialize_current_history().await?;
                    info!(device_id = %member.device_id, "legacy upgrade joined a peer protection group");
                    return Ok(LegacyUpgradePassOutcome::ready(true));
                }
                LegacyUpgradeResponseKind::Admission(_) => {
                    warn!(device_id = %member.device_id, "legacy upgrade admission did not match the selected group");
                }
                LegacyUpgradeResponseKind::Retry
                    if action == LegacyUpgradeAction::CreateLocalGroup =>
                {
                    should_create_local_group = true;
                }
                LegacyUpgradeResponseKind::UpToDate if action == LegacyUpgradeAction::NoAction => {
                    debug!(device_id = %member.device_id, "legacy upgrade peer is already current");
                }
                LegacyUpgradeResponseKind::Rejected
                | LegacyUpgradeResponseKind::Retry
                | LegacyUpgradeResponseKind::UpToDate => {}
            }
        }

        let descriptor = self.deps.protection.snapshot(&member_ids).await?.descriptor;
        if descriptor.is_ready() {
            self.initialize_current_history().await?;
            debug!("legacy upgrade protection group is ready");
            return Ok(LegacyUpgradePassOutcome::ready(false));
        }
        if should_create_local_group
            || (discovery_phase == LegacyDiscoveryPhase::Complete && successful_exchanges == 0)
        {
            self.bootstrap_local_group().await?;
            self.initialize_current_history().await?;
            return Ok(LegacyUpgradePassOutcome::ready(true));
        }
        debug!("legacy upgrade is waiting for a selected peer");
        Ok(LegacyUpgradePassOutcome::waiting())
    }

    pub fn start(
        self: Arc<Self>,
        mut presence_events: broadcast::Receiver<PresenceEvent>,
    ) -> AutomaticLegacyUpgradeRuntime {
        let span = debug_span!("legacy_upgrade.background");
        let task = tokio::spawn(
            async move {
                let started_at = Instant::now();
                let mut presence_open = true;
                let mut first_pass = true;
                let mut next_timer_retry_at = started_at + DISCOVERY_RETRY_DELAY;
                loop {
                    if first_pass {
                        first_pass = false;
                    } else {
                        let timer = tokio::time::sleep_until(next_timer_retry_at);
                        tokio::pin!(timer);
                        tokio::select! {
                            _ = &mut timer => {}
                            event = presence_events.recv(), if presence_open => match event {
                                Ok(event) if event.state == ReachabilityState::Online => {}
                                Ok(_) => continue,
                                Err(broadcast::error::RecvError::Lagged(_)) => {}
                                Err(broadcast::error::RecvError::Closed) => {
                                    presence_open = false;
                                    continue;
                                }
                            },
                        };
                    }
                    let discovery_phase = if started_at.elapsed() >= DISCOVERY_GRACE {
                        LegacyDiscoveryPhase::Complete
                    } else {
                        LegacyDiscoveryPhase::Discovering
                    };
                    match self.reconcile_once(discovery_phase).await {
                        Ok(outcome) => {
                            debug!(?outcome, "automatic legacy security upgrade pass completed");
                        }
                        Err(error) => {
                            warn!(error_kind = "legacy_upgrade_pass", error = %error, retryable = true, "automatic legacy security upgrade pass failed");
                        }
                    }
                    let delay = if started_at.elapsed() < DISCOVERY_GRACE {
                        DISCOVERY_RETRY_DELAY
                    } else {
                        STEADY_RETRY_DELAY
                    };
                    next_timer_retry_at = Instant::now() + delay;
                }
            }
            .instrument(span),
        );
        AutomaticLegacyUpgradeRuntime { task: Some(task) }
    }

    async fn admit_remote(
        &self,
        local_descriptor: LegacyUpgradeDescriptor,
        request: &LegacyUpgradeRequest,
    ) -> Result<LegacyUpgradeResponse, LegacyUpgradeError> {
        let local_device_id = self.deps.device_identity.current_device_id();
        let scope = self
            .peer_scope
            .as_ref()
            .ok_or(LegacyUpgradeError::Unauthorized)?
            .snapshot()
            .await
            .map_err(|_| LegacyUpgradeError::Unauthorized)?;
        if scope.source != CurrentWorkspacePeerScopeSource::Legacy
            || !scope.peer_device_ids.contains(request.source_device_id())
        {
            return Err(LegacyUpgradeError::Unauthorized);
        }
        let mut member_ids = scope.peer_device_ids;
        member_ids.push(local_device_id);
        let protection = self.deps.protection.snapshot(&member_ids).await?;
        if protection
            .protected_members
            .contains(request.source_device_id())
        {
            debug!(
                device_id = %request.source_device_id(),
                "legacy upgrade request is already satisfied"
            );
            return Ok(LegacyUpgradeResponse {
                descriptor: local_descriptor,
                kind: LegacyUpgradeResponseKind::UpToDate,
            });
        }
        let existing_member_ids = protection
            .protected_members
            .into_iter()
            .filter(|device_id| {
                device_id != &local_device_id && device_id != request.source_device_id()
            })
            .collect::<Vec<_>>();
        let admission_result = self
            .deps
            .protection
            .execute(LegacyProtectionCommand::AdmitMember {
                sponsor: local_device_id,
                existing_members: existing_member_ids,
                request: request.clone(),
            })
            .await;
        let admission = match admission_result {
            Err(error) => {
                warn!(
                    error_kind = "admit_member",
                    retryable = true,
                    error = %error,
                    "legacy upgrade admission failed"
                );
                return Err(error);
            }
            Ok(result) => match result {
                LegacyProtectionResult::MemberAdmitted(admission) => admission,
                LegacyProtectionResult::GroupReady(_) => {
                    return Err(LegacyUpgradeError::Internal(
                        "legacy protection returned an unexpected result".into(),
                    ));
                }
            },
        };
        let protection_group_id =
            local_descriptor
                .protection_group_id()
                .cloned()
                .ok_or_else(|| {
                    LegacyUpgradeError::Internal("local protection group is absent".into())
                })?;
        if admission.protection_group_id != protection_group_id {
            return Err(LegacyUpgradeError::InvalidRequest);
        }
        info!(
            device_id = %request.source_device_id(),
            group_epoch = admission.admission.group_epoch,
            "legacy device admitted to the protection group"
        );
        Ok(LegacyUpgradeResponse {
            descriptor: local_descriptor,
            kind: LegacyUpgradeResponseKind::Admission(admission),
        })
    }

    async fn bootstrap_local_group(&self) -> Result<LegacyUpgradeDescriptor, LegacyUpgradeError> {
        let local_device_id = self.deps.device_identity.current_device_id();
        let retained_members = self
            .peer_scope
            .as_ref()
            .ok_or_else(|| LegacyUpgradeError::Internal("current peer scope is absent".into()))?
            .snapshot()
            .await
            .map_err(|_| LegacyUpgradeError::Internal("current peer scope is unavailable".into()))?
            .peer_device_ids;
        let descriptor = match self
            .deps
            .protection
            .execute(LegacyProtectionCommand::CreateGroup {
                sponsor: local_device_id,
                retained_members: retained_members.clone(),
            })
            .await?
        {
            LegacyProtectionResult::GroupReady(descriptor) => descriptor,
            LegacyProtectionResult::MemberAdmitted(_) => {
                return Err(LegacyUpgradeError::Internal(
                    "legacy protection returned an unexpected result".into(),
                ));
            }
        };
        info!(
            pending_member_count = retained_members.len(),
            "legacy space protection group created automatically"
        );
        Ok(descriptor)
    }
}

pub struct AutomaticLegacyUpgradeRuntime {
    task: Option<JoinHandle<()>>,
}

impl AutomaticLegacyUpgradeRuntime {
    pub async fn shutdown(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            if let Err(error) = task.await {
                if !error.is_cancelled() {
                    warn!(
                        event = "task.panicked",
                        task = "automatic_legacy_upgrade.runtime",
                        error = %error,
                        "automatic legacy upgrade runtime panicked"
                    );
                }
            }
        }
    }
}

impl Drop for AutomaticLegacyUpgradeRuntime {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

#[async_trait]
impl LegacyUpgradeEndpointPort for AutomaticLegacyUpgrade {
    #[instrument(
        name = "legacy_upgrade.handle_request",
        level = "info",
        skip_all,
        fields(device_id = %authenticated_peer)
    )]
    async fn handle_legacy_upgrade_request(
        &self,
        authenticated_peer: &uc_core::ids::DeviceId,
        request: LegacyUpgradeRequest,
    ) -> Result<LegacyUpgradeResponse, LegacyUpgradeError> {
        let local_device_id = self.deps.device_identity.current_device_id();
        if authenticated_peer != request.source_device_id()
            || request.target_device_id() != &local_device_id
        {
            warn!(
                reason = "identity_binding",
                "legacy upgrade request rejected"
            );
            return Err(LegacyUpgradeError::Unauthorized);
        }
        if self
            .deps
            .member_repo
            .get(authenticated_peer)
            .await
            .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?
            .is_none()
        {
            warn!(reason = "unknown_member", "legacy upgrade request rejected");
            return Err(LegacyUpgradeError::Unauthorized);
        }
        let inspection = self.deps.protection.inspect_request(&request).await?;
        if inspection == LegacyRequestInspection::Invalid {
            warn!(reason = "proof", "legacy upgrade request rejected");
            return Err(LegacyUpgradeError::Unauthorized);
        }

        let mut local_descriptor = self.deps.protection.snapshot(&[]).await?.descriptor;
        if let LegacyRequestInspection::Replay(admission) = inspection {
            if local_descriptor.protection_group_id() == Some(&admission.protection_group_id) {
                debug!(device_id = %authenticated_peer, "legacy upgrade admission response replayed");
                return Ok(LegacyUpgradeResponse {
                    descriptor: local_descriptor,
                    kind: LegacyUpgradeResponseKind::Admission(admission),
                });
            }
        }
        let mut action = decide_legacy_upgrade(
            &local_device_id,
            &local_descriptor,
            authenticated_peer,
            request.descriptor(),
        );
        if action == LegacyUpgradeAction::CreateLocalGroup {
            local_descriptor = self.bootstrap_local_group().await?;
            action = decide_legacy_upgrade(
                &local_device_id,
                &local_descriptor,
                authenticated_peer,
                request.descriptor(),
            );
        }
        match action {
            LegacyUpgradeAction::AdmitRemote => self.admit_remote(local_descriptor, &request).await,
            LegacyUpgradeAction::NoAction => {
                debug!("legacy upgrade peers already share a protection group");
                Ok(LegacyUpgradeResponse {
                    descriptor: local_descriptor,
                    kind: LegacyUpgradeResponseKind::UpToDate,
                })
            }
            LegacyUpgradeAction::AwaitRemote | LegacyUpgradeAction::JoinRemote => {
                debug!("legacy upgrade will continue from the peer side");
                Ok(LegacyUpgradeResponse {
                    descriptor: local_descriptor,
                    kind: LegacyUpgradeResponseKind::Retry,
                })
            }
            LegacyUpgradeAction::Reject => {
                warn!(reason = "space_mismatch", "legacy upgrade request rejected");
                Ok(LegacyUpgradeResponse {
                    descriptor: local_descriptor,
                    kind: LegacyUpgradeResponseKind::Rejected,
                })
            }
            LegacyUpgradeAction::CreateLocalGroup => Err(LegacyUpgradeError::Internal(
                "legacy upgrade did not advance after bootstrap".into(),
            )),
        }
    }
}

#[cfg(test)]
#[path = "legacy_upgrade_tests.rs"]
mod tests;
