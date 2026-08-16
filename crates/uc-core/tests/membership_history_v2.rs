use sha2::{Digest, Sha256};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionActivationReceipt, AdmissionChangeFacts, AdmissionSecurityCommitmentV1,
    BaseMembershipHistoryPositionV1, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier, LegacyCheckpointAttestationV2, LegacyPrefixCheckpointV2,
    MembershipActivationBaselineV2, MembershipAdmissionV2, MembershipCredential,
    MembershipDecision, MembershipDecisionV1Evidence, MembershipDecisionV2, MembershipEvent,
    MembershipEventId, MembershipEventV1Evidence, MembershipEventV2, MembershipOperation,
    MembershipOperationV2, RemovalDecision, VersionedMembershipDecision, VersionedMembershipEvent,
    VersionedMembershipHistory, ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
    ED25519_SIGNATURE_ALGORITHM_V1, LEGACY_CHECKPOINT_ATTESTATION_FORMAT_V2,
    LEGACY_PREFIX_CHECKPOINT_FORMAT_V2, MEMBERSHIP_DECISION_FORMAT_V2, MEMBERSHIP_EVENT_FORMAT_V2,
};

const LINEAGE: &str = "space-lineage";

struct DeterministicSignatureVerifier;

impl DeterministicSignatureVerifier {
    fn sign(&self, credential: &MembershipCredential, payload: &[u8]) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(b"membership-history-v2-test-signature\0");
        hasher.update(&credential.public_key);
        hasher.update(payload);
        hasher.finalize().to_vec()
    }
}

impl HistoricalMembershipSignatureVerifier for DeterministicSignatureVerifier {
    fn verify(
        &self,
        signature_algorithm_version: u16,
        public_key: &[u8],
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, HistoricalMembershipSignatureError> {
        if signature_algorithm_version != ED25519_SIGNATURE_ALGORITHM_V1 {
            return Err(HistoricalMembershipSignatureError::UnsupportedAlgorithm);
        }
        let credential =
            MembershipCredential::new(signature_algorithm_version, public_key.to_vec());
        Ok(self.sign(&credential, payload) == signature)
    }
}

fn credential(byte: u8) -> MembershipCredential {
    MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![byte; 32])
}

fn admission(device: &str, credential: MembershipCredential) -> MembershipAdmissionV2 {
    let device_id = DeviceId::new(device);
    let member_instance = credential.member_instance_id(&device_id);
    MembershipAdmissionV2 {
        facts: AdmissionChangeFacts {
            member_instance,
            device_id,
            device_name: device.to_owned(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                "ABCD-EFGH-IJKL-MNOP",
            )
            .expect("test fingerprint is valid"),
            transport_public_key: vec![1],
            transport_address_blob: vec![2],
            identity_signature: vec![3],
        },
        membership_credential: credential,
        resume_public_key_digest: [7; 32],
        security_commitment_id: [8; 32],
    }
}

fn event(
    history: &VersionedMembershipHistory,
    parent_event_id: Option<MembershipEventId>,
    author_admission: &MembershipAdmissionV2,
    operation: MembershipOperationV2,
    operation_byte: u8,
    verifier: &DeterministicSignatureVerifier,
) -> MembershipEventV2 {
    let resulting_members_digest = history
        .expected_resulting_members_digest(parent_event_id, &operation)
        .expect("test event extends known history");
    let mut event = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        LINEAGE.to_owned(),
        parent_event_id,
        parent_event_id
            .map(|parent| history.depth(parent).expect("known parent") + 1)
            .unwrap_or(0),
        [operation_byte; 16],
        author_admission.facts.member_instance,
        author_admission.membership_credential.credential_id,
        author_admission
            .membership_credential
            .signature_algorithm_version,
        operation,
        resulting_members_digest,
        [operation_byte.saturating_add(1); 32],
        vec![operation_byte],
        Some([operation_byte.saturating_add(2); 32]),
        Vec::new(),
    );
    event.signature = verifier.sign(
        &author_admission.membership_credential,
        &event.signing_payload(),
    );
    event
}

