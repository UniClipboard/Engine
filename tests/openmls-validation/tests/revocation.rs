use openmls::{
    group::{JoinBuilder, ProcessedWelcome},
    prelude::{tls_codec::*, *},
    treesync::LeafNodeParameters,
};
use openmls_basic_credential::SignatureKeyPair;
use openmls_memory_storage::MemoryStorage;
use openmls_rust_crypto::{OpenMlsRustCrypto, RustCrypto};
use openmls_traits::{signatures::Signer, types::SignatureScheme, OpenMlsProvider};

const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

#[derive(Debug, Default)]
struct SnapshotProvider {
    crypto: RustCrypto,
    storage: MemoryStorage,
}

impl OpenMlsProvider for SnapshotProvider {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = MemoryStorage;

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }

    fn crypto(&self) -> &Self::CryptoProvider {
        &self.crypto
    }

    fn rand(&self) -> &Self::RandProvider {
        &self.crypto
    }
}

fn credential(
    identity: &[u8],
    signature_algorithm: SignatureScheme,
    provider: &impl OpenMlsProvider,
) -> (CredentialWithKey, SignatureKeyPair) {
    let signer = SignatureKeyPair::new(signature_algorithm).unwrap();
    signer.store(provider.storage()).unwrap();

    (
        CredentialWithKey {
            credential: BasicCredential::new(identity.to_vec()).into(),
            signature_key: signer.to_public_vec().into(),
        },
        signer,
    )
}

fn key_package(
    provider: &impl OpenMlsProvider,
    signer: &impl Signer,
    credential_with_key: CredentialWithKey,
) -> KeyPackageBundle {
    KeyPackage::builder()
        .build(CIPHERSUITE, provider, signer, credential_with_key)
        .unwrap()
}

fn merge_commit(group: &mut MlsGroup, provider: &OpenMlsRustCrypto, commit: MlsMessageOut) {
    let processed = group
        .process_message(
            provider,
            inbound(commit).try_into_protocol_message().unwrap(),
        )
        .unwrap();
    let ProcessedMessageContent::StagedCommitMessage(staged_commit) = processed.into_content()
    else {
        panic!("expected staged commit");
    };
    group.merge_staged_commit(provider, *staged_commit).unwrap();
}

fn inbound(message: MlsMessageOut) -> MlsMessageIn {
    let bytes = message.tls_serialize_detached().unwrap();
    MlsMessageIn::tls_deserialize_exact(bytes).unwrap()
}

fn inbound_welcome(message: MlsMessageOut) -> Welcome {
    match inbound(message).extract() {
        MlsMessageBodyIn::Welcome(welcome) => welcome,
        _ => panic!("expected welcome"),
    }
}

