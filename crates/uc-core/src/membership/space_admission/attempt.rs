use sha2::{Digest, Sha256};

use crate::ids::SpaceId;
use crate::membership::{MemberInstanceId, MembershipEventId};

use super::{AdmissionChannelPeerId, InvitationId, SpaceAdmissionId};

pub const SPACE_ADMISSION_ATTEMPT_DURATION_MS: i64 = 300_000;
const ATTEMPT_CONTRACT_DOMAIN_V2: &[u8] = b"uniclipboard/space-admission/attempt/v2\0";
const MEMBER_BINDING_DOMAIN_V2: &[u8] = b"uniclipboard/space-admission/member-binding/v2\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionAttemptContractError {
    #[error("the admission attempt contract encoding is invalid")]
    InvalidEncoding,
    #[error("the admission attempt start time is invalid")]
    InvalidStartTime,
    #[error("the admission attempt deadline overflowed")]
    DeadlineOverflow,
    #[error("the admission attempt deadline is not exactly five minutes")]
    InvalidDeadline,
    #[error("the admission attempt peers must be distinct")]
    IdenticalPeers,
}

/// 本机从用户发起加入起保存的唯一五分钟期限。
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AdmissionAttemptTimeline {
    started_at_ms: i64,
    expires_at_ms: i64,
}

impl AdmissionAttemptTimeline {
    pub fn start(started_at_ms: i64) -> Result<Self, AdmissionAttemptContractError> {
        let expires_at_ms = started_at_ms
            .checked_add(SPACE_ADMISSION_ATTEMPT_DURATION_MS)
            .ok_or(AdmissionAttemptContractError::DeadlineOverflow)?;
        Self::new(started_at_ms, expires_at_ms)
    }

    pub fn new(
        started_at_ms: i64,
        expires_at_ms: i64,
    ) -> Result<Self, AdmissionAttemptContractError> {
        if started_at_ms < 0 {
            return Err(AdmissionAttemptContractError::InvalidStartTime);
        }
        let expected = started_at_ms
            .checked_add(SPACE_ADMISSION_ATTEMPT_DURATION_MS)
            .ok_or(AdmissionAttemptContractError::DeadlineOverflow)?;
        if expires_at_ms != expected {
            return Err(AdmissionAttemptContractError::InvalidDeadline);
        }
        Ok(Self {
            started_at_ms,
            expires_at_ms,
        })
    }

    pub const fn started_at_ms(self) -> i64 {
        self.started_at_ms
    }

    pub const fn expires_at_ms(self) -> i64 {
        self.expires_at_ms
    }

    pub const fn is_expired(self, now_ms: i64) -> bool {
        now_ms >= self.expires_at_ms
    }
}

impl std::fmt::Debug for AdmissionAttemptTimeline {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionAttemptTimeline([REDACTED])")
    }
}

/// 新版准入首次认证必须共同确认的固定尝试边界。
///
/// Space 与正式成员实例在首次认证时尚未知，随后由成员绑定契约关联到本契约摘要。
#[derive(Clone, PartialEq, Eq)]
pub struct AdmissionAttemptContractV2 {
    admission_id: SpaceAdmissionId,
    invitation_id: InvitationId,
    joiner_peer_id: AdmissionChannelPeerId,
    sponsor_peer_id: AdmissionChannelPeerId,
    started_at_ms: i64,
    expires_at_ms: i64,
}

impl AdmissionAttemptContractV2 {
    pub fn start(
        admission_id: SpaceAdmissionId,
        invitation_id: InvitationId,
        joiner_peer_id: AdmissionChannelPeerId,
        sponsor_peer_id: AdmissionChannelPeerId,
        started_at_ms: i64,
    ) -> Result<Self, AdmissionAttemptContractError> {
        let expires_at_ms = started_at_ms
            .checked_add(SPACE_ADMISSION_ATTEMPT_DURATION_MS)
            .ok_or(AdmissionAttemptContractError::DeadlineOverflow)?;
        Self::new(
            admission_id,
            invitation_id,
            joiner_peer_id,
            sponsor_peer_id,
            started_at_ms,
            expires_at_ms,
        )
    }