fn activation_receipt(
    event: &MembershipEventV2,
    admission: &MembershipAdmissionV2,
    verifier: &DeterministicSignatureVerifier,
) -> AdmissionActivationReceipt {
    let mut receipt = AdmissionActivationReceipt::new(
        1,
        [event.operation_id[0]; 32],
        event.event_id(),
        event.resulting_members_digest,
        admission.security_commitment_id,
        admission.facts.member_instance,
        Vec::new(),
    );
    receipt.signature = verifier.sign(&admission.membership_credential, &receipt.signing_payload());
    receipt
}

fn history_with_a_and_b(
    activate_b: bool,
) -> (
    VersionedMembershipHistory,
    MembershipAdmissionV2,
    MembershipAdmissionV2,
    MembershipEventV2,
    MembershipEventV2,
) {
    let verifier = DeterministicSignatureVerifier;
    let a = admission("device-a", credential(1));
    let b = admission("device-b", credential(2));
    let mut history = VersionedMembershipHistory::new(LINEAGE.to_owned());
    let genesis = event(
        &history,
        None,
        &a,
        MembershipOperationV2::AddDevice {
            admission: a.clone(),
        },
        1,
        &verifier,
    );
    history
        .verify_and_receive_event(genesis.clone(), &verifier)
        .expect("genesis verifies");
    let add_b = event(
        &history,
        Some(genesis.event_id()),
        &a,
        MembershipOperationV2::AddDevice {
            admission: b.clone(),
        },
        2,
        &verifier,
    );
    history
        .verify_and_receive_event(add_b.clone(), &verifier)
        .expect("B admission verifies");
    if activate_b {
        history
            .verify_and_record_activation_receipt(
                activation_receipt(&add_b, &b, &verifier),
                &verifier,
            )
            .expect("B activation receipt verifies");
    }
    (history, a, b, genesis, add_b)
}

#[test]
fn removed_members_credential_still_verifies_its_past_events_for_a_new_device() {
    let verifier = DeterministicSignatureVerifier;
    let a = admission("device-a", credential(1));
    let b = admission("device-b", credential(2));
    let c = admission("device-c", credential(3));
    let d = admission("device-d", credential(4));
    let mut history = VersionedMembershipHistory::new(LINEAGE.to_owned());

    let genesis = event(
        &history,
        None,
        &a,
        MembershipOperationV2::AddDevice {
            admission: a.clone(),
        },
        1,
        &verifier,
    );
    history
        .verify_and_receive_event(genesis.clone(), &verifier)
        .expect("genesis verifies");

    let add_b = event(
        &history,
        Some(genesis.event_id()),
        &a,
        MembershipOperationV2::AddDevice {
            admission: b.clone(),
        },
        2,
        &verifier,
    );
    history
        .verify_and_receive_event(add_b.clone(), &verifier)
        .expect("B admission verifies");
    history
        .verify_and_record_activation_receipt(activation_receipt(&add_b, &b, &verifier), &verifier)
        .expect("B activation receipt verifies");

    let add_c = event(
        &history,
        Some(add_b.event_id()),
        &b,
        MembershipOperationV2::AddDevice {
            admission: c.clone(),
        },
        3,
        &verifier,
    );
    history
        .verify_and_receive_event(add_c.clone(), &verifier)
        .expect("B's past authority and credential verify C's admission");
    history
        .verify_and_record_activation_receipt(activation_receipt(&add_c, &c, &verifier), &verifier)
        .expect("C activation receipt verifies");

    let remove_b = event(
        &history,
        Some(add_c.event_id()),
        &a,
        MembershipOperationV2::RemoveDevice {
            member: b.facts.member_instance,
        },
        4,
        &verifier,
    );
    history
        .verify_and_receive_event(remove_b.clone(), &verifier)
        .expect("B removal verifies");

    let add_d = event(
        &history,
        Some(remove_b.event_id()),
        &c,
        MembershipOperationV2::AddDevice {
            admission: d.clone(),
        },
        5,
        &verifier,
    );
    history
        .verify_and_receive_event(add_d, &verifier)
        .expect("D admission verifies after B was removed");

    assert_eq!(
        history.credential_for(b.facts.member_instance),
        Some(&b.membership_credential)
    );
    assert!(!history
        .effective_members()
        .contains(&b.facts.member_instance));
    assert!(history
        .effective_members()
        .contains(&d.facts.member_instance));
}

