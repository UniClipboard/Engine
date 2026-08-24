use crate::space::admission::CurrentJoinStatus;

pub struct JoinSpaceInput {
    pub invitation_code: String,
    pub device_name: Option<String>,
    pub passphrase: String,
    pub preserve_unreadable_history: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinSpaceResult {
    pub status: CurrentJoinStatus,
    pub requires_session_transition: bool,
}
