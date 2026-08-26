//! Command / result payloads for the Space application facade.
//!
//! These are external-facing facade inputs and views. Typed use-case requests
//! and results live with their owning `space/*` modules.

use chrono::{DateTime, Utc};

use uc_core::crypto::domain::Passphrase;
use uc_core::ids::SpaceId;
use uc_core::pairing::InvitationCode;

// ---------------------------------------------------------------------------
// A1 · InitializeSpace
// ---------------------------------------------------------------------------

/// Public application input for initializing a space.
#[derive(Debug)]
pub struct InitializeSpaceInput {
    pub passphrase: Passphrase,
    pub passphrase_confirm: Passphrase,
    pub device_name: Option<String>,
}

// ---------------------------------------------------------------------------
// A2 · UnlockSpace
// ---------------------------------------------------------------------------

/// Public application input for unlocking a space.
#[derive(Debug)]
pub struct UnlockSpaceInput {
    pub passphrase: Passphrase,
}

/// Output of a successful A2 unlock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnlockSpaceResult {
    pub space_id: SpaceId,
}

// ---------------------------------------------------------------------------
// B1 · IssuePairingInvitation
// ---------------------------------------------------------------------------

/// Where a successful invitation can be resolved by another device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvitationAvailability {
    /// Directory-issued invitations can be resolved across networks.
    CrossNetwork,
    /// Locally minted invitations require both devices on the same LAN.
    SameLocalNetwork,
}

/// Output of a successful B1 invitation issuance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuePairingInvitationResult {
    /// Short human-typable code the sponsor shows to the joiner.
    pub code: InvitationCode,
    /// Server-authoritative expiry; UI should display a countdown from
    /// this value rather than computing its own.
    pub expires_at: DateTime<Utc>,
    /// Network scope in which the code can be resolved.
    pub availability: InvitationAvailability,
}
