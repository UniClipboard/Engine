use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SponsorPairingConfirmationStatus {
    AwaitingPeerConfirmation,
    Unconfirmed,
    Confirmed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SponsorPairingConfirmationSummary {
    pub(super) status: SponsorPairingConfirmationStatus,
    pub(super) admission_id: SpaceAdmissionId,
    pub(super) member_instance_id: MemberInstanceId,
    pub(super) add_event_id: MembershipEventId,
}

impl SponsorPairingConfirmationSummary {
    pub const fn status(self) -> SponsorPairingConfirmationStatus {
        self.status
    }

    pub const fn admission_id(self) -> SpaceAdmissionId {
        self.admission_id
    }

    pub const fn member_instance_id(self) -> MemberInstanceId {
        self.member_instance_id
    }

    pub const fn add_event_id(self) -> MembershipEventId {
        self.add_event_id
    }

    pub(super) const fn with_status(self, status: SponsorPairingConfirmationStatus) -> Self {
        Self { status, ..self }
    }
}

impl std::fmt::Debug for SponsorPairingConfirmationSummary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SponsorPairingConfirmationSummary([REDACTED])")
    }
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionSponsorAccepted {
    pub(super) invitation_claim: AdmissionInvitationClaim,
    pub(super) join_request: SpaceAdmissionEnvelopeV1,
    pub(super) join_request_evidence: AdmissionMessageEvidence,
    pub(super) base_snapshot: AdmissionBaseSnapshot,
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionSponsorAccepted {
    pub const fn invitation_claim(&self) -> &AdmissionInvitationClaim {
        &self.invitation_claim
    }

    pub const fn join_request(&self) -> &SpaceAdmissionEnvelopeV1 {
        &self.join_request
    }

    pub const fn join_request_evidence(&self) -> &AdmissionMessageEvidence {
        &self.join_request_evidence
    }

    pub const fn base_snapshot(&self) -> &AdmissionBaseSnapshot {
        &self.base_snapshot
    }

    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn continuation_credential(&self) -> &AdmissionContinuationCredential {
        &self.continuation_credential
    }
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionSponsorCandidate {
    pub(super) invitation_claim: AdmissionInvitationClaim,
    pub(super) base_snapshot: AdmissionBaseSnapshot,
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) staged_security: AdmissionStagedSecurityState,
    pub(super) saved_reply: SavedAdmissionReply,
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionSponsorCommitted {
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) committed_history: AdmissionSignedMembershipHistory,
    pub(super) sealed_security: AdmissionSealedSecurityState,
    pub(super) saved_reply: SavedAdmissionReply,
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionSponsorApplied {
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) committed_history: AdmissionSignedMembershipHistory,
    pub(super) activation_receipt: AdmissionActivationReceipt,
    pub(super) activated_security: AdmissionActivatedSecurityState,
    pub(super) saved_reply: SavedAdmissionReply,
    pub(super) confirmation: Option<SponsorPairingConfirmationSummary>,
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionSponsorApplied {
    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn continuation_credential(&self) -> &AdmissionContinuationCredential {
        &self.continuation_credential
    }

    pub const fn committed_history(&self) -> &AdmissionSignedMembershipHistory {
        &self.committed_history
    }

    pub const fn activation_receipt(&self) -> &AdmissionActivationReceipt {
        &self.activation_receipt
    }

    pub const fn activated_security(&self) -> &AdmissionActivatedSecurityState {
        &self.activated_security
    }

    pub const fn saved_reply(&self) -> &SavedAdmissionReply {
        &self.saved_reply
    }
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionSponsorCommitted {
    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn continuation_credential(&self) -> &AdmissionContinuationCredential {
        &self.continuation_credential
    }

    pub const fn committed_history(&self) -> &AdmissionSignedMembershipHistory {
        &self.committed_history
    }

    pub const fn sealed_security(&self) -> &AdmissionSealedSecurityState {
        &self.sealed_security
    }

    pub const fn saved_reply(&self) -> &SavedAdmissionReply {
        &self.saved_reply
    }
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionSponsorCandidate {
    pub const fn invitation_claim(&self) -> &AdmissionInvitationClaim {
        &self.invitation_claim
    }

    pub const fn base_snapshot(&self) -> &AdmissionBaseSnapshot {
        &self.base_snapshot
    }

    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn continuation_credential(&self) -> &AdmissionContinuationCredential {
        &self.continuation_credential
    }

    pub const fn staged_security(&self) -> &AdmissionStagedSecurityState {
        &self.staged_security
    }

    pub const fn saved_reply(&self) -> &SavedAdmissionReply {
        &self.saved_reply
    }
}

#[derive(PartialEq, Eq)]
pub enum SpaceAdmissionSponsorState {
    Accepted(SpaceAdmissionSponsorAccepted),
    Candidate(SpaceAdmissionSponsorCandidate),
    Committed(SpaceAdmissionSponsorCommitted),
    Applied(SpaceAdmissionSponsorApplied),
}
