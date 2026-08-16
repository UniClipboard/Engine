use uc_core::membership::{AdmissionSecurityCommitmentV1, BaseMembershipHistoryPositionV1};
use zeroize::Zeroize;

use super::mls_group::{MlsClientState, MlsGroupEngine, PendingMlsJoin};

#[derive(Clone, PartialEq, Eq)]
pub struct AdmissionSecurityTransitionInput {
    pub attempt_id: [u8; 32],
    pub base_history_position: BaseMembershipHistoryPositionV1,
    pub candidate_core_digest: [u8; 32],
    pub key_catalog_digest: [u8; 32],
    pub admission_bundle_digest: [u8; 32],
}

impl std::fmt::Debug for AdmissionSecurityTransitionInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionSecurityTransitionInput([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SponsorPreparedSecurityTransition {
    pub staged_state: Vec<u8>,
    pub commit: Vec<u8>,
    pub welcome: Vec<u8>,
    pub public_commitment: AdmissionSecurityCommitmentV1,
}

impl std::fmt::Debug for SponsorPreparedSecurityTransition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SponsorPreparedSecurityTransition")
            .field("staged_state", &"[REDACTED]")
            .field("commit_len", &self.commit.len())
            .field("welcome_len", &self.welcome.len())
            .field("public_commitment", &self.public_commitment)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct JoinerStagedSecurityTransition {
    pub staged_state: Vec<u8>,
    pub public_commitment: AdmissionSecurityCommitmentV1,
}

impl std::fmt::Debug for JoinerStagedSecurityTransition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JoinerStagedSecurityTransition")
            .field("staged_state", &"[REDACTED]")
            .field("public_commitment", &self.public_commitment)
            .finish()
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum AdmissionSecurityTransitionError {
    #[error("admission security state is invalid")]
    InvalidState,
    #[error("admission security commitment does not match")]
    CommitmentMismatch,
}

pub struct AdmissionSecurityTransitionAdapter;

impl AdmissionSecurityTransitionAdapter {
    pub fn prepare_sponsor(
        sponsor_state: &[u8],
        candidate_identity: &[u8],
        key_package: &[u8],
        input: &AdmissionSecurityTransitionInput,
    ) -> Result<SponsorPreparedSecurityTransition, AdmissionSecurityTransitionError> {
        let admission = MlsGroupEngine::admit_member(
            &MlsClientState::from_bytes(sponsor_state.to_vec()),
            candidate_identity,
            key_package,
        )
        .map_err(|_| AdmissionSecurityTransitionError::InvalidState)?;
        let public_commitment = Self::derive_public_commitment(
            admission.sponsor_state.as_bytes(),
            &admission.commit,
            input,
        )?;
        Ok(SponsorPreparedSecurityTransition {
            staged_state: admission.sponsor_state.into_bytes(),
            commit: admission.commit,
            welcome: admission.welcome,
            public_commitment,
        })
    }

    pub fn stage_joiner(
        pending_state: &[u8],
        key_package: &[u8],
        expected_space_id: &[u8],
        welcome: &[u8],
        commit: &[u8],
        input: &AdmissionSecurityTransitionInput,
    ) -> Result<JoinerStagedSecurityTransition, AdmissionSecurityTransitionError> {
        let pending = PendingMlsJoin::new(
            key_package.to_vec(),
            MlsClientState::from_bytes(pending_state.to_vec()),
        );
        let joined = MlsGroupEngine::complete_join(pending, expected_space_id, welcome)
            .map_err(|_| AdmissionSecurityTransitionError::InvalidState)?;
        let public_commitment =
            Self::derive_public_commitment(joined.client_state.as_bytes(), commit, input)?;
        Ok(JoinerStagedSecurityTransition {
            staged_state: joined.client_state.into_bytes(),
            public_commitment,
        })
    }

    pub fn derive_public_commitment(
        staged_state: &[u8],
        commit: &[u8],
        input: &AdmissionSecurityTransitionInput,
    ) -> Result<AdmissionSecurityCommitmentV1, AdmissionSecurityTransitionError> {
        MlsGroupEngine::derive_public_admission_commitment(
            &MlsClientState::from_bytes(staged_state.to_vec()),
            input.attempt_id,
            input.base_history_position.clone(),
            input.candidate_core_digest,
            commit,
            input.key_catalog_digest,
            input.admission_bundle_digest,
        )
        .map_err(|_| AdmissionSecurityTransitionError::InvalidState)
    }

    pub fn signing_public_key(
        staged_state: &[u8],
    ) -> Result<Vec<u8>, AdmissionSecurityTransitionError> {
        MlsGroupEngine::signing_public_key(&MlsClientState::from_bytes(staged_state.to_vec()))
            .map_err(|_| AdmissionSecurityTransitionError::InvalidState)
    }

    pub fn activate(
        staged_state: Vec<u8>,
        commit: &[u8],
        expected: &AdmissionSecurityCommitmentV1,
        input: &AdmissionSecurityTransitionInput,
    ) -> Result<Vec<u8>, AdmissionSecurityTransitionError> {
        let actual = Self::derive_public_commitment(&staged_state, commit, input)?;
        if &actual != expected {
            return Err(AdmissionSecurityTransitionError::CommitmentMismatch);
        }
        MlsGroupEngine::validate_state(
            &MlsClientState::from_bytes(staged_state.clone()),
            expected.lineage_id.as_bytes(),
        )
        .map_err(|_| AdmissionSecurityTransitionError::InvalidState)?;
        Ok(staged_state)
    }

    pub fn discard(mut staged_state: Vec<u8>) {
        staged_state.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use uc_core::membership::{BaseMembershipHistoryPositionV1, MembershipEventId};

    use super::*;

    #[test]
    fn prepared_security_state_can_be_reopened_activated_or_discarded() {
        let sponsor = MlsGroupEngine::create_sponsor(b"space-a", b"alice").unwrap();
        let pending = MlsGroupEngine::prepare_join(b"bob").unwrap();
        let input = AdmissionSecurityTransitionInput {
            attempt_id: [0x31; 32],
            base_history_position: BaseMembershipHistoryPositionV1 {
                event_id: Some(MembershipEventId::from_hex(&"32".repeat(32)).unwrap()),
                depth: 4,
                history_digest: [0x33; 32],
            },
            candidate_core_digest: [0x34; 32],
            key_catalog_digest: [0x35; 32],
            admission_bundle_digest: [0x36; 32],
        };

        let prepared = AdmissionSecurityTransitionAdapter::prepare_sponsor(
            sponsor.as_bytes(),
            b"bob",
            &pending.key_package,
            &input,
        )
        .unwrap();
        let staged = AdmissionSecurityTransitionAdapter::stage_joiner(
            pending.client_state.as_bytes(),
            &pending.key_package,
            b"space-a",
            &prepared.welcome,
            &prepared.commit,
            &input,
        )
        .unwrap();
        assert_eq!(prepared.public_commitment, staged.public_commitment);

        let reopened = AdmissionSecurityTransitionAdapter::derive_public_commitment(
            &staged.staged_state,
            &prepared.commit,
            &input,
        )
        .unwrap();
        assert_eq!(reopened, prepared.public_commitment);
        let active = AdmissionSecurityTransitionAdapter::activate(
            staged.staged_state,
            &prepared.commit,
            &prepared.public_commitment,
            &input,
        )
        .unwrap();
        assert!(!active.is_empty());
        AdmissionSecurityTransitionAdapter::discard(prepared.staged_state);
    }
}
