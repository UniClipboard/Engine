//! 成员记录测试资料：只读成员记录替身，以及带真实签名、可通过生产校验的目标成员历史。

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use openmls_basic_credential::SignatureKeyPair;
use openmls_traits::signatures::Signer;
use openmls_traits::types::SignatureScheme;
use uc_application::deps::{
    MembershipLedgerError, MembershipProjectionPlan, MembershipRecord, MembershipRecordCommit,
    MembershipRecordStorePort, SpaceMembershipRecord,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, MemberInstanceId, MembershipCredential, MembershipLedger,
    MembershipLedgerSnapshot, VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
};

pub(crate) struct FixedMembershipRecords(MembershipRecord);

#[async_trait]
impl MembershipRecordStorePort for FixedMembershipRecords {
    async fn load(&self) -> Result<MembershipRecord, MembershipLedgerError> {
        Ok(self.0.clone())
    }

    async fn commit(&self, _commit: MembershipRecordCommit) -> Result<(), MembershipLedgerError> {
        Err(MembershipLedgerError::unavailable())
    }
}

pub(crate) fn no_space_records() -> Arc<FixedMembershipRecords> {
    Arc::new(FixedMembershipRecords(MembershipRecord::NoSpace {
        revision: 0,
    }))
}

/// 只有沿革、没有成员事件的记录；读取方只能依赖沿革标识。
pub(crate) fn lineage_only_records(lineage_id: &str) -> Arc<FixedMembershipRecords> {
    Arc::new(FixedMembershipRecords(MembershipRecord::Space(Box::new(
        SpaceMembershipRecord {
            ledger: MembershipLedgerSnapshot {
                revision: 1,
                history: VersionedMembershipHistory::new(lineage_id.to_owned()),
                local_device_id: DeviceId::new("local"),
                local_member: MemberInstanceId::from_bytes([0; 32]),
                peers: BTreeMap::new(),
                effects: Vec::new(),
                sync_cursor: None,
            },
            history_exchange: Default::default(),
            branch_recovery: Default::default(),
        },
    ))))
}

/// 调用方不应读取成员记录时使用；任何读取都得到暂不可用。
pub(crate) struct UnavailableMembershipRecords;

#[async_trait]
impl MembershipRecordStorePort for UnavailableMembershipRecords {
    async fn load(&self) -> Result<MembershipRecord, MembershipLedgerError> {
        Err(MembershipLedgerError::unavailable())
    }

    async fn commit(&self, _commit: MembershipRecordCommit) -> Result<(), MembershipLedgerError> {
        Err(MembershipLedgerError::unavailable())
    }
}

/// 两台带真实 Ed25519 身份签名的目标设备（`target-local` 与 `target-peer`）；进程内只生成一次，
/// 使同一测试中的多次调用得到相同成员。
pub(crate) fn signed_target_members() -> &'static [(AdmissionChangeFacts, MembershipCredential)] {
    static MEMBERS: OnceLock<Vec<(AdmissionChangeFacts, MembershipCredential)>> = OnceLock::new();
    MEMBERS.get_or_init(|| {
        [
            ("target-local", "target local", 0x51),
            ("target-peer", "target peer", 0x52),
        ]
        .into_iter()
        .map(|(device, name, key)| {
            let keys = SignatureKeyPair::new(SignatureScheme::ED25519).unwrap();
            let device_id = DeviceId::new(device);
            let credential =
                MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, keys.public().to_vec());
            let mut facts = AdmissionChangeFacts {
                member_instance: credential.member_instance_id(&device_id),
                device_id,
                device_name: name.to_owned(),
                identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                    "ABCD-EFGH-IJKL-MNOP",
                )
                .unwrap(),
                transport_public_key: vec![key],
                transport_address_blob: vec![key, key],
                identity_signature: Vec::new(),
            };
            facts.identity_signature = keys.sign(&facts.signing_payload()).unwrap();
            (facts, credential)
        })
        .collect()
    })
}

/// 按账本当前成员形成的读模型计划：全部有效成员，本机以外的都可信。
pub(crate) fn projection_of_ledger(ledger: &MembershipLedger) -> MembershipProjectionPlan {
    let history = ledger.history();
    let members: Vec<_> = history
        .effective_members()
        .into_iter()
        .map(|member| history.admission_facts_for(member).unwrap().clone())
        .collect();
    MembershipProjectionPlan {
        local_device_id: *ledger.local_device_id(),
        trusted_device_ids: members
            .iter()
            .map(|facts| facts.device_id)
            .filter(|device| device != ledger.local_device_id())
            .collect(),
        members,
    }
}
