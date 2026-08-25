//! Workspace admission channel (ADR-017): the private internal communication
//! implementation of workspace admission, plus the pairing use cases.
//!
//! This module owns Space invitation commands, the complete join command,
//! durable admission progression, and restart recovery. Membership rules and
//! accepted member history are committed through the membership ledger.
//!
//! Sessions and invitations exist only in memory here; process interruption
//! discards them and recovery relies solely on the owner's encrypted saved
//! member changes and admission records.
//!
//! Invitation issuance (B1), redemption (B2), and the complete join command
//! live in this subdomain as well. The join use case owns device-name
//! persistence and the best-effort network preparation before redemption.

pub(crate) mod cancel_space_join;
pub(crate) mod complete_pending_space_transition;
pub(crate) mod handle_space_admission_message;
pub(crate) mod invitation;
pub(crate) mod join_space;
mod model;
pub(crate) mod outbox;
pub(crate) mod query_pending_space_transition;
pub(crate) mod recover_space_admissions;
pub(crate) mod security_transition;
pub(crate) mod space_transition;

pub use model::{CurrentJoinStatus, JoinedSpace, PendingInboundMember};
