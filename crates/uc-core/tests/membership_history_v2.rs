use sha2::{Digest, Sha256};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionActivationReceipt, AdmissionChangeFacts, AdmissionContentKeyCatalogV1,
    AdmissionContentKeyEntryV1, AdmissionSecurityCommitmentV1, BaseMembershipHistoryPositionV1,
    HistoricalMembershipSignatureError, HistoricalMembershipSignatureVerifier,
    LegacyCheckpointAttestationV2, LegacyPrefixCheckpointV2, MembershipActivationBaselineV2,
    MembershipAdmissionV2, MembershipCredential, MembershipDecision, MembershipDecisionV1Evidence,
    MembershipDecisionV2, MembershipEvent, MembershipEventId, MembershipEventV1Evidence,
    MembershipEventV2, MembershipHistoryMessage, MembershipOperation, MembershipOperationV2,
    RemovalDecision, VersionedMembershipDecision, VersionedMembershipEvent,
    VersionedMembershipHistory, ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
    ED25519_SIGNATURE_ALGORITHM_V1, LEGACY_CHECKPOINT_ATTESTATION_FORMAT_V2,
    LEGACY_PREFIX_CHECKPOINT_FORMAT_V2, MAX_MEMBERSHIP_HISTORY_FRAME_SIZE,
    MEMBERSHIP_DECISION_FORMAT_V2, MEMBERSHIP_EVENT_FORMAT_V2,
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

