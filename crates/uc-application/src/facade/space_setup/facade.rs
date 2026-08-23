//! `SpaceFacade` — space-lifecycle entry point (A1 + A2 + shutdown).
//!
//! Owns the two use cases so A1/A2 success can prime presence cache (F1) via
//! `ensure_reachable_all`. Also owns the sponsor-side inbound orchestrator so
//! the rest of the crate never sees the spawn surface (§11.4).
//!
//! Slice 4 P5c: 历史上还持有 `NetworkControlPort` 用于 A1/A2 后调
//! `start_network` (F1) + `on_shutdown` 调 `stop_network` (F2),已退役——
//! iroh router 由 `SyncEngineAssembly` 直接驱动,libp2p 兼容路径整体下线。
//! F1 hook 保留(改名 `auto_prime_presence`),只跑 `ensure_reachable_all`;
//! F2 不再触碰网络层。
//!
//! Network errors during auto-prime are intentionally non-fatal: the
//! underlying space mutation has already committed and isn't safe to roll
//! back, and presence will lazily recover via the adapter's
//! `Connection::closed` watchdog. Failures are surfaced through
//! `tracing::warn!` so ops still sees them.

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::task::JoinHandle;
use tracing::{info, instrument, warn};

use crate::clipboard::write::MobileConsumableBackfill;
use crate::facade::space_setup::commands::{
    CurrentInvitation, InitializeSpaceCommand, InitializeSpaceInput, InitializeSpaceResult,
    IssuePairingInvitationResult, PairingInvitationAddressCandidate, SetupStateView,
    UnlockSpaceCommand, UnlockSpaceInput, UnlockSpaceResult,
};
use crate::facade::space_setup::commands::{
    RedeemPairingInvitationCommand, RedeemPairingInvitationInput, RedeemPairingInvitationResult,
};
use crate::facade::space_setup::deps::{
    SpaceAdmissionDeps, SpaceFacadeDeps, SpaceSessionDeps, SpaceTransitionDeps,
};
use crate::facade::space_setup::errors::{
    CancelInvitationError, FactoryResetError, QuerySetupStateError, RedeemPairingInvitationError,
    ResetSpaceError,
};
use crate::facade::space_setup::errors::{
    InitializeSpaceError, IssuePairingInvitationError, TryResumeSessionError, UnlockSpaceError,
};
use crate::space::admission::adapter::WorkspaceAdmissionOwnerPort;
use crate::space::admission::invitation::InMemoryPairingInvitationHolder;
use crate::space::admission::issue_invitation::IssuePairingInvitationUseCase;
use crate::space::admission::joiner::joiner_handshake::JoinerHandshakeCoordinator;
use crate::space::admission::redeem_invitation::RedeemPairingInvitationUseCase;
use crate::space::admission::sponsor::orchestrator::PairingInboundOrchestrator;
use crate::space::admission::sponsor::sponsor_handshake::SponsorHandshakeCoordinator;
use crate::space::convergence::connectivity::reachability::{
    EnsureReachableAllError, EnsureReachableAllReport, EnsureReachableAllUseCase,
};
use crate::space::convergence::membership::group_update_delivery::GroupUpdateDeliveryPort;
use crate::space::lifecycle::initialize_space::InitializeSpaceUseCase;
use crate::space::lifecycle::unlock_space::UnlockSpaceUseCase;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    CurrentWorkspacePeerScopePort, DeviceManagementResetDataPort, MembershipAdmissionGatePort,
    RelationshipStateResetPort, SpaceSecurityStateResetPort,
};
use uc_core::ports::pairing::PairingSessionPort;
use uc_core::ports::space::{
    AdoptIsolatedSpacePort, FactoryResetSpacePort, ResumeSpaceSessionPort, SpaceAccessError,
};
use uc_core::ports::{
    PeerAddressRepositoryPort, PresenceError, PresencePort, ReachabilityState, SettingsPort,
    SetupStatusPort,
};
use uc_core::setup::SetupStatus;
use uc_core::{MemberRepositoryPort, MemberSyncPreferences, SpaceMember};

struct DeviceManagementResetUseCase {
    legacy_isolation_required: AtomicBool,
    execution_lock: tokio::sync::Mutex<()>,
    adopt_space: Arc<dyn AdoptIsolatedSpacePort>,
    data_transition: Arc<dyn DeviceManagementResetDataPort>,
    relationship_reset: Arc<dyn RelationshipStateResetPort>,
    security_reset: Arc<dyn SpaceSecurityStateResetPort>,
    setup_status: Arc<dyn SetupStatusPort>,
    local_identity: Arc<dyn uc_core::ports::LocalIdentityPort>,
    device_identity: Arc<dyn uc_core::ports::DeviceIdentityPort>,
    settings: Arc<dyn SettingsPort>,
    member_repo: Arc<dyn MemberRepositoryPort>,
    convergence: Arc<crate::space::convergence::WorkspaceConvergence>,
    app_version_state: Arc<dyn uc_core::ports::AppVersionStatePort>,
    current_app_version: String,
}

impl DeviceManagementResetUseCase {
    /// Clear any persisted admission state (pending joins / sponsorships) that
    /// could block a future pairing attempt.
    ///
    /// Used by both the full device-management reset ([`Self::execute_reset`])
    /// and the last-resort factory reset (`SpaceFacade::factory_reset`): a
    /// crash mid-admission leaves a durable "admission in progress" record,
    /// and if that record survives a wipe-and-repair recovery every later
    /// pairing attempt fails with `AdmissionInProgress` until the store is
    /// cleared.
    async fn clear_admission_state(&self) -> Result<(), String> {
        self.convergence
            .reset_admission_for_device_management()
            .await
            .map_err(|error| error.to_string())
    }

