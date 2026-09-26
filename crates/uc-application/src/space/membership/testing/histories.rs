//! 测试用成员历史与本机签名替身。

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, MemberInstanceId, MembershipActivationBaselineV2, MembershipCredential,
    MembershipEventId, VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
};

use crate::space::membership::{
    CurrentMemberSignatureError, CurrentMemberSignaturePort, MembershipRecord,
};

use super::started_record;

pub(crate) const TEST_LINEAGE: &str = "space-a";

pub(crate) fn member_facts(
    device: &str,
    credential_byte: u8,
) -> (AdmissionChangeFacts, MembershipCredential) {
    let device_id = DeviceId::new(device);
    let credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![credential_byte; 32]);
    let member_instance = credential.member_instance_id(&device_id);
    (
        AdmissionChangeFacts {
            member_instance,
            device_id,
            device_name: device.to_owned(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                "ABCD-EFGH-IJKL-MNOP",
            )
            .unwrap(),
            transport_public_key: vec![1],
            transport_address_blob: vec![2],
            identity_signature: vec![3],
        },
        credential,
    )
}

/// 以已建立基线开始的历史：列出的设备都是已激活成员。
pub(crate) fn established_history(
    members: &[(AdmissionChangeFacts, MembershipCredential)],
) -> VersionedMembershipHistory {
    VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::Established {
            lineage_id: TEST_LINEAGE.to_owned(),
            head_event_id: MembershipEventId::from_hex(&"11".repeat(32)).unwrap(),
            head_depth: 0,
            current_members: members.to_vec(),
        },
    )
    .unwrap()
}

/// 本机为 `device-a`、对端为其余设备的已建立 Space。
pub(crate) struct EstablishedSpace {
    pub(crate) members: Vec<(AdmissionChangeFacts, MembershipCredential)>,
    pub(crate) history: VersionedMembershipHistory,
}

impl EstablishedSpace {
    pub(crate) fn new(devices: &[&str]) -> Self {
        let members: Vec<_> = devices
            .iter()
            .enumerate()
            .map(|(index, device)| member_facts(device, 0x41 + index as u8))
            .collect();
        let history = established_history(&members);
        Self { members, history }
    }

    pub(crate) fn facts(&self, device: &str) -> &AdmissionChangeFacts {
        &self
            .members
            .iter()
            .find(|(facts, _)| facts.device_id.as_str() == device)
            .unwrap()
            .0
    }

    pub(crate) fn credential(&self, device: &str) -> &MembershipCredential {
        &self
            .members
            .iter()
            .find(|(facts, _)| facts.device_id.as_str() == device)
            .unwrap()
            .1
    }

    pub(crate) fn member(&self, device: &str) -> MemberInstanceId {
        self.facts(device).member_instance
    }

    /// 以 `local` 为本机建立的记录；其余成员一致、尚未确认本机位置。
    pub(crate) fn record(&self, local: &str, revision: u64) -> MembershipRecord {
        started_record(
            self.history.clone(),
            DeviceId::new(local),
            self.member(local),
            revision,
        )
    }

    pub(crate) fn signer(&self, local: &str) -> TestSigner {
        TestSigner {
            local_device_id: DeviceId::new(local),
            local_member: self.member(local),
            credential: self.credential(local).clone(),
        }
    }
}

pub(crate) struct TestSigner {
    pub(crate) local_device_id: DeviceId,
    pub(crate) local_member: MemberInstanceId,
    pub(crate) credential: MembershipCredential,
}

#[async_trait]
impl CurrentMemberSignaturePort for TestSigner {
    async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError> {
        Ok(1)
    }

    async fn current_membership_credential(
        &self,
        device_id: &DeviceId,
    ) -> Result<MembershipCredential, CurrentMemberSignatureError> {
        assert_eq!(device_id, &self.local_device_id);
        Ok(self.credential.clone())
    }

    async fn current_member_instance(
        &self,
        device_id: &DeviceId,
    ) -> Result<MemberInstanceId, CurrentMemberSignatureError> {
        assert_eq!(device_id, &self.local_device_id);
        Ok(self.local_member)
    }

    async fn sign_current_member_payload(
        &self,
        _payload: &[u8],
    ) -> Result<Vec<u8>, CurrentMemberSignatureError> {
        Ok(vec![0x91])
    }

    async fn verify_current_member_payload(
        &self,
        _member: &DeviceId,
        _payload: &[u8],
        _signature: &[u8],
    ) -> Result<bool, CurrentMemberSignatureError> {
        Ok(true)
    }
}

/// 以 `author` 身份在 `history` 末尾追加一个已激活的新成员，返回其成员实例与加入事件。
pub(crate) fn append_active_peer_to_history(
    history: &mut VersionedMembershipHistory,
    author: MemberInstanceId,
    device: &str,
    credential_byte: u8,
    marker: u8,
) -> (MemberInstanceId, MembershipEventId) {
    use uc_core::membership::{
        AdmissionActivationReceipt, MembershipAdmissionV2, MembershipEventV2,
        MembershipOperationV2, MEMBERSHIP_EVENT_FORMAT_V2,
    };

    use super::AcceptingVerifier;

    let (facts, credential) = member_facts(device, credential_byte);
    let member = facts.member_instance;
    let author_credential = history.credential_for(author).unwrap();
    let parent = history.current_head();
    let operation = MembershipOperationV2::AddDevice {
        admission: MembershipAdmissionV2 {
            facts,
            membership_credential: credential,
            resume_public_key_digest: [marker; 32],
            security_commitment_id: [marker.wrapping_add(1); 32],
        },
    };
    let resulting_members_digest = history
        .expected_resulting_members_digest(parent, &operation)
        .unwrap();
    let event = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        history.lineage_id().to_owned(),
        parent,
        parent.map(|id| history.depth(id).unwrap() + 1).unwrap_or(0),
        [marker; 16],
        author,
        author_credential.credential_id,
        author_credential.signature_algorithm_version,
        operation,
        resulting_members_digest,
        [marker.wrapping_add(1); 32],
        vec![marker],
        Some([marker.wrapping_add(2); 32]),
        vec![marker.wrapping_add(3)],
    );
    let event_id = event.event_id();
    let receipt = AdmissionActivationReceipt::new(
        1,
        [marker.wrapping_add(4); 32],
        event_id,
        event.resulting_members_digest,
        [marker.wrapping_add(1); 32],
        member,
        vec![marker.wrapping_add(5)],
    );
    history
        .verify_and_receive_event(event, &AcceptingVerifier)
        .unwrap();
    history
        .verify_and_record_activation_receipt(receipt, &AcceptingVerifier)
        .unwrap();
    (member, event_id)
}