fn numbered_event(
    history: &VersionedMembershipHistory,
    parent_event_id: Option<MembershipEventId>,
    author_admission: &MembershipAdmissionV2,
    operation: MembershipOperationV2,
    operation_number: u16,
    verifier: &DeterministicSignatureVerifier,
) -> MembershipEventV2 {
    let resulting_members_digest = history
        .expected_resulting_members_digest(parent_event_id, &operation)
        .expect("test event extends known history");
    let mut operation_id = [0; 16];
    operation_id[..2].copy_from_slice(&operation_number.to_be_bytes());
    let marker = operation_number.to_be_bytes()[1];
    let mut event = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        LINEAGE.to_owned(),
        parent_event_id,
        parent_event_id
            .map(|parent| history.depth(parent).expect("known parent") + 1)
            .unwrap_or(0),
        operation_id,
        author_admission.facts.member_instance,
        author_admission.membership_credential.credential_id,
        author_admission
            .membership_credential
            .signature_algorithm_version,
        operation,
        resulting_members_digest,
        [marker; 32],
        vec![marker],
        Some([marker.wrapping_add(1); 32]),
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
fn remote_v2_removal_waits_for_the_local_decision_and_preserves_each_branch() {
    let verifier = DeterministicSignatureVerifier;
    let (mut author_history, a, b, _, add_b) = history_with_a_and_b(true);
    let removal = event(
        &author_history,
        Some(add_b.event_id()),
        &a,
        MembershipOperationV2::RemoveDevice {
            member: b.facts.member_instance,
        },
        3,
        &verifier,
    );
    author_history
        .verify_and_receive_event(removal.clone(), &verifier)
        .expect("the author applies its own removal");

    let mut accepting_history = VersionedMembershipHistory::decode_persisted_v2(
        &history_with_a_and_b(true).0.encode_persisted_v2().unwrap(),
        &verifier,
    )
    .unwrap();
    accepting_history
        .merge_remote_history(&author_history, b.facts.member_instance, &verifier)
        .expect("the remote removal verifies");
    assert_eq!(
        accepting_history.pending_removal_decision(b.facts.member_instance),
        Some(removal.event_id())
    );
    assert!(accepting_history
        .effective_members()
        .contains(&b.facts.member_instance));
    let reopened_pending = VersionedMembershipHistory::decode_persisted_v2(
        &accepting_history.encode_persisted_v2().unwrap(),
        &verifier,
    )
    .expect("pending removal survives reopening");
    assert_eq!(
        reopened_pending.pending_removal_decision(b.facts.member_instance),
        Some(removal.event_id())
    );

    let mut acceptance = MembershipDecisionV2::new(
        MEMBERSHIP_DECISION_FORMAT_V2,
        LINEAGE.to_owned(),
        removal.event_id(),
        b.facts.member_instance,
        b.membership_credential.credential_id,
        b.membership_credential.signature_algorithm_version,
        RemovalDecision::Accept,
        Some(add_b.event_id()),
        removal.resulting_members_digest,
        [4; 16],
        Vec::new(),
    );
    acceptance.signature = verifier.sign(&b.membership_credential, &acceptance.signing_payload());
    accepting_history
        .verify_and_record_local_decision(acceptance, b.facts.member_instance, &verifier)
        .expect("acceptance advances the local branch");
    assert!(!accepting_history
        .effective_members()
        .contains(&b.facts.member_instance));
    let reopened_acceptance = VersionedMembershipHistory::decode_persisted_v2(
        &accepting_history.encode_persisted_v2().unwrap(),
        &verifier,
    )
    .expect("accepted removal survives reopening");
    assert!(!reopened_acceptance
        .effective_members()
        .contains(&b.facts.member_instance));

    let mut rejecting_history = history_with_a_and_b(true).0;
    rejecting_history
        .merge_remote_history(&author_history, b.facts.member_instance, &verifier)
        .expect("the same removal verifies on the rejecting branch");
    let mut rejection = MembershipDecisionV2::new(
        MEMBERSHIP_DECISION_FORMAT_V2,
        LINEAGE.to_owned(),
        removal.event_id(),
        b.facts.member_instance,
        b.membership_credential.credential_id,
        b.membership_credential.signature_algorithm_version,
        RemovalDecision::Reject,
        Some(add_b.event_id()),
        add_b.resulting_members_digest,
        [5; 16],
        Vec::new(),
    );
    rejection.signature = verifier.sign(&b.membership_credential, &rejection.signing_payload());
    rejecting_history
        .verify_and_record_local_decision(rejection, b.facts.member_instance, &verifier)
        .expect("rejection preserves the local branch");
    assert!(rejecting_history
        .effective_members()
        .contains(&b.facts.member_instance));
    assert_eq!(
        rejecting_history.pending_removal_decision(b.facts.member_instance),
        None
    );
    let reopened_rejection = VersionedMembershipHistory::decode_persisted_v2(
        &rejecting_history.encode_persisted_v2().unwrap(),
        &verifier,
    )
    .expect("rejected removal survives reopening");
    assert!(reopened_rejection
        .effective_members()
        .contains(&b.facts.member_instance));
    assert_eq!(
        rejecting_history.removal_decision_recipients_for(b.facts.member_instance),
        [a.facts.member_instance].into()
    );
    assert!(
        rejecting_history.removal_choices_diverge(a.facts.member_instance, b.facts.member_instance)
    );
}

#[test]
fn active_sender_may_omit_receiver_decisions_but_cannot_carry_anothers_new_decision() {
    let verifier = DeterministicSignatureVerifier;
    let (mut sender_history, a, b, _, add_b) = history_with_a_and_b(true);
    let removal = event(
        &sender_history,
        Some(add_b.event_id()),
        &a,
        MembershipOperationV2::RemoveDevice {
            member: b.facts.member_instance,
        },
        3,
        &verifier,
    );
    sender_history
        .verify_and_receive_event(removal.clone(), &verifier)
        .expect("removal verifies");

    let mut receiver_history = sender_history.clone();
    let mut b_decision = MembershipDecisionV2::new(
        MEMBERSHIP_DECISION_FORMAT_V2,
        LINEAGE.to_owned(),
        removal.event_id(),
        b.facts.member_instance,
        b.membership_credential.credential_id,
        b.membership_credential.signature_algorithm_version,
        RemovalDecision::Accept,
        Some(add_b.event_id()),
        removal.resulting_members_digest,
        [6; 16],
        Vec::new(),
    );
    b_decision.signature = verifier.sign(&b.membership_credential, &b_decision.signing_payload());
    receiver_history
        .verify_and_record_peer_decision(b_decision.clone(), &verifier)
        .expect("receiver stores B's decision");

    assert!(sender_history
        .is_authorized_active_member_extension_of(&receiver_history, a.facts.member_instance));
    let mut merged = receiver_history.clone();
    assert!(!merged
        .merge_remote_history(&sender_history, a.facts.member_instance, &verifier)
        .expect("sender may omit a decision already held by the receiver"));
    assert_eq!(
        merged.decision_for(removal.event_id(), b.facts.member_instance),
        Some(&b_decision)
    );

    let previous_without_decision = sender_history.clone();
    let mut smuggled = sender_history;
    smuggled
        .verify_and_record_peer_decision(b_decision, &verifier)
        .expect("the decision is cryptographically valid");
    assert!(!smuggled.is_authorized_active_member_extension_of(
        &previous_without_decision,
        a.facts.member_instance
    ));
}

#[test]
fn runtime_exchange_carries_only_the_senders_decisions() {
    let verifier = DeterministicSignatureVerifier;
    let (mut history, a, b, _, add_b) = history_with_a_and_b(true);
    let removal = event(
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
        .verify_and_receive_event(removal.clone(), &verifier)
        .expect("removal verifies");
    let mut b_decision = MembershipDecisionV2::new(
        MEMBERSHIP_DECISION_FORMAT_V2,
        LINEAGE.to_owned(),
        removal.event_id(),
        b.facts.member_instance,
        b.membership_credential.credential_id,
        b.membership_credential.signature_algorithm_version,
        RemovalDecision::Accept,
        Some(add_b.event_id()),
        removal.resulting_members_digest,
        [7; 16],
        Vec::new(),
    );
    b_decision.signature = verifier.sign(&b.membership_credential, &b_decision.signing_payload());
    history
        .verify_and_record_peer_decision(b_decision, &verifier)
        .expect("A stores B's decision");

    let mut sender_facts = a.facts.clone();
    sender_facts.identity_signature =
        verifier.sign(&a.membership_credential, &sender_facts.signing_payload());
    let pages = history
        .export_reconciliation_pages_v2(sender_facts)
        .expect("A exports a self-authorized runtime view");
    let imported = VersionedMembershipHistory::import_exchange_pages_v2(&pages, &verifier)
        .expect("the runtime view remains self-consistent and verifiable");

    let imported_position = imported.current_position().unwrap();
    let full_position = history.current_position().unwrap();
    assert_eq!(imported_position.event_id, full_position.event_id);
    assert_eq!(imported_position.depth, full_position.depth);
    assert_eq!(
        imported.decision_for(removal.event_id(), b.facts.member_instance),
        None
    );
    assert!(history
        .decision_for(removal.event_id(), b.facts.member_instance)
        .is_some());

    let mut removed_sender_facts = b.facts.clone();
    removed_sender_facts.identity_signature = verifier.sign(
        &b.membership_credential,
        &removed_sender_facts.signing_payload(),
    );
    let removed_sender_pages = history
        .export_reconciliation_pages_v2(removed_sender_facts)
        .expect("removed B may deliver B's own decision");
    let removed_sender_view =
        VersionedMembershipHistory::import_exchange_pages_v2(&removed_sender_pages, &verifier)
            .expect("B's restricted decision view verifies");
    assert_eq!(
        removed_sender_view.decision_for(removal.event_id(), b.facts.member_instance),
        history.decision_for(removal.event_id(), b.facts.member_instance)
    );
}

#[test]
fn history_exchange_pages_require_one_complete_unique_transfer() {
    let verifier = DeterministicSignatureVerifier;
    let (history, a, _b, _genesis, _add_b) = history_with_a_and_b(true);
    let mut sender_facts = a.facts.clone();
    sender_facts.identity_signature =
        verifier.sign(&a.membership_credential, &sender_facts.signing_payload());

    let pages = history
        .export_reconciliation_pages_v2(sender_facts)
        .expect("history exports bounded pages");
    assert!(!pages.is_empty());
    assert!(pages.iter().all(|page| {
        let counts = page.record_counts();
        counts.events <= 256 && counts.activation_receipts <= 256 && counts.decisions <= 256
    }));
    assert_eq!(
        VersionedMembershipHistory::import_exchange_pages_v2(&pages, &verifier)
            .expect("complete pages verify"),
        history
    );

    assert!(VersionedMembershipHistory::import_exchange_pages_v2(&[], &verifier).is_err());
    let mut duplicated = pages.clone();
    duplicated.push(pages[0].clone());
    assert!(VersionedMembershipHistory::import_exchange_pages_v2(&duplicated, &verifier).is_err());
}

#[test]
fn history_exchange_splits_the_256_event_boundary_without_losing_verification() {
    let verifier = DeterministicSignatureVerifier;
    let a = admission("device-a", credential(1));
    let mut history = VersionedMembershipHistory::new(LINEAGE.to_owned());
    let genesis = numbered_event(
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
    let mut head = genesis.event_id();
    for index in 0..128u16 {
        let joining = admission(
            &format!("device-page-{index}"),
            credential(u8::try_from(index + 10).expect("fixture credential fits")),
        );
        let add = numbered_event(
            &history,
            Some(head),
            &a,
            MembershipOperationV2::AddDevice {
                admission: joining.clone(),
            },
            2 + index * 2,
            &verifier,
        );
        history
            .verify_and_receive_event(add.clone(), &verifier)
            .expect("paged add verifies");
        history
            .verify_and_record_activation_receipt(
                activation_receipt(&add, &joining, &verifier),
                &verifier,
            )
            .expect("paged activation verifies");
        let remove = numbered_event(
            &history,
            Some(add.event_id()),
            &a,
            MembershipOperationV2::RemoveDevice {
                member: joining.facts.member_instance,
            },
            3 + index * 2,
            &verifier,
        );
        history
            .verify_and_receive_event(remove.clone(), &verifier)
            .expect("paged removal verifies");
        head = remove.event_id();
    }

    let mut sender_facts = a.facts.clone();
    sender_facts.identity_signature =
        verifier.sign(&a.membership_credential, &sender_facts.signing_payload());
    let pages = history
        .export_reconciliation_pages_v2(sender_facts)
        .expect("large history exports");

    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].record_counts().events, 256);
    assert_eq!(pages[1].record_counts().events, 1);
    assert_eq!(
        VersionedMembershipHistory::import_exchange_pages_v2(&pages, &verifier)
            .expect("large paged history verifies"),
        history
    );
}

#[test]
fn history_exchange_splits_pages_before_the_encoded_frame_limit() {
    let verifier = DeterministicSignatureVerifier;
    let a = admission("device-a", credential(1));
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

    let b = admission(
        "device-large-b",
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![2; 2_100_000]),
    );
    let add_b = event(
        &history,
        Some(genesis.event_id()),
        &a,
        MembershipOperationV2::AddDevice { admission: b },
        2,
        &verifier,
    );
    history
        .verify_and_receive_event(add_b.clone(), &verifier)
        .expect("large B addition verifies");

    let c = admission(
        "device-large-c",
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![3; 2_100_000]),
    );
    let add_c = event(
        &history,
        Some(add_b.event_id()),
        &a,
        MembershipOperationV2::AddDevice { admission: c },
        3,
        &verifier,
    );
    history
        .verify_and_receive_event(add_c, &verifier)
        .expect("large C addition verifies");

    let mut sender_facts = a.facts.clone();
    sender_facts.identity_signature =
        verifier.sign(&a.membership_credential, &sender_facts.signing_payload());
    let pages = history
        .export_reconciliation_pages_v2(sender_facts)
        .expect("large records export into bounded frames");

    assert_eq!(pages.len(), 2);
    assert!(pages.iter().all(|page| {
        let frame = postcard::to_stdvec(&MembershipHistoryMessage::HistoryPageV2(page.clone()))
            .expect("history frame encodes");
        frame.len() + 1 <= MAX_MEMBERSHIP_HISTORY_FRAME_SIZE
    }));
    assert_eq!(
        VersionedMembershipHistory::import_exchange_pages_v2(&pages, &verifier)
            .expect("size-bounded pages remain verifiable"),
        history
    );
}