    async fn remove_remote_members(&self) -> Result<(), String> {
        let local_device_id = self.device_identity.current_device_id();
        let members = self
            .member_repo
            .list()
            .await
            .map_err(|error| error.to_string())?;
        for member in members {
            if member.device_id != local_device_id {
                self.member_repo
                    .remove(&member.device_id)
                    .await
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    async fn mark_completed(&self) -> Result<(), String> {
        self.app_version_state
            .write(&self.current_app_version)
            .await
            .map_err(|error| error.to_string())
    }

    async fn resume_legacy_isolation_if_required(&self) -> Result<(), String> {
        let _guard = self.execution_lock.lock().await;
        let status = self
            .setup_status
            .get_status()
            .await
            .map_err(|error| error.to_string())?;
        if status.re_pairing_required {
            self.remove_remote_members().await?;
            self.convergence
                .repair_incomplete_isolated_space_membership()
                .await
                .map_err(|error| error.to_string())?;
            self.setup_status
                .clear_device_management_reset_target()
                .await
                .map_err(|error| error.to_string())?;
            if self.legacy_isolation_required.load(Ordering::Acquire) {
                self.mark_completed().await?;
            }
            self.legacy_isolation_required
                .store(false, Ordering::Release);
            return Ok(());
        }
        let pending_isolation_target = self
            .setup_status
            .get_device_management_reset_target()
            .await
            .map_err(|error| error.to_string())?;
        if !self.legacy_isolation_required.load(Ordering::Acquire)
            && pending_isolation_target.is_none()
        {
            return Ok(());
        }

        self.execute_reset(pending_isolation_target)
            .await
            .map_err(|error| error.to_string())?;
        self.mark_completed().await?;
        self.legacy_isolation_required
            .store(false, Ordering::Release);
        Ok(())
    }

    async fn execute_user_requested(&self) -> Result<(), ResetSpaceError> {
        let _guard = self.execution_lock.lock().await;
        let pending_target = self
            .setup_status
            .get_device_management_reset_target()
            .await
            .map_err(|error| ResetSpaceError::PreparationFailed(error.to_string()))?;
        let status = self
            .setup_status
            .get_status()
            .await
            .map_err(|error| ResetSpaceError::PreparationFailed(error.to_string()))?;
        if pending_target
            .as_ref()
            .is_some_and(|target| status.space_id.as_ref() == Some(target))
        {
            let target = pending_target
                .as_ref()
                .ok_or_else(|| ResetSpaceError::Internal("reset target disappeared".to_owned()))?;
            self.data_transition
                .finalize_device_management_reset(target)
                .await
                .map_err(|error| ResetSpaceError::FinalizationFailed(error.to_string()))?;
            if !status.re_pairing_required {
                self.setup_status
                    .set_status(&SetupStatus {
                        has_completed: true,
                        space_id: status.space_id,
                        re_pairing_required: true,
                    })
                    .await
                    .map_err(|error| ResetSpaceError::FinalizationFailed(error.to_string()))?;
            }
            self.setup_status
                .clear_device_management_reset_target()
                .await
                .map_err(|error| ResetSpaceError::FinalizationFailed(error.to_string()))?;
            return Ok(());
        }
        if pending_target.is_none() && status.re_pairing_required {
            return Ok(());
        }
        self.execute_reset(pending_target).await
    }

    async fn execute_reset(
        &self,
        pending_isolation_target: Option<SpaceId>,
    ) -> Result<(), ResetSpaceError> {
        let settings = self
            .settings
            .load()
            .await
            .map_err(|error| ResetSpaceError::PreparationFailed(error.to_string()))?;
        let device_name = settings
            .general
            .device_name
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| {
                ResetSpaceError::PreparationFailed("local device name is unavailable".to_owned())
            })?;
        let identity_fingerprint = self
            .local_identity
            .ensure()
            .await
            .map_err(|error| ResetSpaceError::PreparationFailed(error.to_string()))?;
        let space_id = match pending_isolation_target {
            Some(space_id) => space_id,
            None => {
                let space_id = SpaceId::new();
                self.setup_status
                    .set_device_management_reset_target(&space_id)
                    .await
                    .map_err(|error| ResetSpaceError::PreparationFailed(error.to_string()))?;
                space_id
            }
        };
        self.data_transition
            .prepare_device_management_reset(&space_id)
            .await
            .map_err(|error| ResetSpaceError::PreparationFailed(error.to_string()))?;
        self.data_transition
            .stage_device_management_reset_mutations(&space_id)
            .await
            .map_err(|error| ResetSpaceError::StagingFailed(error.to_string()))?;
        self.adopt_space
            .adopt_isolated_space(&space_id)
            .await
            .map_err(|error| ResetSpaceError::RebuildFailed(error.to_string()))?;
        self.clear_admission_state()
            .await
            .map_err(|error| ResetSpaceError::RebuildFailed(error))?;
        self.relationship_reset
            .clear_all_relationships()
            .await
            .map_err(|error| ResetSpaceError::RebuildFailed(error.to_string()))?;
        self.remove_remote_members()
            .await
            .map_err(ResetSpaceError::RebuildFailed)?;

        let member = SpaceMember {
            device_id: self.device_identity.current_device_id(),
            device_name,
            identity_fingerprint,
            joined_at: Utc::now(),
            sync_preferences: MemberSyncPreferences::default(),
        };
        self.member_repo
            .save(&member)
            .await
            .map_err(|error| ResetSpaceError::RebuildFailed(error.to_string()))?;
        self.convergence
            .initialize_new_space_membership()
            .await
            .map_err(|error| ResetSpaceError::RebuildFailed(error.to_string()))?;
        self.data_transition
            .promote_device_management_reset(&space_id)
            .await
            .map_err(|error| ResetSpaceError::CommitFailed(error.to_string()))?;
        self.security_reset
            .clear_space_security_state_except(&space_id)
            .await
            .map_err(|error| ResetSpaceError::FinalizationFailed(error.to_string()))?;
        self.data_transition
            .finalize_device_management_reset(&space_id)
            .await
            .map_err(|error| ResetSpaceError::FinalizationFailed(error.to_string()))?;
        self.setup_status
            .set_status(&SetupStatus {
                has_completed: true,
                space_id: Some(space_id),
                re_pairing_required: true,
            })
            .await
            .map_err(|error| ResetSpaceError::FinalizationFailed(error.to_string()))?;
        self.setup_status
            .clear_device_management_reset_target()
            .await
            .map_err(|error| ResetSpaceError::FinalizationFailed(error.to_string()))?;
        Ok(())
    }
}

/// Space-lifecycle facade (A1 initialise, A2 unlock, B1 issue invitation,
/// B2 redeem invitation, P7e inbound subscriber, F2 shutdown).
pub struct SpaceFacade {
    initialize_space: Arc<InitializeSpaceUseCase>,
    unlock_space: Arc<UnlockSpaceUseCase>,
    issue_pairing_invitation: Arc<IssuePairingInvitationUseCase>,
    redeem_pairing_invitation: Arc<RedeemPairingInvitationUseCase>,
    /// `JoinHandle` for the sponsor-side inbound pairing orchestrator
    /// spawned during construction. Aborted in [`Self::on_shutdown`] so
    /// the event loop doesn't outlive the facade.
    pairing_inbound_handle: JoinHandle<()>,
    /// Held for [`Self::try_resume_session`] — the silent resume path needs
    /// both the setup flag (to decide whether there's anything to resume at
    /// all) and direct access to [`ResumeSpaceSessionPort::try_resume_session`].
    /// Everything else still goes through use cases.
    resume_session: Arc<dyn ResumeSpaceSessionPort>,
    /// Held for [`Self::factory_reset`] — wipes persisted key material before
    /// clearing setup status.
    factory_reset: Arc<dyn FactoryResetSpacePort>,
    relationship_reset: Arc<dyn RelationshipStateResetPort>,
    setup_status: Arc<dyn SetupStatusPort>,
    mobile_consumable_backfill: Arc<dyn MobileConsumableBackfill>,
    /// Slice4 P3 T3.2 · `query_setup_state` reads `device_name` from
    /// `Settings.general`; `cancel_invitation` / `reset` need no
    /// settings access but the field stays `pub(crate)` so a future
    /// query can pick up additional general fields without churn.
    settings: Arc<dyn SettingsPort>,
    /// Slice4 P3 T3.2 · `cancel_invitation` clears the in-memory
    /// pending-invitation map; `query_setup_state` snapshots the
    /// earliest-expiring entry. Held in addition to the use-case-owned
    /// clone so the facade keeps a stable read/write handle.
    invitation_holder: Arc<InMemoryPairingInvitationHolder>,
    /// Slice 2 Phase 1 · T8：F1 hook。A1/A2/B2 成功后
    /// [`Self::auto_prime_presence`] 触发一次全员预连,把 presence 缓存
    /// 填满,让 UI 查 roster 时 online/offline 立刻准。
    ensure_reachable_all: Arc<EnsureReachableAllUseCase>,
    member_repo: Arc<dyn MemberRepositoryPort>,
    peer_scope: Arc<dyn CurrentWorkspacePeerScopePort>,
    /// Held for the desktop keepalive scheduler — `list_paired_peer_device_ids`
    /// reads `peer_addr_repo.list()` and `ensure_reachable_one` forwards to
    /// `presence.ensure_reachable`. Both are thin wrappers driven by the
    /// worker's per-peer backoff state machine; the underlying ports stay
    /// owned by use cases as before.
    peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    presence: Arc<dyn PresencePort>,
    pairing_session: Arc<dyn PairingSessionPort>,
    device_management_reset: DeviceManagementResetUseCase,
    /// `current_device_id()` snapshotted at facade-construction time so
    /// `list_paired_peer_device_ids` can self-filter without grabbing the
    /// `DeviceIdentityPort` lock on every call.
    local_device_id: DeviceId,
}

impl SpaceFacade {
    /// Wire all use cases from a single [`SpaceFacadeDeps`] bundle and
    /// spawn the sponsor-side inbound pairing orchestrator.
    ///
    /// The workspace convergence owner and the group-update delivery are
    /// taken from `admission.convergence`; callers assemble them through
    /// [`SpaceConvergenceAssembly::new`] so the application layer stays the
    /// single construction point (ADR-018).
    pub fn new(deps: SpaceFacadeDeps) -> Self {
        Self::new_internal(deps)
    }

    fn new_internal(deps: SpaceFacadeDeps) -> Self {
        let SpaceFacadeDeps {
            session,
            admission,
            transition,
        } = deps;
        let SpaceSessionDeps {
            space_access,
            setup_status,
            mobile_consumable_backfill,
            legacy_profile_isolation_required,
            app_version_state,
            current_app_version,
        } = session;
        let SpaceAdmissionDeps {
            local_identity,
            device_identity,
            member_repo,
            settings,
            clock,
            pairing_invitation,
            pairing_invitation_addresses,
            pairing_invitation_by_address,
            pairing_session,
            pairing_events,
            proof_port,
            peer_addr_repo,
            presence,
            analytics,
            convergence,
            ..
        } = admission;
        let convergence = Arc::clone(&convergence);
        let workspace_convergence = Arc::clone(&convergence.workspace);
        let peer_scope = convergence.current_peer_scope();
        let group_update_delivery: Arc<dyn GroupUpdateDeliveryPort> =
            convergence.group_update_delivery();
        let SpaceTransitionDeps {
            device_management_reset_data,
            relationship_reset,
            space_security_reset,
        } = transition;

        // Stash the narrow slices the facade itself drives (`try_resume_session`
        // / `factory_reset`) before the bundle's other slices are handed to the
        // use cases below. The facade owns these two paths directly rather than
        // routing through a use case that would only wrap a single port call.
        let resume_session_for_facade = Arc::clone(&space_access.resume_session);
        let factory_reset_for_facade = Arc::clone(&space_access.factory_reset);
        let relationship_reset_for_facade = Arc::clone(&relationship_reset);
        let setup_status_for_facade = Arc::clone(&setup_status);
        let member_repo_for_facade = Arc::clone(&member_repo);
        // Slice4 P3 T3.2 · facade-local handle for `query_setup_state`
        // (reads `Settings.general.device_name`).
        let settings_for_facade = Arc::clone(&settings);
        let device_management_reset = DeviceManagementResetUseCase {
            legacy_isolation_required: AtomicBool::new(legacy_profile_isolation_required),
            execution_lock: tokio::sync::Mutex::new(()),
            adopt_space: Arc::clone(&space_access.adopt_isolated_space),
            data_transition: device_management_reset_data,
            relationship_reset: Arc::clone(&relationship_reset),
            security_reset: space_security_reset,
            setup_status: Arc::clone(&setup_status),
            local_identity: Arc::clone(&local_identity),
            device_identity: Arc::clone(&device_identity),
            settings: Arc::clone(&settings),
            member_repo: Arc::clone(&member_repo),
            convergence: Arc::clone(&workspace_convergence),
            app_version_state,
            current_app_version,
        };

        // Invitation holder is purely an internal flow-state component
        // (§11.4) — construct it here so bootstrap never sees the type.
        let invitation_holder = Arc::new(InMemoryPairingInvitationHolder::new());
        // Slice4 P3 T3.2 · facade-local handle for `cancel_invitation`
        // / `query_setup_state` snapshots; the use case + orchestrator
        // already own their own `Arc::clone`s below.
        let invitation_holder_for_facade = Arc::clone(&invitation_holder);

        let initialize_space = Arc::new(InitializeSpaceUseCase::new(
            Arc::clone(&space_access.initialize),
            Arc::clone(&local_identity),
            Arc::clone(&device_identity),
            Arc::clone(&member_repo),
            Arc::clone(&workspace_convergence)
                as Arc<
                    dyn crate::space::lifecycle::initialize_space::NewSpaceMembershipInitializer,
                >,
            Arc::clone(&setup_status),
            Arc::clone(&settings),
            Arc::clone(&clock),
            Arc::clone(&analytics),
        ));
        let unlock_space = Arc::new(UnlockSpaceUseCase::new(
            Arc::clone(&space_access.unlock),
            Arc::clone(&setup_status),
            Arc::clone(&analytics),
        ));
        let issue_pairing_invitation = Arc::new(IssuePairingInvitationUseCase::new(
            Arc::clone(&pairing_invitation),
            pairing_invitation_addresses,
            pairing_invitation_by_address,
            Arc::clone(&device_identity),
            Arc::clone(&clock),
            Arc::clone(&invitation_holder),
            Arc::clone(&analytics),
            Arc::clone(&workspace_convergence) as Arc<dyn MembershipAdmissionGatePort>,
        ));
        // T8 · F1 hook: construct ensure_reachable_all early so peer_addr_repo /
        // device_identity can still be Arc::clone'd here — both are moved into
        // downstream use cases below.
        //
        // Backoff scheduler (P-keepalive-backoff): the desktop keepalive
        // worker reads `peer_addr_repo` and dials individual peers via the
        // facade thin wrappers — clone before the use case ownership move.
        let peer_addr_repo_for_facade = Arc::clone(&peer_addr_repo);
        let presence_for_facade = Arc::clone(&presence);
        let pairing_session_for_facade = Arc::clone(&pairing_session);
        let ensure_reachable_all = Arc::new(EnsureReachableAllUseCase::new(
            Arc::clone(&peer_addr_repo),
            presence,
            Arc::clone(&device_identity),
            Arc::clone(&peer_scope),
        ));
        // Build the sponsor-side pairing stack: the handshake
        // coordinator owns wire I/O for the AdmissionOffer→Confirm flow;
        // the orchestrator composes it with admit/trust use cases so
        // persistence is done by the already-existing use cases rather
        // than being duplicated here.
        let local_device_id = device_identity.current_device_id();
        // Same id stashed for the keepalive scheduler's self-filter — the
        // original is moved into the inbound orchestrator below.
        let local_device_id_for_facade = local_device_id;
        // Handshake TTL：sponsor 侧从 begin 到 confirm/reject 的 watchdog
        // （P7g），joiner 侧每次 recv 的 timeout（P7h）。180s 是为
        // Tailscale DERP relay 这种跨区中继路径预留的容差 —— 跨洋 DERP
        // RTT 300–800ms 叠 iroh 多 path 协商抖动，60s 不够喂完 4 条
        // 握手消息（实测 #486 复测 13:02 那次 sponsor accept_bi 卡 23s
        // 又 read_exact 卡 34s，joiner 60s TTL 先到 abort）。
        let handshake_ttl = Duration::from_secs(180);

        let sponsor_handshake = SponsorHandshakeCoordinator::new(
            Arc::clone(&pairing_session),
            Arc::clone(&space_access.prepare_admission_offer),
            group_update_delivery,
            Arc::clone(&proof_port),
            Arc::clone(&setup_status),
            handshake_ttl,
        );
        let inbound_orchestrator = Arc::new(PairingInboundOrchestrator::new(
            pairing_events,
            pairing_invitation,
            invitation_holder,
            Arc::clone(&clock),
            sponsor_handshake,
            Arc::clone(&workspace_convergence) as Arc<dyn WorkspaceAdmissionOwnerPort>,
            Arc::clone(&analytics),
        ));
        let pairing_inbound_handle = inbound_orchestrator.spawn();

        // joiner-side symmetric: coordinator holds wire + crypto, use
        // case composes it with the workspace owner's admission saves.
        let joiner_handshake = JoinerHandshakeCoordinator::new(
            pairing_session,
            Arc::clone(&space_access.derive_admission_proof_key),
            Arc::clone(&space_access.prepare_admission_target_access),
            Arc::clone(&space_access.group_admission),
            proof_port,
            local_identity,
            device_identity,
            settings,
            Arc::clone(&workspace_convergence) as Arc<dyn WorkspaceAdmissionOwnerPort>,
            handshake_ttl,
        );
        let redeem_pairing_invitation = Arc::new(RedeemPairingInvitationUseCase::new(
            joiner_handshake,
            setup_status,
            Arc::clone(&resume_session_for_facade),
            Arc::clone(&analytics),
        ));

        Self {
            initialize_space,
            unlock_space,
            issue_pairing_invitation,
            redeem_pairing_invitation,
            pairing_inbound_handle,
            resume_session: resume_session_for_facade,
            factory_reset: factory_reset_for_facade,
            relationship_reset: relationship_reset_for_facade,
            setup_status: setup_status_for_facade,
            mobile_consumable_backfill,
            settings: settings_for_facade,
            invitation_holder: invitation_holder_for_facade,
            ensure_reachable_all,
            member_repo: member_repo_for_facade,
            peer_scope,
            peer_addr_repo: peer_addr_repo_for_facade,
            presence: presence_for_facade,
            pairing_session: pairing_session_for_facade,
            device_management_reset,
            local_device_id: local_device_id_for_facade,
        }
    }

    /// Try to restore the in-memory space session silently, using the
    /// KEK cached in secure storage by a previous `init` / `unlock`.
    ///
    /// Returns `Ok(true)` when the session is now unlocked and ready
    /// for pairing operations; `Ok(false)` when there is nothing to
    /// resume (setup has not completed on this profile). Genuine
    /// problems — corrupt key material, missing keyring entry despite
    /// a keyslot on disk, or adapter faults — surface via
    /// [`TryResumeSessionError`].
    ///
    /// Intended for short-lived CLI processes: every `invite` call
    /// drives this before B1 so the sponsor's `verify_proof` path has
    /// the master key in memory when the joiner's ChallengeResponse
    /// lands. GUI / daemon callers can use it at startup to skip the
    /// passphrase prompt when the keyring still has the KEK.
    #[instrument(skip_all)]
    pub async fn try_resume_session(&self) -> Result<bool, TryResumeSessionError> {
        let status = self
            .setup_status
            .get_status()
            .await
            .map_err(|err| TryResumeSessionError::Internal(err.to_string()))?;
        if !status.has_completed {
            return Ok(false);
        }

        let space_id = status
            .space_id
            .clone()
            .unwrap_or_else(super::legacy_space_id);
        let resumed = match self.resume_session.try_resume_session(&space_id).await {
            Ok(Some(_)) => true,
            // Keyslot missing despite has_completed == true — treat
            // as "nothing to resume" rather than an error: can happen
            // right after factory_reset when setup_status lagged.
            Ok(None) => false,
            Err(SpaceAccessError::CorruptedKeyMaterial) => {
                return Err(TryResumeSessionError::CorruptedKeyMaterial);
            }
            // NotInitialized and WrongPassphrase from load_kek map to
            // "keyring didn't give us what we needed to silently unlock".
            Err(SpaceAccessError::NotInitialized) | Err(SpaceAccessError::WrongPassphrase) => {
                return Err(TryResumeSessionError::KeyringMiss);
            }
            Err(other) => return Err(TryResumeSessionError::Internal(other.to_string())),
        };

        if resumed {
            self.device_management_reset
                .resume_legacy_isolation_if_required()
                .await
                .map_err(TryResumeSessionError::Internal)?;
            self.mobile_consumable_backfill.backfill_best_effort().await;
            self.ensure_relationship_storage_ready()
                .await
                .map_err(TryResumeSessionError::Internal)?;
        }

        Ok(resumed)
    }

    /// A1 · Create the encrypted space on a fresh device. On success the
    /// presence cache is primed (F1).
    #[instrument(skip_all)]
    pub async fn initialize_space(
        &self,
        input: InitializeSpaceInput,
    ) -> Result<InitializeSpaceResult, InitializeSpaceError> {
        let cmd: InitializeSpaceCommand = input.into();
        let out = self.initialize_space.execute(cmd).await?;
        self.ensure_relationship_storage_ready()
            .await
            .map_err(InitializeSpaceError::Internal)?;
        self.auto_prime_presence().await;
        Ok(out)
    }

    /// A2 · Unlock the encrypted space after a restart. On success the
    /// presence cache is primed (F1).
    #[instrument(skip_all)]
    pub async fn unlock_space(
        &self,
        input: UnlockSpaceInput,
    ) -> Result<UnlockSpaceResult, UnlockSpaceError> {
        let cmd: UnlockSpaceCommand = input.into();
        let out = self.unlock_space.execute(cmd).await?;

        self.device_management_reset
            .resume_legacy_isolation_if_required()
            .await
            .map_err(UnlockSpaceError::Internal)?;

        self.mobile_consumable_backfill.backfill_best_effort().await;

        self.ensure_relationship_storage_ready()
            .await
            .map_err(UnlockSpaceError::Internal)?;

        self.auto_prime_presence().await;
        Ok(out)
    }

    /// B1 · Ask the rendezvous service for a fresh invitation code and
    /// park the resulting aggregate in the application-layer holder.
    ///
    /// Does **not** auto-start the network: the adapter surfaces
    /// [`IssuePairingInvitationError::NetworkNotStarted`] if the runtime
    /// isn't up, letting the UI prompt the user to complete A1/A2 first.
    #[instrument(skip_all)]
    pub async fn issue_pairing_invitation(
        &self,
    ) -> Result<IssuePairingInvitationResult, IssuePairingInvitationError> {
        self.issue_pairing_invitation.execute().await
    }

    /// 按指定本机地址签发配对邀请。
    #[instrument(skip_all, fields(selected_ip = %selected_ip))]
    pub async fn issue_pairing_invitation_for_address(
        &self,
        selected_ip: IpAddr,
    ) -> Result<IssuePairingInvitationResult, IssuePairingInvitationError> {
        self.issue_pairing_invitation
            .execute_for_address(selected_ip)
            .await
    }

    /// 列出当前可用于签发配对邀请的本机地址。
    #[instrument(skip_all)]
    pub async fn list_pairing_invitation_addresses(
        &self,
    ) -> Result<Vec<PairingInvitationAddressCandidate>, IssuePairingInvitationError> {
        self.issue_pairing_invitation.list_addresses().await
    }

    /// B2 · Redeem a sponsor-issued invitation (joiner side).
    ///
    /// Primes presence before dialing because, unlike A1/A2, the joiner's
    /// entry point may be the first user action on this device (no prior
    /// `initialize_space` / `unlock_space` to have triggered F1). Prime
    /// failures are logged but not propagated — the subsequent dial will
    /// fail with [`RedeemPairingInvitationError::SponsorUnreachable`] /
    /// `ServiceUnavailable` if presence is genuinely unusable, which is
    /// the more actionable surface for the UI.
    #[instrument(skip_all)]
    pub async fn redeem_pairing_invitation(
        &self,
        input: RedeemPairingInvitationInput,
    ) -> Result<RedeemPairingInvitationResult, RedeemPairingInvitationError> {
        self.auto_prime_presence().await;
        let cmd: RedeemPairingInvitationCommand = input.into();
        self.redeem_pairing_invitation.execute(cmd).await
    }

    /// Slice4 P3 T3.2 · Cancel any in-flight pairing invitation parked
    /// in the in-memory holder.
    ///
    /// Maps to `POST /v2/setup/cancel`. Returns
    /// [`CancelInvitationError::NotIssued`] when the holder is empty so
    /// the daemon can surface HTTP 409 and the UI can distinguish
    /// "nothing to cancel" from a transport failure.
    ///
    /// Does **not** touch `SetupStatus` — only Pending invitation
    /// aggregates are cleared. The rendezvous server is **not**
    /// notified: stateless v2 model treats invitations as pure local
    /// state, and any joiner that races a redeem against this cancel
    /// will simply hit `take_matching → NotFound` on the sponsor side.
    #[instrument(skip_all)]
    pub async fn cancel_invitation(&self) -> Result<(), CancelInvitationError> {
        let removed = self.invitation_holder.cancel_all().await;
        if removed == 0 {
            return Err(CancelInvitationError::NotIssued);
        }
        info!(count = removed, "cancelled in-flight pairing invitations");
        Ok(())
    }

    /// Rebuild this profile as a single-device space while retaining local
    /// content, settings, identity, and unlock material.
    #[instrument(skip_all)]
    pub async fn reset(&self) -> Result<(), ResetSpaceError> {
        let dropped = self.invitation_holder.cancel_all().await;
        self.device_management_reset
            .execute_user_requested()
            .await?;
        info!(
            cancelled_invitations = dropped,
            "device management reset rebuilt the local space"
        );
        Ok(())
    }

    pub async fn has_committed_device_management_reset(&self) -> Result<bool, ResetSpaceError> {
        let pending_target = self
            .device_management_reset
            .setup_status
            .get_device_management_reset_target()
            .await
            .map_err(|error| ResetSpaceError::FinalizationFailed(error.to_string()))?;
        let status = self
            .device_management_reset
            .setup_status
            .get_status()
            .await
            .map_err(|error| ResetSpaceError::FinalizationFailed(error.to_string()))?;
        Ok(pending_target
            .as_ref()
            .is_some_and(|target| status.space_id.as_ref() == Some(target)))
    }

    /// User-driven "重置并重新开始" — wipe key material **and** clear setup
    /// status so a user who has forgotten their passphrase can re-run A1
    /// from a fresh-install state.
    ///
    /// Distinct from [`Self::reset`], which intentionally preserves the
    /// keyslot for operator-driven recovery: that path is no use to a user
    /// who can't recall the passphrase — `InitializeSpaceUseCase` would
    /// reject the next setup attempt with `AlreadyInitialized` because the
    /// keyslot is still on disk.
    ///
    /// Step order matters:
    ///
    /// 1. `FactoryResetSpacePort::factory_reset` — wipe keyslot + KEK first. If
    ///    this fails we leave `setup_status.has_completed = true` so the
    ///    UI still routes the user to `UnlockPage` (where they can retry)
    ///    rather than `SetupPage` (which would immediately fail with
    ///    `AlreadyInitialized` due to the residual keyslot).
    /// 2. Clear `SetupStatus` so `EncryptionFacade::state()` flips
    ///    `initialized = false` and the UI routes to `SetupPage`.
    /// 3. Clear persisted admission state — a crash mid-admission leaves a
    ///    durable "admission in progress" record that would otherwise block
    ///    every later pairing attempt.
    /// 4. Cancel any in-flight invitations — same hygiene as
    ///    [`Self::reset`].
    ///
    /// The `space_id` passed to the port is an opaque handle: the
    /// `SpaceAccessAdapter` keys off the current profile, not this value.
    /// We mint a fresh one rather than reading from `SetupStatus` because
    /// the use-case may run when `SetupStatus.space_id` is `None` (e.g.
    /// `setup_status` is partially populated from a prior abort).
    #[instrument(skip_all)]
    pub async fn factory_reset(&self) -> Result<(), FactoryResetError> {
        let space_id = SpaceId::new();
        self.factory_reset
            .factory_reset(&space_id)
            .await
            .map_err(|err| FactoryResetError::KeyMaterialWipeFailed(err.to_string()))?;
        self.clear_space_peer_state().await?;
        // A crash mid-admission leaves a durable "admission in progress" record.
        // The factory reset is the last-resort recovery path, so it must clear
        // that state too — otherwise every later pairing attempt fails with
        // `AdmissionInProgress` and the user is locked out of re-pairing.
        self.device_management_reset
            .clear_admission_state()
            .await
            .map_err(FactoryResetError::AdmissionStateResetFailed)?;
        self.setup_status
            .set_status(&SetupStatus::default())
            .await
            .map_err(|err| FactoryResetError::StorageFailed(err.to_string()))?;
        let dropped = self.invitation_holder.cancel_all().await;
        info!(
            cancelled_invitations = dropped,
            "factory reset wiped key material, cleared setup status, dropped invitations"
        );
        Ok(())
    }

    async fn clear_space_peer_state(&self) -> Result<(), FactoryResetError> {
        self.presence.disconnect_all().await;
        self.relationship_reset
            .clear_all_relationships()
            .await
            .map_err(|err| FactoryResetError::StorageFailed(err.to_string()))
    }

    /// Slice4 P3 T3.2 · Read-only snapshot of setup state for the
    /// stateless v2 UI flow.
    ///
    /// Maps to `GET /v2/setup/state`. Composes three independent
    /// reads into a single response so the UI doesn't have to
    /// orchestrate them itself:
    /// * `has_completed` from [`SetupStatusPort`].
    /// * `current_invitation` from the in-memory holder
    ///   (earliest-expiring Pending entry; `None` when the holder is
    ///   empty).
    /// * `device_name` from `Settings.general.device_name`.
    #[instrument(skip_all)]
    pub async fn query_setup_state(&self) -> Result<SetupStateView, QuerySetupStateError> {
        let status = self
            .setup_status
            .get_status()
            .await
            .map_err(|err| QuerySetupStateError::StorageFailed(err.to_string()))?;
        let current_invitation = self
            .invitation_holder
            .snapshot_earliest()
            .await
            .map(|(code, expires_at)| CurrentInvitation { code, expires_at });
        let settings = self
            .settings
            .load()
            .await
            .map_err(|err| QuerySetupStateError::StorageFailed(err.to_string()))?;
        Ok(SetupStateView {
            has_completed: status.has_completed,
            space_id: status.space_id,
            current_invitation,
            device_name: settings.general.device_name,
            re_pairing_required: status.re_pairing_required,
        })
    }

    /// Slice 2 Phase 1 · T10 · CLI `members` 入口:主动触发一轮
    /// `ensure_reachable_all`,把 `IrohPresenceAdapter` 的缓存刷新到最新,
    /// 然后 CLI 再调 `MemberRosterFacade::list_with_presence` 读缓存 →
    /// 查询结果天然满足"B 重启后 ≤ 10s 内显示 online"的验收条款。
    ///
    /// 与 F1 hook 里 `auto_prime_presence` 自动触发的那一轮的区别:本方法
    /// 暴露 `ensure_reachable_all` 使用例的结果,让 CLI 决定如何展示
    /// (fatal 错误 / 个别 peer 失败计数);F1 路径吞错只 warn。
    ///
    /// UseCase 本身保持 `pub(crate)`(§11.4),只通过本 facade thin wrapper
    /// 对外,后续 Tauri / GUI 也复用同一入口。
    #[instrument(skip_all)]
    pub async fn refresh_presence(
        &self,
    ) -> Result<EnsureReachableAllReport, EnsureReachableAllError> {
        self.ensure_reachable_all.execute().await
    }

    /// List `DeviceId`s of every effective paired peer (local and removed
    /// devices filtered out).
    ///
    /// Thin wrapper over `peer_addr_repo.list()` consumed by the desktop
    /// keepalive scheduler so its per-peer backoff state can discover new
    /// peers and prune removed ones each tick. Mirrors the effective target
    /// set `EnsureReachableAllUseCase` uses internally — peers without an addr
    /// blob are silently absent rather than surfaced as "no address" errors.
    pub async fn list_paired_peer_device_ids(
        &self,
    ) -> Result<Vec<DeviceId>, EnsureReachableAllError> {
        let records = self.peer_addr_repo.list().await.map_err(|err| {
            EnsureReachableAllError::Repository(format!("peer_addr_repo.list: {err}"))
        })?;
        let scope = self.peer_scope.snapshot().await.map_err(|error| {
            EnsureReachableAllError::Repository(format!("current peer scope: {error:?}"))
        })?;
        let mut peers = Vec::with_capacity(records.len());
        for record in records {
            if record.device_id == self.local_device_id
                || !scope.peer_device_ids.contains(&record.device_id)
            {
                continue;
            }
            peers.push(record.device_id);
        }
        Ok(peers)
    }

    pub(crate) async fn deliver_join_completion_ack(
        &self,
        pending: crate::space::convergence::PendingJoinerCompleteAck,
    ) -> Result<(), RedeemPairingInvitationError> {
        let address = self
            .peer_addr_repo
            .get(&pending.sponsor_device_id)
            .await
            .map_err(|error| {
                RedeemPairingInvitationError::Internal(format!(
                    "load admission continuation address: {error}"
                ))
            })?
            .ok_or(RedeemPairingInvitationError::SponsorUnreachable)?;
        let session = self
            .pairing_session
            .dial_admission_continuation(&address.addr_blob)
            .await
            .map_err(|error| {
                RedeemPairingInvitationError::Internal(format!(
                    "dial admission continuation: {error}"
                ))
            })?;
        let result = self
            .pairing_session
            .send(
                &session,
                uc_core::pairing::PairingSessionMessage::DurableAdmission(pending.frame),
            )
            .await;
        self.pairing_session.close(&session, None).await;
        result.map_err(|error| {
            RedeemPairingInvitationError::Internal(format!(
                "send admission completion acknowledgment: {error}"
            ))
        })
    }

    /// Ensure a single peer is reachable; thin wrapper over
    /// `PresencePort::ensure_reachable`.
    ///
    /// The keepalive scheduler calls this only for peers whose backoff has
    /// elapsed. `ensure_reachable` (not `verify_reachable`) is intentional:
    /// when our outbound `peers` map already holds an alive connection the
    /// fast-path returns Online without dialing — exactly what the
    /// scheduler wants to avoid burning UDP probes on healthy peers.
    pub async fn ensure_reachable_one(
        &self,
        device: &DeviceId,
    ) -> Result<ReachabilityState, PresenceError> {
        self.presence.ensure_reachable(device).await
    }

    /// F2 · Tear down facade-owned background work cleanly on app exit.
    ///
    /// Slice 4 P5c: 历史上还会调 `network_control.stop_network()`,libp2p 走
    /// 完后 iroh router 由 `SyncEngineAssembly::shutdown` 直接收口,本入口
    /// 现在只剩 abort 入站 pairing orchestrator——让它的 `subscribe` receiver
    /// 立刻 drop,底层 adapter 才能释放事件 channel。
    #[instrument(skip_all)]
    pub async fn on_shutdown(&self) {
        self.pairing_inbound_handle.abort();
    }

    /// Best-effort presence prime after a successful space-lifecycle action.
    /// Does not propagate errors: A1/A2 already committed the space mutation
    /// and rolling that back is worse than leaving presence stale.
    ///
    /// **Slice 2 Phase 1 · T8 · F1 hook**(P5c 改名 `auto_prime_presence`):
    /// 跑一次 `ensure_reachable_all` —— 对所有已知 paired peer 并发探测,
    /// 把 presence 缓存填满,让 UI 下一次 `list_with_presence` 就能拿到
    /// 正确的 online/offline 而不是全是 `Unknown`。预连失败不传给调用方:
    /// A1/A2/B2 的空间变更已经落盘,单个 peer 拨不通属正常情形,
    /// adapter 的 `Connection::closed` watchdog 会按正常流程 lazy 补齐。
    async fn auto_prime_presence(&self) {
        self.presence.activate().await;
        match self.ensure_reachable_all.execute().await {
            Ok(report) => {
                info!(
                    total = report.total,
                    online = report.online,
                    offline = report.offline,
                    errors = report.errors.len(),
                    "F1 ensure_reachable_all completed"
                );
            }
            Err(err) => {
                warn!(
                    error = %err,
                    "ensure_reachable_all failed; presence will recover lazily \
                     on next adapter probe"
                );
            }
        }
    }

    async fn ensure_relationship_storage_ready(&self) -> Result<(), String> {
        self.member_repo
            .list()
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    //! Thin smoke tests — the two use cases themselves are covered
    //! exhaustively in `usecases::setup::{initialize_space,unlock_space}`.
    //! Here we only prove that `SpaceFacade` wires them up and
    //! forwards arguments and error codes unchanged.

    use super::*;

    use std::sync::Mutex as StdMutex;

    use async_trait::async_trait;

    use chrono::{DateTime, Utc};

    use tokio::sync::mpsc;
    use uc_core::crypto::domain::{ActiveSpace, Passphrase};
    use uc_core::ids::{DeviceId, SpaceId};
    use uc_core::membership::{
        GroupEpoch, GroupRevocationPort, GroupRevocationResult, GroupUpdateDispatchError,
        GroupUpdateDispatchPort, KeyEpochError, MembershipError, PendingGroupUpdate,
        RelationshipStateResetError, RelationshipStateResetPort, RevocationId, SpaceMember,
        SpaceSecurityStateResetError, SpaceSecurityStateResetPort,
    };
    use uc_core::pairing::invitation::InvitationCode;
    use uc_core::pairing::PairingSessionMessage;
    use uc_core::ports::pairing::{
        DialError, DialOutcome, PairingEventPort, PairingSessionEvent, PairingSessionId,
        PairingSessionPort, SessionError,
    };
    use uc_core::ports::pairing_invitation::{
        CodeOrigin, ConsumeInvitationError, InvitationError, IssuedInvitation,
        PairingInvitationAddressQueryPort, PairingInvitationByAddressPort, PairingInvitationPort,
    };
    use uc_core::ports::space::{
        AdoptIsolatedSpacePort, DeriveAdmissionProofKeyPort, DeriveSpaceSubkeyPort,
        FactoryResetSpacePort, GroupAdmissionPort, InitializeSpacePort, IsSpaceUnlockedPort,
        LockSpacePort, PrepareAdmissionOfferPort, ProofPort, ResumeSpaceSessionPort,
        SpaceAccessError, UnlockSpacePort, VerifyKeychainAccessPort,
    };
    use uc_core::ports::{
        AppVersionStateError, AppVersionStatePort, ClockPort, DeviceIdentityPort,
        LocalIdentityError, LocalIdentityPort, SettingsPort, SetupStatusPort,
    };

    use crate::deps::SpaceAccessPorts;
    use crate::space::convergence::assembly::SpaceConvergenceAssembly;
    use crate::space::convergence::tests::MemoryWorkspaceRepository;
    use uc_core::security::IdentityFingerprint;
    use uc_core::settings::model::Settings;
    use uc_core::setup::SetupStatus;
    use uc_core::space_access::{
        AdmissionOffer, GroupAdmission, PreparedAdmissionOffer, PreparedGroupJoin, ProofDerivedKey,
        SpaceAccessProofArtifact,
    };

    use uc_core::trusted_peer::{TrustedPeer, TrustedPeerError, TrustedPeerRepositoryPort};
    use uc_core::SessionId;

    // ── mock ports ───────────────────────────────────────────────────────

    #[derive(Default)]
    struct InMemoryAppVersionState(StdMutex<Option<String>>);

    #[async_trait]
    impl AppVersionStatePort for InMemoryAppVersionState {
        async fn read(&self) -> Result<Option<String>, AppVersionStateError> {
            Ok(self.0.lock().unwrap().clone())
        }

        async fn write(&self, version: &str) -> Result<(), AppVersionStateError> {
            *self.0.lock().unwrap() = Some(version.to_owned());
            Ok(())
        }
    }

    mockall::mock! {
        SpaceAccess {}

        #[async_trait]
        impl InitializeSpacePort for SpaceAccess {
            async fn initialize(
                &self,
                space_id: &SpaceId,
                passphrase: &Passphrase,
            ) -> Result<ActiveSpace, SpaceAccessError>;
        }
        #[async_trait]
        impl UnlockSpacePort for SpaceAccess {
            async fn unlock(
                &self,
                space_id: &SpaceId,
                passphrase: &Passphrase,
            ) -> Result<ActiveSpace, SpaceAccessError>;
        }
        #[async_trait]
        impl IsSpaceUnlockedPort for SpaceAccess {
            async fn is_unlocked(&self, space_id: &SpaceId) -> bool;
        }
        #[async_trait]
        impl LockSpacePort for SpaceAccess {
            async fn lock(&self, space_id: &SpaceId) -> Result<(), SpaceAccessError>;
        }
        #[async_trait]
        impl FactoryResetSpacePort for SpaceAccess {
            async fn factory_reset(&self, space_id: &SpaceId) -> Result<(), SpaceAccessError>;
        }
        #[async_trait]
        impl ResumeSpaceSessionPort for SpaceAccess {
            async fn try_resume_session(
                &self,
                space_id: &SpaceId,
            ) -> Result<Option<ActiveSpace>, SpaceAccessError>;
        }
        #[async_trait]
        impl VerifyKeychainAccessPort for SpaceAccess {
            async fn verify_keychain_access(&self) -> Result<bool, SpaceAccessError>;
        }
        #[async_trait]
        impl DeriveSpaceSubkeyPort for SpaceAccess {
            async fn derive_subkey(
                &self,
                salt: &[u8],
                info: &[u8],
            ) -> Result<[u8; 32], SpaceAccessError>;
        }
        #[async_trait]
        impl PrepareAdmissionOfferPort for SpaceAccess {
            async fn prepare_admission_offer(
                &self,
                space_id: &SpaceId,
                invitation: &InvitationCode,
                pairing_session_id: &SessionId,
            ) -> Result<PreparedAdmissionOffer, SpaceAccessError>;
        }
        #[async_trait]
        impl DeriveAdmissionProofKeyPort for SpaceAccess {
            async fn derive_admission_proof_key(
                &self,
                offer: &AdmissionOffer,
                passphrase: &Passphrase,
                invitation: &InvitationCode,
                pairing_session_id: &SessionId,
            ) -> Result<ProofDerivedKey, SpaceAccessError>;
        }
        #[async_trait]
        impl uc_core::ports::space::PrepareAdmissionTargetAccessPort for SpaceAccess {
            async fn prepare_target_access(
                &self,
                target_space_id: &SpaceId,
                passphrase: &Passphrase,
            ) -> Result<uc_core::space_access::PreparedAdmissionTargetAccess, SpaceAccessError>;
        }
        #[async_trait]
        impl GroupAdmissionPort for SpaceAccess {
            async fn prepare_group_join(
                &self,
                device_id: &DeviceId,
            ) -> Result<PreparedGroupJoin, SpaceAccessError>;
            async fn admit_group_member(
                &self,
                space_id: &SpaceId,
                sponsor_device_id: &DeviceId,
                joiner_device_id: &DeviceId,
                existing_member_ids: &[DeviceId],
                key_package: &[u8],
            ) -> Result<GroupAdmission, SpaceAccessError>;
            async fn install_group_join(
                &self,
                space_id: &SpaceId,
                passphrase: &Passphrase,
                pending: PreparedGroupJoin,
                welcome: &[u8],
                encrypted_key_catalog: &[u8],
                group_epoch: u64,
            ) -> Result<(), SpaceAccessError>;
        }
        #[async_trait]
        impl uc_core::membership::PrepareSponsorAdmissionSecurityPort for SpaceAccess {
            async fn prepare_sponsor_admission_security(
                &self,
                request: uc_core::membership::SponsorAdmissionSecurityRequest,
            ) -> Result<
                uc_core::membership::SponsorPreparedAdmissionSecurity,
                uc_core::membership::AdmissionSecurityTransitionError,
            >;
        }
        #[async_trait]
        impl uc_core::membership::ActivateSponsorAdmissionSecurityPort for SpaceAccess {
            async fn activate_sponsor_admission_security(
                &self,
                request: uc_core::membership::ActivateSponsorAdmissionSecurityRequest,
            ) -> Result<(), uc_core::membership::AdmissionSecurityTransitionError>;
        }
        #[async_trait]
        impl uc_core::membership::ActivateCompletionHelperAdmissionSecurityPort for SpaceAccess {
            async fn activate_completion_helper_admission_security(
                &self,
                request: uc_core::membership::ActivateCompletionHelperAdmissionSecurityRequest,
            ) -> Result<(), uc_core::membership::AdmissionSecurityTransitionError>;
        }
        #[async_trait]
        impl uc_core::membership::GroupRevocationPort for SpaceAccess {
            async fn revoke_group_member(
                &self,
                target: &DeviceId,
                retained_recipients: &[DeviceId],
                now_ms: i64,
            ) -> Result<uc_core::membership::GroupRevocationResult, uc_core::membership::KeyEpochError>;
            async fn acknowledge_group_update(
                &self,
                revocation_id: &uc_core::membership::RevocationId,
                recipient: &DeviceId,
                now_ms: i64,
            ) -> Result<uc_core::membership::GroupRevocationResult, uc_core::membership::KeyEpochError>;
            async fn apply_group_epoch_update(
                &self,
                payload: &[u8],
            ) -> Result<uc_core::membership::GroupEpoch, uc_core::membership::KeyEpochError>;
            async fn pending_group_updates(
                &self,
                revocation_id: &uc_core::membership::RevocationId,
            ) -> Result<Vec<uc_core::membership::PendingGroupUpdate>, uc_core::membership::KeyEpochError>;
            async fn query_group_revocation(
                &self,
                revocation_id: &uc_core::membership::RevocationId,
            ) -> Result<Option<uc_core::membership::GroupRevocationResult>, uc_core::membership::KeyEpochError>;
            async fn resume_group_revocations(
                &self,
                now_ms: i64,
            ) -> Result<Vec<uc_core::membership::GroupRevocationResult>, uc_core::membership::KeyEpochError>;
            async fn pending_space_group_updates(
                &self,
            ) -> Result<Vec<uc_core::membership::PendingGroupUpdate>, uc_core::membership::KeyEpochError>;
            async fn acknowledge_space_group_update(
                &self,
                update_id: &str,
                now_ms: i64,
            ) -> Result<bool, uc_core::membership::KeyEpochError>;
        }
        #[async_trait]
        impl uc_core::membership::GroupBootstrapPort for SpaceAccess {
            async fn bootstrap_legacy_space(
                &self,
                sponsor: &DeviceId,
                retained_members: &[DeviceId],
                now_ms: i64,
            ) -> Result<uc_core::membership::GroupBootstrapResult, uc_core::membership::BootstrapError>;
            async fn acknowledge_legacy_readmission(
                &self,
                bootstrap_id: &uc_core::membership::BootstrapId,
                member: &DeviceId,
                now_ms: i64,
            ) -> Result<uc_core::membership::GroupBootstrapResult, uc_core::membership::BootstrapError>;
            async fn withdraw_legacy_readmission(
                &self,
                bootstrap_id: &uc_core::membership::BootstrapId,
                member: &DeviceId,
                now_ms: i64,
            ) -> Result<uc_core::membership::GroupBootstrapResult, uc_core::membership::BootstrapError>;
            async fn query_legacy_bootstrap(
                &self,
                bootstrap_id: &uc_core::membership::BootstrapId,
            ) -> Result<Option<uc_core::membership::GroupBootstrapResult>, uc_core::membership::BootstrapError>;
            async fn resume_legacy_bootstraps(
                &self,
                now_ms: i64,
            ) -> Result<Vec<uc_core::membership::GroupBootstrapResult>, uc_core::membership::BootstrapError>;
        }
        #[async_trait]
        impl uc_core::membership::SpaceProtectionStatusPort for SpaceAccess {
            async fn query_space_protection(
                &self,
                members: &[DeviceId],
            ) -> Result<uc_core::membership::SpaceProtectionSnapshot, uc_core::membership::SpaceProtectionError>;
        }
    }

    #[async_trait]
    impl AdoptIsolatedSpacePort for MockSpaceAccess {
        async fn adopt_isolated_space(
            &self,
            space_id: &SpaceId,
        ) -> Result<ActiveSpace, SpaceAccessError> {
            Ok(ActiveSpace::new(space_id.clone()))
        }
    }

    fn configured_space_access(
        unlock_error: Option<SpaceAccessError>,
        factory_reset_result: Option<Result<(), SpaceAccessError>>,
        expected_resume_spaces: Vec<SpaceId>,
        resume_success: bool,
    ) -> Arc<MockSpaceAccess> {
        let mut mock = MockSpaceAccess::new();
        mock.expect_initialize()
            .returning(|space_id, _| Ok(ActiveSpace::new(space_id.clone())));
        match unlock_error {
            Some(error) => {
                mock.expect_unlock()
                    .times(1)
                    .return_once(move |_, _| Err(error));
            }
            None => {
                mock.expect_unlock()
                    .returning(|space_id, _| Ok(ActiveSpace::new(space_id.clone())));
            }
        }
        mock.expect_is_unlocked().returning(|_| true);
        mock.expect_lock().returning(|_| Ok(()));
        match factory_reset_result {
            Some(result) => {
                mock.expect_factory_reset()
                    .times(1)
                    .return_once(move |_| result);
            }
            None => {
                mock.expect_factory_reset().returning(|_| Ok(()));
            }
        }
        match expected_resume_spaces.is_empty() {
            false => {
                let expected = expected_resume_spaces;
                let expected_count = expected.len();
                mock.expect_try_resume_session()
                    .withf(move |space_id| expected.contains(space_id))
                    .times(expected_count)
                    .returning(move |space_id| {
                        Ok(resume_success.then(|| ActiveSpace::new(space_id.clone())))
                    });
            }
            true => {
                mock.expect_try_resume_session().returning(|_| Ok(None));
            }
        }
        mock.expect_verify_keychain_access().returning(|| Ok(true));
        mock.expect_derive_subkey().returning(|_, _| Ok([0; 32]));
        Arc::new(mock)
    }

    fn space_access() -> Arc<MockSpaceAccess> {
        configured_space_access(None, None, Vec::new(), false)
    }

    struct FakeLocalIdentity {
        fp: IdentityFingerprint,
    }
    #[async_trait]
    impl LocalIdentityPort for FakeLocalIdentity {
        async fn create(&self) -> Result<IdentityFingerprint, LocalIdentityError> {
            Ok(self.fp.clone())
        }
        async fn ensure(&self) -> Result<IdentityFingerprint, LocalIdentityError> {
            Ok(self.fp.clone())
        }
        async fn get_current_fingerprint(
            &self,
        ) -> Result<Option<IdentityFingerprint>, LocalIdentityError> {
            Ok(Some(self.fp.clone()))
        }
    }

    struct FixedDeviceIdentity {
        id: DeviceId,
    }
    impl DeviceIdentityPort for FixedDeviceIdentity {
        fn current_device_id(&self) -> DeviceId {
            self.id.clone()
        }
    }

    #[derive(Default)]
    struct InMemoryMemberRepo {
        rows: StdMutex<Vec<SpaceMember>>,
    }
    #[async_trait]
    impl uc_core::membership::MemberRepositoryPort for InMemoryMemberRepo {
        async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .find(|m| &m.device_id == device_id)
                .cloned())
        }
        async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
            Ok(self.rows.lock().unwrap().clone())
        }
        async fn save(&self, member: &SpaceMember) -> Result<(), MembershipError> {
            self.rows.lock().unwrap().push(member.clone());
            Ok(())
        }
        async fn remove(&self, device_id: &DeviceId) -> Result<bool, MembershipError> {
            let mut rows = self.rows.lock().unwrap();
            let before = rows.len();
            rows.retain(|member| &member.device_id != device_id);
            Ok(rows.len() != before)
        }
    }