    pub fn new(
        admission_id: SpaceAdmissionId,
        invitation_id: InvitationId,
        joiner_peer_id: AdmissionChannelPeerId,
        sponsor_peer_id: AdmissionChannelPeerId,
        started_at_ms: i64,
        expires_at_ms: i64,
    ) -> Result<Self, AdmissionAttemptContractError> {
        if started_at_ms < 0 {
            return Err(AdmissionAttemptContractError::InvalidStartTime);
        }
        let expected = started_at_ms
            .checked_add(SPACE_ADMISSION_ATTEMPT_DURATION_MS)
            .ok_or(AdmissionAttemptContractError::DeadlineOverflow)?;
        if expires_at_ms != expected {
            return Err(AdmissionAttemptContractError::InvalidDeadline);
        }
        if joiner_peer_id == sponsor_peer_id {
            return Err(AdmissionAttemptContractError::IdenticalPeers);
        }
        Ok(Self {
            admission_id,
            invitation_id,
            joiner_peer_id,
            sponsor_peer_id,
            started_at_ms,
            expires_at_ms,
        })
    }

    pub const fn admission_id(&self) -> SpaceAdmissionId {
        self.admission_id
    }

    pub const fn invitation_id(&self) -> InvitationId {
        self.invitation_id
    }

    pub const fn joiner_peer_id(&self) -> AdmissionChannelPeerId {
        self.joiner_peer_id
    }

    pub const fn sponsor_peer_id(&self) -> AdmissionChannelPeerId {
        self.sponsor_peer_id
    }

    pub const fn started_at_ms(&self) -> i64 {
        self.started_at_ms
    }

    pub const fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }

    pub const fn timeline(&self) -> AdmissionAttemptTimeline {
        AdmissionAttemptTimeline {
            started_at_ms: self.started_at_ms,
            expires_at_ms: self.expires_at_ms,
        }
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(178);
        encoded.extend_from_slice(ATTEMPT_CONTRACT_DOMAIN_V2);
        encoded.extend_from_slice(self.admission_id.as_bytes());
        encoded.extend_from_slice(self.invitation_id.as_bytes());
        encoded.extend_from_slice(self.joiner_peer_id.as_bytes());
        encoded.extend_from_slice(self.sponsor_peer_id.as_bytes());
        encoded.extend_from_slice(&self.started_at_ms.to_be_bytes());
        encoded.extend_from_slice(&self.expires_at_ms.to_be_bytes());
        encoded
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, AdmissionAttemptContractError> {
        const VALUE_BYTES: usize = 32 * 4 + 8 * 2;
        let values = bytes
            .strip_prefix(ATTEMPT_CONTRACT_DOMAIN_V2)
            .filter(|values| values.len() == VALUE_BYTES)
            .ok_or(AdmissionAttemptContractError::InvalidEncoding)?;
        let admission_id = copy_array::<32>(&values[0..32])?;
        let invitation_id = copy_array::<32>(&values[32..64])?;
        let joiner_peer_id = copy_array::<32>(&values[64..96])?;
        let sponsor_peer_id = copy_array::<32>(&values[96..128])?;
        let started_at_ms = i64::from_be_bytes(copy_array::<8>(&values[128..136])?);
        let expires_at_ms = i64::from_be_bytes(copy_array::<8>(&values[136..144])?);
        Self::new(
            SpaceAdmissionId::from_bytes(admission_id)
                .ok_or(AdmissionAttemptContractError::InvalidEncoding)?,
            InvitationId::from_bytes(invitation_id)
                .ok_or(AdmissionAttemptContractError::InvalidEncoding)?,
            AdmissionChannelPeerId::from_bytes(joiner_peer_id)
                .ok_or(AdmissionAttemptContractError::InvalidEncoding)?,
            AdmissionChannelPeerId::from_bytes(sponsor_peer_id)
                .ok_or(AdmissionAttemptContractError::InvalidEncoding)?,
            started_at_ms,
            expires_at_ms,
        )
    }

    pub fn digest(&self) -> [u8; 32] {
        Sha256::digest(self.canonical_bytes()).into()
    }
}

