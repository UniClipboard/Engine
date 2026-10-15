//! Joiner 解析短邀请码的来源记录：谁先给出邀请，以及两条来源各自的结果类别。
//!
//! 只记录固定类别，不含邀请码、票据、URL、地址或服务端返回的任何文本。
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// 提供邀请的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvitationSource {
    Cloud,
    Mdns,
    FullInvitation,
}

/// 单条解析来源的结果类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvitationSourceOutcome {
    /// 本次没有启动该来源（完整邀请、仅局域网模式）。
    NotRun,
    /// 另一来源先成功，该来源尚未给出结果。
    Pending,
    Found,
    NotFound,
    Expired,
    Conflict,
    TransportFailed,
    ServiceUnavailable,
    /// 服务端返回了未预期的状态或无法解析的响应。
    InvalidResponse,
    /// 来源返回了内容，但邀请本身无效或路由不可用。
    InvalidInvitation,
    /// 本机来源不可用（例如 mDNS 套接字绑定失败）。
    LocalUnavailable,
}

/// 一次邀请解析的来源摘要。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvitationResolution {
    pub winner: Option<InvitationSource>,
    pub cloud: InvitationSourceOutcome,
    pub mdns: InvitationSourceOutcome,
}

impl InvitationResolution {
    pub fn full_invitation() -> Self {
        Self {
            winner: Some(InvitationSource::FullInvitation),
            cloud: InvitationSourceOutcome::NotRun,
            mdns: InvitationSourceOutcome::NotRun,
        }
    }

    pub(super) fn fields(&self, fields: &mut Map<String, Value>) {
        if let Some(winner) = self.winner {
            fields.insert("winner".into(), json!(winner));
        }
        fields.insert("cloud_outcome".into(), json!(self.cloud));
        fields.insert("mdns_outcome".into(), json!(self.mdns));
    }
}
