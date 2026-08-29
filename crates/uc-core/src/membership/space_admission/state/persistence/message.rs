use super::*;

impl From<AdmissionPeerBinding> for PersistedPeerBindingV1 {
    fn from(binding: AdmissionPeerBinding) -> Self {
        Self {
            local_peer_id: *binding.local_peer_id().as_bytes(),
            remote_peer_id: *binding.remote_peer_id().as_bytes(),
        }
    }
}

impl PersistedPeerBindingV1 {
    pub(super) fn into_domain(
        self,
    ) -> Result<AdmissionPeerBinding, SpaceAdmissionPersistenceError> {
        AdmissionPeerBinding::new(
            AdmissionChannelPeerId::from_bytes(self.local_peer_id)
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            AdmissionChannelPeerId::from_bytes(self.remote_peer_id)
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
        )
        .ok_or(SpaceAdmissionPersistenceError::InvalidState)
    }
}

impl From<&AdmissionMessageEvidence> for PersistedMessageEvidenceV1 {
    fn from(evidence: &AdmissionMessageEvidence) -> Self {
        Self {
            sender_role: encode_role(evidence.sender_role()),
            sender_sequence: evidence.sender_sequence(),
            message_id: *evidence.message_id().as_bytes(),
            predecessor_message_id: evidence
                .predecessor_message_id()
                .map(|message_id| *message_id.as_bytes()),
            canonical_digest: *evidence.canonical_digest(),
        }
    }
}

impl PersistedMessageEvidenceV1 {
    pub(super) fn into_domain(
        self,
    ) -> Result<AdmissionMessageEvidence, SpaceAdmissionPersistenceError> {
        let predecessor_message_id = decode_optional_message_id(self.predecessor_message_id)?;
        AdmissionMessageEvidence::new(
            decode_role(self.sender_role)?,
            self.sender_sequence,
            AdmissionMessageId::from_bytes(self.message_id)
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            predecessor_message_id,
            self.canonical_digest,
        )
        .ok_or(SpaceAdmissionPersistenceError::InvalidState)
    }
}

impl TryFrom<&SavedAdmissionReply> for PersistedSavedCandidateReplyV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(reply: &SavedAdmissionReply) -> Result<Self, Self::Error> {
        Ok(Self {
            inbound_evidence: PersistedMessageEvidenceV1::from(reply.inbound_evidence()),
            exact_reply: PersistedCandidateEnvelopeV1::try_from(reply.exact_reply_envelope())?,
        })
    }
}

impl PersistedSavedCandidateReplyV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<SavedAdmissionReply, SpaceAdmissionPersistenceError> {
        let inbound_evidence = self.inbound_evidence.into_domain()?;
        let exact_reply = self.exact_reply.into_domain()?;
        SavedAdmissionReply::new(admission_id, inbound_evidence, exact_reply)
            .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)
    }
}

impl From<&SpaceAdmissionEnvelopeHeaderV1> for PersistedEnvelopeHeaderV1 {
    fn from(header: &SpaceAdmissionEnvelopeHeaderV1) -> Self {
        Self {
            protocol_version: header.protocol_version().as_u16(),
            admission_id: *header.admission_id().as_bytes(),
            sender_role: encode_role(header.sender_role()),
            sender_sequence: header.sender_sequence(),
            message_id: *header.message_id().as_bytes(),
            predecessor_message_id: header
                .predecessor_message_id()
                .map(|message_id| *message_id.as_bytes()),
        }
    }
}

impl PersistedEnvelopeHeaderV1 {
    pub(super) fn into_domain_parts(
        self,
    ) -> Result<
        (
            SpaceAdmissionId,
            AdmissionRole,
            u64,
            AdmissionMessageId,
            Option<AdmissionMessageId>,
        ),
        SpaceAdmissionPersistenceError,
    > {
        if SpaceAdmissionProtocolVersion::from_u16(self.protocol_version)
            != Some(SpaceAdmissionProtocolVersion::V1)
        {
            return Err(SpaceAdmissionPersistenceError::UnsupportedVersion);
        }
        Ok((
            SpaceAdmissionId::from_bytes(self.admission_id)
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            decode_role(self.sender_role)?,
            self.sender_sequence,
            AdmissionMessageId::from_bytes(self.message_id)
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            decode_optional_message_id(self.predecessor_message_id)?,
        ))
    }
}

