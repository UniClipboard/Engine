use std::collections::BTreeSet;

use crate::ids::DeviceId;
use crate::membership::{
    AdmissionActivationReceipt, AdmissionChangeFacts, AdmissionCompletionV1,
    AdmissionSecurityCommitmentV1, BaseMembershipHistoryPosition, MemberInstanceId,
    MembershipAdmissionV2, MembershipCredential, MembershipEventV2, MembershipOperationV2,
    PreparedAdmissionProofV1, ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
    ED25519_SIGNATURE_ALGORITHM_V1, MEMBERSHIP_EVENT_FORMAT_V2,
};
use crate::security::IdentityFingerprint;

use super::{
    AdmissionAppliedV1, AdmissionArtifactError, AdmissionBaseSnapshot, AdmissionCandidateV1,
    AdmissionChannelPeerId, AdmissionCommitV1, AdmissionCompleteAckV1, AdmissionCompleteV1,
    AdmissionContinuationCredential, AdmissionContinuationRoute, AdmissionEffect,
    AdmissionEncryptedPasswordEquivalent, AdmissionErrorCategory, AdmissionEvidenceRelation,
    AdmissionIdentitySignature, AdmissionInboundDecision, AdmissionInboundExpectation,
    AdmissionInvitationClaim, AdmissionJoinRequestV1, AdmissionJoinerPrivateState,
    AdmissionKeyPackage, AdmissionMessageEvidence, AdmissionMessageHeaderError, AdmissionMessageId,
    AdmissionMlsCommit, AdmissionMlsWelcome, AdmissionPeerBinding, AdmissionPendingExchangeError,
    AdmissionPendingRecovery, AdmissionPreparedV1, AdmissionProtocolMessageError,
    AdmissionRecoveryCategory, AdmissionRecoveryPublicKey, AdmissionReplayDecision,
    AdmissionReplayError, AdmissionRetryState, AdmissionRole, AdmissionSealedRecoveryMaterial,
    AdmissionSettledV1, AdmissionSignedMembershipHistory, AdmissionSourceSnapshot,
    AdmissionStagedSecurityState, AdmissionStagedTargetInput, InvitationId, JoinId,
    PendingAdmissionExchange, SavedAdmissionReply, SpaceAdmissionActiveState,
    SpaceAdmissionAggregate, SpaceAdmissionAggregateError, SpaceAdmissionBodyV1,
    SpaceAdmissionCompletionHelperState, SpaceAdmissionEnvelopeHeaderV1, SpaceAdmissionEnvelopeV1,
    SpaceAdmissionId, SpaceAdmissionJoinerChannelState, SpaceAdmissionJoinerState,
    SpaceAdmissionMessageKind, SpaceAdmissionProtocolVersion, SpaceAdmissionRecordState,
    SpaceAdmissionRejectedState, SpaceAdmissionRejectionReason, SpaceAdmissionRoute,
    SpaceAdmissionSponsorState, SpaceAdmissionTerminalState, SponsorAdmission,
    UnreadableHistoryPolicy,
};

mod exchange;
mod id;
mod message;
mod state;

fn candidate_body_fixture() -> AdmissionCandidateV1 {
    let sponsor_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0xb1; 32]);
    let joiner_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0xb2; 32]);
    let joiner_device = DeviceId::new("candidate-joiner");
    let joiner_member = joiner_credential.member_instance_id(&joiner_device);
    let admission = MembershipAdmissionV2 {
        facts: AdmissionChangeFacts {
            member_instance: joiner_member,
            device_id: joiner_device,
            device_name: "candidate-joiner".to_owned(),
            identity_fingerprint: IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
                .expect("valid fingerprint fixture"),
            transport_public_key: vec![0xb3; 32],
            transport_address_blob: vec![0xb4; 16],
            identity_signature: vec![0xb5; 64],
        },
        membership_credential: joiner_credential,
        resume_public_key_digest: [0xb6; 32],
        security_commitment_id: [0xb7; 32],
    };
    let candidate_event = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        "lineage".to_owned(),
        None,
        0,
        [0xb8; 16],
        MemberInstanceId::from_bytes([0xb9; 32]),
        sponsor_credential.credential_id,
        ED25519_SIGNATURE_ALGORITHM_V1,
        MembershipOperationV2::AddDevice { admission },
        [0xba; 32],
        [0xbb; 32],
        vec![0xbc],
        Some([0xbd; 32]),
        vec![0xbe; 64],
    );
    let base_position = BaseMembershipHistoryPosition {
        event_id: None,
        depth: 0,
        history_digest: [0xbf; 32],
    };
    let security_commitment = AdmissionSecurityCommitmentV1::new(
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
        "lineage".to_owned(),
        vec![0xc0; 16],
        [0xc1; 32],
        base_position,
        [0xc2; 32],
        1,
        0,
        1,
        [0xc3; 32],
        [0xc4; 32],
        [0xc5; 32],
        [0xc6; 32],
        [0xc7; 32],
    )
    .expect("valid security commitment fixture");

    AdmissionCandidateV1::new(
        AdmissionSignedMembershipHistory::from_bytes(vec![0xc8; 64])
            .expect("bounded history fixture"),
        candidate_event,
        security_commitment,
        AdmissionMlsCommit::from_bytes(vec![0xc9; 64]).expect("bounded MLS commit fixture"),
        AdmissionMlsWelcome::from_bytes(vec![0xca; 64]).expect("bounded MLS welcome fixture"),
        AdmissionContinuationRoute::from_bytes(vec![0xcb; 32])
            .expect("bounded continuation route fixture"),
    )
    .expect("AddDevice candidate fixture")
}
