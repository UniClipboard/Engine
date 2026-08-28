//! Opaque invitation credential carried between sponsor and joiner.
//!
//! Slice 1 decision Q-ε: core does not validate the wire shape of the code —
//! the adapter (rendezvous client) owns format, length, and character-set
//! rules. Core only treats it as an identifier that travels through domain
//! types without dropping back to `String`.

use serde::{Deserialize, Serialize};

pub const MAX_FULL_INVITATION_LEN: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvitationCredentialError {
    #[error("the invitation is empty")]
    Empty,
    #[error("the invitation exceeds its size limit")]
    Oversized,
}

/// Short invitation code (sponsor→joiner handshake credential).
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InvitationCode(String);

impl InvitationCode {
    /// Wrap an adapter-provided string without performing format validation.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for InvitationCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InvitationCode([REDACTED])")
    }
}

impl std::fmt::Display for InvitationCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct FullInvitation(String);

impl FullInvitation {
    pub fn new(value: impl Into<String>) -> Result<Self, InvitationCredentialError> {
        let value = value.into();
        if value.is_empty() {
            return Err(InvitationCredentialError::Empty);
        }
        if value.len() > MAX_FULL_INVITATION_LEN {
            return Err(InvitationCredentialError::Oversized);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for FullInvitation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("FullInvitation([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invitation_entries_are_bounded_and_redacted() {
        assert_eq!(
            FullInvitation::new(String::new()),
            Err(InvitationCredentialError::Empty)
        );
        assert_eq!(
            FullInvitation::new("x".repeat(MAX_FULL_INVITATION_LEN + 1)),
            Err(InvitationCredentialError::Oversized)
        );

        let code = InvitationCode::new("ABCD-1234");
        let full = FullInvitation::new("ucspace1_sensitive-route").expect("valid invitation");

        assert_eq!(format!("{code:?}"), "InvitationCode([REDACTED])");
        assert_eq!(format!("{full:?}"), "FullInvitation([REDACTED])");
        assert_eq!(full.as_str(), "ucspace1_sensitive-route");
    }
}