impl TryFrom<&SpaceAdmissionEnvelopeV1> for PersistedEnvelopeV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(envelope: &SpaceAdmissionEnvelopeV1) -> Result<Self, Self::Error> {
        let body = match envelope.body() {
            SpaceAdmissionBodyV1::JoinRequest(body) => {
                PersistedBodyV1::JoinRequest(PersistedJoinRequestV1::from(body))
            }
            SpaceAdmissionBodyV1::Candidate(body) => {
                PersistedBodyV1::Candidate(PersistedCandidateV1::from(body))
            }
            SpaceAdmissionBodyV1::Prepared(body) => PersistedBodyV1::Prepared(body.proof().clone()),
            SpaceAdmissionBodyV1::Commit(body) => PersistedBodyV1::Commit {
                exact_candidate: PersistedCandidateV1::from(body.exact_candidate()),
                target_membership_history: body.target_membership_history().as_bytes().to_vec(),
                sealed_recovery_material: body.sealed_recovery_material().as_bytes().to_vec(),
            },
            SpaceAdmissionBodyV1::Applied(body) => {
                PersistedBodyV1::Applied(body.activation_receipt().clone())
            }
            SpaceAdmissionBodyV1::Complete(body) => {
                PersistedBodyV1::Complete(body.completion().clone())
            }
            SpaceAdmissionBodyV1::CompleteAck(body) => {
                PersistedBodyV1::CompleteAck(*body.completion_digest())
            }
            SpaceAdmissionBodyV1::Settled(body) => {
                PersistedBodyV1::Settled(*body.completion_ack_digest())
            }
            SpaceAdmissionBodyV1::CancelRequested => PersistedBodyV1::CancelRequested,
            SpaceAdmissionBodyV1::Rejected { reason } => {
                PersistedBodyV1::Rejected(encode_rejection_reason(*reason))
            }
        };
        Ok(Self {
            header: PersistedEnvelopeHeaderV1::from(envelope.header()),
            body,
        })
    }
}

impl PersistedEnvelopeV1 {
    pub(super) fn into_domain(
        self,
    ) -> Result<SpaceAdmissionEnvelopeV1, SpaceAdmissionPersistenceError> {
        let (admission_id, sender_role, sender_sequence, message_id, predecessor_message_id) =
            self.header.into_domain_parts()?;
        let body = match self.body {
            PersistedBodyV1::JoinRequest(body) => {
                SpaceAdmissionBodyV1::JoinRequest(body.into_domain()?)
            }
            PersistedBodyV1::Candidate(body) => {
                SpaceAdmissionBodyV1::Candidate(body.into_domain()?)
            }
            PersistedBodyV1::Prepared(proof) => {
                if proof.proof_format_version != PREPARED_ADMISSION_PROOF_FORMAT_V1
                    || proof.attempt_id != *admission_id.as_bytes()
                {
                    return Err(SpaceAdmissionPersistenceError::InvalidState);
                }
                SpaceAdmissionBodyV1::Prepared(AdmissionPreparedV1::new(proof))
            }
            PersistedBodyV1::Commit {
                exact_candidate,
                target_membership_history,
                sealed_recovery_material,
            } => SpaceAdmissionBodyV1::Commit(AdmissionCommitV1::new(
                exact_candidate.into_domain()?,
                AdmissionSignedMembershipHistory::from_bytes(target_membership_history)
                    .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
                AdmissionSealedRecoveryMaterial::from_bytes(sealed_recovery_material)
                    .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            )),
            PersistedBodyV1::Applied(receipt) => {
                validate_activation_receipt(&receipt, admission_id)?;
                SpaceAdmissionBodyV1::Applied(AdmissionAppliedV1::new(receipt))
            }
            PersistedBodyV1::Complete(completion) => {
                if completion.completion_format_version != ADMISSION_COMPLETION_FORMAT_V1
                    || completion.attempt_id != *admission_id.as_bytes()
                {
                    return Err(SpaceAdmissionPersistenceError::InvalidState);
                }
                SpaceAdmissionBodyV1::Complete(AdmissionCompleteV1::new(completion))
            }
            PersistedBodyV1::CompleteAck(digest) => SpaceAdmissionBodyV1::CompleteAck(
                AdmissionCompleteAckV1::new(digest)
                    .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            ),
            PersistedBodyV1::Settled(digest) => SpaceAdmissionBodyV1::Settled(
                AdmissionSettledV1::new(digest)
                    .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            ),
            PersistedBodyV1::CancelRequested => SpaceAdmissionBodyV1::CancelRequested,
            PersistedBodyV1::Rejected(reason) => SpaceAdmissionBodyV1::Rejected {
                reason: decode_rejection_reason(reason)?,
            },
        };
        SpaceAdmissionEnvelopeV1::new(
            admission_id,
            sender_role,
            sender_sequence,
            message_id,
            predecessor_message_id,
            body,
        )
        .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)
    }
}

