use std::fmt;

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::ids::{SessionId, SpaceId};
use crate::membership::{MemberInstanceId, PendingGroupUpdate};

#[derive(Clone, Debug)]
pub struct SpaceAccessProofArtifact {
    pub pairing_session_id: SessionId,
    pub space_id: SpaceId,
    pub challenge_nonce: [u8; 32],
    pub proof_bytes: Vec<u8>,
}

/// Sponsor 发给 joiner 的 pairing offer——空间接入流程的载体。
///
/// `keyslot_blob` 是 adapter 自序列化的不透明字节（承载 KEK wrap 后的 MasterKey
/// 等加密物料），领域层不关心其布局；`challenge_nonce` 是 sponsor 产生的 32
/// 字节挑战值，joiner 用它 + 自身派生的 MasterKey 构造 proof 回传，sponsor
/// 验证 proof 以确认 joiner 拿到正确口令。
#[derive(Clone, Debug)]
pub struct JoinOffer {
    pub space_id: SpaceId,
    pub keyslot_blob: Vec<u8>,
    pub challenge_nonce: [u8; 32],
}

/// Joiner-side opaque MLS preparation. Only `key_package` is sent to the
/// sponsor; `private_state` must remain local until the Welcome is installed.
///
/// `member_instance` is the member identity derived from this admission's
/// freshly generated credential (device id + admission credential signature
/// key). A rejoining device must identify itself by this instance, not by
/// resolving a possibly stale view.
pub struct PreparedGroupJoin {
    pub key_package: Vec<u8>,
    private_state: Zeroizing<Vec<u8>>,
    member_instance: Option<MemberInstanceId>,
}

/// Opaque local access material prepared for a durable admission attempt.
/// The infrastructure owns its format; the application only persists the
/// bytes inside the encrypted admission record.
pub struct PreparedAdmissionTargetAccess(Zeroizing<Vec<u8>>);

impl PreparedAdmissionTargetAccess {
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(mut self) -> Vec<u8> {
        std::mem::take(self.0.as_mut())
    }
}

impl fmt::Debug for PreparedAdmissionTargetAccess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreparedAdmissionTargetAccess([REDACTED])")
    }
}

impl PreparedGroupJoin {
    pub fn new(key_package: Vec<u8>, private_state: Vec<u8>) -> Self {
        Self {
            key_package,
            private_state: Zeroizing::new(private_state),
            member_instance: None,
        }
    }

    /// Bind the member instance derived from this admission's credential.
    pub fn with_member_instance(mut self, instance: MemberInstanceId) -> Self {
        self.member_instance = Some(instance);
        self
    }

    /// The member instance of this admission, when the credential was
    /// generated before this preparation completed.
    pub fn member_instance(&self) -> Option<MemberInstanceId> {
        self.member_instance
    }

    pub fn private_state(&self) -> &[u8] {
        &self.private_state
    }

    pub fn into_parts(mut self) -> (Vec<u8>, Vec<u8>) {
        (
            self.key_package,
            std::mem::take(self.private_state.as_mut()),
        )
    }
}

impl fmt::Debug for PreparedGroupJoin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedGroupJoin")
            .field("key_package_len", &self.key_package.len())
            .field("private_state", &"[REDACTED]")
            .finish()
    }
}

/// Sponsor-side payload delivered only after the admission proof succeeds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupAdmission {
    pub welcome: Vec<u8>,
    pub encrypted_key_catalog: Vec<u8>,
    pub existing_member_updates: Vec<PendingGroupUpdate>,
    pub group_epoch: u64,
}

/// Pairing proof 链路上的不透明派生密钥。
///
/// 由 `DeriveProofKeyPort::derive_master_key_for_proof` 构造（adapter 内部
/// 从 keyslot 解出原始密钥字节后包装），传给 `ProofPort::build_proof`
/// 用于 HMAC 计算。两端都看不到 `MasterKey`——领域层只看到一段
/// "本次 proof 链路专用的 32 字节秘密"。
///
/// 不可 Clone / Serialize，drop 时自动清零。
pub struct ProofDerivedKey([u8; 32]);

impl ProofDerivedKey {
    /// adapter 内部按需构造——领域代码不应直接调用。
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// 借用底层字节用于 HMAC 计算等场景。
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for ProofDerivedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProofDerivedKey([REDACTED])")
    }
}

impl Drop for ProofDerivedKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