    struct SwitchingInMemoryMemberRepo {
        staged: Arc<AtomicBool>,
        active_rows: StdMutex<Vec<SpaceMember>>,
        working_rows: StdMutex<Vec<SpaceMember>>,
    }

    impl SwitchingInMemoryMemberRepo {
        fn rows(&self) -> &StdMutex<Vec<SpaceMember>> {
            if self.staged.load(Ordering::Acquire) {
                &self.working_rows
            } else {
                &self.active_rows
            }
        }
    }

    #[async_trait]
    impl uc_core::membership::MemberRepositoryPort for SwitchingInMemoryMemberRepo {
        async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            Ok(self
                .rows()
                .lock()
                .unwrap()
                .iter()
                .find(|member| &member.device_id == device_id)
                .cloned())
        }

        async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
            Ok(self.rows().lock().unwrap().clone())
        }

        async fn save(&self, member: &SpaceMember) -> Result<(), MembershipError> {
            self.rows().lock().unwrap().push(member.clone());
            Ok(())
        }

        async fn remove(&self, device_id: &DeviceId) -> Result<bool, MembershipError> {
            let mut rows = self.rows().lock().unwrap();
            let before = rows.len();
            rows.retain(|member| &member.device_id != device_id);
            Ok(rows.len() != before)
        }
    }

    struct UnreadableMemberRepo;

    #[async_trait]
    impl uc_core::membership::MemberRepositoryPort for UnreadableMemberRepo {
        async fn get(&self, _device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            Err(MembershipError::Repository(
                "relationship store unavailable".into(),
            ))
        }

        async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
            Err(MembershipError::Repository(
                "relationship store unavailable".into(),
            ))
        }

        async fn save(&self, _member: &SpaceMember) -> Result<(), MembershipError> {
            Err(MembershipError::Repository(
                "relationship store unavailable".into(),
            ))
        }

        async fn remove(&self, _device_id: &DeviceId) -> Result<bool, MembershipError> {
            Err(MembershipError::Repository(
                "relationship store unavailable".into(),
            ))
        }
    }

    struct NoopRelationshipStateReset;

    struct NoopDeviceManagementResetData;

    #[async_trait]
    impl DeviceManagementResetDataPort for NoopDeviceManagementResetData {
        async fn prepare_device_management_reset(
            &self,
            _target_space_id: &SpaceId,
        ) -> Result<(), uc_core::membership::AdmissionSpaceTransitionError> {
            Ok(())
        }

        async fn stage_device_management_reset_mutations(
            &self,
            _target_space_id: &SpaceId,
        ) -> Result<(), uc_core::membership::AdmissionSpaceTransitionError> {
            Ok(())
        }

        async fn promote_device_management_reset(
            &self,
            _target_space_id: &SpaceId,
        ) -> Result<(), uc_core::membership::AdmissionSpaceTransitionError> {
            Ok(())
        }

        async fn finalize_device_management_reset(
            &self,
            _target_space_id: &SpaceId,
        ) -> Result<(), uc_core::membership::AdmissionSpaceTransitionError> {
            Ok(())
        }
    }

    struct FailOnceDeviceManagementResetData {
        fail_promotion: AtomicBool,
        staged: Arc<AtomicBool>,
        prepared_targets: StdMutex<Vec<SpaceId>>,
    }

    #[async_trait]
    impl DeviceManagementResetDataPort for FailOnceDeviceManagementResetData {
        async fn prepare_device_management_reset(
            &self,
            target_space_id: &SpaceId,
        ) -> Result<(), uc_core::membership::AdmissionSpaceTransitionError> {
            self.prepared_targets
                .lock()
                .unwrap()
                .push(target_space_id.clone());
            Ok(())
        }

        async fn stage_device_management_reset_mutations(
            &self,
            _target_space_id: &SpaceId,
        ) -> Result<(), uc_core::membership::AdmissionSpaceTransitionError> {
            self.staged.store(true, Ordering::Release);
            Ok(())
        }

        async fn promote_device_management_reset(
            &self,
            _target_space_id: &SpaceId,
        ) -> Result<(), uc_core::membership::AdmissionSpaceTransitionError> {
            if self.fail_promotion.swap(false, Ordering::AcqRel) {
                self.staged.store(false, Ordering::Release);
                return Err(uc_core::membership::AdmissionSpaceTransitionError::storage(
                    "space transition storage failed",
                ));
            }
            Ok(())
        }

        async fn finalize_device_management_reset(
            &self,
            _target_space_id: &SpaceId,
        ) -> Result<(), uc_core::membership::AdmissionSpaceTransitionError> {
            Ok(())
        }
    }

    #[async_trait]
    impl RelationshipStateResetPort for NoopRelationshipStateReset {
        async fn clear_all_relationships(&self) -> Result<(), RelationshipStateResetError> {
            Ok(())
        }
    }

    struct NoopSpaceSecurityStateReset;

    #[async_trait]
    impl SpaceSecurityStateResetPort for NoopSpaceSecurityStateReset {
        async fn clear_space_security_state_except(
            &self,
            _active_space_id: &SpaceId,
        ) -> Result<(), SpaceSecurityStateResetError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct InMemorySetupStatus {
        status: StdMutex<SetupStatus>,
        device_management_reset_target: StdMutex<Option<SpaceId>>,
    }
    #[async_trait]
    impl SetupStatusPort for InMemorySetupStatus {
        async fn get_status(&self) -> anyhow::Result<SetupStatus> {
            Ok(self.status.lock().unwrap().clone())
        }
        async fn set_status(&self, status: &SetupStatus) -> anyhow::Result<()> {
            *self.status.lock().unwrap() = status.clone();
            Ok(())
        }
        async fn get_device_management_reset_target(&self) -> anyhow::Result<Option<SpaceId>> {
            Ok(self.device_management_reset_target.lock().unwrap().clone())
        }
        async fn set_device_management_reset_target(
            &self,
            space_id: &SpaceId,
        ) -> anyhow::Result<()> {
            *self.device_management_reset_target.lock().unwrap() = Some(space_id.clone());
            Ok(())
        }
        async fn clear_device_management_reset_target(&self) -> anyhow::Result<()> {
            *self.device_management_reset_target.lock().unwrap() = None;
            Ok(())
        }
    }

    #[derive(Default)]
    struct InMemorySettings {
        settings: StdMutex<Settings>,
    }
    #[async_trait]
    impl SettingsPort for InMemorySettings {
        async fn load(&self) -> anyhow::Result<Settings> {
            Ok(self.settings.lock().unwrap().clone())
        }
        async fn save(&self, settings: &Settings) -> anyhow::Result<()> {
            *self.settings.lock().unwrap() = settings.clone();
            Ok(())
        }
    }

    struct FixedClock(i64);
    impl ClockPort for FixedClock {
        fn now_ms(&self) -> i64 {
            self.0
        }
    }

    #[derive(Default)]
    struct FakeInvitationPort {
        calls: StdMutex<u32>,
        next_err: StdMutex<Option<InvitationError>>,
    }

    #[async_trait]
    impl PairingInvitationPort for FakeInvitationPort {
        async fn issue_invitation(&self) -> Result<IssuedInvitation, InvitationError> {
            *self.calls.lock().unwrap() += 1;
            if let Some(err) = self.next_err.lock().unwrap().take() {
                return Err(err);
            }
            Ok(IssuedInvitation {
                code: InvitationCode::new("SMOKE-0001"),
                expires_at: DateTime::parse_from_rfc3339("2026-04-20T10:05:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
                code_origin: CodeOrigin::DirectoryIssued,
            })
        }

        async fn consume_invitation(
            &self,
            _code: &InvitationCode,
        ) -> Result<(), ConsumeInvitationError> {
            // Smoke tests don't exercise P7e inbound path.
            Ok(())
        }
    }

    #[async_trait]
    impl PairingInvitationAddressQueryPort for FakeInvitationPort {
        async fn list_invitation_addresses(
            &self,
        ) -> Result<Vec<PairingInvitationAddressCandidate>, InvitationError> {
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl PairingInvitationByAddressPort for FakeInvitationPort {
        async fn issue_invitation_for_address(
            &self,
            _selected_ip: IpAddr,
        ) -> Result<IssuedInvitation, InvitationError> {
            self.issue_invitation().await
        }
    }

    /// Minimal fakes for the Slice 1 pairing session/event ports. The
    /// smoke tests here only verify A1/A2/B1 forwarding and shutdown side
    /// effects; inbound event handling is covered exhaustively in
    /// `pairing_inbound::orchestrator::tests`.
    #[derive(Default)]
    struct NoopSessionPort;

    #[async_trait]
    impl PairingSessionPort for NoopSessionPort {
        async fn dial_by_invitation(
            &self,
            _code: &uc_core::pairing::invitation::InvitationCode,
        ) -> Result<DialOutcome, DialError> {
            unreachable!("smoke tests never dial")
        }
        async fn send(
            &self,
            _session: &PairingSessionId,
            _message: PairingSessionMessage,
        ) -> Result<(), SessionError> {
            Ok(())
        }
        async fn recv_next(
            &self,
            _session: &PairingSessionId,
        ) -> Result<Option<PairingSessionMessage>, SessionError> {
            unreachable!("smoke tests never recv")
        }
        async fn close(&self, _session: &PairingSessionId, _reason: Option<String>) {}
    }

    /// Hands out a single empty receiver; the orchestrator will idle until
    /// the facade is dropped (and `on_shutdown` aborts the task).
    struct IdleEventPort {
        rx: StdMutex<Option<mpsc::Receiver<PairingSessionEvent>>>,
    }
    impl IdleEventPort {
        fn new() -> Self {
            let (_tx, rx) = mpsc::channel(1);
            // Drop the sender on purpose — the channel closes when the
            // receiver's `recv` is awaited. That's fine: the orchestrator's
            // run_loop exits cleanly on channel close.
            Self {
                rx: StdMutex::new(Some(rx)),
            }
        }
    }
    #[async_trait]
    impl PairingEventPort for IdleEventPort {
        async fn subscribe(&self) -> anyhow::Result<mpsc::Receiver<PairingSessionEvent>> {
            self.rx
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| anyhow::anyhow!("IdleEventPort already subscribed"))
        }
    }

    /// Smoke-test stub: proof verification is not exercised here —
    /// the inbound handshake flow is covered in
    /// `pairing_inbound::orchestrator::tests`.
    struct NoopProofPort;
    #[async_trait]
    impl ProofPort for NoopProofPort {
        async fn build_proof(
            &self,
            _pairing_session_id: &SessionId,
            _space_id: &SpaceId,
            _challenge_nonce: [u8; 32],
            _derived_key: &ProofDerivedKey,
        ) -> anyhow::Result<SpaceAccessProofArtifact> {
            unreachable!("smoke tests never drive verification")
        }
        async fn verify_proof(
            &self,
            _proof: &SpaceAccessProofArtifact,
            _expected_nonce: [u8; 32],
        ) -> anyhow::Result<bool> {
            unreachable!("smoke tests never drive verification")
        }
    }

    #[derive(Default)]
    struct NoopTrustedPeerRepo;
    #[async_trait]
    impl TrustedPeerRepositoryPort for NoopTrustedPeerRepo {
        async fn get(&self, _: &DeviceId) -> Result<Option<TrustedPeer>, TrustedPeerError> {
            Ok(None)
        }
        async fn list(&self) -> Result<Vec<TrustedPeer>, TrustedPeerError> {
            Ok(vec![])
        }
        async fn save(&self, _: &TrustedPeer) -> Result<(), TrustedPeerError> {
            Ok(())
        }
        async fn remove(&self, _: &DeviceId) -> Result<bool, TrustedPeerError> {
            Ok(false)
        }
    }

    #[derive(Default)]
    struct NoopGroupRevocation;
    #[async_trait]
    impl GroupRevocationPort for NoopGroupRevocation {
        async fn revoke_group_member(
            &self,
            _target: &DeviceId,
            _retained_recipients: &[DeviceId],
            _now_ms: i64,
        ) -> Result<uc_core::membership::GroupRevocationResult, KeyEpochError> {
            unreachable!("smoke tests never revoke")
        }
        async fn acknowledge_group_update(
            &self,
            _revocation_id: &RevocationId,
            _recipient: &DeviceId,
            _now_ms: i64,
        ) -> Result<uc_core::membership::GroupRevocationResult, KeyEpochError> {
            unreachable!("smoke tests never acknowledge")
        }
        async fn apply_group_epoch_update(
            &self,
            _payload: &[u8],
        ) -> Result<GroupEpoch, KeyEpochError> {
            unreachable!("smoke tests never apply updates")
        }
        async fn pending_group_updates(
            &self,
            _revocation_id: &RevocationId,
        ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError> {
            Ok(vec![])
        }
        async fn query_group_revocation(
            &self,
            _revocation_id: &RevocationId,
        ) -> Result<Option<GroupRevocationResult>, KeyEpochError> {
            Ok(None)
        }
        async fn resume_group_revocations(
            &self,
            _now_ms: i64,
        ) -> Result<Vec<GroupRevocationResult>, KeyEpochError> {
            Ok(vec![])
        }
        async fn pending_space_group_updates(
            &self,
        ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError> {
            Ok(vec![])
        }
        async fn acknowledge_space_group_update(
            &self,
            _update_id: &str,
            _now_ms: i64,
        ) -> Result<bool, KeyEpochError> {
            Ok(false)
        }
    }

    #[derive(Default)]
    struct NoopGroupUpdateDispatch;
    #[async_trait]
    impl GroupUpdateDispatchPort for NoopGroupUpdateDispatch {
        async fn dispatch_group_update(
            &self,
            _update: &PendingGroupUpdate,
        ) -> Result<(), GroupUpdateDispatchError> {
            Ok(())
        }
    }

    // Slice 2 Phase 1 · T5/T8 note:
    //
    // * T5:pairing 收尾点(orchestrator / redeem_invitation)会对 peer_addr_repo
    //   做 upsert——行为契约在各自的测试里覆盖,不在本文件。
    // * T8:F1 hook `auto_prime_presence` 在 A1/A2/B2 成功后会 unconditionally
    //   调 `peer_addr_repo.list()` 喂给 `EnsureReachableAllUseCase`。
    //
    // 因此本 helper 换成一个 FakePeerAddrRepo:`list()` 默认返回空 vec
    // (→ ensure_reachable_all 跑完一轮,不触发 presence.ensure_reachable),
    // 并记录 list() 调用次数让 F1 acceptance tests 断言"跑过一次"。
    // 其他 repo 方法保持 "unreachable!()" —— 本 smoke 测试集不该走它们。
    #[derive(Default)]
    struct FakePeerAddrRepo {
        list_calls: StdMutex<u32>,
    }
    impl FakePeerAddrRepo {
        fn list_calls(&self) -> u32 {
            *self.list_calls.lock().unwrap()
        }
    }
    #[async_trait]
    impl uc_core::ports::PeerAddressRepositoryPort for FakePeerAddrRepo {
        async fn get(
            &self,
            _device: &DeviceId,
        ) -> Result<Option<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError>
        {
            unreachable!("smoke tests don't read individual peer addresses")
        }
        async fn upsert(
            &self,
            _record: &uc_core::ports::PeerAddressRecord,
        ) -> Result<(), uc_core::ports::PeerAddressError> {
            unreachable!("pairing finalise covered in orchestrator tests, not here")
        }
        async fn list(
            &self,
        ) -> Result<Vec<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError>
        {
            *self.list_calls.lock().unwrap() += 1;
            Ok(vec![])
        }
        async fn remove(&self, _device: &DeviceId) -> Result<(), uc_core::ports::PeerAddressError> {
            unreachable!("removal covered in other suites")
        }
    }

    // T8:`ensure_reachable_all` 构造必须拿一个 `Arc<dyn PresencePort>`。
    // 本 smoke 集的 peer_addr_repo 始终返回空 vec,所以 `ensure_reachable`
    // 永远不会被触发;`current_state` / `subscribe` 也不走。3 个方法全
    // `unreachable!()` —— 若某测试路径意外调用到 presence,会立刻 panic
    // 而不是静默通过。
    struct FakePresence;
    #[async_trait]
    impl uc_core::ports::PresencePort for FakePresence {
        async fn ensure_reachable(
            &self,
            _device: &DeviceId,
        ) -> Result<uc_core::ports::ReachabilityState, uc_core::ports::PresenceError> {
            unreachable!("empty peer_addr_repo must keep ensure_reachable untouched")
        }
        async fn current_state(&self, _device: &DeviceId) -> uc_core::ports::ReachabilityState {
            unreachable!("current_state is the roster facade's path, not this one")
        }
        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<uc_core::ports::PresenceEvent> {
            unreachable!("subscribe is the roster facade's path, not this one")
        }
    }

    fn default_fingerprint() -> IdentityFingerprint {
        IdentityFingerprint::from_raw_string("ABCDEFGHIJKLMNOP").unwrap()
    }

    use crate::test_support::{CountingMobileConsumableBackfill, NoopMobileConsumableBackfill};

    fn make_facade(
        space_access: Arc<MockSpaceAccess>,
        setup_status: Arc<dyn SetupStatusPort>,
        settings: Arc<dyn SettingsPort>,
    ) -> (SpaceFacade, Arc<FakeInvitationPort>, Arc<FakePeerAddrRepo>) {
        make_facade_with(
            space_access,
            setup_status,
            settings,
            Arc::new(NoopMobileConsumableBackfill),
        )
    }

    fn make_facade_with(
        space_access: Arc<MockSpaceAccess>,
        setup_status: Arc<dyn SetupStatusPort>,
        settings: Arc<dyn SettingsPort>,
        mobile_consumable_backfill: Arc<dyn MobileConsumableBackfill>,
    ) -> (SpaceFacade, Arc<FakeInvitationPort>, Arc<FakePeerAddrRepo>) {
        make_facade_with_member_repo(
            space_access,
            setup_status,
            settings,
            mobile_consumable_backfill,
            Arc::new(InMemoryMemberRepo::default()),
        )
    }

    fn make_facade_with_member_repo(
        space_access: Arc<MockSpaceAccess>,
        setup_status: Arc<dyn SetupStatusPort>,
        settings: Arc<dyn SettingsPort>,
        mobile_consumable_backfill: Arc<dyn MobileConsumableBackfill>,
        member_repo: Arc<dyn MemberRepositoryPort>,
    ) -> (SpaceFacade, Arc<FakeInvitationPort>, Arc<FakePeerAddrRepo>) {
        make_facade_with_member_repo_and_isolation(
            space_access,
            setup_status,
            settings,
            mobile_consumable_backfill,
            member_repo,
            false,
        )
    }

    fn make_facade_with_member_repo_and_isolation(
        space_access: Arc<MockSpaceAccess>,
        setup_status: Arc<dyn SetupStatusPort>,
        settings: Arc<dyn SettingsPort>,
        mobile_consumable_backfill: Arc<dyn MobileConsumableBackfill>,
        member_repo: Arc<dyn MemberRepositoryPort>,
        legacy_profile_isolation_required: bool,
    ) -> (SpaceFacade, Arc<FakeInvitationPort>, Arc<FakePeerAddrRepo>) {
        make_facade_with_member_repo_isolation_and_reset_data(
            space_access,
            setup_status,
            settings,
            mobile_consumable_backfill,
            member_repo,
            legacy_profile_isolation_required,
            Arc::new(NoopDeviceManagementResetData),
        )
    }

    fn make_facade_with_member_repo_isolation_and_reset_data(
        space_access: Arc<MockSpaceAccess>,
        setup_status: Arc<dyn SetupStatusPort>,
        settings: Arc<dyn SettingsPort>,
        mobile_consumable_backfill: Arc<dyn MobileConsumableBackfill>,
        member_repo: Arc<dyn MemberRepositoryPort>,
        legacy_profile_isolation_required: bool,
        reset_data: Arc<dyn DeviceManagementResetDataPort>,
    ) -> (SpaceFacade, Arc<FakeInvitationPort>, Arc<FakePeerAddrRepo>) {
        let pairing_invitation = Arc::new(FakeInvitationPort::default());
        let peer_addr_repo = Arc::new(FakePeerAddrRepo::default());
        let facade = SpaceFacade::new(SpaceFacadeDeps {
            session: SpaceSessionDeps {
                space_access: SpaceAccessPorts::from_adapter(space_access),
                setup_status,
                mobile_consumable_backfill,
                legacy_profile_isolation_required,
                app_version_state: Arc::new(InMemoryAppVersionState::default()),
                current_app_version: "1.1.0".to_owned(),
            },
            admission: SpaceAdmissionDeps {
                local_identity: Arc::new(FakeLocalIdentity {
                    fp: default_fingerprint(),
                }),
                device_identity: Arc::new(FixedDeviceIdentity {
                    id: DeviceId::new("device-1"),
                }),
                member_repo,
                settings,
                clock: Arc::new(FixedClock(0)),
                pairing_invitation: pairing_invitation.clone(),
                pairing_invitation_addresses: pairing_invitation.clone(),
                pairing_invitation_by_address: pairing_invitation.clone(),
                pairing_session: Arc::new(NoopSessionPort),
                pairing_events: Arc::new(IdleEventPort::new()),
                proof_port: Arc::new(NoopProofPort),
                trusted_peer_repo: Arc::new(NoopTrustedPeerRepo),
                peer_addr_repo: Arc::clone(&peer_addr_repo)
                    as Arc<dyn uc_core::ports::PeerAddressRepositoryPort>,
                presence: Arc::new(FakePresence),
                analytics: Arc::new(uc_observability_contract::analytics::NoopAnalyticsFacade),
                convergence: Arc::new(SpaceConvergenceAssembly::new(
                    crate::space::convergence::assembly::SpaceConvergenceDeps {
                        workspace: crate::space::convergence::tests::test_deps(
                            Arc::new(MemoryWorkspaceRepository::default()),
                            "device-1",
                            Vec::new(),
                        ),
                        membership: crate::space::convergence::discovery::testing::test_deps(),
                        group_revocation: Arc::new(NoopGroupRevocation),
                        group_update_dispatch: Arc::new(NoopGroupUpdateDispatch),
                    },
                )),
            },
            transition: SpaceTransitionDeps {
                device_management_reset_data: reset_data,
                relationship_reset: Arc::new(NoopRelationshipStateReset),
                space_security_reset: Arc::new(NoopSpaceSecurityStateReset),
            },
        });
        (facade, pairing_invitation, peer_addr_repo)
    }

    fn settings_with_device_name(name: &str) -> Arc<InMemorySettings> {
        let holder = InMemorySettings::default();
        {
            let mut s = holder.settings.lock().unwrap();
            s.general.device_name = Some(name.to_string());
        }
        Arc::new(holder)
    }

    // ── tests ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn initialize_space_forwards_happy_path() {
        let (facade, _inv, _peer) = make_facade(
            space_access(),
            Arc::new(InMemorySetupStatus::default()),
            settings_with_device_name("mac"),
        );
        let cmd = InitializeSpaceInput {
            passphrase: "hunter22hunter22".to_string(),
            passphrase_confirm: "hunter22hunter22".to_string(),
            device_name: None,
        };
        let out = facade.initialize_space(cmd).await.expect("A1 ok");
        assert_eq!(out.fingerprint, default_fingerprint());
    }

    #[tokio::test]
    async fn initialize_space_forwards_passphrase_mismatch() {
        let (facade, _inv, _peer) = make_facade(
            space_access(),
            Arc::new(InMemorySetupStatus::default()),
            settings_with_device_name("mac"),
        );
        let cmd = InitializeSpaceInput {
            passphrase: "hunter22hunter22".to_string(),
            passphrase_confirm: "different22else2".to_string(),
            device_name: None,
        };
        let err = facade.initialize_space(cmd).await.unwrap_err();
        assert!(matches!(err, InitializeSpaceError::PassphraseMismatch));
    }

    #[tokio::test]
    async fn unlock_space_forwards_happy_path() {
        let setup_status = InMemorySetupStatus::default();
        *setup_status.status.lock().unwrap() = SetupStatus {
            has_completed: true,
            space_id: None,
            re_pairing_required: false,
        };
        let backfill = Arc::new(CountingMobileConsumableBackfill::default());
        let (facade, _inv, _peer) = make_facade_with(
            space_access(),
            Arc::new(setup_status),
            Arc::new(InMemorySettings::default()),
            backfill.clone(),
        );
        let cmd = UnlockSpaceInput {
            passphrase: "hunter22hunter22".to_string(),
        };
        facade.unlock_space(cmd).await.expect("A2 ok");
        assert_eq!(backfill.calls(), 1);
    }

    #[tokio::test]
    async fn unlock_space_forwards_setup_not_completed() {
        let (facade, _inv, _peer) = make_facade(
            space_access(),
            Arc::new(InMemorySetupStatus::default()),
            Arc::new(InMemorySettings::default()),
        );
        let cmd = UnlockSpaceInput {
            passphrase: "hunter22hunter22".to_string(),
        };
        let err = facade.unlock_space(cmd).await.unwrap_err();
        assert!(matches!(err, UnlockSpaceError::SetupNotCompleted));
    }

    #[tokio::test]
    async fn unlock_space_forwards_wrong_passphrase() {
        let setup_status = InMemorySetupStatus::default();
        *setup_status.status.lock().unwrap() = SetupStatus {
            has_completed: true,
            space_id: None,
            re_pairing_required: false,
        };
        let space_access = configured_space_access(
            Some(SpaceAccessError::WrongPassphrase),
            None,
            Vec::new(),
            false,
        );
        let (facade, _inv, _peer) = make_facade(
            space_access,
            Arc::new(setup_status),
            Arc::new(InMemorySettings::default()),
        );
        let cmd = UnlockSpaceInput {
            passphrase: "hunter22hunter22".to_string(),
        };
        let err = facade.unlock_space(cmd).await.unwrap_err();
        assert!(matches!(err, UnlockSpaceError::WrongPassphrase));
    }

    // ── F2 shutdown ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn on_shutdown_completes_without_panicking() {
        // Slice 4 P5c: F2 hook 不再调 stop_network(NetworkControlPort 已退役),
        // 这里只确认 abort 入站 orchestrator 后 facade 能正常清理,不 panic、
        // 不阻塞。
        let (facade, _inv, _peer) = make_facade(
            space_access(),
            Arc::new(InMemorySetupStatus::default()),
            Arc::new(InMemorySettings::default()),
        );
        facade.on_shutdown().await;
    }

    // ── T8 · F1 hook: auto_prime_presence triggers ensure_reachable_all ─
    //
    // 契约(plan §7.1 验收点):
    // * A1 / A2 / B2 成功 → auto_prime_presence → ensure_reachable_all 跑一次
    //   (以 peer_addr_repo.list() 被调计数代理——空 repo 路径下也跑过 list)
    // * ensure_reachable_all 失败 → A1/A2 结果不受影响(本集下用空 repo,
    //   ensure_reachable_all 不会失败,只验证"跑过")

    #[tokio::test]
    async fn f1_hook_initialize_space_success_triggers_ensure_reachable_all() {
        let (facade, _inv, peer) = make_facade(
            space_access(),
            Arc::new(InMemorySetupStatus::default()),
            settings_with_device_name("mac"),
        );
        let cmd = InitializeSpaceInput {
            passphrase: "hunter22hunter22".to_string(),
            passphrase_confirm: "hunter22hunter22".to_string(),
            device_name: None,
        };
        facade.initialize_space(cmd).await.expect("A1 ok");
        assert_eq!(
            peer.list_calls(),
            1,
            "A1 success must trigger ensure_reachable_all (list invoked once)",
        );
    }

    #[tokio::test]
    async fn f1_hook_unlock_space_success_triggers_ensure_reachable_all() {
        let setup_status = InMemorySetupStatus::default();
        *setup_status.status.lock().unwrap() = SetupStatus {
            has_completed: true,
            space_id: None,
            re_pairing_required: false,
        };
        let (facade, _inv, peer) = make_facade(
            space_access(),
            Arc::new(setup_status),
            Arc::new(InMemorySettings::default()),
        );
        let cmd = UnlockSpaceInput {
            passphrase: "hunter22hunter22".to_string(),
        };
        facade.unlock_space(cmd).await.expect("A2 ok");
        assert_eq!(
            peer.list_calls(),
            1,
            "A2 success must trigger ensure_reachable_all",
        );
    }

    #[tokio::test]
    async fn unlock_fails_when_relationship_storage_is_unreadable() {
        let setup_status = InMemorySetupStatus::default();
        *setup_status.status.lock().unwrap() = SetupStatus {
            has_completed: true,
            space_id: None,
            re_pairing_required: false,
        };
        let (facade, _inv, peer) = make_facade_with_member_repo(
            space_access(),
            Arc::new(setup_status),
            Arc::new(InMemorySettings::default()),
            Arc::new(NoopMobileConsumableBackfill),
            Arc::new(UnreadableMemberRepo),
        );

        let error = facade
            .unlock_space(UnlockSpaceInput {
                passphrase: "hunter22hunter22".to_string(),
            })
            .await
            .unwrap_err();

        assert!(matches!(error, UnlockSpaceError::Internal(_)));
        assert_eq!(peer.list_calls(), 0);
    }

    #[tokio::test]
    async fn f1_hook_skipped_when_lifecycle_action_fails() {
        // A1 失败(passphrase mismatch)→ 不跑 ensure_reachable_all。
        // 验证 guard 顺序正确(失败短路在 prime 之前)。
        let (facade, _inv, peer) = make_facade(
            space_access(),
            Arc::new(InMemorySetupStatus::default()),
            settings_with_device_name("mac"),
        );
        let cmd = InitializeSpaceInput {
            passphrase: "hunter22hunter22".to_string(),
            passphrase_confirm: "different22else2".to_string(),
            device_name: None,
        };
        let _ = facade.initialize_space(cmd).await.unwrap_err();
        assert_eq!(peer.list_calls(), 0);
    }

    // ── B1 · issue pairing invitation wiring ─────────────────────────────

    #[tokio::test]
    async fn issue_pairing_invitation_forwards_happy_path() {
        let (facade, inv, _peer) = make_facade(
            space_access(),
            Arc::new(InMemorySetupStatus::default()),
            Arc::new(InMemorySettings::default()),
        );
        let out = facade.issue_pairing_invitation().await.expect("B1 ok");
        assert_eq!(out.code.as_str(), "SMOKE-0001");
        assert_eq!(*inv.calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn issue_pairing_invitation_forwards_network_not_started() {
        let (facade, inv, _peer) = make_facade(
            space_access(),
            Arc::new(InMemorySetupStatus::default()),
            Arc::new(InMemorySettings::default()),
        );
        *inv.next_err.lock().unwrap() = Some(InvitationError::NetworkNotStarted);
        let err = facade.issue_pairing_invitation().await.unwrap_err();
        assert!(matches!(
            err,
            IssuePairingInvitationError::NetworkNotStarted
        ));
    }

    // ── Slice4 P3 T3.2 · cancel / reset / query_setup_state ────────────

    #[tokio::test]
    async fn cancel_invitation_returns_not_issued_when_holder_empty() {
        let (facade, _inv, _peer) = make_facade(
            space_access(),
            Arc::new(InMemorySetupStatus::default()),
            Arc::new(InMemorySettings::default()),
        );
        let err = facade.cancel_invitation().await.unwrap_err();
        assert!(matches!(err, CancelInvitationError::NotIssued));
    }

    #[tokio::test]
    async fn cancel_invitation_clears_pending_after_issue() {
        let (facade, _inv, _peer) = make_facade(
            space_access(),
            Arc::new(InMemorySetupStatus::default()),
            Arc::new(InMemorySettings::default()),
        );
        facade.issue_pairing_invitation().await.expect("B1 ok");
        assert_eq!(facade.invitation_holder.len().await, 1);
        facade.cancel_invitation().await.expect("cancel ok");
        assert_eq!(facade.invitation_holder.len().await, 0);
    }

    #[tokio::test]
    async fn reset_rebuilds_a_single_device_space_and_clears_invitations() {
        let setup_status = Arc::new(InMemorySetupStatus::default());
        *setup_status.status.lock().unwrap() = SetupStatus {
            has_completed: true,
            space_id: Some(SpaceId::from("old-space")),
            re_pairing_required: false,
        };
        let member_repo = Arc::new(InMemoryMemberRepo::default());
        member_repo.rows.lock().unwrap().push(SpaceMember {
            device_id: DeviceId::new("old-peer"),
            device_name: "Old peer".to_owned(),
            identity_fingerprint: default_fingerprint(),
            joined_at: Utc::now(),
            sync_preferences: MemberSyncPreferences::default(),
        });
        let (facade, _inv, _peer) = make_facade_with_member_repo(
            space_access(),
            setup_status,
            settings_with_device_name("Current device"),
            Arc::new(NoopMobileConsumableBackfill),
            member_repo.clone(),
        );
        facade.issue_pairing_invitation().await.expect("B1 ok");
        assert_eq!(facade.invitation_holder.len().await, 1);

        facade.reset().await.expect("reset ok");

        assert_eq!(facade.invitation_holder.len().await, 0);
        let view = facade.query_setup_state().await.expect("query ok");
        assert!(view.has_completed);
        assert_ne!(view.space_id.as_ref().map(AsRef::as_ref), Some("old-space"));
        assert!(view.re_pairing_required);
        assert!(view.current_invitation.is_none());
        assert_eq!(
            member_repo
                .list()
                .await
                .unwrap()
                .into_iter()
                .map(|member| member.device_id)
                .collect::<Vec<_>>(),
            vec![DeviceId::new("device-1")]
        );
    }

    #[tokio::test]
    async fn repeated_reset_reuses_the_completed_device_management_reset() {
        let setup_status = Arc::new(InMemorySetupStatus::default());
        *setup_status.status.lock().unwrap() = SetupStatus {
            has_completed: true,
            space_id: Some(SpaceId::from("old-space")),
            re_pairing_required: false,
        };
        let (facade, _inv, _peer) = make_facade(
            space_access(),
            setup_status,
            settings_with_device_name("Current device"),
        );

        facade.reset().await.expect("first reset");
        let first = facade.query_setup_state().await.expect("first state");
        facade.reset().await.expect("repeated reset");
        let repeated = facade.query_setup_state().await.expect("repeated state");

        assert_eq!(repeated.space_id, first.space_id);
        assert!(repeated.re_pairing_required);
    }

    #[tokio::test]
    async fn interrupted_reset_reuses_its_persisted_target() {
        let setup_status = Arc::new(InMemorySetupStatus::default());
        *setup_status.status.lock().unwrap() = SetupStatus {
            has_completed: true,
            space_id: Some(SpaceId::from("old-space")),
            re_pairing_required: false,
        };
        let reset_data = Arc::new(FailOnceDeviceManagementResetData {
            fail_promotion: AtomicBool::new(true),
            staged: Arc::new(AtomicBool::new(false)),
            prepared_targets: StdMutex::new(Vec::new()),
        });
        let (facade, _inv, _peer) = make_facade_with_member_repo_isolation_and_reset_data(
            space_access(),
            setup_status.clone(),
            settings_with_device_name("Current device"),
            Arc::new(NoopMobileConsumableBackfill),
            Arc::new(InMemoryMemberRepo::default()),
            false,
            reset_data.clone(),
        );

        let error = facade.reset().await.expect_err("promotion fails once");
        assert_eq!(
            error.to_string(),
            "failed to commit device management reset: space transition storage failed"
        );
        let pending_target = setup_status
            .device_management_reset_target
            .lock()
            .unwrap()
            .clone()
            .expect("reset target remains durable");
        facade.reset().await.expect("retry reset");

        let prepared_targets = reset_data.prepared_targets.lock().unwrap().clone();
        assert_eq!(
            prepared_targets,
            vec![pending_target.clone(), pending_target.clone()]
        );
        let completed = facade.query_setup_state().await.expect("completed state");
        assert_eq!(completed.space_id, Some(pending_target));
        assert!(completed.re_pairing_required);
        assert!(setup_status
            .device_management_reset_target
            .lock()
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn failed_reset_does_not_remove_members_from_the_active_space() {
        let setup_status = Arc::new(InMemorySetupStatus::default());
        *setup_status.status.lock().unwrap() = SetupStatus {
            has_completed: true,
            space_id: Some(SpaceId::from("old-space")),
            re_pairing_required: false,
        };
        let staged = Arc::new(AtomicBool::new(false));
        let old_member = SpaceMember {
            device_id: DeviceId::new("old-peer"),
            device_name: "Old peer".to_owned(),
            identity_fingerprint: default_fingerprint(),
            joined_at: Utc::now(),
            sync_preferences: MemberSyncPreferences::default(),
        };
        let member_repo = Arc::new(SwitchingInMemoryMemberRepo {
            staged: Arc::clone(&staged),
            active_rows: StdMutex::new(vec![old_member.clone()]),
            working_rows: StdMutex::new(vec![old_member]),
        });
        let reset_data = Arc::new(FailOnceDeviceManagementResetData {
            fail_promotion: AtomicBool::new(true),
            staged,
            prepared_targets: StdMutex::new(Vec::new()),
        });
        let (facade, _inv, _peer) = make_facade_with_member_repo_isolation_and_reset_data(
            space_access(),
            setup_status.clone(),
            settings_with_device_name("Current device"),
            Arc::new(NoopMobileConsumableBackfill),
            member_repo.clone(),
            false,
            reset_data,
        );

        assert!(facade.reset().await.is_err());

        assert_eq!(
            setup_status.status.lock().unwrap().space_id.as_ref(),
            Some(&SpaceId::from("old-space"))
        );
        assert_eq!(
            member_repo
                .list()
                .await
                .unwrap()
                .into_iter()
                .map(|member| member.device_id)
                .collect::<Vec<_>>(),
            vec![DeviceId::new("old-peer")]
        );
    }

    #[tokio::test]
    async fn committed_reset_finishes_status_without_rebuilding_the_target_space() {
        let target = SpaceId::from("committed-target");
        let setup_status = Arc::new(InMemorySetupStatus::default());
        *setup_status.status.lock().unwrap() = SetupStatus {
            has_completed: true,
            space_id: Some(target.clone()),
            re_pairing_required: true,
        };
        *setup_status.device_management_reset_target.lock().unwrap() = Some(target.clone());
        let (facade, _inv, _peer) = make_facade_with_member_repo_isolation_and_reset_data(
            space_access(),
            setup_status.clone(),
            settings_with_device_name("Current device"),
            Arc::new(NoopMobileConsumableBackfill),
            Arc::new(UnreadableMemberRepo),
            false,
            Arc::new(NoopDeviceManagementResetData),
        );

        facade.reset().await.expect("finish committed reset");

        assert!(setup_status
            .device_management_reset_target
            .lock()
            .unwrap()
            .is_none());
        assert_eq!(
            setup_status.status.lock().unwrap().space_id.as_ref(),
            Some(&target)
        );
    }

    #[tokio::test]
    async fn factory_reset_wipes_key_material_and_clears_setup_status() {
        let setup_status = InMemorySetupStatus::default();
        *setup_status.status.lock().unwrap() = SetupStatus {
            has_completed: true,
            space_id: None,
            re_pairing_required: false,
        };
        let space_access = configured_space_access(None, Some(Ok(())), Vec::new(), false);
        let (facade, _inv, _peer) = make_facade(
            space_access.clone(),
            Arc::new(setup_status),
            Arc::new(InMemorySettings::default()),
        );
        facade.issue_pairing_invitation().await.expect("B1 ok");
        assert_eq!(facade.invitation_holder.len().await, 1);

        facade.factory_reset().await.expect("factory_reset ok");

        assert_eq!(facade.invitation_holder.len().await, 0);
        let view = facade.query_setup_state().await.expect("query ok");
        assert!(!view.has_completed);
    }

    #[tokio::test]
    async fn factory_reset_preserves_setup_status_when_key_wipe_fails() {
        // 关键不变式: keyslot 删除失败时 setup_status 必须保留 `has_completed=true`,
        // 否则 UI 会跳到 SetupPage,用户再走 init 立即撞到 AlreadyInitialized,体验更糟。
        let setup_status = InMemorySetupStatus::default();
        *setup_status.status.lock().unwrap() = SetupStatus {
            has_completed: true,
            space_id: None,
            re_pairing_required: false,
        };
        let space_access = configured_space_access(
            None,
            Some(Err(SpaceAccessError::Internal("disk i/o".to_string()))),
            Vec::new(),
            false,
        );
        let (facade, _inv, _peer) = make_facade(
            space_access.clone(),
            Arc::new(setup_status),
            Arc::new(InMemorySettings::default()),
        );

        let err = facade.factory_reset().await.unwrap_err();

        assert!(matches!(err, FactoryResetError::KeyMaterialWipeFailed(_)));
        let view = facade.query_setup_state().await.expect("query ok");
        assert!(
            view.has_completed,
            "setup_status must remain completed when key wipe fails"
        );
    }

    #[tokio::test]
    async fn query_setup_state_reports_fresh_install_defaults() {
        let (facade, _inv, _peer) = make_facade(
            space_access(),
            Arc::new(InMemorySetupStatus::default()),
            Arc::new(InMemorySettings::default()),
        );
        let view = facade.query_setup_state().await.expect("query ok");
        assert!(!view.has_completed);
        assert!(view.current_invitation.is_none());
        assert!(view.device_name.is_none());
    }

    #[tokio::test]
    async fn query_setup_state_reflects_completed_status_and_device_name() {
        let setup_status = InMemorySetupStatus::default();
        *setup_status.status.lock().unwrap() = SetupStatus {
            has_completed: true,
            space_id: Some(SpaceId::from("space-restore")),
            re_pairing_required: false,
        };
        let (facade, _inv, _peer) = make_facade(
            space_access(),
            Arc::new(setup_status),
            settings_with_device_name("MacBook"),
        );
        let view = facade.query_setup_state().await.expect("query ok");
        assert!(view.has_completed);
        assert_eq!(
            view.space_id.as_ref().map(AsRef::as_ref),
            Some("space-restore")
        );
        assert_eq!(view.device_name.as_deref(), Some("MacBook"));
        assert!(view.current_invitation.is_none());
    }

    #[tokio::test]
    async fn query_setup_state_surfaces_pending_invitation_after_issue() {
        let (facade, _inv, _peer) = make_facade(
            space_access(),
            Arc::new(InMemorySetupStatus::default()),
            Arc::new(InMemorySettings::default()),
        );
        facade.issue_pairing_invitation().await.expect("B1 ok");
        let view = facade.query_setup_state().await.expect("query ok");
        let inv = view.current_invitation.expect("invitation present");
        assert_eq!(inv.code.as_str(), "SMOKE-0001");
        assert_eq!(
            inv.expires_at,
            DateTime::parse_from_rfc3339("2026-04-20T10:05:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[tokio::test]
    async fn issue_pairing_invitation_does_not_prime_presence() {
        // B1 不是 space-lifecycle 动作,不应触发 auto_prime_presence
        // (presence 缓存只该被 A1 / A2 / B2 触动,B1 出码不涉及与对端互联)。
        let (facade, _inv, peer) = make_facade(
            space_access(),
            Arc::new(InMemorySetupStatus::default()),
            Arc::new(InMemorySettings::default()),
        );
        facade.issue_pairing_invitation().await.expect("B1 ok");
        assert_eq!(
            peer.list_calls(),
            0,
            "B1 must not trigger ensure_reachable_all",
        );
    }

    #[tokio::test]
    async fn try_resume_session_resumes_silent_unlock() {
        // helper 默认返回 Ok(None)，模拟没有
        // keyslot 的场景——`try_resume_session` 应返回 Ok(false)。
        let setup_status = InMemorySetupStatus::default();
        *setup_status.status.lock().unwrap() = SetupStatus {
            has_completed: true,
            space_id: None,
            re_pairing_required: false,
        };
        let (facade, _inv, _peer) = make_facade(
            space_access(),
            Arc::new(setup_status),
            Arc::new(InMemorySettings::default()),
        );
        let resumed = facade.try_resume_session().await.expect("resume ok");
        // helper 默认 Ok(None) → "nothing to resume"
        assert!(!resumed);
    }

    #[tokio::test]
    async fn try_resume_session_uses_the_canonical_setup_space() {
        let setup_status = InMemorySetupStatus::default();
        *setup_status.status.lock().unwrap() = SetupStatus {
            has_completed: true,
            space_id: Some(SpaceId::from("canonical-space")),
            re_pairing_required: false,
        };
        let space_access =
            configured_space_access(None, None, vec![SpaceId::from("canonical-space")], false);
        let (facade, _inv, _peer) = make_facade(
            space_access.clone(),
            Arc::new(setup_status),
            Arc::new(InMemorySettings::default()),
        );

        assert!(!facade.try_resume_session().await.expect("resume check"));
    }

    #[tokio::test]
    async fn try_resume_session_isolates_a_legacy_profile() {
        let setup_status = Arc::new(InMemorySetupStatus::default());
        *setup_status.status.lock().unwrap() = SetupStatus {
            has_completed: true,
            space_id: Some(SpaceId::from("legacy-space")),
            re_pairing_required: false,
        };
        let space_access = configured_space_access(
            None,
            None,
            vec![SpaceId::from("legacy-space"), SpaceId::from("paired-space")],
            true,
        );
        let member_repo = Arc::new(InMemoryMemberRepo::default());
        member_repo.rows.lock().unwrap().push(SpaceMember {
            device_id: DeviceId::new("legacy-peer"),
            device_name: "Legacy peer".to_owned(),
            identity_fingerprint: default_fingerprint(),
            joined_at: Utc::now(),
            sync_preferences: MemberSyncPreferences::default(),
        });
        let (facade, _, _) = make_facade_with_member_repo_and_isolation(
            space_access,
            setup_status.clone(),
            settings_with_device_name("Legacy device"),
            Arc::new(NoopMobileConsumableBackfill),
            member_repo.clone(),
            true,
        );

        assert!(facade.try_resume_session().await.expect("resume legacy"));
        assert!(setup_status.status.lock().unwrap().re_pairing_required);
        assert!(setup_status
            .device_management_reset_target
            .lock()
            .unwrap()
            .is_none());
        assert_eq!(
            member_repo
                .list()
                .await
                .unwrap()
                .iter()
                .map(|member| member.device_id.as_str())
                .collect::<Vec<_>>(),
            vec!["device-1"]
        );

        *setup_status.status.lock().unwrap() = SetupStatus {
            has_completed: true,
            space_id: Some(SpaceId::from("paired-space")),
            re_pairing_required: false,
        };
        member_repo.rows.lock().unwrap().push(SpaceMember {
            device_id: DeviceId::new("new-peer"),
            device_name: "New peer".to_owned(),
            identity_fingerprint: default_fingerprint(),
            joined_at: Utc::now(),
            sync_preferences: MemberSyncPreferences::default(),
        });

        assert!(facade
            .try_resume_session()
            .await
            .expect("resume after pairing"));
        assert!(member_repo
            .list()
            .await
            .unwrap()
            .iter()
            .any(|member| member.device_id == DeviceId::new("new-peer")));
    }
}
