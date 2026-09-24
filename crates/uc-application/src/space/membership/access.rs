//! 入站对端访问判定。
//!
//! 只有本机为有效成员、对端为当前成员且与本机历史一致时放行；正在离开或已不在成员中的设备一律拒绝。
//! 一方正在等待决定一项移除时仍放行连接，以便交换完成决定所需的受限历史与决定本身（ADR-020）；
//! 普通内容由当前成员范围暂停，不经此处放行。判定只来自已验证的成员账本，不读取成员读模型。

use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    LedgerMemberStatus, PeerAdmissionError, PeerAdmissionPort, PeerLink, PeerRelation,
};

use super::{MembershipLedgerError, MembershipRecordStorePort, MembershipView};

pub(crate) struct PeerAccess {
    store: Arc<dyn MembershipRecordStorePort>,
}

/// 网络入口在成员状态负责人组装之前就需要访问判定，因此直接读取已提交的成员记录。
pub fn build_membership_peer_admission(
    store: Arc<dyn MembershipRecordStorePort>,
) -> Arc<dyn PeerAdmissionPort> {
    Arc::new(PeerAccess { store })
}

#[async_trait]
impl PeerAdmissionPort for PeerAccess {
    async fn is_admitted(&self, device_id: &DeviceId) -> Result<bool, PeerAdmissionError> {
        let record = self.store.load().await.map_err(admission_error)?;
        let view = MembershipView::from_record(record).map_err(admission_error)?;
        Ok(admits_peer(&view, device_id))
    }
}

pub(crate) fn admits_peer(view: &MembershipView, device_id: &DeviceId) -> bool {
    let Some(space) = view.space() else {
        return false;
    };
    let ledger = space.ledger();
    ledger.local_status() == LedgerMemberStatus::Active
        && matches!(
            ledger.peer(device_id),
            Some(PeerLink::Member(link)) if matches!(
                link.relation(),
                PeerRelation::Consistent
                    | PeerRelation::AwaitingLocalDecision
                    | PeerRelation::AwaitingPeerDecision
            )
        )
}

#[cfg(test)]
mod tests;

fn admission_error(error: MembershipLedgerError) -> PeerAdmissionError {
    match error {
        MembershipLedgerError::Locked
        | MembershipLedgerError::Conflict
        | MembershipLedgerError::Unavailable => PeerAdmissionError::Unavailable,
        MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
            PeerAdmissionError::InvalidState
        }
    }
}