#[test]
fn removed_member_cannot_use_the_new_epoch() {
    let alice_provider = OpenMlsRustCrypto::default();
    let bob_provider = OpenMlsRustCrypto::default();
    let charlie_provider = OpenMlsRustCrypto::default();

    let (alice_credential, alice_signer) =
        credential(b"alice", CIPHERSUITE.signature_algorithm(), &alice_provider);
    let (bob_credential, bob_signer) =
        credential(b"bob", CIPHERSUITE.signature_algorithm(), &bob_provider);
    let (charlie_credential, charlie_signer) = credential(
        b"charlie",
        CIPHERSUITE.signature_algorithm(),
        &charlie_provider,
    );
    let bob_key_package = key_package(&bob_provider, &bob_signer, bob_credential);
    let charlie_key_package = key_package(&charlie_provider, &charlie_signer, charlie_credential);

    let create_config = MlsGroupCreateConfig::builder()
        .ciphersuite(CIPHERSUITE)
        .use_ratchet_tree_extension(true)
        .build();
    let join_config = create_config.join_config();
    let mut alice_group = MlsGroup::new(
        &alice_provider,
        &alice_signer,
        &create_config,
        alice_credential,
    )
    .unwrap();

    let (_, welcome, _) = alice_group
        .add_members(
            &alice_provider,
            &alice_signer,
            &[
                bob_key_package.key_package().clone(),
                charlie_key_package.key_package().clone(),
            ],
        )
        .unwrap();
    alice_group.merge_pending_commit(&alice_provider).unwrap();

    let mut bob_group = StagedWelcome::new_from_welcome(
        &bob_provider,
        join_config,
        inbound_welcome(welcome.clone()),
        None,
    )
    .unwrap()
    .into_group(&bob_provider)
    .unwrap();
    let mut charlie_group = StagedWelcome::new_from_welcome(
        &charlie_provider,
        join_config,
        inbound_welcome(welcome),
        None,
    )
    .unwrap()
    .into_group(&charlie_provider)
    .unwrap();

    let old_secret = charlie_group
        .export_secret(charlie_provider.crypto(), "content-key", b"", 32)
        .unwrap();
    assert_eq!(
        old_secret,
        alice_group
            .export_secret(alice_provider.crypto(), "content-key", b"", 32)
            .unwrap()
    );

    let charlie_index = charlie_group.own_leaf_index();
    let (remove_commit, _, _) = alice_group
        .remove_members(&alice_provider, &alice_signer, &[charlie_index])
        .unwrap();
    alice_group.merge_pending_commit(&alice_provider).unwrap();
    merge_commit(&mut bob_group, &bob_provider, remove_commit.clone());
    merge_commit(&mut charlie_group, &charlie_provider, remove_commit);

    let alice_new_secret = alice_group
        .export_secret(alice_provider.crypto(), "content-key", b"", 32)
        .unwrap();
    let bob_new_secret = bob_group
        .export_secret(bob_provider.crypto(), "content-key", b"", 32)
        .unwrap();
    assert_eq!(alice_new_secret, bob_new_secret);
    assert_ne!(old_secret, alice_new_secret);
    assert!(!charlie_group.is_active());
    assert!(charlie_group
        .export_secret(charlie_provider.crypto(), "content-key", b"", 32)
        .is_err());

    let new_message = alice_group
        .create_message(&alice_provider, &alice_signer, b"post-revocation")
        .unwrap();
    assert!(charlie_group
        .process_message(
            &charlie_provider,
            inbound(new_message).try_into_protocol_message().unwrap(),
        )
        .is_err());

    let group_id = alice_group.group_id().clone();
    drop(alice_group);
    let restored = MlsGroup::load(alice_provider.storage(), &group_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        restored
            .export_secret(alice_provider.crypto(), "content-key", b"", 32)
            .unwrap(),
        bob_new_secret
    );
}

#[test]
fn retained_offline_member_catches_up_in_epoch_order() {
    let alice_provider = OpenMlsRustCrypto::default();
    let bob_provider = OpenMlsRustCrypto::default();
    let charlie_provider = OpenMlsRustCrypto::default();

    let (alice_credential, alice_signer) =
        credential(b"alice", CIPHERSUITE.signature_algorithm(), &alice_provider);
    let (bob_credential, bob_signer) =
        credential(b"bob", CIPHERSUITE.signature_algorithm(), &bob_provider);
    let (charlie_credential, charlie_signer) = credential(
        b"charlie",
        CIPHERSUITE.signature_algorithm(),
        &charlie_provider,
    );
    let bob_key_package = key_package(&bob_provider, &bob_signer, bob_credential);
    let charlie_key_package = key_package(&charlie_provider, &charlie_signer, charlie_credential);
    let create_config = MlsGroupCreateConfig::builder()
        .ciphersuite(CIPHERSUITE)
        .use_ratchet_tree_extension(true)
        .build();
    let join_config = create_config.join_config();
    let mut alice_group = MlsGroup::new(
        &alice_provider,
        &alice_signer,
        &create_config,
        alice_credential,
    )
    .unwrap();
    let (_, welcome, _) = alice_group
        .add_members(
            &alice_provider,
            &alice_signer,
            &[
                bob_key_package.key_package().clone(),
                charlie_key_package.key_package().clone(),
            ],
        )
        .unwrap();
    alice_group.merge_pending_commit(&alice_provider).unwrap();
    let mut bob_group = StagedWelcome::new_from_welcome(
        &bob_provider,
        join_config,
        inbound_welcome(welcome.clone()),
        None,
    )
    .unwrap()
    .into_group(&bob_provider)
    .unwrap();
    let charlie_group = StagedWelcome::new_from_welcome(
        &charlie_provider,
        join_config,
        inbound_welcome(welcome),
        None,
    )
    .unwrap()
    .into_group(&charlie_provider)
    .unwrap();

    let (remove_commit, _, _) = alice_group
        .remove_members(
            &alice_provider,
            &alice_signer,
            &[charlie_group.own_leaf_index()],
        )
        .unwrap();
    alice_group.merge_pending_commit(&alice_provider).unwrap();
    let update_commit = alice_group
        .self_update(
            &alice_provider,
            &alice_signer,
            LeafNodeParameters::default(),
        )
        .unwrap()
        .into_commit();
    alice_group.merge_pending_commit(&alice_provider).unwrap();

    assert!(bob_group
        .process_message(
            &bob_provider,
            inbound(update_commit.clone())
                .try_into_protocol_message()
                .unwrap(),
        )
        .is_err());
    merge_commit(&mut bob_group, &bob_provider, remove_commit);
    merge_commit(&mut bob_group, &bob_provider, update_commit);

    assert_eq!(alice_group.epoch(), bob_group.epoch());
    assert_eq!(
        alice_group
            .export_secret(alice_provider.crypto(), "content-key", b"", 32)
            .unwrap(),
        bob_group
            .export_secret(bob_provider.crypto(), "content-key", b"", 32)
            .unwrap()
    );
}