#[test]
fn removed_member_can_sign_its_decision_from_the_removals_exact_parent() {
    let verifier = DeterministicSignatureVerifier;
    let a = admission("device-a", credential(1));
    let b = admission("device-b", credential(2));
    let mut history = VersionedMembershipHistory::new(LINEAGE.to_owned());

    let genesis = event(
        &history,
        None,
        &a,
        MembershipOperationV2::AddDevice {
            admission: a.clone(),
        },
        1,
        &verifier,
    );
    history
        .verify_and_receive_event(genesis.clone(), &verifier)
        .expect("genesis verifies");
    let add_b = event(
        &history,
        Some(genesis.event_id()),
        &a,
        MembershipOperationV2::AddDevice {
            admission: b.clone(),
        },
        2,
        &verifier,
    );
    history
        .verify_and_receive_event(add_b.clone(), &verifier)
        .expect("B admission verifies");
    history
        .verify_and_record_activation_receipt(activation_receipt(&add_b, &b, &verifier), &verifier)
        .expect("B activation receipt verifies");
    let remove_b = event(
        &history,
        Some(add_b.event_id()),
        &a,
        MembershipOperationV2::RemoveDevice {
            member: b.facts.member_instance,
        },
        3,
        &verifier,
    );
    history
        .verify_and_receive_event(remove_b.clone(), &verifier)
        .expect("B removal verifies");

    let mut decision = MembershipDecisionV2::new(
        MEMBERSHIP_DECISION_FORMAT_V2,
        LINEAGE.to_owned(),
        remove_b.event_id(),
        b.facts.member_instance,
        b.membership_credential.credential_id,
        b.membership_credential.signature_algorithm_version,
        RemovalDecision::Accept,
        Some(add_b.event_id()),
        remove_b.resulting_members_digest,
        [9; 16],
        Vec::new(),
    );
    decision.signature = verifier.sign(&b.membership_credential, &decision.signing_payload());

    history
        .verify_and_record_peer_decision(decision.clone(), &verifier)
        .expect("removed B's decision verifies from the removal parent");
    assert_eq!(
        history.decision_for(remove_b.event_id(), b.facts.member_instance),
        Some(&decision)
    );
}

#[test]
fn v1_evidence_accepts_only_the_exact_reconstructed_payload_and_original_id() {
    let a = admission("device-a", credential(1));
    let semantic_event = MembershipEvent::new(
        LINEAGE.to_owned(),
        None,
        0,
        [1; 16],
        a.facts.member_instance,
        MembershipOperation::AddDevice {
            admission: a.facts.clone(),
        },
        [2; 32],
        [3; 32],
        vec![4],
        Some([5; 32]),
        vec![6],
    );

    let evidence = MembershipEventV1Evidence::new(
        semantic_event.clone(),
        semantic_event.signing_payload(),
        semantic_event.signature.clone(),
        semantic_event.event_id(),
    )
    .expect("exact V1 evidence verifies");
    assert!(matches!(
        VersionedMembershipEvent::V1Evidence(evidence),
        VersionedMembershipEvent::V1Evidence(_)
    ));

    let mut altered_payload = semantic_event.signing_payload();
    altered_payload[0] ^= 1;
    assert_eq!(
        MembershipEventV1Evidence::new(
            semantic_event.clone(),
            altered_payload,
            semantic_event.signature.clone(),
            semantic_event.event_id(),
        ),
        Err(uc_core::membership::MembershipHistoryV2Error::InvalidLegacyEvidence)
    );
}