#[test]
fn history_exchange_rejects_a_record_larger_than_one_frame() {
    let verifier = DeterministicSignatureVerifier;
    let a = admission("device-a", credential(1));
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

    let oversized = admission(
        "device-oversized",
        MembershipCredential::new(
            ED25519_SIGNATURE_ALGORITHM_V1,
            vec![2; MAX_MEMBERSHIP_HISTORY_FRAME_SIZE],
        ),
    );
    let add_oversized = event(
        &history,
        Some(genesis.event_id()),
        &a,
        MembershipOperationV2::AddDevice {
            admission: oversized,
        },
        2,
        &verifier,
    );
    history
        .verify_and_receive_event(add_oversized, &verifier)
        .expect("the history may verify before transport sizing");

    let mut sender_facts = a.facts.clone();
    sender_facts.identity_signature =
        verifier.sign(&a.membership_credential, &sender_facts.signing_payload());
    assert!(history
        .export_reconciliation_pages_v2(sender_facts)
        .is_err());
}

#[test]
fn completion_recovery_challenge_binds_all_three_members_and_transport_identities() {
    use uc_core::membership::{
        AdmissionAttemptId, AdmissionCompletionRecoveryChallengeV1,
        AdmissionCompletionRecoveryHelloV1, AdmissionCompletionRecoveryTransportBindingV1,
        AdmissionCompletionRecoveryValidationError, MemberInstanceId,
    };

    let hello = AdmissionCompletionRecoveryHelloV1::new(
        AdmissionAttemptId::from_bytes([0x91; 32]),
        LINEAGE.to_owned(),
        MembershipEventId::from_hex(&"92".repeat(32)).unwrap(),
        MemberInstanceId::from_bytes([0x93; 32]),
        MemberInstanceId::from_bytes([0x94; 32]),
        MemberInstanceId::from_bytes([0x95; 32]),
        vec![0x96; 32],
    )
    .unwrap();
    let binding = AdmissionCompletionRecoveryTransportBindingV1 {
        joiner_transport_identity_digest: [0x97; 32],
        helper_transport_identity_digest: [0x98; 32],
    };
    let challenge = AdmissionCompletionRecoveryChallengeV1::new(
        &hello,
        binding,
        7,
        [0x99; 32],
        [0x9e; 32],
        [0x9f; 32],
        credential(9).credential_id,
        BaseMembershipHistoryPositionV1 {
            event_id: Some(MembershipEventId::from_hex(&"9a".repeat(32)).unwrap()),
            depth: 12,
            history_digest: [0x9b; 32],
        },
    )
    .unwrap();

    let original = challenge.signing_payload();
    let mut changed_counter = challenge.clone();
    changed_counter.challenge_counter += 1;
    assert_ne!(changed_counter.signing_payload(), original);
    let mut changed_joiner_transport = challenge.clone();
    changed_joiner_transport
        .transport_binding
        .joiner_transport_identity_digest = [0x9c; 32];
    assert_ne!(changed_joiner_transport.signing_payload(), original);

    let mut changed_helper = hello;
    changed_helper.helper_member_instance = MemberInstanceId::from_bytes([0x9d; 32]);
    assert_ne!(changed_helper.digest(), challenge.hello_digest);

    let mut unknown_version = challenge;
    unknown_version.format_version += 1;
    assert_eq!(
        unknown_version.validate(),
        Err(AdmissionCompletionRecoveryValidationError::UpgradeRequired)
    );
}

