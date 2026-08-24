//! Sponsor-side pairing internals: the inbound half of the workspace
//! admission channel (ADR-017).
//!
//! Bridges [`PairingEventPort::subscribe`] to the in-memory invitation
//! holder and the rendezvous consume path, drives the restricted pairing
//! session, and hands every decision and save boundary to the workspace
//! owner through the admission seam ([`super::super::adapter`]).
//!
//! Per `uc-application/AGENTS.md` §11.4 everything here is `pub(crate)`;
//! external callers reach pairing exclusively through the facade.

mod durable_flow;
pub(crate) mod orchestrator;
mod owner;
pub(crate) mod sponsor_handshake;

pub(in crate::space) use durable_flow::{confirm_complete_delivery, confirm_rejected_delivery};
pub(crate) use owner::SponsorAdmissionOwnerPort;
