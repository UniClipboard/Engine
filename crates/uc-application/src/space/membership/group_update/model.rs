use uc_core::error_class::ErrorClass;
use uc_core::ids::SpaceId;
use uc_core::membership::KeyEpochError;
use uc_observability_contract::diagnostics::ObservationContext;

pub struct GroupUpdateIncoming {
    pub payload: Vec<u8>,
    pub observation: ObservationContext,
}

pub enum GroupUpdateReceiveOutcome {
    Applied,
    Unavailable(KeyEpochError),
    Rejected(KeyEpochError),
}

#[derive(Debug, thiserror::Error)]
pub enum GroupUpdateApplyError {
    #[error("group update temporarily unavailable")]
    Unavailable(#[source] KeyEpochError),
    #[error("group update rejected")]
    Rejected(#[source] KeyEpochError),
    #[error("group update local operation failed")]
    Local(#[source] KeyEpochError),
}

/// 只由成员 owner 核对当前已验证分支后构造；下层不能从 epoch 自行补造历史证据。
pub struct VerifiedHistoryGroupUpdate {
    pub(super) payload: Vec<u8>,
    pub(super) space_id: SpaceId,
    pub(super) catalog_payloads: Vec<Vec<u8>>,
    pub(super) prepared_material: Option<Vec<u8>>,
}
impl VerifiedHistoryGroupUpdate {
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    pub fn space_id(&self) -> &SpaceId {
        &self.space_id
    }
    pub fn prepared_material(&self) -> Option<&[u8]> {
        self.prepared_material.as_deref()
    }
    pub fn catalog_payloads(&self) -> &[Vec<u8>] {
        &self.catalog_payloads
    }
}

impl ErrorClass for GroupUpdateApplyError {
    fn class(&self) -> &'static str {
        match self {
            Self::Unavailable(_) => "unavailable",
            Self::Rejected(_) => "rejected",
            Self::Local(_) => "local_failure",
        }
    }
}

/// 仅供本机签名移除与对应效果提交使用，私有材料不会进入成员 wire。
pub struct PreparedGroupUpdate {
    pub payload: Vec<u8>,
    pub private_material: Vec<u8>,
}

pub(crate) struct PreparedRemoval {
    pub(crate) update: Option<PreparedGroupUpdate>,
    pub(crate) _guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}