#[test]
fn completion_recovery_response_binds_the_exact_commit_receipt_and_helper_delivery() {
    use uc_core::membership::{
        AdmissionCompletionRecoveryBundleV1, AdmissionCompletionRecoveryResponseV1,
        AdmissionCompletionRecoveryValidationError, SponsorAdmissionSecurityDelivery,
    };

    let bundle = AdmissionCompletionRecoveryBundleV1 {
        format_version: 1,
        candidate_event: vec![1],
        candidate_key_package: vec![2],
        security_commitment: vec![3],
        security_commit: vec![4],
        security_welcome: vec![5],
        target_protection_group_id: "protection-group".to_owned(),
        target_key_catalog: vec![6],
        existing_member_deliveries: vec![SponsorAdmissionSecurityDelivery {
            recipient: DeviceId::new("device-helper"),
            credential_id: credential(9).credential_id,
            payload: vec![7],
        }],
        activation_receipt: vec![8],
        resume_public_key: vec![9; 32],
    };
    let response =
        AdmissionCompletionRecoveryResponseV1::new([0xa1; 32], [0xa2; 32], bundle).unwrap();
    let original = response.signing_payload();

    let mut changed_receipt = response.clone();
    changed_receipt.bundle.activation_receipt.push(0xff);
    assert_ne!(changed_receipt.signing_payload(), original);
    let mut changed_delivery = response;
    changed_delivery.bundle.existing_member_deliveries[0]
        .payload
        .push(0xfe);
    assert_ne!(changed_delivery.signing_payload(), original);

    let mut unknown_version = changed_delivery;
    unknown_version.format_version += 1;
    assert_eq!(
        unknown_version.validate(),
        Err(AdmissionCompletionRecoveryValidationError::UpgradeRequired)
    );
}