impl TryFrom<&PendingAdmissionExchange> for PersistedAnyPendingExchangeV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(exchange: &PendingAdmissionExchange) -> Result<Self, Self::Error> {
        Ok(Self {
            route: exchange.route().as_bytes().to_vec(),
            request: PersistedEnvelopeV1::try_from(exchange.request_envelope())?,
            expected_reply_kind: encode_message_kind(exchange.exact_expected_reply_kind()),
            retry_attempt_count: exchange.retry_state().attempt_count(),
            retry_next_attempt_at_ms: exchange.retry_state().next_attempt_at_ms(),
        })
    }
}

impl PersistedAnyPendingExchangeV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<PendingAdmissionExchange, SpaceAdmissionPersistenceError> {
        let request = self.request.into_domain()?;
        if request.header().admission_id() != admission_id {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        }
        PendingAdmissionExchange::new(
            SpaceAdmissionRoute::from_bytes(self.route)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            request,
            decode_message_kind(self.expected_reply_kind)?,
            AdmissionRetryState::new(self.retry_attempt_count, self.retry_next_attempt_at_ms)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
        )
        .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)
    }
}

impl TryFrom<&SavedAdmissionReply> for PersistedSavedReplyV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(reply: &SavedAdmissionReply) -> Result<Self, Self::Error> {
        Ok(Self {
            inbound_evidence: PersistedMessageEvidenceV1::from(reply.inbound_evidence()),
            exact_reply: PersistedEnvelopeV1::try_from(reply.exact_reply_envelope())?,
        })
    }
}

impl PersistedSavedReplyV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<SavedAdmissionReply, SpaceAdmissionPersistenceError> {
        SavedAdmissionReply::new(
            admission_id,
            self.inbound_evidence.into_domain()?,
            self.exact_reply.into_domain()?,
        )
        .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)
    }
}

impl TryFrom<&SpaceAdmissionEnvelopeV1> for PersistedCandidateEnvelopeV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(envelope: &SpaceAdmissionEnvelopeV1) -> Result<Self, Self::Error> {
        let SpaceAdmissionBodyV1::Candidate(candidate) = envelope.body() else {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        };
        Ok(Self {
            header: PersistedEnvelopeHeaderV1::from(envelope.header()),
            body: PersistedCandidateV1::from(candidate),
        })
    }
}

impl PersistedCandidateEnvelopeV1 {
    pub(super) fn into_domain(
        self,
    ) -> Result<SpaceAdmissionEnvelopeV1, SpaceAdmissionPersistenceError> {
        let (admission_id, sender_role, sender_sequence, message_id, predecessor_message_id) =
            self.header.into_domain_parts()?;
        SpaceAdmissionEnvelopeV1::new(
            admission_id,
            sender_role,
            sender_sequence,
            message_id,
            predecessor_message_id,
            SpaceAdmissionBodyV1::Candidate(self.body.into_domain()?),
        )
        .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)
    }
}

