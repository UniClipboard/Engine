use super::*;
use tracing::debug;

impl MembershipConvergence {
    pub(crate) fn subscribe_shared_device_refresh(
        &self,
    ) -> broadcast::Receiver<SharedDeviceRefreshStatus> {
        self.shared_device_refresh_events.subscribe()
    }

    pub(crate) async fn shared_device_refresh_status(
        &self,
        request_id: &str,
    ) -> Option<SharedDeviceRefreshStatus> {
        self.shared_device_refresh
            .lock()
            .await
            .as_ref()
            .filter(|refresh| refresh.status.request_id == request_id)
            .map(|refresh| refresh.status.clone())
    }

    pub(crate) async fn start_shared_device_refresh(
        self: &Arc<Self>,
    ) -> Result<SharedDeviceRefreshStarted, MembershipConvergenceError> {
        let state = self.deps.security_updates.current_state().await?;
        let (started, status) = {
            let mut active = self.shared_device_refresh.lock().await;
            if let Some(existing) = active.as_ref().filter(|refresh| {
                refresh.space_id == state.space_id && refresh.initial_round_active
            }) {
                return Ok(SharedDeviceRefreshStarted {
                    request_id: existing.status.request_id.clone(),
                });
            }
            let request_id = Uuid::now_v7().to_string();
            let status = SharedDeviceRefreshStatus::new(request_id.clone());
            *active = Some(ActiveSharedDeviceRefresh {
                space_id: state.space_id.clone(),
                initial_round_active: true,
                status: status.clone(),
            });
            (
                SharedDeviceRefreshStarted {
                    request_id: request_id.clone(),
                },
                status,
            )
        };
        let _ = self.shared_device_refresh_events.send(status);
        self.wake.notify_one();
        Ok(started)
    }

    pub(super) async fn pending_shared_device_refresh(&self) -> Option<(String, SpaceId)> {
        self.shared_device_refresh
            .lock()
            .await
            .as_ref()
            .filter(|refresh| refresh.initial_round_active)
            .map(|refresh| (refresh.status.request_id.clone(), refresh.space_id.clone()))
    }

    /// Schedule one shared device lookup after a session recovery.
    ///
    /// Repeated scheduling within one recovery is merged: only the first
    /// call moves the coordinator from `Idle` to `Pending`. A completed
    /// lookup also suppresses re-scheduling for a short cooldown, so
    /// duplicate unlock or readiness notifications cannot start a second
    /// round. Locking resets the coordinator, so the next unlock always
    /// starts one fresh round.
    pub(super) async fn schedule_auto_shared_device_refresh(&self) {
        let state = match self.deps.security_updates.current_state().await {
            Ok(state) => state,
            Err(_) => return,
        };
        let now_ms = self.deps.clock.now_ms();
        let mut auto = self.auto_shared_device_refresh.lock().await;
        let already_tracked = match &auto.mode {
            AutoSharedDeviceRefreshMode::Pending { space_id }
            | AutoSharedDeviceRefreshMode::WaitingForSourceOnline {
                space_id,
                unavailable_sources: _,
            } => space_id == &state.space_id,
            AutoSharedDeviceRefreshMode::Idle => false,
        };
        if already_tracked {
            return;
        }
        if auto
            .last_completed_at_ms
            .map(|completed_at| {
                now_ms.saturating_sub(completed_at) < AUTO_SHARED_DEVICE_REFRESH_COOLDOWN_MS
            })
            .unwrap_or(false)
        {
            return;
        }
        auto.mode = AutoSharedDeviceRefreshMode::Pending {
            space_id: state.space_id,
        };
        debug!("auto shared device refresh scheduled after session recovery");
        self.wake.notify_one();
    }

    pub(super) async fn auto_shared_device_refresh_pending(&self) -> bool {
        matches!(
            self.auto_shared_device_refresh.lock().await.mode,
            AutoSharedDeviceRefreshMode::Pending { .. }
        )
    }

    pub(super) async fn promote_auto_shared_device_refresh_to_pending(
        &self,
        online_device: &uc_core::ids::DeviceId,
    ) {
        let mut auto = self.auto_shared_device_refresh.lock().await;
        if let AutoSharedDeviceRefreshMode::WaitingForSourceOnline {
            space_id,
            unavailable_sources,
        } = &auto.mode
        {
            let should_retry = unavailable_sources
                .as_ref()
                .map(|sources| sources.iter().any(|source| source == online_device))
                .unwrap_or(true);
            if should_retry {
                auto.mode = AutoSharedDeviceRefreshMode::Pending {
                    space_id: space_id.clone(),
                };
            }
        }
    }