impl std::fmt::Debug for AdmissionAttemptContractV2 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionAttemptContractV2([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionMemberBindingError {
    #[error("the admission member binding encoding is invalid")]
    InvalidEncoding,
    #[error("the admission member binding attempt digest is invalid")]
    InvalidAttemptDigest,
}

/// 正式 Add 生成后，把成员实例精确绑定到最初的五分钟尝试。
#[derive(Clone, PartialEq, Eq)]
pub struct AdmissionMemberBindingV2 {
    attempt_digest: [u8; 32],
    space_id: SpaceId,
    member_instance_id: MemberInstanceId,
    add_event_id: MembershipEventId,
}

impl AdmissionMemberBindingV2 {
    pub fn new(
        attempt_digest: [u8; 32],
        space_id: SpaceId,
        member_instance_id: MemberInstanceId,
        add_event_id: MembershipEventId,
    ) -> Result<Self, AdmissionMemberBindingError> {
        if attempt_digest == [0; 32] {
            return Err(AdmissionMemberBindingError::InvalidAttemptDigest);
        }
        Ok(Self {
            attempt_digest,
            space_id,
            member_instance_id,
            add_event_id,
        })
    }

    pub const fn attempt_digest(&self) -> &[u8; 32] {
        &self.attempt_digest
    }

    pub const fn space_id(&self) -> &SpaceId {
        &self.space_id
    }

    pub const fn member_instance_id(&self) -> MemberInstanceId {
        self.member_instance_id
    }

    pub const fn add_event_id(&self) -> MembershipEventId {
        self.add_event_id
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        let space_id = self.space_id.as_ref().as_bytes();
        let mut encoded = Vec::with_capacity(104 + space_id.len());
        encoded.extend_from_slice(MEMBER_BINDING_DOMAIN_V2);
        encoded.extend_from_slice(&self.attempt_digest);
        encoded.extend_from_slice(&(space_id.len() as u64).to_be_bytes());
        encoded.extend_from_slice(space_id);
        encoded.extend_from_slice(self.member_instance_id.as_bytes());
        encoded.extend_from_slice(self.add_event_id.as_bytes());
        encoded
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, AdmissionMemberBindingError> {
        const FIXED_BYTES: usize = 32 + 8 + 32 + 32;
        let values = bytes
            .strip_prefix(MEMBER_BINDING_DOMAIN_V2)
            .filter(|values| values.len() >= FIXED_BYTES)
            .ok_or(AdmissionMemberBindingError::InvalidEncoding)?;
        let attempt_digest = copy_array::<32>(&values[0..32])
            .map_err(|_| AdmissionMemberBindingError::InvalidEncoding)?;
        let space_id_len = u64::from_be_bytes(
            copy_array::<8>(&values[32..40])
                .map_err(|_| AdmissionMemberBindingError::InvalidEncoding)?,
        );
        let space_id_len = usize::try_from(space_id_len)
            .map_err(|_| AdmissionMemberBindingError::InvalidEncoding)?;
        let expected = FIXED_BYTES
            .checked_add(space_id_len)
            .ok_or(AdmissionMemberBindingError::InvalidEncoding)?;
        if values.len() != expected {
            return Err(AdmissionMemberBindingError::InvalidEncoding);
        }
        let space_id_end = 40 + space_id_len;
        let space_id = std::str::from_utf8(&values[40..space_id_end])
            .map_err(|_| AdmissionMemberBindingError::InvalidEncoding)?;
        let member_instance = copy_array::<32>(&values[space_id_end..space_id_end + 32])
            .map_err(|_| AdmissionMemberBindingError::InvalidEncoding)?;
        let add_event = copy_array::<32>(&values[space_id_end + 32..space_id_end + 64])
            .map_err(|_| AdmissionMemberBindingError::InvalidEncoding)?;
        Self::new(
            attempt_digest,
            SpaceId::from_str(space_id),
            MemberInstanceId::from_bytes(member_instance),
            MembershipEventId::from_bytes(add_event),
        )
    }

    pub fn digest(&self) -> [u8; 32] {
        Sha256::digest(self.canonical_bytes()).into()
    }
}

fn copy_array<const N: usize>(bytes: &[u8]) -> Result<[u8; N], AdmissionAttemptContractError> {
    bytes
        .try_into()
        .map_err(|_| AdmissionAttemptContractError::InvalidEncoding)
}

impl std::fmt::Debug for AdmissionMemberBindingV2 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionMemberBindingV2([REDACTED])")
    }
}
