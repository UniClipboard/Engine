use super::super::*;
use async_trait::async_trait;
use uc_core::membership::{MembershipInitializationError, SpaceMembershipInitializerPort};

impl WorkspaceConvergence {
    pub(crate) async fn repair_incomplete_isolated_space_membership(
        &self,
    ) -> Result<(), WorkspaceConvergenceError> {
        match self.verified_admission_base_history().await {
            Ok(_) => Ok(()),
            Err(
                WorkspaceConvergenceError::RecoveryRequired
                | WorkspaceConvergenceError::NotAMember
                | WorkspaceConvergenceError::OwnInstanceRemoved,
            ) => {
                let previous_history = self
                    .deps
                    .admission_attempts
                    .load_membership_history_v2()
                    .await
                    .map_err(crate::space::convergence::admission::map_repository_error)?;
                self.rebuild_new_space_membership().await?;
                let repaired_history = self
                    .verified_legacy_admission_base_history()
                    .await?
                    .encode_persisted_v2()
                    .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
                self.deps
                    .admission_attempts
                    .compare_and_replace_membership_history_v2(
                        previous_history.as_deref(),
                        &repaired_history,
                    )
                    .await
                    .map_err(crate::space::convergence::admission::map_repository_error)?;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn recover_legacy_migration_marker(
        &self,
    ) -> Result<(), WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let (mut state, was_persisted) = self.load_state_with_presence().await?;
        let initialized_missing_legacy_state = !was_persisted && state.migrated_from_pre_adr_020;
        if initialized_missing_legacy_state
            || Self::clear_legacy_migration_marker_if_current_history_exists(&mut state)
        {
            self.persist(&state).await?;
        }
        Ok(())
    }
    /// Build the locally signed facts that a joiner returns after its group
    /// session is active. The facts remain inside the pairing exchange until
    /// the sponsor commits the admission chain.
    ///
    /// `member_instance` overrides the security-view resolution: a joining
    /// device must identify itself by the instance derived from this
    /// admission's freshly generated credential. The security view can still
    /// carry a stale instance of the same device from an earlier admission
    /// (a removed device cannot receive group updates), so the view must not
    /// be the source of truth for a fresh admission.
    pub async fn local_admission_facts(
        &self,
        member_instance: Option<uc_core::membership::MemberInstanceId>,
    ) -> Result<uc_core::membership::AdmissionChangeFacts, WorkspaceConvergenceError> {
        let material = self
            .deps
            .announcement_material
            .current_announcement_material()
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let member_instance = match member_instance {
            Some(instance) => instance,
            None => self
                .load_state()
                .await?
                .own_instance
                .ok_or(WorkspaceConvergenceError::NotAMember)?,
        };
        let mut facts = uc_core::membership::AdmissionChangeFacts {
            member_instance,
            device_id: material.device_id,
            device_name: material.device_name,
            identity_fingerprint: material.identity_fingerprint,
            transport_public_key: material.transport_public_key,
            transport_address_blob: material.transport_address_blob,
            identity_signature: Vec::new(),
        };
        facts.identity_signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&facts.signing_payload())
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        Ok(facts)
    }

    /// Save the sponsor's in-flight admission record before it starts
    /// waiting for the joiner's readiness. Survives restarts so the sponsor
    /// re-awaits the same joiner's readiness instead of saving a second
    /// member instance or a duplicated change. Idempotent for the same
    /// session and joiner.
    pub async fn begin_admission(
        &self,
        session: &uc_core::ports::pairing::PairingSessionId,
        joiner_device_id: &DeviceId,
        invitation_generation: u64,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        if let Some(persisted_instance) = state.own_instance {
            let current_instance = self
                .deps
                .member_signatures
                .current_member_instance(&self.deps.own_device)
                .await
                .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
            if current_instance != persisted_instance {
                state
                    .apply(
                        WorkspaceConvergenceEvent::IntegrityFailure(
                            uc_core::membership::WorkspaceFailureCategory::IdentityMismatch,
                        ),
                        now_ms,
                    )
                    .map_err(|_| {
                        WorkspaceConvergenceError::Inconsistent(
                            "current member identity mismatch could not be recorded".to_owned(),
                        )
                    })?;
                self.persist(&state).await?;
                self.publish(&state);
                self.notify();
                return Err(WorkspaceConvergenceError::Inconsistent(
                    "current member identity does not match persisted membership history"
                        .to_owned(),
                ));
            }
        }
        let (outcome, effect) = state
            .apply(
                WorkspaceConvergenceEvent::AdmissionBegan {
                    session: session.clone(),
                    joiner_device_id: joiner_device_id.clone(),
                    invitation_generation,
                },
                now_ms,
            )
            .map_err(|_| {
                WorkspaceConvergenceError::Inconsistent("admission begin rejected".to_owned())
            })?;
        if matches!(outcome, WorkspaceMergeOutcome::Updated) && effect.persist {
            self.persist(&state).await?;
        }
        self.notify();
        Ok(state.snapshot())
    }

    /// The sponsor's saved in-flight admission record for a pairing session,
    /// if any. Used after a restart to re-await the same joiner's readiness.
    pub async fn pending_admission(
        &self,
        session: &uc_core::ports::pairing::PairingSessionId,
    ) -> Result<Option<uc_core::membership::PendingAdmissionRecord>, WorkspaceConvergenceError>
    {
        let state = self.load_state().await?;
        Ok(state.pending_admissions.get(session).cloned())
    }

    /// Commit the readiness-confirmed admission in the single owner. On the
    /// first pairing the sponsor's already active member instance is seeded
    /// together with the joining instance in one repository save. The
    /// admission change, the joiner's pending handoff facts and the
    /// confirmation material are saved in the same commit; the in-flight
    /// admission record is cleared there as well. The returned confirmation
    /// is sent back to the joiner over the pairing channel.
    pub async fn commit_joiner_admission(
        &self,
        session: &uc_core::ports::pairing::PairingSessionId,
        joiner: uc_core::membership::AdmissionChangeFacts,
        security_update_payload: Vec<u8>,
    ) -> Result<uc_core::membership::AdmissionSavedFacts, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        if state.removed {
            return Err(WorkspaceConvergenceError::OwnInstanceRemoved);
        }
        if let Some(record) = state.pending_admissions.get(session) {
            if record.joiner_device_id != joiner.device_id {
                return Err(WorkspaceConvergenceError::Inconsistent(
                    "joiner readiness does not match the in-flight admission".to_owned(),
                ));
            }
            if record.invitation_generation < Self::admission_generation(&state) {
                // The admission generation advanced after the invitation was
                // bound; an old invitation cannot recover its old authority.
                return Err(WorkspaceConvergenceError::AdmissionGenerationAdvanced);
            }
        }
        let own_instance = match state.own_instance {
            Some(instance) => instance,
            None => self
                .deps
                .member_signatures
                .current_member_instance(&self.deps.own_device)
                .await
                .map_err(|_| WorkspaceConvergenceError::Unavailable)?,
        };
        let own = self.local_admission_facts(Some(own_instance)).await?;
        let mut additions = Vec::new();
        if !state.effective_members().contains(&own.member_instance) {
            additions.push(own.clone());
        }
        if !state.effective_members().contains(&joiner.member_instance) {
            additions.push(joiner.clone());
        }
        if additions.is_empty() {
            return Err(WorkspaceConvergenceError::Inconsistent(
                "admission unchanged".to_owned(),
            ));
        }
        // The local instance owns the admission event signatures. Establish
        // its durable history before the first event is appended.
        if state.own_instance.is_none() {
            state
                .apply(
                    WorkspaceConvergenceEvent::LocalAdmissionReady {
                        own_instance: own.member_instance,
                    },
                    now_ms,
                )
                .map_err(|_| {
                    WorkspaceConvergenceError::Inconsistent("own instance rejected".to_owned())
                })?;
        }
        // The roster persistence failures abort the commit before any
        // workspace change is persisted, keeping the save boundary intact.
        self.save_member_facts(&joiner, now_ms).await?;
        let security_state_digest = sha2::Sha256::digest(&security_update_payload).into();
        for facts in &additions {
            let event_security_update = (facts.member_instance == joiner.member_instance)
                .then_some(security_update_payload.clone())
                .unwrap_or_default();
            self.record_local_admission_history(
                &mut state,
                facts,
                security_state_digest,
                event_security_update,
            )
            .await?;
        }
        let _ = state.apply(
            WorkspaceConvergenceEvent::AdmissionCleared {
                session: session.clone(),
            },
            now_ms,
        );
        self.persist(&state).await?;
        self.publish(&state);
        self.notify();
        info!(joiner_device_id = %joiner.device_id.as_str(), "workspace admission change recorded");
        let history = state
            .membership_reconciliation
            .as_ref()
            .ok_or(WorkspaceConvergenceError::NotAMember)?;
        let history_digest = history.applied_members_digest().ok_or_else(|| {
            WorkspaceConvergenceError::Inconsistent(
                "admission produced no history digest".to_owned(),
            )
        })?;
        Ok(uc_core::membership::AdmissionSavedFacts {
            history_digest,
            history_event_count: history.known_event_count() as u64,
            sponsor_facts: own,
        })
    }