    pub(super) async fn reset_auto_shared_device_refresh(&self) {
        let mut auto = self.auto_shared_device_refresh.lock().await;
        auto.mode = AutoSharedDeviceRefreshMode::Idle;
        auto.last_completed_at_ms = None;
    }

    async fn complete_auto_shared_device_refresh(
        &self,
        space_id: &SpaceId,
        unavailable_sources: Option<Vec<uc_core::ids::DeviceId>>,
    ) {
        let now_ms = self.deps.clock.now_ms();
        let mut auto = self.auto_shared_device_refresh.lock().await;
        let tracked = match &auto.mode {
            AutoSharedDeviceRefreshMode::Pending { space_id: current }
            | AutoSharedDeviceRefreshMode::WaitingForSourceOnline {
                space_id: current, ..
            } => current == space_id,
            AutoSharedDeviceRefreshMode::Idle => false,
        };
        if !tracked {
            return;
        }
        let sources_unavailable = match &unavailable_sources {
            None => true,
            Some(sources) => !sources.is_empty(),
        };
        match unavailable_sources {
            None => {
                auto.mode = AutoSharedDeviceRefreshMode::WaitingForSourceOnline {
                    space_id: space_id.clone(),
                    unavailable_sources: None,
                };
            }
            Some(sources) if sources.is_empty() => {
                auto.mode = AutoSharedDeviceRefreshMode::Idle;
                auto.last_completed_at_ms = Some(now_ms);
            }
            Some(sources) => {
                auto.mode = AutoSharedDeviceRefreshMode::WaitingForSourceOnline {
                    space_id: space_id.clone(),
                    unavailable_sources: Some(sources),
                };
            }
        }
        debug!(
            sources_unavailable,
            "auto shared device refresh round completed"
        );
    }

    async fn update_shared_device_refresh<F>(
        &self,
        request_id: &str,
        update: F,
    ) -> Option<SharedDeviceRefreshStatus>
    where
        F: FnOnce(&mut ActiveSharedDeviceRefresh) -> bool,
    {
        let status = {
            let mut active = self.shared_device_refresh.lock().await;
            let refresh = active
                .as_mut()
                .filter(|refresh| refresh.status.request_id == request_id)?;
            if !update(refresh) {
                return None;
            }
            refresh.status.recount();
            refresh.status.clone()
        };
        let _ = self.shared_device_refresh_events.send(status.clone());
        Some(status)
    }

    async fn set_shared_device_refresh_phase(
        &self,
        request_id: &str,
        phase: SharedDeviceRefreshPhase,
        initial_round_active: Option<bool>,
    ) {
        let _ = self
            .update_shared_device_refresh(request_id, |refresh| {
                let mut changed = false;
                if refresh.status.phase != phase {
                    refresh.status.phase = phase;
                    changed = true;
                }
                if let Some(active) = initial_round_active {
                    if refresh.initial_round_active != active {
                        refresh.initial_round_active = active;
                        changed = true;
                    }
                }
                changed
            })
            .await;
    }

    async fn set_shared_device_refresh_device(
        &self,
        request_id: &str,
        device_id: uc_core::ids::DeviceId,
        device_name: String,
        state: SharedDeviceRefreshDeviceState,
    ) {
        let _ = self
            .update_shared_device_refresh(request_id, |refresh| {
                if let Some(device) = refresh
                    .status
                    .devices
                    .iter_mut()
                    .find(|device| device.device_id == device_id)
                {
                    if device.state == SharedDeviceRefreshDeviceState::Connected
                        && state == SharedDeviceRefreshDeviceState::AlreadyPresent
                    {
                        return false;
                    }
                    if device.device_name == device_name && device.state == state {
                        return false;
                    }
                    device.device_name = device_name;
                    device.state = state;
                } else {
                    refresh.status.devices.push(SharedDeviceRefreshDevice {
                        device_id,
                        device_name,
                        state,
                    });
                    refresh.status.devices.sort_by(|left, right| {
                        left.device_id.as_str().cmp(right.device_id.as_str())
                    });
                }
                true
            })
            .await;
    }

    async fn record_shared_device_refresh_source_unavailable(&self, request_id: &str) {
        let _ = self
            .update_shared_device_refresh(request_id, |refresh| {
                refresh.status.unavailable_source_count =
                    refresh.status.unavailable_source_count.saturating_add(1);
                true
            })
            .await;
    }

