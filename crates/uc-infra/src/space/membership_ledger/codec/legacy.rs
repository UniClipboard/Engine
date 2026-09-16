use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionActivationReceipt, AdmissionChangeFacts, BaseMembershipHistoryPosition,
    MembershipDecisionV2, MembershipEventV2,
};

/// 旧在途帧仅用于读取 V1/V2/V3 账本，迁移后重新核对，不恢复旧执行路径。
#[derive(Serialize, Deserialize)]
pub(super) struct LegacyInboundTransfer {
    source_device_id: DeviceId,
    transfer_id: [u8; 32],
    page_count: u32,
    pages: BTreeMap<u32, LegacySuffixPage>,
    total_bytes: usize,
}

#[derive(Serialize, Deserialize)]
struct LegacySuffixPage {
    format_version: u16,
    transfer_id: [u8; 32],
    page_index: u32,
    page_count: u32,
    lineage_id: String,
    base_position: BaseMembershipHistoryPosition,
    target_position: BaseMembershipHistoryPosition,
    sender_admission: AdmissionChangeFacts,
    events: Vec<MembershipEventV2>,
    activation_receipts: Vec<AdmissionActivationReceipt>,
    decisions: Vec<MembershipDecisionV2>,
}