#[test]
fn new_member_can_relay_the_sponsors_commit_to_an_offline_existing_member() {
    let alice_provider = OpenMlsRustCrypto::default();
    let bob_provider = OpenMlsRustCrypto::default();
    let charlie_provider = OpenMlsRustCrypto::default();

    let (alice_credential, alice_signer) =
        credential(b"alice", CIPHERSUITE.signature_algorithm(), &alice_provider);
    let (bob_credential, bob_signer) =
        credential(b"bob", CIPHERSUITE.signature_algorithm(), &bob_provider);
    let (charlie_credential, charlie_signer) = credential(
        b"charlie",
        CIPHERSUITE.signature_algorithm(),
        &charlie_provider,
    );
    let create_config = MlsGroupCreateConfig::builder()
        .ciphersuite(CIPHERSUITE)
        .use_ratchet_tree_extension(true)
        .build();
    let join_config = create_config.join_config();
    let mut alice_group = MlsGroup::new(
        &alice_provider,
        &alice_signer,
        &create_config,
        alice_credential,
    )
    .unwrap();

    let bob_key_package = key_package(&bob_provider, &bob_signer, bob_credential);
    let (_, bob_welcome, _) = alice_group
        .add_members(
            &alice_provider,
            &alice_signer,
            &[bob_key_package.key_package().clone()],
        )
        .unwrap();
    alice_group.merge_pending_commit(&alice_provider).unwrap();
    let mut bob_group = StagedWelcome::new_from_welcome(
        &bob_provider,
        join_config,
        inbound_welcome(bob_welcome),
        None,
    )
    .unwrap()
    .into_group(&bob_provider)
    .unwrap();

    let charlie_key_package = key_package(&charlie_provider, &charlie_signer, charlie_credential);
    let (charlie_commit, charlie_welcome, _) = bob_group
        .add_members(
            &bob_provider,
            &bob_signer,
            &[charlie_key_package.key_package().clone()],
        )
        .unwrap();
    bob_group.merge_pending_commit(&bob_provider).unwrap();
    let charlie_group = StagedWelcome::new_from_welcome(
        &charlie_provider,
        join_config,
        inbound_welcome(charlie_welcome),
        None,
    )
    .unwrap()
    .into_group(&charlie_provider)
    .unwrap();

    // Charlie only relays Bob's signed commit; Alice verifies it against
    // the group state she already trusts.
    merge_commit(&mut alice_group, &alice_provider, charlie_commit);

    assert_eq!(alice_group.epoch(), charlie_group.epoch());
    assert_eq!(
        alice_group
            .export_secret(alice_provider.crypto(), "content-key", b"", 32)
            .unwrap(),
        charlie_group
            .export_secret(charlie_provider.crypto(), "content-key", b"", 32)
            .unwrap()
    );
}