#[test]
fn v1_decision_evidence_preserves_the_exact_signed_record() {
    let a = admission("device-a", credential(1));
    let removal_event_id =
        MembershipEventId::from_hex(&"11".repeat(32)).expect("test removal event id is valid");
    let semantic_decision = MembershipDecision::new(
        LINEAGE.to_owned(),
        removal_event_id,
        a.facts.member_instance,
        RemovalDecision::Accept,
        Some(removal_event_id),
        [2; 32],
        [3; 16],
        vec![4],
    );

    let evidence = MembershipDecisionV1Evidence::new(
        semantic_decision.clone(),
        semantic_decision.signing_payload(),
        semantic_decision.signature.clone(),
        semantic_decision.decision_id(),
    )
    .expect("exact V1 decision evidence verifies");
    assert!(matches!(
        VersionedMembershipDecision::V1Evidence(evidence),
        VersionedMembershipDecision::V1Evidence(_)
    ));

    let different_id = MembershipDecision::new(
        LINEAGE.to_owned(),
        removal_event_id,
        a.facts.member_instance,
        RemovalDecision::Reject,
        Some(removal_event_id),
        [2; 32],
        [3; 16],
        vec![4],
    )
    .decision_id();
    assert_eq!(
        MembershipDecisionV1Evidence::new(
            semantic_decision.clone(),
            semantic_decision.signing_payload(),
            semantic_decision.signature.clone(),
            different_id,
        ),
        Err(uc_core::membership::MembershipHistoryV2Error::InvalidLegacyEvidence)
    );
}

#[test]
fn legacy_checkpoint_identity_is_independent_of_member_input_order() {
    let a = admission("device-a", credential(1));
    let c = admission("device-c", credential(3));
    let head = MembershipEventId::from_hex(&"22".repeat(32)).expect("test head is valid");

    let first = LegacyPrefixCheckpointV2::new(
        LEGACY_PREFIX_CHECKPOINT_FORMAT_V2,
        LINEAGE.to_owned(),
        head,
        7,
        [4; 32],
        [5; 32],
        [6; 32],
        vec![
            (c.facts.member_instance, c.membership_credential.clone()),
            (a.facts.member_instance, a.membership_credential.clone()),
        ],
    )
    .expect("checkpoint inputs are valid");
    let second = LegacyPrefixCheckpointV2::new(
        LEGACY_PREFIX_CHECKPOINT_FORMAT_V2,
        LINEAGE.to_owned(),
        head,
        7,
        [4; 32],
        [5; 32],
        [6; 32],
        vec![
            (a.facts.member_instance, a.membership_credential.clone()),
            (c.facts.member_instance, c.membership_credential.clone()),
        ],
    )
    .expect("checkpoint inputs are valid");

    assert_eq!(first, second);
    assert_eq!(first.checkpoint_id, second.checkpoint_id);
}

#[test]
fn checkpoint_attestations_are_additive_and_do_not_change_checkpoint_identity() {
    let verifier = DeterministicSignatureVerifier;
    let a = admission("device-a", credential(1));
    let c = admission("device-c", credential(3));
    let head = MembershipEventId::from_hex(&"22".repeat(32)).expect("test head is valid");
    let checkpoint = LegacyPrefixCheckpointV2::new(
        LEGACY_PREFIX_CHECKPOINT_FORMAT_V2,
        LINEAGE.to_owned(),
        head,
        7,
        [4; 32],
        [5; 32],
        [6; 32],
        vec![
            (a.facts.member_instance, a.membership_credential.clone()),
            (c.facts.member_instance, c.membership_credential.clone()),
        ],
    )
    .expect("checkpoint inputs are valid");

    let mut a_attestation = LegacyCheckpointAttestationV2::new(
        LEGACY_CHECKPOINT_ATTESTATION_FORMAT_V2,
        checkpoint.checkpoint_id,
        a.facts.member_instance,
        a.membership_credential.credential_id,
        Vec::new(),
    );
    a_attestation.signature =
        verifier.sign(&a.membership_credential, &a_attestation.signing_payload());
    let mut c_attestation = LegacyCheckpointAttestationV2::new(
        LEGACY_CHECKPOINT_ATTESTATION_FORMAT_V2,
        checkpoint.checkpoint_id,
        c.facts.member_instance,
        c.membership_credential.credential_id,
        Vec::new(),
    );
    c_attestation.signature =
        verifier.sign(&c.membership_credential, &c_attestation.signing_payload());

    a_attestation
        .verify(&checkpoint, &verifier)
        .expect("A can attest the checkpoint");
    c_attestation
        .verify(&checkpoint, &verifier)
        .expect("C can attest the checkpoint");
    assert_ne!(a_attestation, c_attestation);
    assert_eq!(a_attestation.checkpoint_id, checkpoint.checkpoint_id);
    assert_eq!(c_attestation.checkpoint_id, checkpoint.checkpoint_id);
}

