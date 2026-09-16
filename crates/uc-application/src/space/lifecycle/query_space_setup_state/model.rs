use chrono::{DateTime, Utc};
use uc_core::ids::SpaceId;
use uc_core::pairing::invitation::FullInvitation;
use uc_core::pairing::InvitationCode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupStateView {
    pub has_completed: bool,
    pub space_id: Option<SpaceId>,
    pub current_invitation: Option<CurrentInvitation>,
    pub device_name: Option<String>,
    pub re_pairing_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentInvitation {
    pub code: InvitationCode,
    pub full_invitation: FullInvitation,
    pub expires_at: DateTime<Utc>,
}