#[test]
fn a_gapped_commit_cannot_be_applied_directly() {
    let alice_provider = OpenMlsRustCrypto::default();
    let bob_provider = OpenMlsRustCrypto::default();
    let charlie_provider = OpenMlsRustCrypto::default();

    let (alice_credential, alice_signer) =
        credential(b"alice", CIPHERSUITE.signature_algorithm(), &alice_provider);
    let (bob_credential, bob_signer) =
        credential(b"bob", CIPHERSUITE.signature_algorithm(), &bob_provider);
    let (charlie_credential, charlie_signer) = credential(
        b"charlie",
        CIPHERSUITE.signature_algorithm(),
        &charlie_provider,
    );
    let bob_key_package = key_package(&bob_provider, &bob_signer, bob_credential);
    let charlie_key_package = key_package(&charlie_provider, &charlie_signer, charlie_credential);

    let create_config = MlsGroupCreateConfig::builder()
        .ciphersuite(CIPHERSUITE)
        .use_ratchet_tree_extension(true)
        .build();
    let mut alice_group = MlsGroup::new(
        &alice_provider,
        &alice_signer,
        &create_config,
        alice_credential,
    )
    .unwrap();

    let (_, bob_welcome, _) = alice_group
        .add_members(
            &alice_provider,
            &alice_signer,
            &[bob_key_package.key_package().clone()],
        )
        .unwrap();
    alice_group.merge_pending_commit(&alice_provider).unwrap();

    // Bob goes offline after epoch 1. Alice adds Charlie (epoch 2), then
    // makes a self update (epoch 3) without delivering either to Bob.
    let (charlie_commit, _, _) = alice_group
        .add_members(
            &alice_provider,
            &alice_signer,
            &[charlie_key_package.key_package().clone()],
        )
        .unwrap();
    alice_group.merge_pending_commit(&alice_provider).unwrap();
    let epoch_two_secret = alice_group
        .export_secret(alice_provider.crypto(), "content-key", b"", 32)
        .unwrap();
    let later_commit = alice_group
        .self_update(
            &alice_provider,
            &alice_signer,
            LeafNodeParameters::default(),
        )
        .unwrap();
    alice_group.merge_pending_commit(&alice_provider).unwrap();
    assert_eq!(alice_group.epoch(), GroupEpoch::from(3));

    let mut bob_group = StagedWelcome::new_from_welcome(
        &bob_provider,
        create_config.join_config(),
        inbound_welcome(bob_welcome),
        None,
    )
    .unwrap()
    .into_group(&bob_provider)
    .unwrap();

    // The epoch-3 commit alone cannot be applied over epoch 1: the epoch-2
    // add is missing, so the commit is rejected instead of being merged.
    let gapped = inbound(later_commit.commit().clone())
        .try_into_protocol_message()
        .unwrap();
    assert!(bob_group.process_message(&bob_provider, gapped).is_err());
    assert_eq!(bob_group.epoch(), GroupEpoch::from(1));

    // The gapped member must apply the missing epoch-2 commit first.
    merge_commit(&mut bob_group, &bob_provider, charlie_commit);
    assert_eq!(bob_group.epoch(), GroupEpoch::from(2));
    assert_eq!(
        epoch_two_secret,
        bob_group
            .export_secret(bob_provider.crypto(), "content-key", b"", 32)
            .unwrap()
    );
}

