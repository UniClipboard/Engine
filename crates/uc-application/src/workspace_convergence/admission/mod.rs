//! Workspace admission channel (ADR-017): the private internal communication
//! implementation of workspace admission.
//!
//! This module is **not** a second public entry point. It is the internal
//! pairing stack that the workspace convergence owner uses inside the join
//! flow: invitation verification, secret and identity verification, the
//! restricted session and the transfer of admission security material. It
//! does not save member state, does not drive convergence, does not announce
//! success, and does not define restart recovery — all of that belongs to
//! [`super::WorkspaceConvergence`].
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

pub(crate) mod adapter;
pub(crate) mod invitation;
pub(crate) mod joiner;
pub(crate) mod sponsor;
