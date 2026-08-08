use openmls::{
    group::MlsGroup,
    prelude::{tls_codec::*, *},
    treesync::LeafNodeParameters,
};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::{signatures::Signer, types::SignatureScheme, OpenMlsProvider};

const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

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

fn fresh_key_package(
    provider: &impl OpenMlsProvider,
    signer: &impl Signer,
    credential_with_key: &CredentialWithKey,
) -> KeyPackageBundle {
    KeyPackage::builder()
        .build(CIPHERSUITE, provider, signer, credential_with_key.clone())
        .unwrap()
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

fn export_secret(group: &MlsGroup, provider: &OpenMlsRustCrypto) -> Result<Vec<u8>, ()> {
    group
        .export_secret(
            provider.crypto(),
            "uniclipboard-key-catalog-wrap-v1",
            b"",
            32,
        )
        .map_err(|_| ())
}

/// ADR-015 前置验证 1 与 2：验证并暂存来自当前因果成员视图的移除提议，
/// 并把多个移除目标放进同一个向前提交。
#[test]
fn multiple_removal_targets_become_one_forward_commit() {
    let alice_provider = OpenMlsRustCrypto::default();
    let bob_provider = OpenMlsRustCrypto::default();
    let charlie_provider = OpenMlsRustCrypto::default();
    let dave_provider = OpenMlsRustCrypto::default();

    let (alice_credential, alice_signer) =
        credential(b"alice", CIPHERSUITE.signature_algorithm(), &alice_provider);
    let (bob_credential, bob_signer) =
        credential(b"bob", CIPHERSUITE.signature_algorithm(), &bob_provider);
    let (charlie_credential, charlie_signer) = credential(
        b"charlie",
        CIPHERSUITE.signature_algorithm(),
        &charlie_provider,
    );
    let (dave_credential, dave_signer) =
        credential(b"dave", CIPHERSUITE.signature_algorithm(), &dave_provider);

    let bob_key_package = key_package(&bob_provider, &bob_signer, bob_credential);
    let charlie_key_package = key_package(&charlie_provider, &charlie_signer, charlie_credential);
    let dave_key_package = key_package(&dave_provider, &dave_signer, dave_credential);

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
                dave_key_package.key_package().clone(),
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
        inbound_welcome(welcome.clone()),
        None,
    )
    .unwrap()
    .into_group(&charlie_provider)
    .unwrap();
    let dave_group = StagedWelcome::new_from_welcome(
        &dave_provider,
        join_config,
        inbound_welcome(welcome),
        None,
    )
    .unwrap()
    .into_group(&dave_provider)
    .unwrap();

    let old_epoch = alice_group.epoch().as_u64();

    // 从当前因果成员视图识别目标 leaf 索引。
    let bob_index = alice_group
        .members()
        .find(|member| {
            BasicCredential::try_from(member.credential.clone())
                .is_ok_and(|credential| credential.identity() == b"bob")
        })
        .map(|member| member.index)
        .unwrap();
    let charlie_index = alice_group
        .members()
        .find(|member| {
            BasicCredential::try_from(member.credential.clone())
                .is_ok_and(|credential| credential.identity() == b"charlie")
        })
        .map(|member| member.index)
        .unwrap();

    // 把多个移除提议放进同一个提交并暂存，而不是分多次提交。
    let staged = alice_group
        .commit_builder()
        .propose_removals([bob_index, charlie_index])
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
    let (commit, _, _) = staged.into_contents();
    alice_group.merge_pending_commit(&alice_provider).unwrap();

    // 单个提交同时推进一个 epoch。
    assert_eq!(alice_group.epoch().as_u64(), old_epoch + 1);

    merge_commit(&mut bob_group, &bob_provider, commit.clone());
    merge_commit(&mut charlie_group, &charlie_provider, commit.clone());
    let mut dave_group_after = merge_commit_owned(dave_group, &dave_provider, commit);

    let alice_secret = export_secret(&alice_group, &alice_provider).unwrap();
    let dave_secret = export_secret(&dave_group_after, &dave_provider).unwrap();
    assert_eq!(alice_secret, dave_secret);

    // 被移除成员无法导出新状态。
    assert!(!bob_group.is_active());
    assert!(export_secret(&bob_group, &bob_provider).is_err());
    assert!(!charlie_group.is_active());
    assert!(export_secret(&charlie_group, &charlie_provider).is_err());

    // 新内容只在保留成员之间可处理。
    let new_message = alice_group
        .create_message(&alice_provider, &alice_signer, b"post-removal")
        .unwrap();
    assert!(bob_group
        .process_message(
            &bob_provider,
            inbound(new_message.clone())
                .try_into_protocol_message()
                .unwrap(),
        )
        .is_err());
    assert!(dave_group_after
        .process_message(
            &dave_provider,
            inbound(new_message).try_into_protocol_message().unwrap(),
        )
        .is_ok());
}

