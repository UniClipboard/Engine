use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::LoadMembershipLedgerPort;
use uc_core::ids::DeviceId;
use uc_core::membership::{MembershipHistoryRelationship, PeerAdmissionError, PeerAdmissionPort};

/// 从持久化成员账本解析网络准入；成员历史是复杂拓扑下唯一的授权事实来源。
pub struct MlsPeerAdmissionAdapter {
    ledger: Arc<dyn LoadMembershipLedgerPort>,
}

impl MlsPeerAdmissionAdapter {
    pub fn new(ledger: Arc<dyn LoadMembershipLedgerPort>) -> Self {
        Self { ledger }
    }
}

#[async_trait]
impl PeerAdmissionPort for MlsPeerAdmissionAdapter {
    async fn is_admitted(&self, device_id: &DeviceId) -> Result<bool, PeerAdmissionError> {
        let ledger =
            self.ledger.load().await.map_err(|_| {
                PeerAdmissionError::Internal("failed to load membership ledger".into())
            })?;
        if !ledger.local_join_active || ledger.lineage_id.is_none() {
            return Ok(false);
        }
        Ok(matches!(
            ledger.peer_reconciliation.get(device_id),
            Some(record)
                if record.relationship == MembershipHistoryRelationship::Consistent
        ))
    }
}