    /// Complete the joiner's admission only after it has saved the sponsor's
    /// member facts and recovered the exact history progress the sponsor
    /// committed. The sponsor is already authenticated by the pairing
    /// channel, so it may relay earlier members' individually signed events;
    /// every event is still verified against its actual author before it is
    /// persisted.
    pub async fn record_admission_saved(
        &self,
        confirmation: uc_core::membership::AdmissionSavedFacts,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let now_ms = self.deps.clock.now_ms();
        self.save_member_facts(&confirmation.sponsor_facts, now_ms)
            .await?;
        self.reconcile_membership_history_with_sponsor(&confirmation.sponsor_facts.device_id)
            .await?;

        let _guard = self.state_lock.lock().await;
        let state = self.load_state().await?;
        let history = state
            .membership_reconciliation
            .as_ref()
            .ok_or(WorkspaceConvergenceError::NotAMember)?;
        if history.known_event_count() as u64 != confirmation.history_event_count
            || history.applied_members_digest() != Some(confirmation.history_digest)
        {
            tracing::debug!(
                local_history_event_count = history.known_event_count(),
                sponsor_history_event_count = confirmation.history_event_count,
                digest_matches =
                    history.applied_members_digest() == Some(confirmation.history_digest),
                "sponsor admission history did not match the saved confirmation"
            );
            return Err(WorkspaceConvergenceError::Inconsistent(
                "sponsor admission history is incomplete or mismatched".to_owned(),
            ));
        }
        Ok(state.snapshot())
    }