fn merge_commit_owned(
    mut group: MlsGroup,
    provider: &OpenMlsRustCrypto,
    commit: MlsMessageOut,
) -> MlsGroup {
    merge_commit(&mut group, provider, commit);
    group
}

/// ADR-015 前置验证 3 与 4：链式离线移除（A 移除 B、B 移除 C）形成的两个分叉分支，
/// 由确定执行者 A 从自己的分叉成员集合生成恢复资料，只保留 A。
#[test]
fn chained_offline_removal_converges_on_the_executor() {
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

    let fork_point = alice_group.epoch().as_u64();

    // 分支 1：A 移除 B。
    let (a_commit, _, _) = alice_group
        .remove_members(
            &alice_provider,
            &alice_signer,
            &[bob_group.own_leaf_index()],
        )
        .unwrap();
    alice_group.merge_pending_commit(&alice_provider).unwrap();

    // 分支 2：B 移除 C。
    let (b_commit, _, _) = bob_group
        .remove_members(
            &bob_provider,
            &bob_signer,
            &[charlie_group.own_leaf_index()],
        )
        .unwrap();
    let _ = &b_commit;
    bob_group.merge_pending_commit(&bob_provider).unwrap();

    // 两个分支处于同一 epoch，但 confirmation tag 不同，提交不能互相拼接。
    assert_eq!(alice_group.epoch().as_u64(), fork_point + 1);
    assert_eq!(bob_group.epoch().as_u64(), fork_point + 1);
    assert_ne!(alice_group.confirmation_tag(), bob_group.confirmation_tag());
    assert!(bob_group
        .process_message(
            &bob_provider,
            inbound(a_commit.clone())
                .try_into_protocol_message()
                .unwrap(),
        )
        .is_err());

    // 执行者 A 基于自己的分叉成员集合生成恢复资料：own_partition 只有 A，
    // 不提供任何重新加入的 key package，因此 C 不会恢复。
    let staged = alice_group
        .recover_fork_by_readding(&[alice_group.own_leaf_index()])
        .unwrap()
        .provide_key_packages(Vec::new())
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
    let (recovery_commit, welcome, _) = staged.into_contents();
    assert!(welcome.is_none());
    alice_group.merge_pending_commit(&alice_provider).unwrap();

    let alice_secret = export_secret(&alice_group, &alice_provider).unwrap();

    // B 与 A 处于不同分支：B 不能处理 A 的恢复提交，也不能取得 A 的最终秘密。
    assert!(bob_group
        .process_message(
            &bob_provider,
            inbound(recovery_commit.clone())
                .try_into_protocol_message()
                .unwrap(),
        )
        .is_err());
    // C 从未参与任何分支：它先线性追上 A 分支（移除 B 的提交），
    // 再处理恢复提交后被移除，无法导出或处理新状态。
    merge_commit(&mut charlie_group, &charlie_provider, a_commit);
    assert!(charlie_group.is_active());
    merge_commit(&mut charlie_group, &charlie_provider, recovery_commit);
    assert!(!charlie_group.is_active());
    assert!(export_secret(&charlie_group, &charlie_provider).is_err());
    let new_message = alice_group
        .create_message(&alice_provider, &alice_signer, b"post-recovery")
        .unwrap();
    assert!(charlie_group
        .process_message(
            &charlie_provider,
            inbound(new_message).try_into_protocol_message().unwrap(),
        )
        .is_err());

    // 旧密钥不恢复：恢复后的秘密不同于两个分支在分叉点之后的秘密。
    let bob_fork_secret = bob_group
        .export_secret(
            bob_provider.crypto(),
            "uniclipboard-key-catalog-wrap-v1",
            b"",
            32,
        )
        .unwrap();
    assert_ne!(alice_secret, bob_fork_secret);
}