    async fn shared_device_refresh_sources(
        &self,
    ) -> Result<Vec<uc_core::ids::DeviceId>, MembershipConvergenceError> {
        let local_device_id = self.deps.device_identity.current_device_id();
        let members = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?;
        let trusted = self
            .deps
            .trusted_peer_repo
            .list()
            .await
            .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?;
        let mut sources = trusted
            .into_iter()
            .filter(|peer| peer.local_device_id == local_device_id)
            .map(|peer| peer.peer_device_id)
            .filter(|device_id| {
                *device_id != local_device_id
                    && members.iter().any(|member| member.device_id == *device_id)
            })
            .collect::<Vec<_>>();
        sources.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        sources.dedup();
        Ok(sources)
    }

    pub(super) async fn run_shared_device_refresh(
        self: Arc<Self>,
        request_id: String,
        space_id: SpaceId,
    ) {
        self.set_shared_device_refresh_phase(
            &request_id,
            SharedDeviceRefreshPhase::Discovering,
            None,
        )
        .await;
        let sources = match self.shared_device_refresh_sources().await {
            Ok(sources) => sources,
            Err(_) => {
                self.record_shared_device_refresh_source_unavailable(&request_id)
                    .await;
                self.complete_auto_shared_device_refresh(&space_id, None)
                    .await;
                self.set_shared_device_refresh_phase(
                    &request_id,
                    SharedDeviceRefreshPhase::RoundCompleted,
                    Some(false),
                )
                .await;
                return;
            }
        };
        let mut unavailable_sources = Vec::new();
        let mut record_unavailable_source = |source_device_id: &uc_core::ids::DeviceId| {
            unavailable_sources.push(source_device_id.clone());
        };

        for source_device_id in sources {
            let mut after_device_id = None;
            loop {
                let response = self
                    .deps
                    .transport
                    .exchange(
                        &source_device_id,
                        MembershipGossipMessage::RequestSharedDevicePage(
                            MembershipSharedDevicePageRequest {
                                space_id: space_id.clone(),
                                after_device_id,
                            },
                        ),
                    )
                    .await;
                let page = match response {
                    Ok(MembershipGossipMessage::SharedDevicePage(page))
                        if page.space_id == space_id
                            && page.validate_transfer_bounds().is_ok()
                            && page
                                .seeds
                                .iter()
                                .all(|seed| seed.source_device_id == source_device_id) =>
                    {
                        page
                    }
                    _ => {
                        self.record_shared_device_refresh_source_unavailable(&request_id)
                            .await;
                        record_unavailable_source(&source_device_id);
                        break;
                    }
                };
                if page.seeds.iter().any(|seed| {
                    after_device_id
                        .as_ref()
                        .map(|after| seed.device_id.as_str() <= after.as_str())
                        .unwrap_or(false)
                }) {
                    self.record_shared_device_refresh_source_unavailable(&request_id)
                        .await;
                    record_unavailable_source(&source_device_id);
                    break;
                }
                for seed in page.seeds {
                    self.process_shared_device_refresh_seed(
                        &request_id,
                        &space_id,
                        &source_device_id,
                        seed,
                    )
                    .await;
                }
                match page.next_after_device_id {
                    Some(next)
                        if after_device_id
                            .as_ref()
                            .map(|after| next.as_str() > after.as_str())
                            .unwrap_or(true) =>
                    {
                        after_device_id = Some(next);
                    }
                    Some(_) => {
                        self.record_shared_device_refresh_source_unavailable(&request_id)
                            .await;
                        record_unavailable_source(&source_device_id);
                        break;
                    }
                    None => break,
                }
            }
        }
        self.complete_auto_shared_device_refresh(&space_id, Some(unavailable_sources))
            .await;
        self.set_shared_device_refresh_phase(
            &request_id,
            SharedDeviceRefreshPhase::RoundCompleted,
            Some(false),
        )
        .await;
    }

