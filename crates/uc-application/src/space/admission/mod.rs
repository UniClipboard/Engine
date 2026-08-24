//! Workspace admission channel (ADR-017): the private internal communication
//! implementation of workspace admission, plus the pairing use cases.
//!
//! This module owns Space invitation commands, the complete join command,
//! durable admission progression, and restart recovery. Membership rules and
//! accepted member history remain owned by
//! [`crate::space::workspace_membership::WorkspaceMembership`].
//!
//! Joiner and sponsor each own their durable protocol flow and private owner
//! interface. Shared transactions and restart recovery remain under
//! `durable`; product callers only use the complete admission use cases.
//!
//! Sessions and invitations exist only in memory here; process interruption
//! discards them and recovery relies solely on the owner's encrypted saved
//! member changes and admission records.
//!
//! Invitation issuance (B1), redemption (B2), and the complete join command
//! live in this subdomain as well. The join use case owns device-name
//! persistence and the best-effort network preparation before redemption.

pub(crate) mod adapter;
pub(crate) mod cancel_space_join;
pub(crate) mod complete_pending_space_transition;
pub(crate) mod durable;
pub(crate) mod invitation;
pub(crate) mod join_space;
pub(crate) mod joiner;
mod model;
mod owner;
pub(crate) mod query_pending_space_transition;
pub(crate) mod query_space_join_status;
pub(crate) mod recover_pending_admissions;
pub(crate) mod recover_space_join_completion;
mod reset;
pub(crate) mod security_transition;
pub(crate) mod space_transition;
pub(crate) mod sponsor;

pub(crate) use cancel_space_join::CancelSpaceJoinUseCase;
pub use model::{CurrentJoinStatus, JoinedSpace, PendingInboundMember};
pub(crate) use owner::SpaceAdmission;
pub(crate) use recover_pending_admissions::RecoverPendingAdmissionsUseCase;
pub use recover_space_join_completion::PendingJoinerCompleteAck;
pub(crate) use reset::{PriorSpaceAdmissionStateReset, SpaceAdmissionResetPort};