#[test]
fn concurrent_membership_fork_recovers_to_one_group() {
    let alice_provider = OpenMlsRustCrypto::default();
    let bob_provider = OpenMlsRustCrypto::default();
    let charlie_provider = OpenMlsRustCrypto::default();

    let (alice_credential, alice_signer) =
        credential(b"alice", CIPHERSUITE.signature_algorithm(), &alice_provider);
    let (bob_credential, bob_signer) =
        credential(b"bob", CIPHERSUITE.signature_algorithm(), &bob_provider);
    let (charlie_credential, charlie_signer) = credential(
        b"charlie",
        CIPHERSUITE.signature_algorithm(),
        &charlie_provider,
    );
    let bob_key_package = key_package(&bob_provider, &bob_signer, bob_credential.clone());
    let create_config = MlsGroupCreateConfig::builder()
        .ciphersuite(CIPHERSUITE)
        .use_ratchet_tree_extension(true)
        .build();
    let join_config = create_config.join_config();
    let mut alice_group = MlsGroup::new(
        &alice_provider,
        &alice_signer,
        &create_config,
        alice_credential,
    )
    .unwrap();
    let (_, welcome, _) = alice_group
        .add_members(
            &alice_provider,
            &alice_signer,
            &[bob_key_package.key_package().clone()],
        )
        .unwrap();
    alice_group.merge_pending_commit(&alice_provider).unwrap();
    let mut bob_group =
        StagedWelcome::new_from_welcome(&bob_provider, join_config, inbound_welcome(welcome), None)
            .unwrap()
            .into_group(&bob_provider)
            .unwrap();

    let charlie_key_package = key_package(&charlie_provider, &charlie_signer, charlie_credential);
    let (_, alice_welcome, _) = alice_group
        .add_members(
            &alice_provider,
            &alice_signer,
            &[charlie_key_package.key_package().clone()],
        )
        .unwrap();
    bob_group
        .add_members(
            &bob_provider,
            &bob_signer,
            &[charlie_key_package.key_package().clone()],
        )
        .unwrap();
    alice_group.merge_pending_commit(&alice_provider).unwrap();
    bob_group.merge_pending_commit(&bob_provider).unwrap();
    let mut charlie_group = StagedWelcome::new_from_welcome(
        &charlie_provider,
        join_config,
        inbound_welcome(alice_welcome),
        None,
    )
    .unwrap()
    .into_group(&charlie_provider)
    .unwrap();

    assert_eq!(
        alice_group.confirmation_tag(),
        charlie_group.confirmation_tag()
    );
    assert_ne!(alice_group.confirmation_tag(), bob_group.confirmation_tag());

    let bob_rejoin_key_package = key_package(&bob_provider, &bob_signer, bob_credential);
    let local_partition = &[alice_group.own_leaf_index(), charlie_group.own_leaf_index()];
    let recovery = alice_group
        .recover_fork_by_readding(local_partition)
        .unwrap()
        .provide_key_packages(vec![bob_rejoin_key_package.key_package().clone()])
        .load_psks(alice_provider.storage())
        .unwrap()
        .build(
            alice_provider.rand(),
            alice_provider.crypto(),
            &alice_signer,
            |_| true,
        )
        .unwrap()
        .stage_commit(&alice_provider)
        .unwrap();
    let (recovery_commit, recovery_welcome, _) = recovery.into_contents();
    let processed_welcome =
        ProcessedWelcome::new_from_welcome(&bob_provider, join_config, recovery_welcome.unwrap())
            .unwrap();
    let bob_group = JoinBuilder::new(&bob_provider, processed_welcome)
        .replace_old_group()
        .build()
        .unwrap()
        .into_group(&bob_provider)
        .unwrap();
    alice_group.merge_pending_commit(&alice_provider).unwrap();
    merge_commit(&mut charlie_group, &charlie_provider, recovery_commit);

    let alice_secret = alice_group
        .export_secret(alice_provider.crypto(), "content-key", b"", 32)
        .unwrap();
    assert_eq!(
        alice_secret,
        bob_group
            .export_secret(bob_provider.crypto(), "content-key", b"", 32)
            .unwrap()
    );
    assert_eq!(
        alice_secret,
        charlie_group
            .export_secret(charlie_provider.crypto(), "content-key", b"", 32)
            .unwrap()
    );
}

#[test]
fn group_state_survives_a_cold_storage_round_trip() {
    let provider = SnapshotProvider::default();
    let (alice_credential, alice_signer) =
        credential(b"alice", CIPHERSUITE.signature_algorithm(), &provider);
    let create_config = MlsGroupCreateConfig::builder()
        .ciphersuite(CIPHERSUITE)
        .use_ratchet_tree_extension(true)
        .build();
    let mut group =
        MlsGroup::new(&provider, &alice_signer, &create_config, alice_credential).unwrap();
    group
        .self_update(&provider, &alice_signer, LeafNodeParameters::default())
        .unwrap();
    group.merge_pending_commit(&provider).unwrap();
    let group_id = group.group_id().clone();
    let expected_secret = group
        .export_secret(provider.crypto(), "content-key", b"", 32)
        .unwrap();

    let mut snapshot = Vec::new();
    provider.storage.serialize(&mut snapshot).unwrap();
    assert!(!snapshot.is_empty());
    drop(group);
    drop(provider);

    let storage = MemoryStorage::deserialize(&mut snapshot.as_slice()).unwrap();
    let restarted_provider = SnapshotProvider {
        crypto: RustCrypto::default(),
        storage,
    };
    let restored_group = MlsGroup::load(restarted_provider.storage(), &group_id)
        .unwrap()
        .unwrap();
    assert_eq!(restored_group.epoch(), GroupEpoch::from(1));
    assert_eq!(
        restored_group
            .export_secret(restarted_provider.crypto(), "content-key", b"", 32)
            .unwrap(),
        expected_secret
    );
}