impl From<&AdmissionCandidateV1> for PersistedCandidateV1 {
    fn from(candidate: &AdmissionCandidateV1) -> Self {
        Self {
            base_membership_history: candidate.base_membership_history().as_bytes().to_vec(),
            candidate_event: candidate.candidate_event().clone(),
            security_commitment: candidate.security_commitment().clone(),
            mls_commit: candidate.mls_commit().as_bytes().to_vec(),
            mls_welcome: candidate.mls_welcome().as_bytes().to_vec(),
            continuation_route: candidate.continuation_route().as_bytes().to_vec(),
        }
    }
}

impl PersistedCandidateV1 {
    pub(super) fn into_domain(
        self,
    ) -> Result<AdmissionCandidateV1, SpaceAdmissionPersistenceError> {
        AdmissionCandidateV1::new(
            AdmissionSignedMembershipHistory::from_bytes(self.base_membership_history)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            self.candidate_event,
            self.security_commitment,
            AdmissionMlsCommit::from_bytes(self.mls_commit)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            AdmissionMlsWelcome::from_bytes(self.mls_welcome)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            AdmissionContinuationRoute::from_bytes(self.continuation_route)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
        )
        .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)
    }
}

impl TryFrom<&SpaceAdmissionEnvelopeV1> for PersistedPreparedEnvelopeV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(envelope: &SpaceAdmissionEnvelopeV1) -> Result<Self, Self::Error> {
        let SpaceAdmissionBodyV1::Prepared(prepared) = envelope.body() else {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        };
        Ok(Self {
            header: PersistedEnvelopeHeaderV1::from(envelope.header()),
            proof: prepared.proof().clone(),
        })
    }
}

impl PersistedPreparedEnvelopeV1 {
    pub(super) fn into_domain(
        self,
    ) -> Result<SpaceAdmissionEnvelopeV1, SpaceAdmissionPersistenceError> {
        if self.proof.proof_format_version != PREPARED_ADMISSION_PROOF_FORMAT_V1 {
            return Err(SpaceAdmissionPersistenceError::UnsupportedVersion);
        }
        let (admission_id, sender_role, sender_sequence, message_id, predecessor_message_id) =
            self.header.into_domain_parts()?;
        SpaceAdmissionEnvelopeV1::new(
            admission_id,
            sender_role,
            sender_sequence,
            message_id,
            predecessor_message_id,
            SpaceAdmissionBodyV1::Prepared(AdmissionPreparedV1::new(self.proof)),
        )
        .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)
    }
}

impl TryFrom<&PendingAdmissionExchange> for PersistedPreparedPendingExchangeV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(exchange: &PendingAdmissionExchange) -> Result<Self, Self::Error> {
        Ok(Self {
            route: exchange.route().as_bytes().to_vec(),
            request: PersistedPreparedEnvelopeV1::try_from(exchange.request_envelope())?,
            expected_reply_kind: encode_message_kind(exchange.exact_expected_reply_kind()),
            retry_attempt_count: exchange.retry_state().attempt_count(),
            retry_next_attempt_at_ms: exchange.retry_state().next_attempt_at_ms(),
        })
    }
}

impl PersistedPreparedPendingExchangeV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
        candidate_evidence: &AdmissionMessageEvidence,
    ) -> Result<PendingAdmissionExchange, SpaceAdmissionPersistenceError> {
        let request = self.request.into_domain()?;
        let SpaceAdmissionBodyV1::Prepared(prepared) = request.body() else {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        };
        if request.header().admission_id() != admission_id
            || request.header().sender_sequence() != 1
            || prepared.proof().attempt_id != *admission_id.as_bytes()
        {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        }
        let exchange = PendingAdmissionExchange::new(
            SpaceAdmissionRoute::from_bytes(self.route)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            request,
            decode_message_kind(self.expected_reply_kind)?,
            AdmissionRetryState::new(self.retry_attempt_count, self.retry_next_attempt_at_ms)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
        )
        .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?;
        if exchange.exact_expected_reply_kind() != SpaceAdmissionMessageKind::Commit
            || exchange.exact_reply_for(candidate_evidence).is_none()
        {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        }
        Ok(exchange)
    }
}

