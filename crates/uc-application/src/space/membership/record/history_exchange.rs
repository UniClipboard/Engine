use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use uc_core::ids::DeviceId;
use uc_core::membership::MembershipHistorySuffixPageV4;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundMembershipTransfer {
    pub source_device_id: DeviceId,
    pub transfer_id: [u8; 32],
    pub page_count: u32,
    pub pages: BTreeMap<u32, MembershipHistorySuffixPageV4>,
    pub total_bytes: usize,
}

impl std::fmt::Debug for InboundMembershipTransfer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InboundMembershipTransfer")
            .field("source_device_id", &"[REDACTED]")
            .field("transfer_id", &"[REDACTED]")
            .field("page_count", &self.page_count)
            .field("saved_page_count", &self.pages.len())
            .field("total_bytes", &self.total_bytes)
            .finish()
    }
}
