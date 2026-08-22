//! Workspace admission channel (ADR-017): the private internal communication
//! implementation of workspace admission, plus the pairing use cases.
//!
//! This module is **not** a second public entry point. It is the internal
//! pairing stack that the workspace convergence owner uses inside the join
//! flow: invitation verification, secret and identity verification, the
//! restricted session and the transfer of admission security material. It
//! does not save member state, does not drive convergence, does not announce
//! success, and does not define restart recovery — all of that belongs to
//! [`crate::space::workspace_membership::WorkspaceMembership`].
//!
//! ## The adapter seam
//!
//! [`adapter::WorkspaceAdmissionOwnerPort`] is the private seam between the
//! workspace owner and this channel. The channel only does three things
//! through it:
//!
//! 1. **Request joiner verification** — invitation, joiner identity and the
//!    verified material; the owner returns the allow/reject decision.
//! 2. **Submit joiner readiness facts** — the saved, verifiable readiness
//!    facts are handed back to the owner, which saves the admission change
//!    and returns the "admission change saved" confirmation.
//! 3. **Receive workspace decisions** — accept the join, reject it, or
//!    require the session to be closed.
//!
//! The owner does not learn the dialing, framing or cryptographic handshake
//! details; communication replacement, independent tests and protocol
//! evolution stay behind this seam. Two independent test surfaces are
//! required: the workspace side verifies the five-step join order and the
//! save boundaries against a channel double; the channel side verifies
//! communication and verification against a workspace double. Neither side
//! depends on a real network or a real owner.
//!
//! Sessions and invitations exist only in memory here; process interruption
//! discards them and recovery relies solely on the owner's encrypted saved
//! member changes and admission records.
//!
//! Invitation issuance (B1) and redemption (B2) use cases live in this
//! subdomain as well; `coordinator` holds the join / switch entry
//! orchestration.

pub(crate) mod adapter;
pub(crate) mod coordinator;
pub(crate) mod durable;
pub(crate) mod invitation;
pub(crate) mod issue_invitation;
pub(crate) mod joiner;
mod profile;
pub(crate) mod redeem_invitation;
mod reset;
pub(crate) mod sponsor;

pub(crate) use reset::{PriorSpaceAdmissionStateReset, SpaceAdmissionResetPort};

use std::sync::Arc;

use crate::space::workspace_membership::WorkspaceMembership;
use durable::DurableAdmissionTransaction;

pub(crate) struct SpaceAdmission {
    pub(in crate::space) membership: Arc<WorkspaceMembership>,
    pub(in crate::space) admission: DurableAdmissionTransaction,
}

pub struct ProfileSpaceAdmission {
    admission: durable::DurableAdmissionProjection,
    admission_attempts: Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort>,
    own_device: uc_core::DeviceId,
    clock: Arc<dyn uc_core::ports::ClockPort>,
    active:
        tokio::sync::RwLock<Option<Arc<crate::space::workspace_membership::WorkspaceMembership>>>,
    active_event_task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    events: tokio::sync::broadcast::Sender<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingInboundMember {
    pub device_id: uc_core::DeviceId,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinedSpace {
    pub sponsor_device_id: uc_core::DeviceId,
    pub sponsor_identity_fingerprint: uc_core::security::IdentityFingerprint,
    pub space_id: String,
    pub self_device_id: uc_core::DeviceId,
    pub self_identity_fingerprint: uc_core::security::IdentityFingerprint,
    pub migrated_records: Option<u64>,
    pub preserved_unreadable_records: Option<u64>,
}

pub struct PendingJoinerCompleteAck {
    pub sponsor_device_id: uc_core::DeviceId,
    pub frame: uc_core::pairing::DurableAdmissionFrame,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurrentJoinStatus {
    Active {
        join_id: [u8; 16],
        joined_space: JoinedSpace,
    },
    Pending {
        join_id: [u8; 16],
        target_space_id: Option<String>,
        sponsor_device_id: Option<uc_core::DeviceId>,
        sponsor_identity_fingerprint: Option<uc_core::security::IdentityFingerprint>,
        cancel_requested: bool,
    },
    Rejected {
        join_id: [u8; 16],
        reason: uc_core::membership::AdmissionRejectionReasonV1,
    },
}

#[async_trait::async_trait]
pub trait SpaceTransitionRecoveryPort: Send + Sync {
    async fn requires_session_transition(
        &self,
    ) -> Result<bool, crate::space::workspace_membership::WorkspaceConvergenceError>;

    async fn recover_after_session_drain(
        &self,
    ) -> Result<usize, crate::space::workspace_membership::WorkspaceConvergenceError>;
}

impl SpaceAdmission {
    pub(crate) fn new(membership: Arc<WorkspaceMembership>) -> Arc<Self> {
        let admission = DurableAdmissionTransaction::new(
            Arc::clone(&membership.deps.admission_attempts),
            Arc::clone(&membership.deps.historical_membership_signatures),
            Arc::clone(&membership.deps.admission_security_transition),
            Arc::clone(&membership.deps.admission_space_transition),
        );
        Arc::new(Self {
            membership,
            admission,
        })
    }

    pub(crate) async fn current_join(
        &self,
    ) -> Result<
        Option<CurrentJoinStatus>,
        crate::space::workspace_membership::WorkspaceConvergenceError,
    > {
        self.admission.current_local_join().await
    }
}