#[test]
fn admission_security_commitment_has_a_canonical_public_identity() {
    let head = MembershipEventId::from_hex(&"22".repeat(32)).expect("test head is valid");
    let commitment = AdmissionSecurityCommitmentV1::new(
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
        LINEAGE.to_owned(),
        vec![1, 2],
        [3; 32],
        BaseMembershipHistoryPositionV1 {
            event_id: Some(head),
            depth: 7,
            history_digest: [4; 32],
        },
        [5; 32],
        0x0001,
        8,
        9,
        [6; 32],
        [7; 32],
        [8; 32],
        [9; 32],
        [10; 32],
    )
    .expect("public security commitment is valid");
    let same = AdmissionSecurityCommitmentV1::new(
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
        LINEAGE.to_owned(),
        vec![1, 2],
        [3; 32],
        commitment.base_history_position.clone(),
        [5; 32],
        0x0001,
        8,
        9,
        [6; 32],
        [7; 32],
        [8; 32],
        [9; 32],
        [10; 32],
    )
    .expect("same public security commitment is valid");
    let different_attempt = AdmissionSecurityCommitmentV1::new(
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
        LINEAGE.to_owned(),
        vec![1, 2],
        [30; 32],
        commitment.base_history_position.clone(),
        [5; 32],
        0x0001,
        8,
        9,
        [6; 32],
        [7; 32],
        [8; 32],
        [9; 32],
        [10; 32],
    )
    .expect("different attempt commitment is valid");

    assert_eq!(
        commitment.security_commitment_id,
        same.security_commitment_id
    );
    assert_ne!(
        commitment.security_commitment_id,
        different_attempt.security_commitment_id
    );
}

#[test]
fn verified_and_legacy_migrations_create_explicit_activation_baselines() {
    let verifier = DeterministicSignatureVerifier;
    let a = admission("device-a", credential(1));
    let c = admission("device-c", credential(3));
    let d = admission("device-d", credential(4));
    let head = MembershipEventId::from_hex(&"22".repeat(32)).expect("test head is valid");
    let current_credentials = vec![
        (a.facts.member_instance, a.membership_credential.clone()),
        (c.facts.member_instance, c.membership_credential.clone()),
    ];

    let mut fully_verified = VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::FullyVerifiedMigration {
            lineage_id: LINEAGE.to_owned(),
            head_event_id: head,
            head_depth: 7,
            current_member_credentials: current_credentials.clone(),
        },
    )
    .expect("fully verified migration baseline is valid");
    assert_eq!(fully_verified.active_members().len(), 2);
    let add_d = event(
        &fully_verified,
        Some(head),
        &a,
        MembershipOperationV2::AddDevice {
            admission: d.clone(),
        },
        8,
        &verifier,
    );
    fully_verified
        .verify_and_receive_event(add_d, &verifier)
        .expect("V2 history continues from fully verified migration head");

    let checkpoint = LegacyPrefixCheckpointV2::new(
        LEGACY_PREFIX_CHECKPOINT_FORMAT_V2,
        LINEAGE.to_owned(),
        head,
        7,
        [4; 32],
        [5; 32],
        [6; 32],
        current_credentials,
    )
    .expect("legacy checkpoint is valid");
    let legacy = VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::LegacyAccepted { checkpoint },
    )
    .expect("legacy accepted baseline is valid");
    assert_eq!(legacy.active_members().len(), 2);
}

