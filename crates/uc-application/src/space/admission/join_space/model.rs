use uc_core::crypto::domain::Passphrase;
use uc_core::pairing::InvitationCode;

use crate::space::admission::CurrentJoinStatus;

pub struct JoinSpaceInput {
    pub invitation_code: InvitationCode,
    pub device_name: Option<String>,
    pub passphrase: Passphrase,
    pub preserve_unreadable_history: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinSpaceResult {
    pub status: CurrentJoinStatus,
    pub requires_session_transition: bool,
}