    /// Persist the admitted member's roster facts (member instance, trust
    /// record and transport address) as part of the admission save boundary.
    pub(in crate::space::convergence) async fn save_member_facts(
        &self,
        facts: &uc_core::membership::AdmissionChangeFacts,
        now_ms: i64,
    ) -> Result<(), WorkspaceConvergenceError> {
        let joined_at = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms)
            .ok_or_else(|| WorkspaceConvergenceError::AdmissionStorage("clock".to_owned()))?;
        let sync_preferences = match self.deps.member_repo.get(&facts.device_id).await {
            Ok(Some(existing)) => existing.sync_preferences,
            _ => uc_core::MemberSyncPreferences::default(),
        };
        let member = uc_core::SpaceMember {
            device_id: facts.device_id.clone(),
            device_name: facts.device_name.clone(),
            identity_fingerprint: facts.identity_fingerprint.clone(),
            joined_at,
            sync_preferences,
        };
        self.deps
            .member_repo
            .save(&member)
            .await
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let peer = uc_core::trusted_peer::TrustedPeer {
            local_device_id: self.deps.device_identity.current_device_id(),
            peer_device_id: facts.device_id.clone(),
            peer_fingerprint: facts.identity_fingerprint.clone(),
            trusted_at: joined_at,
        };
        self.deps
            .trusted_peer_repo
            .save(&peer)
            .await
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        if !facts.transport_address_blob.is_empty() {
            self.deps
                .peer_addr_repo
                .upsert(&uc_core::ports::PeerAddressRecord {
                    device_id: facts.device_id.clone(),
                    addr_blob: facts.transport_address_blob.clone(),
                    observed_at: joined_at,
                })
                .await
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        }
        Ok(())
    }

    async fn record_local_admission_history(
        &self,
        state: &mut WorkspaceConvergenceState,
        facts: &uc_core::membership::AdmissionChangeFacts,
        security_state_digest: [u8; 32],
        security_update_payload: Vec<u8>,
    ) -> Result<(), WorkspaceConvergenceError> {
        use sha2::{Digest, Sha256};

        let own_instance = state
            .own_instance
            .ok_or(WorkspaceConvergenceError::NotAMember)?;
        let (parent_event_id, parent_depth, mut members) = {
            let history = state.membership_reconciliation.get_or_insert_with(|| {
                uc_core::membership::MembershipReconciliation::new(
                    state.space_lineage.clone(),
                    own_instance,
                )
            });
            let (parent_event_id, parent_depth) = history.next_event_position();
            (parent_event_id, parent_depth, history.effective_members())
        };
        members.insert(facts.member_instance);
        let mut members_hasher = Sha256::new();
        members_hasher.update(b"uniclipboard-membership-members/v1\0");
        members_hasher.update(state.space_lineage.as_bytes());
        for member in members {
            members_hasher.update(member.as_bytes());
        }
        let resulting_members_digest = members_hasher.finalize().into();
        let admission_bundle_digest = Some(Sha256::digest(facts.signing_payload()).into());
        let operation_id = uuid::Uuid::new_v4().into_bytes();
        let unsigned = uc_core::membership::MembershipEvent::new(
            state.space_lineage.clone(),
            parent_event_id,
            parent_depth,
            operation_id,
            own_instance,
            uc_core::membership::MembershipOperation::AddDevice {
                admission: facts.clone(),
            },
            resulting_members_digest,
            security_state_digest,
            security_update_payload,
            admission_bundle_digest,
            Vec::new(),
        );
        let signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&unsigned.signing_payload())
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let event = uc_core::membership::MembershipEvent::new(
            unsigned.lineage_id,
            unsigned.parent_event_id,
            unsigned.parent_depth,
            unsigned.operation_id,
            unsigned.author_member_instance_id,
            unsigned.operation,
            unsigned.resulting_members_digest,
            unsigned.security_state_digest,
            unsigned.security_update_payload,
            unsigned.admission_bundle_digest,
            signature,
        );
        state
            .membership_reconciliation
            .as_mut()
            .ok_or(WorkspaceConvergenceError::NotAMember)?
            .receive_verified(event)
            .map_err(|_| {
                WorkspaceConvergenceError::Inconsistent("admission history rejected".to_owned())
            })?;
        Ok(())
    }

    fn clear_legacy_migration_marker_if_current_history_exists(
        state: &mut WorkspaceConvergenceState,
    ) -> bool {
        if !state.migrated_from_pre_adr_020 {
            return false;
        }
        if state
            .membership_reconciliation
            .as_ref()
            .is_some_and(|history| history.applied_head().is_some())
        {
            state.migrated_from_pre_adr_020 = false;
            return true;
        }
        false
    }

    pub(super) async fn record_local_removal_history(
        &self,
        state: &mut WorkspaceConvergenceState,
        removed_member: uc_core::membership::MemberInstanceId,
        security_state_digest: [u8; 32],
    ) -> Result<(), WorkspaceConvergenceError> {
        use sha2::{Digest, Sha256};

        let own_instance = state
            .own_instance
            .ok_or(WorkspaceConvergenceError::NotAMember)?;
        let (parent_event_id, parent_depth, mut members) = {
            let history = state
                .membership_reconciliation
                .as_ref()
                .ok_or(WorkspaceConvergenceError::NotAMember)?;
            let (parent_event_id, parent_depth) = history.next_event_position();
            (parent_event_id, parent_depth, history.effective_members())
        };
        if !members.remove(&removed_member) {
            return Err(WorkspaceConvergenceError::UnknownTarget);
        }
        let mut members_hasher = Sha256::new();
        members_hasher.update(b"uniclipboard-membership-members/v1\0");
        members_hasher.update(state.space_lineage.as_bytes());
        for member in members {
            members_hasher.update(member.as_bytes());
        }
        let resulting_members_digest = members_hasher.finalize().into();
        let operation_id = uuid::Uuid::new_v4().into_bytes();
        let unsigned = uc_core::membership::MembershipEvent::new(
            state.space_lineage.clone(),
            parent_event_id,
            parent_depth,
            operation_id,
            own_instance,
            uc_core::membership::MembershipOperation::RemoveDevice {
                member: removed_member,
            },
            resulting_members_digest,
            security_state_digest,
            Vec::new(),
            None,
            Vec::new(),
        );
        let signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&unsigned.signing_payload())
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let event = uc_core::membership::MembershipEvent::new(
            unsigned.lineage_id,
            unsigned.parent_event_id,
            unsigned.parent_depth,
            unsigned.operation_id,
            unsigned.author_member_instance_id,
            unsigned.operation,
            unsigned.resulting_members_digest,
            unsigned.security_state_digest,
            unsigned.security_update_payload,
            unsigned.admission_bundle_digest,
            signature,
        );
        state
            .membership_reconciliation
            .as_mut()
            .ok_or(WorkspaceConvergenceError::NotAMember)?
            .receive_verified(event)
            .map_err(|_| {
                WorkspaceConvergenceError::Inconsistent("removal history rejected".to_owned())
            })?;
        Ok(())
    }

    /// Record the local member instance and its readiness record after a
    /// successful admission (the joiner's local readiness; the sponsor
    /// records the admission change only after this readiness).
    ///
    /// A re-admission with a new member instance discards the previous
    /// instance's local chain, confirmations and removal facts: the old
    /// instance's history must not constrain the new one (ADR-015 new
    /// instance rule). The lineage is preserved.
    pub async fn record_local_readiness(
        &self,
        own_instance: uc_core::membership::MemberInstanceId,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        if state.removed
            || state
                .own_instance
                .is_some_and(|previous| previous != own_instance)
        {
            let lineage = state.space_lineage.clone();
            state = WorkspaceConvergenceState::fresh(lineage, now_ms);
            self.persist(&state).await?;
        }
        let (outcome, effect) = state
            .apply(
                WorkspaceConvergenceEvent::LocalAdmissionReady { own_instance },
                now_ms,
            )
            .map_err(|_| {
                WorkspaceConvergenceError::Inconsistent("readiness rejected".to_owned())
            })?;
        if matches!(outcome, WorkspaceMergeOutcome::Updated) && effect.persist {
            self.persist(&state).await?;
        }
        Ok(state.snapshot())
    }

    /// Establish the single current-history starting point after a retained
    /// pre-1.1 space has completed its shared protection upgrade.
    pub async fn initialize_upgraded_legacy_space(
        &self,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        self.initialize_legacy_space_membership(false).await
    }

    async fn initialize_legacy_space_membership(
        &self,
        force_local_initializer: bool,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let own_instance = self
            .deps
            .member_signatures
            .current_member_instance(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let own_facts = self.local_admission_facts(Some(own_instance)).await?;
        let members = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let is_stable_initializer = members
            .iter()
            .map(|member| &member.device_id)
            .chain(std::iter::once(&self.deps.own_device))
            .min_by(|left, right| left.as_str().cmp(right.as_str()))
            == Some(&self.deps.own_device);
        let is_initializer = force_local_initializer || is_stable_initializer;
        let security_state = self.deps.security_updates.current_state().await?;
        let mut digest = sha2::Sha256::new();
        digest.update(b"uniclipboard-membership-security/v1\0");
        digest.update(security_state.space_id.as_ref().as_bytes());
        digest.update(security_state.group_epoch.to_be_bytes());

        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        if state.own_instance.is_none() {
            state
                .apply(
                    WorkspaceConvergenceEvent::LocalAdmissionReady { own_instance },
                    now_ms,
                )
                .map_err(|_| {
                    WorkspaceConvergenceError::Inconsistent(
                        "legacy upgrade readiness rejected".to_owned(),
                    )
                })?;
        }
        let history_empty = state
            .membership_reconciliation
            .as_ref()
            .is_none_or(|history| history.known_event_count() == 0);
        if is_initializer && history_empty {
            self.record_local_admission_history(
                &mut state,
                &own_facts,
                digest.finalize().into(),
                Vec::new(),
            )
            .await?;
        }
        Self::clear_legacy_migration_marker_if_current_history_exists(&mut state);
        self.persist(&state).await?;
        self.publish(&state);
        self.notify();
        Ok(state.snapshot())
    }

    /// Finish the membership baseline for a newly-created Space before A1 is
    /// allowed to report success.
    pub(crate) async fn initialize_new_space_membership(
        &self,
    ) -> Result<(), WorkspaceConvergenceError> {
        let result = self
            .deps
            .group_bootstrap
            .bootstrap_legacy_space(&self.deps.own_device, &[], self.deps.clock.now_ms())
            .await
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        if !matches!(
            result,
            uc_core::membership::GroupBootstrapResult::Complete { .. }
        ) {
            return Err(WorkspaceConvergenceError::Inconsistent(
                "new space protection group did not complete".to_owned(),
            ));
        }
        self.rebuild_new_space_membership().await
    }

    async fn rebuild_new_space_membership(&self) -> Result<(), WorkspaceConvergenceError> {
        let lineage = self
            .deps
            .membership_identity
            .current_membership_identity()
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?
            .space_id
            .as_ref()
            .to_owned();
        {
            let _guard = self.state_lock.lock().await;
            let state = WorkspaceConvergenceState::fresh(lineage, self.deps.clock.now_ms());
            self.persist(&state).await?;
        }
        self.initialize_legacy_space_membership(true).await?;
        Ok(())
    }

    /// Complete a retained legacy member's protection-group join by fetching
    /// the sponsor's authoritative current membership history before normal
    /// peer reconciliation resumes.
    pub async fn complete_upgraded_legacy_join(
        &self,
        sponsor: &DeviceId,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let own_instance = self
            .deps
            .member_signatures
            .current_member_instance(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        self.record_local_readiness(own_instance).await?;
        self.reconcile_membership_history_with_sponsor(sponsor)
            .await?;
        self.query().await
    }
}

#[async_trait]
impl SpaceMembershipInitializerPort for WorkspaceConvergence {
    async fn initialize(&self) -> Result<(), MembershipInitializationError> {
        self.initialize_new_space_membership()
            .await
            .map_err(map_membership_initialization_error)
    }
}