/// ADR-015 前置验证 3、4 与 5：两个分叉分支分别移除不同成员（A 移除 B、C 移除 D），
/// 执行者 A 重新加入所有有效保留成员，只有它们导出相同的新秘密。
#[test]
fn fork_recovery_readds_retained_members_only() {
    let alice_provider = OpenMlsRustCrypto::default();
    let bob_provider = OpenMlsRustCrypto::default();
    let charlie_provider = OpenMlsRustCrypto::default();
    let dave_provider = OpenMlsRustCrypto::default();

    let (alice_credential, alice_signer) =
        credential(b"alice", CIPHERSUITE.signature_algorithm(), &alice_provider);
    let (bob_credential, bob_signer) =
        credential(b"bob", CIPHERSUITE.signature_algorithm(), &bob_provider);
    let (charlie_credential, charlie_signer) = credential(
        b"charlie",
        CIPHERSUITE.signature_algorithm(),
        &charlie_provider,
    );
    let (dave_credential, dave_signer) =
        credential(b"dave", CIPHERSUITE.signature_algorithm(), &dave_provider);
    let bob_key_package = key_package(&bob_provider, &bob_signer, bob_credential);
    let charlie_key_package = key_package(
        &charlie_provider,
        &charlie_signer,
        charlie_credential.clone(),
    );
    let dave_key_package = key_package(&dave_provider, &dave_signer, dave_credential);

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
                dave_key_package.key_package().clone(),
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
        inbound_welcome(welcome.clone()),
        None,
    )
    .unwrap()
    .into_group(&charlie_provider)
    .unwrap();
    let mut dave_group = StagedWelcome::new_from_welcome(
        &dave_provider,
        join_config,
        inbound_welcome(welcome),
        None,
    )
    .unwrap()
    .into_group(&dave_provider)
    .unwrap();

    // 分支 1：A 移除 B。B 处理包含自身移除的提交后进入非活跃状态。
    let (a_commit, _, _) = alice_group
        .remove_members(
            &alice_provider,
            &alice_signer,
            &[bob_group.own_leaf_index()],
        )
        .unwrap();
    alice_group.merge_pending_commit(&alice_provider).unwrap();
    merge_commit(&mut bob_group, &bob_provider, a_commit);
    assert!(!bob_group.is_active());
    assert!(export_secret(&bob_group, &bob_provider).is_err());

    // 分支 2：C 移除 D。
    let (c_commit, _, _) = charlie_group
        .remove_members(
            &charlie_provider,
            &charlie_signer,
            &[dave_group.own_leaf_index()],
        )
        .unwrap();
    charlie_group
        .merge_pending_commit(&charlie_provider)
        .unwrap();
    assert!(bob_group
        .process_message(
            &bob_provider,
            inbound(c_commit.clone())
                .try_into_protocol_message()
                .unwrap(),
        )
        .is_err());
    assert!(alice_group
        .process_message(
            &alice_provider,
            inbound(c_commit.clone())
                .try_into_protocol_message()
                .unwrap(),
        )
        .is_err());
    // D 未参与任何分支：它先线性追上 C 分支（包含自身移除的提交）后进入非活跃。
    merge_commit(&mut dave_group, &dave_provider, c_commit);
    assert!(!dave_group.is_active());
    assert!(export_secret(&dave_group, &dave_provider).is_err());

    // 有效保留成员集合为 {A, C}。C 提供一个新的 key package 用于重新加入。
    let charlie_fresh = fresh_key_package(&charlie_provider, &charlie_signer, &charlie_credential);

    // 执行者 A 从自己的分叉成员集合生成恢复资料：own_partition 只有 A，
    // complement 中的 C 用新 key package 重新加入，D 没有 key package 而永久移除。
    let staged = alice_group
        .recover_fork_by_readding(&[alice_group.own_leaf_index()])
        .unwrap()
        .provide_key_packages(vec![charlie_fresh.key_package().clone()])
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
    let (recovery_commit, welcome, _) = staged.into_contents();
    let welcome = welcome.expect("retained member must get a welcome");
    alice_group.merge_pending_commit(&alice_provider).unwrap();

    let alice_secret = export_secret(&alice_group, &alice_provider).unwrap();

    // C 删除旧状态并通过恢复资料加入新分支，导出与 A 相同的秘密。
    charlie_group.delete(charlie_provider.storage()).unwrap();
    let ratchet_tree = alice_group.export_ratchet_tree();
    let mut charlie_group = StagedWelcome::new_from_welcome(
        &charlie_provider,
        join_config,
        welcome,
        Some(ratchet_tree.into()),
    )
    .unwrap()
    .into_group(&charlie_provider)
    .unwrap();
    assert_eq!(alice_group.epoch(), charlie_group.epoch());
    assert_eq!(
        export_secret(&charlie_group, &charlie_provider).unwrap(),
        alice_secret
    );

    // 被移除成员 B、D 无法导出或处理新状态。
    assert!(export_secret(&bob_group, &bob_provider).is_err());
    assert!(bob_group
        .process_message(
            &bob_provider,
            inbound(recovery_commit.clone())
                .try_into_protocol_message()
                .unwrap(),
        )
        .is_err());
    assert!(export_secret(&dave_group, &dave_provider).is_err());
    assert!(dave_group
        .process_message(
            &dave_provider,
            inbound(recovery_commit)
                .try_into_protocol_message()
                .unwrap(),
        )
        .is_err());

    // 新内容只能在保留成员之间处理。
    let new_message = alice_group
        .create_message(&alice_provider, &alice_signer, b"post-recovery")
        .unwrap();
    assert!(charlie_group
        .process_message(
            &charlie_provider,
            inbound(new_message).try_into_protocol_message().unwrap(),
        )
        .is_ok());
}

