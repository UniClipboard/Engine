use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceAdmissionTerminationReason {
    Cancelled,
    Expired,
    Superseded,
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionLocalJoinerTerminated {
    pub(super) join_id: JoinId,
    pub(super) local_join_ordinal: u64,
    pub(super) reason: SpaceAdmissionTerminationReason,
    pub(super) cleanup: Option<AdmissionCleanupObligation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionCommitKnowledge {
    Unknown,
    Known,
}

#[derive(PartialEq, Eq)]
pub struct AdmissionCleanupObligation {
    pub(super) commit_knowledge: AdmissionCommitKnowledge,
    pub(super) member_binding: Option<AdmissionMemberBindingV2>,
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) pending_exchange: Option<PendingAdmissionExchange>,
    pub(super) local_space_transition: Option<AdmissionSpaceTransition>,
}

impl AdmissionCleanupObligation {
    pub const fn commit_knowledge(&self) -> AdmissionCommitKnowledge {
        self.commit_knowledge
    }

    pub const fn member_binding(&self) -> Option<&AdmissionMemberBindingV2> {
        self.member_binding.as_ref()
    }

    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn continuation_credential(&self) -> &AdmissionContinuationCredential {
        &self.continuation_credential
    }

    pub const fn pending_exchange(&self) -> Option<&PendingAdmissionExchange> {
        self.pending_exchange.as_ref()
    }

    /// 终止前已经保存的本机切换计划；只用于幂等隔离该尝试的目标。
    pub const fn local_space_transition(&self) -> Option<&AdmissionSpaceTransition> {
        self.local_space_transition.as_ref()
    }
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionActivePendingSettlement {
    pub(super) join_id: JoinId,
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) completion_evidence: AdmissionMessageEvidence,
    pub(super) transition_result: AdmissionSpaceTransitionResult,
    pub(super) pending_exchange: PendingAdmissionExchange,
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionActiveSettled {
    pub(super) join_id: JoinId,
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) last_received: AdmissionMessageEvidence,
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionActiveSettled {
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
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionActivePendingSettlement {
    pub const fn join_id(&self) -> JoinId {
        self.join_id
    }

    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn continuation_credential(&self) -> &AdmissionContinuationCredential {
        &self.continuation_credential
    }

    pub const fn completion_evidence(&self) -> &AdmissionMessageEvidence {
        &self.completion_evidence
    }

    pub const fn transition_result(&self) -> &AdmissionSpaceTransitionResult {
        &self.transition_result
    }

    pub const fn pending_exchange(&self) -> &PendingAdmissionExchange {
        &self.pending_exchange
    }
}

#[derive(PartialEq, Eq)]
pub enum SpaceAdmissionActiveState {
    PendingSettlement(SpaceAdmissionActivePendingSettlement),
    Settled(SpaceAdmissionActiveSettled),
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionCompletedTerminal {
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) saved_reply: SavedAdmissionReply,
    pub(super) confirmation: Option<SponsorPairingConfirmationSummary>,
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionSponsorRejected {
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) reason: SpaceAdmissionRejectionReason,
    pub(super) saved_reply: SavedAdmissionReply,
    pub(super) abandonment_cleanup: Option<SponsorAbandonmentCleanup>,
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionSponsorExpired {
    pub(super) abandonment_cleanup: SponsorAbandonmentCleanup,
}

#[derive(PartialEq, Eq)]
pub enum SponsorAbandonmentCleanup {
    NotRequired,
    Known(AdmissionMemberBindingV2),
    Unknown {
        attempt_digest: [u8; 32],
        member_instance_id: MemberInstanceId,
        add_event_id: MembershipEventId,
    },
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionLocalJoinerRejected {
    pub(super) join_id: JoinId,
    pub(super) reason: SpaceAdmissionRejectionReason,
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionLocalJoinerRejected {
    pub const fn join_id(&self) -> JoinId {
        self.join_id
    }

    pub const fn reason(&self) -> SpaceAdmissionRejectionReason {
        self.reason
    }
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionJoinerRejected {
    pub(super) join_id: JoinId,
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) reason: SpaceAdmissionRejectionReason,
    pub(super) last_received: AdmissionMessageEvidence,
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionJoinerRejected {
    pub const fn join_id(&self) -> JoinId {
        self.join_id
    }

    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn continuation_credential(&self) -> &AdmissionContinuationCredential {
        &self.continuation_credential
    }

    pub const fn reason(&self) -> SpaceAdmissionRejectionReason {
        self.reason
    }

    pub const fn last_received(&self) -> &AdmissionMessageEvidence {
        &self.last_received
    }
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionSponsorRejected {
    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn continuation_credential(&self) -> &AdmissionContinuationCredential {
        &self.continuation_credential
    }

    pub const fn reason(&self) -> SpaceAdmissionRejectionReason {
        self.reason
    }

    pub const fn saved_reply(&self) -> &SavedAdmissionReply {
        &self.saved_reply
    }
}

#[derive(PartialEq, Eq)]
pub enum SpaceAdmissionRejectedState {
    LocalJoiner(SpaceAdmissionLocalJoinerRejected),
    Joiner(SpaceAdmissionJoinerRejected),
    Sponsor(SpaceAdmissionSponsorRejected),
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionSupersededTerminal {
    pub(super) join_id: JoinId,
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) last_received: AdmissionMessageEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpaceAdmissionRecoveryRequiredTerminal {
    pub(super) category: AdmissionRecoveryCategory,
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionRecoveryRequiredTerminal {
    pub const fn category(&self) -> AdmissionRecoveryCategory {
        self.category
    }
}

#[derive(PartialEq, Eq)]
pub enum SpaceAdmissionSupersededState {
    Initiated {
        join_id: JoinId,
    },
    Authenticated {
        join_id: JoinId,
        peer_binding: AdmissionPeerBinding,
        continuation_credential: AdmissionContinuationCredential,
    },
    Candidate(SpaceAdmissionSupersededTerminal),
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionSupersededTerminal {
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
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionCompletedTerminal {
    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn continuation_credential(&self) -> &AdmissionContinuationCredential {
        &self.continuation_credential
    }

    pub const fn saved_reply(&self) -> &SavedAdmissionReply {
        &self.saved_reply
    }
}