    async fn process_shared_device_refresh_seed(
        &self,
        request_id: &str,
        space_id: &SpaceId,
        source_device_id: &uc_core::ids::DeviceId,
        seed: SponsorCandidateSeed,
    ) {
        if seed.space_id != *space_id || seed.source_device_id != *source_device_id {
            self.set_shared_device_refresh_device(
                request_id,
                seed.device_id,
                seed.device_name_hint,
                SharedDeviceRefreshDeviceState::Rejected,
            )
            .await;
            return;
        }
        match self.deps.member_repo.get(&seed.device_id).await {
            Ok(Some(member)) if member.identity_fingerprint == seed.identity_fingerprint_hint => {
                self.set_shared_device_refresh_device(
                    request_id,
                    member.device_id,
                    member.device_name,
                    SharedDeviceRefreshDeviceState::AlreadyPresent,
                )
                .await;
                return;
            }
            Ok(Some(_)) | Err(_) => {
                self.set_shared_device_refresh_device(
                    request_id,
                    seed.device_id,
                    seed.device_name_hint,
                    SharedDeviceRefreshDeviceState::Rejected,
                )
                .await;
                return;
            }
            Ok(None) => {}
        }
        let device_id = seed.device_id;
        let device_name = seed.device_name_hint.clone();
        match self.accept_sponsor_seed(seed).await {
            Ok(
                CandidateMergeOutcome::IdentityConflict
                | CandidateMergeOutcome::AnnouncementConflict
                | CandidateMergeOutcome::SecurityHistoryConflict,
            )
            | Err(_) => {
                self.set_shared_device_refresh_device(
                    request_id,
                    device_id,
                    device_name,
                    SharedDeviceRefreshDeviceState::Rejected,
                )
                .await;
            }
            Ok(_) => {
                self.set_shared_device_refresh_device(
                    request_id,
                    device_id,
                    device_name.clone(),
                    SharedDeviceRefreshDeviceState::Discovered,
                )
                .await;
                self.set_shared_device_refresh_phase(
                    request_id,
                    SharedDeviceRefreshPhase::Connecting,
                    None,
                )
                .await;
                self.set_shared_device_refresh_device(
                    request_id,
                    device_id,
                    device_name.clone(),
                    SharedDeviceRefreshDeviceState::Connecting,
                )
                .await;
                let result = self.confirm_candidate(space_id, &device_id).await;
                let state = match result {
                    Ok(()) => SharedDeviceRefreshDeviceState::Connected,
                    Err(MembershipConvergenceError::WaitingForUpdate) => {
                        SharedDeviceRefreshDeviceState::WaitingForUpdate
                    }
                    Err(MembershipConvergenceError::PeerUnavailable) => {
                        self.shared_device_refresh_candidate_state(space_id, &device_id)
                            .await
                    }
                    Err(_) => SharedDeviceRefreshDeviceState::Rejected,
                };
                self.set_shared_device_refresh_device(request_id, device_id, device_name, state)
                    .await;
            }
        }
    }

    async fn shared_device_refresh_candidate_state(
        &self,
        space_id: &SpaceId,
        device_id: &uc_core::ids::DeviceId,
    ) -> SharedDeviceRefreshDeviceState {
        match self.deps.candidate_repo.get(space_id, device_id).await {
            Ok(Some(candidate))
                if candidate.last_failure() == Some(CandidateFailure::VersionIncompatible) =>
            {
                SharedDeviceRefreshDeviceState::VersionIncompatible
            }
            Ok(Some(candidate)) if candidate.status() == CandidateStatus::WaitingForUpdate => {
                SharedDeviceRefreshDeviceState::WaitingForUpdate
            }
            Ok(Some(candidate)) if candidate.status() == CandidateStatus::WaitingForPeer => {
                SharedDeviceRefreshDeviceState::WaitingForPeer
            }
            _ => SharedDeviceRefreshDeviceState::Rejected,
        }
    }

    pub(super) async fn mark_shared_device_refresh_candidate_connected(
        &self,
        space_id: &SpaceId,
        device_id: &uc_core::ids::DeviceId,
    ) {
        let waiting_refresh_device = {
            let active = self.shared_device_refresh.lock().await;
            active
                .as_ref()
                .filter(|refresh| refresh.space_id == *space_id)
                .and_then(|refresh| {
                    refresh
                        .status
                        .devices
                        .iter()
                        .find(|device| {
                            device.device_id == *device_id
                                && matches!(
                                    device.state,
                                    SharedDeviceRefreshDeviceState::WaitingForPeer
                                        | SharedDeviceRefreshDeviceState::WaitingForUpdate
                                        | SharedDeviceRefreshDeviceState::VersionIncompatible
                                )
                        })
                        .map(|device| {
                            (
                                refresh.status.request_id.clone(),
                                device.device_name.clone(),
                            )
                        })
                })
        };
        let Some((request_id, device_name)) = waiting_refresh_device else {
            return;
        };
        self.set_shared_device_refresh_device(
            &request_id,
            device_id.clone(),
            device_name,
            SharedDeviceRefreshDeviceState::Connected,
        )
        .await;
    }
}