#[test]
fn parent_authorization_rejects_unactivated_removed_and_wrong_credential_authors() {
    let verifier = DeterministicSignatureVerifier;
    let c = admission("device-c", credential(3));

    let (mut awaiting_history, _a, b, _genesis, add_b) = history_with_a_and_b(false);
    let awaiting_event = event(
        &awaiting_history,
        Some(add_b.event_id()),
        &b,
        MembershipOperationV2::AddDevice {
            admission: c.clone(),
        },
        3,
        &verifier,
    );
    assert_eq!(
        awaiting_history.verify_and_receive_event(awaiting_event, &verifier),
        Err(uc_core::membership::MembershipHistoryV2Error::AwaitingActivationReceipt)
    );

    let (mut history, a, b, _genesis, add_b) = history_with_a_and_b(true);
    let mut wrong_credential = event(
        &history,
        Some(add_b.event_id()),
        &b,
        MembershipOperationV2::AddDevice {
            admission: c.clone(),
        },
        3,
        &verifier,
    );
    wrong_credential.author_credential_id = a.membership_credential.credential_id;
    wrong_credential.signature = verifier.sign(
        &b.membership_credential,
        &wrong_credential.signing_payload(),
    );
    assert_eq!(
        history.verify_and_receive_event(wrong_credential, &verifier),
        Err(uc_core::membership::MembershipHistoryV2Error::InvalidCredential)
    );

    let remove_b = event(
        &history,
        Some(add_b.event_id()),
        &a,
        MembershipOperationV2::RemoveDevice {
            member: b.facts.member_instance,
        },
        4,
        &verifier,
    );
    history
        .verify_and_receive_event(remove_b.clone(), &verifier)
        .expect("B removal verifies");
    let removed_author = event(
        &history,
        Some(remove_b.event_id()),
        &b,
        MembershipOperationV2::AddDevice { admission: c },
        5,
        &verifier,
    );
    assert_eq!(
        history.verify_and_receive_event(removed_author, &verifier),
        Err(uc_core::membership::MembershipHistoryV2Error::UnauthorizedAuthor)
    );
}

#[test]
fn history_rejects_self_removal_replay_and_altered_result_digest() {
    let verifier = DeterministicSignatureVerifier;
    let (mut history, a, b, _genesis, add_b) = history_with_a_and_b(true);

    let self_removal = event(
        &history,
        Some(add_b.event_id()),
        &a,
        MembershipOperationV2::RemoveDevice {
            member: a.facts.member_instance,
        },
        3,
        &verifier,
    );
    assert_eq!(
        history.verify_and_receive_event(self_removal, &verifier),
        Err(uc_core::membership::MembershipHistoryV2Error::InvalidOperation)
    );

    let mut altered_digest = event(
        &history,
        Some(add_b.event_id()),
        &a,
        MembershipOperationV2::RemoveDevice {
            member: b.facts.member_instance,
        },
        4,
        &verifier,
    );
    altered_digest.resulting_members_digest = [99; 32];
    altered_digest.signature =
        verifier.sign(&a.membership_credential, &altered_digest.signing_payload());
    assert_eq!(
        history.verify_and_receive_event(altered_digest, &verifier),
        Err(uc_core::membership::MembershipHistoryV2Error::ResultingMembersDigestMismatch)
    );

    let remove_b = event(
        &history,
        Some(add_b.event_id()),
        &a,
        MembershipOperationV2::RemoveDevice {
            member: b.facts.member_instance,
        },
        5,
        &verifier,
    );
    history
        .verify_and_receive_event(remove_b.clone(), &verifier)
        .expect("B removal verifies");
    let mut replay = event(
        &history,
        Some(remove_b.event_id()),
        &a,
        MembershipOperationV2::AddDevice {
            admission: admission("device-c", credential(3)),
        },
        6,
        &verifier,
    );
    replay.operation_id = remove_b.operation_id;
    replay.signature = verifier.sign(&a.membership_credential, &replay.signing_payload());
    assert_eq!(
        history.verify_and_receive_event(replay, &verifier),
        Err(uc_core::membership::MembershipHistoryV2Error::OperationReplay)
    );
}

