use super::*;

#[derive(PartialEq, Eq)]
pub enum SpaceAdmissionInvitationResolutionState {
    Ready {
        short_code: AdmissionShortInvitationCode,
    },
    Started,
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionJoinerResolvingInvitation {
    pub(super) join_id: JoinId,
    pub(super) local_join_ordinal: u64,
    pub(super) source_snapshot: AdmissionSourceSnapshot,
    pub(super) start_context: AdmissionJoinerStartContext,
    pub(super) resolution: SpaceAdmissionInvitationResolutionState,
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionJoinerResolvedInvitation {
    pub(super) join_id: JoinId,
    pub(super) local_join_ordinal: u64,
    pub(super) source_snapshot: AdmissionSourceSnapshot,
    pub(super) start_context: AdmissionJoinerStartContext,
    pub(super) full_invitation: FullInvitation,
}

#[derive(PartialEq, Eq)]
pub enum SpaceAdmissionJoinerChannelState {
    AwaitingAuthentication {
        encrypted_password_equivalent: AdmissionEncryptedPasswordEquivalent,
    },
    Authenticated {
        peer_binding: AdmissionPeerBinding,
        continuation_credential: AdmissionContinuationCredential,
    },
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionJoinerInitiated {
    pub(super) join_id: JoinId,
    pub(super) local_join_ordinal: u64,
    pub(super) source_snapshot: AdmissionSourceSnapshot,
    pub(super) private_state: AdmissionJoinerPrivateState,
    pub(super) channel_state: SpaceAdmissionJoinerChannelState,
    pub(super) pending_exchange: PendingAdmissionExchange,
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionJoinerCandidate {
    pub(super) join_id: JoinId,
    pub(super) local_join_ordinal: u64,
    pub(super) source_snapshot: AdmissionSourceSnapshot,
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) candidate: SpaceAdmissionEnvelopeV1,
    pub(super) candidate_evidence: AdmissionMessageEvidence,
    pub(super) staged_target_input: AdmissionStagedTargetInput,
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionJoinerPrepared {
    pub(super) join_id: JoinId,
    pub(super) local_join_ordinal: u64,
    pub(super) source_snapshot: AdmissionSourceSnapshot,
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) candidate_evidence: AdmissionMessageEvidence,
    pub(super) verified_history: AdmissionSignedMembershipHistory,
    pub(super) staged_target: AdmissionStagedTarget,
    pub(super) pending_exchange: PendingAdmissionExchange,
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionJoinerCommitted {
    pub(super) join_id: JoinId,
    pub(super) local_join_ordinal: u64,
    pub(super) source_snapshot: AdmissionSourceSnapshot,
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) exact_commit: SpaceAdmissionEnvelopeV1,
    pub(super) commit_evidence: AdmissionMessageEvidence,
    pub(super) staged_target: AdmissionStagedTarget,
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionJoinerApplied {
    pub(super) join_id: JoinId,
    pub(super) local_join_ordinal: u64,
    pub(super) source_snapshot: AdmissionSourceSnapshot,
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) exact_commit: SpaceAdmissionEnvelopeV1,
    pub(super) commit_evidence: AdmissionMessageEvidence,
    pub(super) staged_target: AdmissionStagedTarget,
    pub(super) pending_exchange: PendingAdmissionExchange,
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionJoinerActivating {
    pub(super) join_id: JoinId,
    pub(super) local_join_ordinal: u64,
    pub(super) source_snapshot: AdmissionSourceSnapshot,
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) exact_commit: SpaceAdmissionEnvelopeV1,
    pub(super) staged_target: AdmissionStagedTarget,
    pub(super) completion: SpaceAdmissionEnvelopeV1,
    pub(super) completion_evidence: AdmissionMessageEvidence,
    pub(super) space_transition: AdmissionSpaceTransition,
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionJoinerCancelling {
    pub(super) join_id: JoinId,
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) last_received: AdmissionMessageEvidence,
    pub(super) pending_exchange: PendingAdmissionExchange,
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionJoinerCancelling {
    pub const fn join_id(&self) -> JoinId {
        self.join_id
    }

    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn continuation_credential(&self) -> &AdmissionContinuationCredential {
        &self.continuation_credential
    }

    pub const fn last_received(&self) -> &AdmissionMessageEvidence {
        &self.last_received
    }

    pub const fn pending_exchange(&self) -> &PendingAdmissionExchange {
        &self.pending_exchange
    }
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionJoinerActivating {
    pub const fn join_id(&self) -> JoinId {
        self.join_id
    }

    pub const fn local_join_ordinal(&self) -> u64 {
        self.local_join_ordinal
    }

    pub const fn source_snapshot(&self) -> &AdmissionSourceSnapshot {
        &self.source_snapshot
    }

    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn continuation_credential(&self) -> &AdmissionContinuationCredential {
        &self.continuation_credential
    }

    pub const fn exact_commit(&self) -> &SpaceAdmissionEnvelopeV1 {
        &self.exact_commit
    }

    pub const fn staged_target(&self) -> &AdmissionStagedTarget {
        &self.staged_target
    }

    pub const fn completion(&self) -> &SpaceAdmissionEnvelopeV1 {
        &self.completion
    }

    pub const fn completion_evidence(&self) -> &AdmissionMessageEvidence {
        &self.completion_evidence
    }

    pub const fn space_transition(&self) -> &AdmissionSpaceTransition {
        &self.space_transition
    }
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionJoinerApplied {
    pub const fn join_id(&self) -> JoinId {
        self.join_id
    }

    pub const fn local_join_ordinal(&self) -> u64 {
        self.local_join_ordinal
    }

    pub const fn source_snapshot(&self) -> &AdmissionSourceSnapshot {
        &self.source_snapshot
    }

    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn continuation_credential(&self) -> &AdmissionContinuationCredential {
        &self.continuation_credential
    }

    pub const fn exact_commit(&self) -> &SpaceAdmissionEnvelopeV1 {
        &self.exact_commit
    }

    pub const fn commit_evidence(&self) -> &AdmissionMessageEvidence {
        &self.commit_evidence
    }

    pub const fn staged_target(&self) -> &AdmissionStagedTarget {
        &self.staged_target
    }

    pub const fn pending_exchange(&self) -> &PendingAdmissionExchange {
        &self.pending_exchange
    }
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionJoinerCommitted {
    pub const fn join_id(&self) -> JoinId {
        self.join_id
    }

    pub const fn local_join_ordinal(&self) -> u64 {
        self.local_join_ordinal
    }

    pub const fn source_snapshot(&self) -> &AdmissionSourceSnapshot {
        &self.source_snapshot
    }

    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn continuation_credential(&self) -> &AdmissionContinuationCredential {
        &self.continuation_credential
    }

    pub const fn exact_commit(&self) -> &SpaceAdmissionEnvelopeV1 {
        &self.exact_commit
    }

    pub const fn commit_evidence(&self) -> &AdmissionMessageEvidence {
        &self.commit_evidence
    }

    pub const fn staged_target(&self) -> &AdmissionStagedTarget {
        &self.staged_target
    }
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionJoinerCandidate {
    pub const fn join_id(&self) -> JoinId {
        self.join_id
    }

    pub const fn local_join_ordinal(&self) -> u64 {
        self.local_join_ordinal
    }

    pub const fn source_snapshot(&self) -> &AdmissionSourceSnapshot {
        &self.source_snapshot
    }

    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn continuation_credential(&self) -> &AdmissionContinuationCredential {
        &self.continuation_credential
    }

    pub const fn candidate(&self) -> &SpaceAdmissionEnvelopeV1 {
        &self.candidate
    }

    pub const fn candidate_evidence(&self) -> &AdmissionMessageEvidence {
        &self.candidate_evidence
    }

    pub const fn staged_target_input(&self) -> &AdmissionStagedTargetInput {
        &self.staged_target_input
    }
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionJoinerPrepared {
    pub const fn join_id(&self) -> JoinId {
        self.join_id
    }

    pub const fn local_join_ordinal(&self) -> u64 {
        self.local_join_ordinal
    }

    pub const fn source_snapshot(&self) -> &AdmissionSourceSnapshot {
        &self.source_snapshot
    }

    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn continuation_credential(&self) -> &AdmissionContinuationCredential {
        &self.continuation_credential
    }

    pub const fn candidate_evidence(&self) -> &AdmissionMessageEvidence {
        &self.candidate_evidence
    }

    pub const fn verified_history(&self) -> &AdmissionSignedMembershipHistory {
        &self.verified_history
    }

    pub const fn staged_target(&self) -> &AdmissionStagedTarget {
        &self.staged_target
    }

    pub const fn pending_exchange(&self) -> &PendingAdmissionExchange {
        &self.pending_exchange
    }
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionJoinerInitiated {
    pub const fn join_id(&self) -> JoinId {
        self.join_id
    }

    pub const fn local_join_ordinal(&self) -> u64 {
        self.local_join_ordinal
    }

    pub const fn source_snapshot(&self) -> &AdmissionSourceSnapshot {
        &self.source_snapshot
    }

    pub const fn channel_state(&self) -> &SpaceAdmissionJoinerChannelState {
        &self.channel_state
    }

    pub const fn pending_exchange(&self) -> &PendingAdmissionExchange {
        &self.pending_exchange
    }
}

#[derive(PartialEq, Eq)]
pub enum SpaceAdmissionJoinerState {
    ResolvingInvitation(SpaceAdmissionJoinerResolvingInvitation),
    ResolvedInvitation(SpaceAdmissionJoinerResolvedInvitation),
    Initiated(SpaceAdmissionJoinerInitiated),
    Candidate(SpaceAdmissionJoinerCandidate),
    Prepared(SpaceAdmissionJoinerPrepared),
    Committed(SpaceAdmissionJoinerCommitted),
    Applied(SpaceAdmissionJoinerApplied),
    Activating(SpaceAdmissionJoinerActivating),
    Cancelling(SpaceAdmissionJoinerCancelling),
}
