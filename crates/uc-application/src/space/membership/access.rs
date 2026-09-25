//! 入站对端访问判定与身份目录。
//!
//! 放行规则由 Core 成员账本 `admits_inbound_peer` 给出：只有本机为有效成员、对端为当前成员且与本机历史
//! 一致时放行，正在离开或已不在成员中的设备一律拒绝。普通内容由当前成员范围暂停，不经此处放行。
//!
//! 放行判定与身份目录都读取成员状态负责人发布的同一状态，不读取成员读模型；负责人按数据库代号在
//! 控制世代切换、恢复出厂后重新加载，因此判定始终与当前世代的成员记录一致。

use std::sync::{Arc, PoisonError, RwLock};

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{PeerAdmissionError, PeerAdmissionPort};
use uc_core::security::IdentityFingerprint;

use super::{MembershipLedgerError, MembershipOwner, MembershipProjectionPlan, MembershipView};

/// 入站身份解析可识别的一台设备。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownPeerIdentity {
    pub device_id: DeviceId,
    pub identity_fingerprint: IdentityFingerprint,
}

/// 入站身份解析的候选设备：本机、当前有效成员、正在离开的对端与仍有未完成成员效果的设备，即成员
/// 读模型中的全部成员；身份指纹取自已签名的准入事实。没有当前 Space 时为空。
#[async_trait]
pub trait PeerIdentityDirectoryPort: Send + Sync {
    async fn known_peer_identities(&self) -> Result<Vec<KnownPeerIdentity>, MembershipLedgerError>;
}

/// 网络入口使用的对端访问判定与身份目录。
///
/// 网络入口在成员状态负责人组装之前就要构造，因此先以未绑定状态交给网络入口，由 Space 应用组装时
/// 绑定负责人；未绑定时判定与目录都按暂不可用失败，不放行任何对端。
pub struct PeerAccess {
    owner: RwLock<Option<Arc<MembershipOwner>>>,
}

impl PeerAccess {
    pub fn unbound() -> Arc<Self> {
        Arc::new(Self {
            owner: RwLock::new(None),
        })
    }

    pub(crate) fn bind(&self, owner: Arc<MembershipOwner>) {
        *self.owner.write().unwrap_or_else(PoisonError::into_inner) = Some(owner);
    }

    async fn view(&self) -> Result<Arc<MembershipView>, MembershipLedgerError> {
        let owner = self
            .owner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(MembershipLedgerError::unavailable)?;
        owner.load().await
    }
}

#[async_trait]
impl PeerAdmissionPort for PeerAccess {
    async fn is_admitted(&self, device_id: &DeviceId) -> Result<bool, PeerAdmissionError> {
        let view = self.view().await.map_err(admission_error)?;
        Ok(admits_peer(&view, device_id))
    }
}

#[async_trait]
impl PeerIdentityDirectoryPort for PeerAccess {
    async fn known_peer_identities(&self) -> Result<Vec<KnownPeerIdentity>, MembershipLedgerError> {
        known_peer_identities(&*self.view().await?)
    }
}

pub(crate) fn known_peer_identities(
    view: &MembershipView,
) -> Result<Vec<KnownPeerIdentity>, MembershipLedgerError> {
    let Some(space) = view.space() else {
        return Ok(Vec::new());
    };
    Ok(MembershipProjectionPlan::from_ledger(space.ledger())?
        .members
        .into_iter()
        .map(|facts| KnownPeerIdentity {
            device_id: facts.device_id,
            identity_fingerprint: facts.identity_fingerprint,
        })
        .collect())
}

pub(crate) fn admits_peer(view: &MembershipView, device_id: &DeviceId) -> bool {
    view.space()
        .is_some_and(|space| space.ledger().admits_inbound_peer(device_id))
}

#[cfg(test)]
mod tests;

fn admission_error(error: MembershipLedgerError) -> PeerAdmissionError {
    match error {
        MembershipLedgerError::Locked
        | MembershipLedgerError::Conflict
        | MembershipLedgerError::Unavailable { .. } => PeerAdmissionError::Unavailable,
        MembershipLedgerError::Corrupt { .. } | MembershipLedgerError::RecoveryRequired => {
            PeerAdmissionError::InvalidState
        }
    }
}