#[test]
fn active_rejecting_member_can_deliver_its_decision_to_the_accepted_branch() {
    let verifier = DeterministicSignatureVerifier;
    let (mut base, a, b, _, add_b) = history_with_a_and_b(true);
    let c = admission("device-c", credential(3));
    let add_c = event(
        &base,
        Some(add_b.event_id()),
        &a,
        MembershipOperationV2::AddDevice {
            admission: c.clone(),
        },
        3,
        &verifier,
    );
    base.verify_and_receive_event(add_c.clone(), &verifier)
        .expect("C admission verifies");
    base.verify_and_record_activation_receipt(activation_receipt(&add_c, &c, &verifier), &verifier)
        .expect("C activation verifies");
    let removal = event(
        &base,
        Some(add_c.event_id()),
        &a,
        MembershipOperationV2::RemoveDevice {
            member: b.facts.member_instance,
        },
        4,
        &verifier,
    );

    let mut accepted = base.clone();
    accepted
        .verify_and_receive_event(removal.clone(), &verifier)
        .expect("A applies its removal");

    let mut rejected = base;
    rejected
        .merge_remote_history(&accepted, c.facts.member_instance, &verifier)
        .expect("C receives the pending removal");
    let mut rejection = MembershipDecisionV2::new(
        MEMBERSHIP_DECISION_FORMAT_V2,
        LINEAGE.to_owned(),
        removal.event_id(),
        c.facts.member_instance,
        c.membership_credential.credential_id,
        c.membership_credential.signature_algorithm_version,
        RemovalDecision::Reject,
        Some(add_c.event_id()),
        add_c.resulting_members_digest,
        [8; 16],
        Vec::new(),
    );
    rejection.signature = verifier.sign(&c.membership_credential, &rejection.signing_payload());
    rejected
        .verify_and_record_local_decision(rejection.clone(), c.facts.member_instance, &verifier)
        .expect("C keeps the parent branch");

    assert!(rejected.is_authorized_decision_delivery_of(&accepted, c.facts.member_instance));
    assert!(accepted
        .merge_remote_history(&rejected, a.facts.member_instance, &verifier)
        .expect("A stores C's decision without moving its accepted head"));
    assert_eq!(
        accepted.decision_for(removal.event_id(), c.facts.member_instance),
        Some(&rejection)
    );
    assert_eq!(accepted.current_head(), Some(removal.event_id()));
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
    let current_members = vec![
        (a.facts.clone(), a.membership_credential.clone()),
        (c.facts.clone(), c.membership_credential.clone()),
    ];

    let mut fully_verified = VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::FullyVerifiedMigration {
            lineage_id: LINEAGE.to_owned(),
            head_event_id: head,
            head_depth: 7,
            current_members,
        },
    )
    .expect("fully verified migration baseline is valid");
    assert_eq!(fully_verified.active_members().len(), 2);
    assert_eq!(
        fully_verified.device_for_member(
            &a.facts.member_instance,
            &[DeviceId::new("device-a"), DeviceId::new("device-c")]
        ),
        Some(DeviceId::new("device-a"))
    );
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
    assert_eq!(
        legacy.device_for_member(
            &c.facts.member_instance,
            &[DeviceId::new("device-a"), DeviceId::new("device-c")]
        ),
        Some(DeviceId::new("device-c"))
    );
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
fn unknown_event_decision_receipt_and_signature_versions_require_upgrade() {
    let verifier = DeterministicSignatureVerifier;
    let (mut event_history, a, b, _genesis, add_b) = history_with_a_and_b(true);

    let mut future_event = event(
        &event_history,
        Some(add_b.event_id()),
        &a,
        MembershipOperationV2::RemoveDevice {
            member: b.facts.member_instance,
        },
        0xa1,
        &verifier,
    );
    future_event.event_format_version = MEMBERSHIP_EVENT_FORMAT_V2 + 1;
    assert_eq!(
        event_history.verify_and_receive_event(future_event, &verifier),
        Err(uc_core::membership::MembershipHistoryV2Error::UpgradeRequired)
    );

    let mut unsupported_algorithm = event(
        &event_history,
        Some(add_b.event_id()),
        &a,
        MembershipOperationV2::RemoveDevice {
            member: b.facts.member_instance,
        },
        0xa2,
        &verifier,
    );
    unsupported_algorithm.author_signature_algorithm_version = ED25519_SIGNATURE_ALGORITHM_V1 + 1;
    unsupported_algorithm.signature = vec![0xa3; 32];
    assert_eq!(
        event_history.verify_and_receive_event(unsupported_algorithm, &verifier),
        Err(uc_core::membership::MembershipHistoryV2Error::UpgradeRequired)
    );

    let remove_b = event(
        &event_history,
        Some(add_b.event_id()),
        &a,
        MembershipOperationV2::RemoveDevice {
            member: b.facts.member_instance,
        },
        0xa4,
        &verifier,
    );
    event_history
        .verify_and_receive_event(remove_b.clone(), &verifier)
        .expect("removal verifies");
    let mut future_decision = MembershipDecisionV2::new(
        MEMBERSHIP_DECISION_FORMAT_V2 + 1,
        LINEAGE.to_owned(),
        remove_b.event_id(),
        b.facts.member_instance,
        b.membership_credential.credential_id,
        ED25519_SIGNATURE_ALGORITHM_V1,
        RemovalDecision::Accept,
        Some(add_b.event_id()),
        remove_b.resulting_members_digest,
        [0xa5; 16],
        Vec::new(),
    );
    future_decision.signature =
        verifier.sign(&b.membership_credential, &future_decision.signing_payload());
    assert_eq!(
        event_history.verify_and_record_peer_decision(future_decision, &verifier),
        Err(uc_core::membership::MembershipHistoryV2Error::UpgradeRequired)
    );

    let (mut receipt_history, _a, b, _genesis, add_b) = history_with_a_and_b(false);
    let mut future_receipt = activation_receipt(&add_b, &b, &verifier);
    future_receipt.receipt_format_version += 1;
    assert_eq!(
        receipt_history.verify_and_record_activation_receipt(future_receipt, &verifier),
        Err(uc_core::membership::MembershipHistoryV2Error::UpgradeRequired)
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

#[test]
fn persisted_history_reopens_when_an_activated_joiner_authors_the_next_admission() {
    let verifier = DeterministicSignatureVerifier;
    let (mut history, _a, b, _genesis, add_b) = history_with_a_and_b(true);
    let c = admission("device-c", credential(3));
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
        .expect("activated B may admit C");
    history
        .verify_and_record_activation_receipt(activation_receipt(&add_c, &c, &verifier), &verifier)
        .expect("C activation receipt verifies");

    let encoded = history.encode_persisted_v2().unwrap();
    let reopened = VersionedMembershipHistory::decode_persisted_v2(&encoded, &verifier)
        .expect("saved activation receipts authorize later event authors after reopen");

    assert_eq!(reopened, history);
    assert!(reopened.active_members().contains(&c.facts.member_instance));
}

#[test]
fn remote_history_merge_applies_activation_before_later_events_by_that_member() {
    let verifier = DeterministicSignatureVerifier;
    let (mut incoming, a, b, genesis, add_b) = history_with_a_and_b(true);
    let c = admission("device-c", credential(3));
    let add_c = event(
        &incoming,
        Some(add_b.event_id()),
        &b,
        MembershipOperationV2::AddDevice {
            admission: c.clone(),
        },
        3,
        &verifier,
    );
    incoming
        .verify_and_receive_event(add_c, &verifier)
        .expect("activated B may admit C");

    let mut local = VersionedMembershipHistory::new(LINEAGE.to_owned());
    local
        .verify_and_receive_event(genesis, &verifier)
        .expect("local genesis verifies");

    assert!(local
        .merge_remote_history(&incoming, a.facts.member_instance, &verifier)
        .expect("incoming activation authorizes B's later event"));
    assert!(local.active_members().contains(&b.facts.member_instance));
    assert!(local.effective_members().contains(&c.facts.member_instance));
}

#[test]
fn admission_content_key_catalog_is_canonical_and_rejects_incomplete_history() {
    let first = AdmissionContentKeyEntryV1::new("legacy-v1", 0, vec![0x41; 32]).unwrap();
    let current = AdmissionContentKeyEntryV1::new("content-2", 2, vec![0x42; 32]).unwrap();
    let forward =
        AdmissionContentKeyCatalogV1::new("content-2", 2, vec![first.clone(), current.clone()])
            .unwrap();
    let reversed = AdmissionContentKeyCatalogV1::new("content-2", 2, vec![current, first]).unwrap();

    assert_eq!(forward, reversed);
    assert_eq!(forward.digest(), reversed.digest());
    assert_eq!(
        AdmissionContentKeyCatalogV1::decode(&forward.encode().unwrap()).unwrap(),
        forward
    );
    assert!(AdmissionContentKeyCatalogV1::new(
        "content-2",
        2,
        vec![AdmissionContentKeyEntryV1::new("content-2", 2, vec![0x42; 32]).unwrap()],
    )
    .is_err());
}
