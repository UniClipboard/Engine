//! Command / result payloads for the Slice 1 application facade.
//!
//! These are external-facing facade inputs and views. Typed use-case requests
//! and results live with their owning `space/*` modules.

use chrono::{DateTime, Utc};

use uc_core::crypto::domain::Passphrase;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::pairing::InvitationCode;
use uc_core::security::IdentityFingerprint;

// ---------------------------------------------------------------------------
// A1 · InitializeSpace
// ---------------------------------------------------------------------------

/// Public application input for initializing a space.
#[derive(Debug)]
pub struct InitializeSpaceInput {
    pub passphrase: String,
    pub passphrase_confirm: String,
    pub device_name: Option<String>,
}

// ---------------------------------------------------------------------------
// A2 · UnlockSpace
// ---------------------------------------------------------------------------

/// Public application input for unlocking a space.
#[derive(Debug)]
pub struct UnlockSpaceInput {
    pub passphrase: String,
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

// ---------------------------------------------------------------------------
// B2 · RedeemPairingInvitation  (joiner side)
// ---------------------------------------------------------------------------

/// Public application input for redeeming a pairing invitation.
#[derive(Debug)]
pub struct RedeemPairingInvitationInput {
    pub code: String,
    pub passphrase: String,
    pub preserve_unreadable_history: bool,
}

/// Internal command for [`crate::space::admission::joiner::RedeemPairingInvitationUseCase`].
///
/// Joiner-side UX gathers both fields up front: the user types the
/// invitation code the sponsor shared and the space passphrase the sponsor
/// chose during A1. Slice 1 does not support a two-step flow where the
/// passphrase is entered after receiving the keyslot offer.
#[derive(Debug)]
pub(crate) struct RedeemPairingInvitationCommand {
    /// Invitation code the user typed (or scanned from the sponsor's UI).
    pub code: InvitationCode,
    /// Same passphrase the sponsor used in A1 `InitializeSpace`.
    pub passphrase: Passphrase,
    pub preserve_unreadable_history: bool,
}

impl From<RedeemPairingInvitationInput> for RedeemPairingInvitationCommand {
    fn from(input: RedeemPairingInvitationInput) -> Self {
        Self {
            code: InvitationCode::new(input.code),
            passphrase: Passphrase::new(input.passphrase),
            preserve_unreadable_history: input.preserve_unreadable_history,
        }
    }
}

/// Output of a successful B2 redemption.
///
/// Returned fields let the UI show a "you are connected to X" confirmation
/// without having to re-read the freshly-persisted `SpaceMember` /
/// `TrustedPeer` rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedeemPairingInvitationResult {
    /// Sponsor device now persisted locally as both `SpaceMember` and
    /// `TrustedPeer`.
    pub sponsor_device_id: DeviceId,
    /// Sponsor's stable identity fingerprint (F-036 concept 2).
    pub sponsor_identity_fingerprint: IdentityFingerprint,
    /// Sponsor's space id, adopted as the joiner's local space id.
    pub space_id: SpaceId,
    /// This device's own id, as persisted on the sponsor side through the
    /// in-flight `JoinerRequest` — surfaced here so the UI does not need
    /// to query `DeviceIdentityPort` separately for the confirmation
    /// screen.
    pub self_device_id: DeviceId,
    /// This device's stable identity fingerprint.
    pub self_identity_fingerprint: IdentityFingerprint,
}
