//! 移除意图验证适配器:因果证明、视图成员与签名验证。
//!
//! 接收方只接受满足全部条件的意图:空间沿革一致、签名可以由对应因果视图
//! 验证、发起者和目标都是该视图中的不同成员实例、意图标识与不可变内容一致、
//! 所需因果历史完整且未发现完整性错误。设备时间、接收时间、网络路径和到达
//! 顺序不参与判断。

use async_trait::async_trait;

use openmls_rust_crypto::RustCrypto;
use openmls_traits::crypto::OpenMlsCrypto;
use openmls_traits::types::SignatureScheme;
use uc_core::membership::{
    MemberInstanceId, RemovalCausalProof, RemovalIntentVerificationError,
    RemovalIntentVerificationPort, SignedRemovalIntent,
};

pub struct RemovalIntentVerificationAdapter;

impl RemovalIntentVerificationAdapter {
    fn proof_is_well_formed(proof: &RemovalCausalProof) -> bool {
        proof.members.iter().all(|member| {
            MemberInstanceId::derive(member.device_id.as_str(), &member.signing_public_key)
                == member.instance
        })
    }
}

#[async_trait]
impl RemovalIntentVerificationPort for RemovalIntentVerificationAdapter {
    async fn verify_intent(
        &self,
        intent: &SignedRemovalIntent,
    ) -> Result<(), RemovalIntentVerificationError> {
        intent
            .content
            .validate()
            .map_err(|_| RemovalIntentVerificationError::InvalidMembership)?;
        if intent.intent_id != intent.content.intent_id()
            || !intent.causal_proof.matches_content(&intent.content)
            || !Self::proof_is_well_formed(&intent.causal_proof)
        {
            return Err(RemovalIntentVerificationError::InvalidProof);
        }
        // 发起者与目标必须是证明中的成员实例,且不是同一个实例。
        let initiator = intent
            .causal_proof
            .members
            .iter()
            .find(|member| member.instance == intent.content.initiator)
            .ok_or(RemovalIntentVerificationError::InvalidMembership)?;
        let target = intent
            .causal_proof
            .members
            .iter()
            .find(|member| member.instance == intent.content.target)
            .ok_or(RemovalIntentVerificationError::InvalidMembership)?;
        if initiator.instance == target.instance {
            return Err(RemovalIntentVerificationError::InvalidMembership);
        }
        if RustCrypto::default()
            .verify_signature(
                SignatureScheme::ED25519,
                &intent.content.canonical_bytes(),
                &initiator.signing_public_key,
                &intent.signature,
            )
            .is_err()
        {
            return Err(RemovalIntentVerificationError::BadSignature);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use uc_core::membership::{
        MemberInstanceId, RemovalCausalProof, RemovalCausalProofMember,
        RemovalIntentVerificationPort, SignedRemovalIntent,
    };

    use super::super::mls_group::MlsGroupEngine;
    use super::RemovalIntentVerificationAdapter;

    fn member(device: &str, key: u8) -> MemberInstanceId {
        MemberInstanceId::derive(device, &[key; 32])
    }

    #[test]
    fn causal_proof_rejects_a_member_instance_that_does_not_match_its_public_key() {
        let alice = member("alice", 1);
        let bob = member("bob", 2);
        let proof = RemovalCausalProof::new(
            7,
            vec![
                RemovalCausalProofMember {
                    device_id: uc_core::DeviceId::new("alice"),
                    instance: alice,
                    signing_public_key: vec![1; 32],
                },
                RemovalCausalProofMember {
                    device_id: uc_core::DeviceId::new("bob"),
                    instance: bob,
                    signing_public_key: vec![9; 32],
                },
            ],
        );

        assert!(!RemovalIntentVerificationAdapter::proof_is_well_formed(
            &proof
        ));
    }

    #[tokio::test]
    async fn verifies_from_a_public_proof_without_serializing_client_state() {
        let alice = MlsGroupEngine::create_sponsor(b"space-a", b"alice").unwrap();
        let bob_pending = MlsGroupEngine::prepare_join(b"bob").unwrap();
        let admission =
            MlsGroupEngine::admit_member(&alice, b"bob", &bob_pending.key_package).unwrap();
        let sponsor_state = admission.sponsor_state;
        let epoch = MlsGroupEngine::current_epoch(&sponsor_state).unwrap();
        let proof = RemovalCausalProof::new(
            epoch,
            MlsGroupEngine::view_members(&sponsor_state)
                .unwrap()
                .into_iter()
                .map(|identity| {
                    let device_id = uc_core::DeviceId::try_new(
                        String::from_utf8_lossy(&identity.device_identity).into_owned(),
                    )
                    .unwrap();
                    let instance =
                        MemberInstanceId::derive(device_id.as_str(), &identity.signature_key);
                    RemovalCausalProofMember {
                        device_id,
                        instance,
                        signing_public_key: identity.signature_key,
                    }
                })
                .collect(),
        );
        let alice_instance = proof
            .members
            .iter()
            .find(|member| member.device_id.as_str() == "alice")
            .unwrap()
            .instance;
        let bob_instance = proof
            .members
            .iter()
            .find(|member| member.device_id.as_str() == "bob")
            .unwrap()
            .instance;
        let content = uc_core::membership::RemovalIntentContent {
            space_lineage: "space-a".to_owned(),
            view_epoch: epoch,
            view_members: proof.members(),
            initiator: alice_instance,
            target: bob_instance,
        };
        let signature =
            MlsGroupEngine::sign_member_payload(&sponsor_state, &content.canonical_bytes())
                .unwrap();
        let intent = SignedRemovalIntent::new(content, signature, proof);

        RemovalIntentVerificationAdapter
            .verify_intent(&intent)
            .await
            .unwrap();

        let public_json = serde_json::to_string(&intent.causal_proof).unwrap();
        assert!(!public_json.contains("serialized_storage"));
        assert!(!public_json.contains("signer_public"));

        let private_snapshot = MlsGroupEngine::create_sponsor(b"space-b", b"other")
            .unwrap()
            .into_bytes();
        let private_json = String::from_utf8(private_snapshot).unwrap();
        assert!(private_json.contains("serialized_storage"));
    }
}