pub(super) fn validate_envelope_evidence(
    envelope: &SpaceAdmissionEnvelopeV1,
    evidence: &AdmissionMessageEvidence,
) -> Result<(), SpaceAdmissionPersistenceError> {
    let reconstructed = envelope
        .evidence(*evidence.canonical_digest())
        .ok_or(SpaceAdmissionPersistenceError::InvalidState)?;
    if &reconstructed != evidence {
        return Err(SpaceAdmissionPersistenceError::InvalidState);
    }
    Ok(())
}

fn decode_optional_message_id(
    value: Option<[u8; 32]>,
) -> Result<Option<AdmissionMessageId>, SpaceAdmissionPersistenceError> {
    value
        .map(|message_id| {
            AdmissionMessageId::from_bytes(message_id)
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)
        })
        .transpose()
}

impl TryFrom<&PendingAdmissionExchange> for PersistedPendingExchangeV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(exchange: &PendingAdmissionExchange) -> Result<Self, Self::Error> {
        Ok(Self {
            route: exchange.route().as_bytes().to_vec(),
            request: PersistedJoinRequestEnvelopeV1::try_from(exchange.request_envelope())?,
            expected_reply_kind: encode_message_kind(exchange.exact_expected_reply_kind()),
            retry_attempt_count: exchange.retry_state().attempt_count(),
            retry_next_attempt_at_ms: exchange.retry_state().next_attempt_at_ms(),
        })
    }
}

impl PersistedPendingExchangeV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<PendingAdmissionExchange, SpaceAdmissionPersistenceError> {
        let request = self.request.into_domain()?;
        if request.header().admission_id() != admission_id {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        }
        PendingAdmissionExchange::new(
            SpaceAdmissionRoute::from_bytes(self.route)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            request,
            decode_message_kind(self.expected_reply_kind)?,
            AdmissionRetryState::new(self.retry_attempt_count, self.retry_next_attempt_at_ms)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
        )
        .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)
    }
}

impl TryFrom<&SpaceAdmissionEnvelopeV1> for PersistedJoinRequestEnvelopeV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(envelope: &SpaceAdmissionEnvelopeV1) -> Result<Self, Self::Error> {
        let SpaceAdmissionBodyV1::JoinRequest(body) = envelope.body() else {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        };
        let header = envelope.header();
        Ok(Self {
            protocol_version: header.protocol_version().as_u16(),
            admission_id: *header.admission_id().as_bytes(),
            sender_role: encode_role(header.sender_role()),
            sender_sequence: header.sender_sequence(),
            message_id: *header.message_id().as_bytes(),
            predecessor_message_id: header
                .predecessor_message_id()
                .map(|message_id| *message_id.as_bytes()),
            body: PersistedJoinRequestV1::from(body),
        })
    }
}

impl PersistedJoinRequestEnvelopeV1 {
    pub(super) fn into_domain(
        self,
    ) -> Result<SpaceAdmissionEnvelopeV1, SpaceAdmissionPersistenceError> {
        if SpaceAdmissionProtocolVersion::from_u16(self.protocol_version)
            != Some(SpaceAdmissionProtocolVersion::V1)
        {
            return Err(SpaceAdmissionPersistenceError::UnsupportedVersion);
        }
        let predecessor_message_id = match self.predecessor_message_id {
            Some(message_id) => Some(
                AdmissionMessageId::from_bytes(message_id)
                    .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            ),
            None => None,
        };
        SpaceAdmissionEnvelopeV1::new(
            SpaceAdmissionId::from_bytes(self.admission_id)
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            decode_role(self.sender_role)?,
            self.sender_sequence,
            AdmissionMessageId::from_bytes(self.message_id)
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            predecessor_message_id,
            SpaceAdmissionBodyV1::JoinRequest(self.body.into_domain()?),
        )
        .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)
    }
}