/// ADR-015 前置验证的收尾：旧分叉提交不能拼接，也不能让最后到达的提交覆盖其他分支；
/// 恢复资料与目标成员集合绑定后，所有保留成员从同一状态继续。
#[test]
fn recovery_is_deterministic_regardless_of_delivery_order() {
    let alice_provider = OpenMlsRustCrypto::default();
    let bob_provider = OpenMlsRustCrypto::default();

    let (alice_credential, alice_signer) =
        credential(b"alice", CIPHERSUITE.signature_algorithm(), &alice_provider);
    let (bob_credential, bob_signer) =
        credential(b"bob", CIPHERSUITE.signature_algorithm(), &bob_provider);
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

    // 同一视图上 A 与 B 分别生成互不包含的向前提交（模拟各自离线移除）。
    let a_commit = alice_group
        .self_update(
            &alice_provider,
            &alice_signer,
            LeafNodeParameters::default(),
        )
        .unwrap()
        .into_commit();
    alice_group.merge_pending_commit(&alice_provider).unwrap();
    let b_commit = bob_group
        .self_update(&bob_provider, &bob_signer, LeafNodeParameters::default())
        .unwrap()
        .into_commit();
    bob_group.merge_pending_commit(&bob_provider).unwrap();

    // 任何一方的提交都不能在另一方直接应用：分叉不能自动拼接。
    assert!(bob_group
        .process_message(
            &bob_provider,
            inbound(a_commit).try_into_protocol_message().unwrap(),
        )
        .is_err());
    assert!(alice_group
        .process_message(
            &alice_provider,
            inbound(b_commit).try_into_protocol_message().unwrap(),
        )
        .is_err());

    // 恢复后两个分支的旧密钥都不再使用：新秘密既不同于 A 分支也不同于 B 分支。
    let alice_fork_secret = alice_group
        .export_secret(
            alice_provider.crypto(),
            "uniclipboard-key-catalog-wrap-v1",
            b"",
            32,
        )
        .unwrap();
    let bob_fork_secret = bob_group
        .export_secret(
            bob_provider.crypto(),
            "uniclipboard-key-catalog-wrap-v1",
            b"",
            32,
        )
        .unwrap();

    // 执行者 A 通过恢复资料重建统一状态（这里 B 重新加入）。
    let bob_fresh = fresh_key_package(&bob_provider, &bob_signer, &bob_credential);
    let staged = alice_group
        .recover_fork_by_readding(&[alice_group.own_leaf_index()])
        .unwrap()
        .provide_key_packages(vec![bob_fresh.key_package().clone()])
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
    let (_recovery_commit, welcome, _) = staged.into_contents();
    let welcome = welcome.expect("retained member must get a welcome");
    alice_group.merge_pending_commit(&alice_provider).unwrap();

    let recovered_secret = export_secret(&alice_group, &alice_provider).unwrap();
    assert_ne!(recovered_secret, alice_fork_secret);
    assert_ne!(recovered_secret, bob_fork_secret);

    bob_group.delete(bob_provider.storage()).unwrap();
    let ratchet_tree = alice_group.export_ratchet_tree();
    let bob_group = StagedWelcome::new_from_welcome(
        &bob_provider,
        join_config,
        welcome,
        Some(ratchet_tree.into()),
    )
    .unwrap()
    .into_group(&bob_provider)
    .unwrap();
    assert_eq!(
        export_secret(&bob_group, &bob_provider).unwrap(),
        recovered_secret
    );
}
