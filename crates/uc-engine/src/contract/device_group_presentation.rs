use super::DeviceMembershipSummary;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceGroupChoiceDeviceSummary {
    pub device_id: String,
    pub display_name: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceGroupChoiceMemberSummary {
    pub device_id: String,
    pub display_name: String,
    pub is_local: bool,
    pub active: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceGroupChoiceImpactSummary {
    /// 预期同步范围，不表示已经完成恢复或获得实际发送权限。
    pub sync_scope_device_ids: Vec<String>,
    pub paused_device_ids: Vec<String>,
    pub pending_confirmation_device_ids: Vec<String>,
    pub requires_rejoin_device_ids: Vec<String>,
    pub local_device_outcome: DeviceMembershipSummary,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceGroupChoiceReasonKind {
    #[default]
    Unknown,
    PendingRemoval,
    DifferentRemovals,
    RemovalDecisionDisagreement,
    DivergedHistory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceGroupChangeSideSummary {
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceGroupChangeKindSummary {
    AddedDevice,
    RemovedDevice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceGroupRemovalDecisionSummary {
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceGroupChangeSummary {
    pub side: DeviceGroupChangeSideSummary,
    pub kind: DeviceGroupChangeKindSummary,
    pub actor: DeviceGroupChoiceDeviceSummary,
    pub target: DeviceGroupChoiceDeviceSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceGroupDecisionSummary {
    pub device: DeviceGroupChoiceDeviceSummary,
    pub decision: DeviceGroupRemovalDecisionSummary,
    pub target: DeviceGroupChoiceDeviceSummary,
}

/// 语言无关的已验证事实；产品负责翻译，设备名称不翻译。
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceGroupChoiceReasonSummary {
    pub kind: DeviceGroupChoiceReasonKind,
    pub changes: Vec<DeviceGroupChangeSummary>,
    pub decisions: Vec<DeviceGroupDecisionSummary>,
    pub details_complete: bool,
}

impl std::fmt::Debug for DeviceGroupChoiceDeviceSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DeviceGroupChoiceDeviceSummary(REDACTED)")
    }
}

impl std::fmt::Debug for DeviceGroupChoiceMemberSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceGroupChoiceMemberSummary")
            .field("is_local", &self.is_local)
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for DeviceGroupChoiceImpactSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceGroupChoiceImpactSummary")
            .field("scope_count", &self.sync_scope_device_ids.len())
            .field("paused_count", &self.paused_device_ids.len())
            .field("pending_count", &self.pending_confirmation_device_ids.len())
            .field("local_device_outcome", &self.local_device_outcome)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for super::DeviceGroupChoiceOptionSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceGroupChoiceOptionSummary")
            .field("is_current_group", &self.is_current_group)
            .field("members_complete", &self.members_complete)
            .field("member_count", &self.members.len())
            .field("requires_re_pairing", &self.requires_re_pairing)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for super::DeviceGroupChoiceIssueSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceGroupChoiceIssueSummary")
            .field("choice_count", &self.choices.len())
            .field("reason", &self.reason.kind)
            .finish_non_exhaustive()
    }
}