#[test]
fn activation_receipts_require_the_event_and_conflicts_fail_closed() {
    let verifier = DeterministicSignatureVerifier;
    let (mut history, _a, b, _genesis, add_b) = history_with_a_and_b(false);
    let mut before_event = activation_receipt(&add_b, &b, &verifier);
    before_event.event_id =
        MembershipEventId::from_hex(&"33".repeat(32)).expect("test event id is valid");
    before_event.signature =
        verifier.sign(&b.membership_credential, &before_event.signing_payload());
    assert!(matches!(
        history.verify_and_record_activation_receipt(before_event, &verifier),
        Err(uc_core::membership::MembershipHistoryV2Error::MissingMembershipEvent(_))
    ));

    let receipt = activation_receipt(&add_b, &b, &verifier);
    history
        .verify_and_record_activation_receipt(receipt.clone(), &verifier)
        .expect("first receipt verifies");
    let mut conflict = receipt;
    conflict.attempt_id = [44; 32];
    conflict.signature = verifier.sign(&b.membership_credential, &conflict.signing_payload());
    assert_eq!(
        history.verify_and_record_activation_receipt(conflict, &verifier),
        Err(uc_core::membership::MembershipHistoryV2Error::ActivationReceiptConflict)
    );
}

#[test]
fn same_device_rejoins_as_a_new_instance_without_losing_old_credential() {
    let verifier = DeterministicSignatureVerifier;
    let (mut history, a, old_b, _genesis, add_b) = history_with_a_and_b(true);
    let remove_b = event(
        &history,
        Some(add_b.event_id()),
        &a,
        MembershipOperationV2::RemoveDevice {
            member: old_b.facts.member_instance,
        },
        3,
        &verifier,
    );
    history
        .verify_and_receive_event(remove_b.clone(), &verifier)
        .expect("old B is removed");

    let new_b = admission("device-b", credential(22));
    let rejoin = event(
        &history,
        Some(remove_b.event_id()),
        &a,
        MembershipOperationV2::AddDevice {
            admission: new_b.clone(),
        },
        4,
        &verifier,
    );
    history
        .verify_and_receive_event(rejoin, &verifier)
        .expect("same device rejoins with a new credential and instance");

    assert_ne!(old_b.facts.member_instance, new_b.facts.member_instance);
    assert_eq!(
        history.credential_for(old_b.facts.member_instance),
        Some(&old_b.membership_credential)
    );
    assert_eq!(
        history.credential_for(new_b.facts.member_instance),
        Some(&new_b.membership_credential)
    );
}

#[test]
fn canonical_records_reject_tampered_identity_after_loading() {
    let a = admission("device-a", credential(1));
    let head = MembershipEventId::from_hex(&"22".repeat(32)).expect("test head is valid");
    let mut checkpoint = LegacyPrefixCheckpointV2::new(
        LEGACY_PREFIX_CHECKPOINT_FORMAT_V2,
        LINEAGE.to_owned(),
        head,
        7,
        [4; 32],
        [5; 32],
        [6; 32],
        vec![(a.facts.member_instance, a.membership_credential.clone())],
    )
    .expect("checkpoint inputs are valid");
    checkpoint.checkpoint_id[0] ^= 1;
    assert_eq!(
        checkpoint.validate(),
        Err(uc_core::membership::MembershipHistoryV2Error::InvalidLegacyEvidence)
    );

    let mut commitment = AdmissionSecurityCommitmentV1::new(
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
        LINEAGE.to_owned(),
        vec![1],
        [2; 32],
        BaseMembershipHistoryPositionV1 {
            event_id: Some(head),
            depth: 7,
            history_digest: [3; 32],
        },
        [4; 32],
        1,
        8,
        9,
        [5; 32],
        [6; 32],
        [7; 32],
        [8; 32],
        [9; 32],
    )
    .expect("commitment inputs are valid");
    commitment.security_commitment_id[0] ^= 1;
    assert_eq!(
        commitment.validate(),
        Err(uc_core::membership::MembershipHistoryV2Error::InvalidSecurityCommitment)
    );
}

#[test]
fn verified_history_persistence_round_trip_preserves_authority_and_receipts() {
    let (history, _a, b, _genesis, add_b) = history_with_a_and_b(true);

    let encoded = history.encode_persisted_v2().unwrap();
    let reopened =
        VersionedMembershipHistory::decode_persisted_v2(&encoded, &DeterministicSignatureVerifier)
            .unwrap();

    assert_eq!(reopened, history);
    assert!(reopened.active_members().contains(&b.facts.member_instance));
    assert_eq!(reopened.depth(add_b.event_id()), Some(1));
}
