use uc_core::membership::{
    AdmissionMessageId, AdmissionRetryState, JoinerAdmission, JoinerAdmissionTransition,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct JoinerCancellationCommitToken([u8; 32]);

impl JoinerCancellationCommitToken {
    pub fn from_bytes(bytes: [u8; 32]) -> Option<Self> {
        (bytes != [0; 32]).then_some(Self(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for JoinerCancellationCommitToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JoinerCancellationCommitToken([REDACTED])")
    }
}

pub struct LoadedCurrentJoin {
    admission: JoinerAdmission,
    commit_token: JoinerCancellationCommitToken,
}

impl LoadedCurrentJoin {
    pub fn new(admission: JoinerAdmission, commit_token: JoinerCancellationCommitToken) -> Self {
        Self {
            admission,
            commit_token,
        }
    }

    pub fn into_parts(self) -> (JoinerAdmission, JoinerCancellationCommitToken) {
        (self.admission, self.commit_token)
    }
}

pub struct JoinerCancellationMaterial {
    message_id: AdmissionMessageId,
    retry_state: AdmissionRetryState,
}

impl JoinerCancellationMaterial {
    pub fn new(message_id: AdmissionMessageId, retry_state: AdmissionRetryState) -> Self {
        Self {
            message_id,
            retry_state,
        }
    }

    pub fn into_parts(self) -> (AdmissionMessageId, AdmissionRetryState) {
        (self.message_id, self.retry_state)
    }
}

pub struct JoinerCancellationMutation {
    transition: JoinerAdmissionTransition,
}

impl JoinerCancellationMutation {
    pub const fn new(transition: JoinerAdmissionTransition) -> Self {
        Self { transition }
    }

    pub fn into_transition(self) -> JoinerAdmissionTransition {
        self.transition
    }
}
