use super::*;

impl TryFrom<&SpaceAdmissionSponsorCommitted> for PersistedSponsorCommittedV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(state: &SpaceAdmissionSponsorCommitted) -> Result<Self, Self::Error> {
        Ok(Self {
            peer_binding: PersistedPeerBindingV1::from(state.peer_binding),
            continuation_credential: state.continuation_credential.as_bytes().to_vec(),
            committed_history: state.committed_history.as_bytes().to_vec(),
            sealed_security: state.sealed_security.as_bytes().to_vec(),
            saved_reply: PersistedSavedReplyV1::try_from(&state.saved_reply)?,
        })
    }
}

impl PersistedSponsorCommittedV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<SpaceAdmissionSponsorCommitted, SpaceAdmissionPersistenceError> {
        let saved_reply = self.saved_reply.into_domain(admission_id)?;
        if saved_reply.exact_reply_envelope().kind() != SpaceAdmissionMessageKind::Commit {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        }
        Ok(SpaceAdmissionSponsorCommitted {
            peer_binding: self.peer_binding.into_domain()?,
            continuation_credential: decode_continuation_credential(self.continuation_credential)?,
            committed_history: AdmissionSignedMembershipHistory::from_bytes(self.committed_history)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            sealed_security: AdmissionSealedSecurityState::from_bytes(self.sealed_security)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            saved_reply,
        })
    }
}

impl TryFrom<&SpaceAdmissionSponsorApplied> for PersistedSponsorAppliedV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(state: &SpaceAdmissionSponsorApplied) -> Result<Self, Self::Error> {
        Ok(Self {
            peer_binding: PersistedPeerBindingV1::from(state.peer_binding),
            continuation_credential: state.continuation_credential.as_bytes().to_vec(),
            committed_history: state.committed_history.as_bytes().to_vec(),
            activation_receipt: state.activation_receipt.clone(),
            activated_security: state.activated_security.as_bytes().to_vec(),
            saved_reply: PersistedSavedReplyV1::try_from(&state.saved_reply)?,
        })
    }
}

impl PersistedSponsorAppliedV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<SpaceAdmissionSponsorApplied, SpaceAdmissionPersistenceError> {
        validate_activation_receipt(&self.activation_receipt, admission_id)?;
        let saved_reply = self.saved_reply.into_domain(admission_id)?;
        if saved_reply.exact_reply_envelope().kind() != SpaceAdmissionMessageKind::Complete {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        }
        Ok(SpaceAdmissionSponsorApplied {
            peer_binding: self.peer_binding.into_domain()?,
            continuation_credential: decode_continuation_credential(self.continuation_credential)?,
            committed_history: AdmissionSignedMembershipHistory::from_bytes(self.committed_history)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            activation_receipt: self.activation_receipt,
            activated_security: AdmissionActivatedSecurityState::from_bytes(
                self.activated_security,
            )
            .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            saved_reply,
        })
    }
}