impl From<&AdmissionJoinRequestV1> for PersistedJoinRequestV1 {
    fn from(request: &AdmissionJoinRequestV1) -> Self {
        let credential = request.membership_credential();
        Self {
            invitation_id: *request.invitation_id().as_bytes(),
            device_id: request.device_id().as_str().to_owned(),
            identity_facts: request.identity_facts().clone(),
            credential_format_version: credential.credential_format_version,
            credential_signature_algorithm_version: credential.signature_algorithm_version,
            credential_public_key: credential.public_key.clone(),
            credential_id: *credential.credential_id.as_bytes(),
            key_package: request.key_package().as_bytes().to_vec(),
            recovery_public_key: *request.recovery_public_key().as_bytes(),
            identity_signature: request.identity_signature().as_bytes().to_vec(),
            unreadable_history_policy: encode_unreadable_history_policy(
                request.unreadable_history_policy(),
            ),
        }
    }
}

impl PersistedJoinRequestV1 {
    pub(super) fn into_domain(
        self,
    ) -> Result<AdmissionJoinRequestV1, SpaceAdmissionPersistenceError> {
        let credential = MembershipCredential::new(
            self.credential_signature_algorithm_version,
            self.credential_public_key,
        );
        if credential.credential_format_version != self.credential_format_version
            || credential.credential_id.as_bytes() != &self.credential_id
        {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        }
        AdmissionJoinRequestV1::new(
            InvitationId::from_bytes(self.invitation_id)
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            DeviceId::try_new(self.device_id)
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            self.identity_facts,
            credential,
            AdmissionKeyPackage::from_bytes(self.key_package)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            AdmissionRecoveryPublicKey::from_bytes(self.recovery_public_key)
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            AdmissionIdentitySignature::from_bytes(self.identity_signature)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            decode_unreadable_history_policy(self.unreadable_history_policy)?,
        )
        .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)
    }
}

const fn encode_role(role: AdmissionRole) -> u8 {
    match role {
        AdmissionRole::Joiner => 1,
        AdmissionRole::Sponsor => 2,
        AdmissionRole::CompletionHelper => 3,
    }
}

fn decode_role(value: u8) -> Result<AdmissionRole, SpaceAdmissionPersistenceError> {
    match value {
        1 => Ok(AdmissionRole::Joiner),
        2 => Ok(AdmissionRole::Sponsor),
        3 => Ok(AdmissionRole::CompletionHelper),
        _ => Err(SpaceAdmissionPersistenceError::InvalidState),
    }
}

const fn encode_message_kind(kind: SpaceAdmissionMessageKind) -> u8 {
    match kind {
        SpaceAdmissionMessageKind::JoinRequest => 1,
        SpaceAdmissionMessageKind::Candidate => 2,
        SpaceAdmissionMessageKind::Prepared => 3,
        SpaceAdmissionMessageKind::Commit => 4,
        SpaceAdmissionMessageKind::Applied => 5,
        SpaceAdmissionMessageKind::Complete => 6,
        SpaceAdmissionMessageKind::CompleteAck => 7,
        SpaceAdmissionMessageKind::Settled => 8,
        SpaceAdmissionMessageKind::CancelRequested => 9,
        SpaceAdmissionMessageKind::Rejected => 10,
    }
}

fn decode_message_kind(
    value: u8,
) -> Result<SpaceAdmissionMessageKind, SpaceAdmissionPersistenceError> {
    match value {
        1 => Ok(SpaceAdmissionMessageKind::JoinRequest),
        2 => Ok(SpaceAdmissionMessageKind::Candidate),
        3 => Ok(SpaceAdmissionMessageKind::Prepared),
        4 => Ok(SpaceAdmissionMessageKind::Commit),
        5 => Ok(SpaceAdmissionMessageKind::Applied),
        6 => Ok(SpaceAdmissionMessageKind::Complete),
        7 => Ok(SpaceAdmissionMessageKind::CompleteAck),
        8 => Ok(SpaceAdmissionMessageKind::Settled),
        9 => Ok(SpaceAdmissionMessageKind::CancelRequested),
        10 => Ok(SpaceAdmissionMessageKind::Rejected),
        _ => Err(SpaceAdmissionPersistenceError::InvalidState),
    }
}
