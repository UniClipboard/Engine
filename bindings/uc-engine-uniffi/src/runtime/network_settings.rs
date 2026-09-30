//! 移动端网络设置：`QuerySettings` / `UpdateSettings` 网络部分的投影。
//!
//! 设置只有 Engine 一份状态，校验只在 Application 的设置门面；这里不保存状态、不校验输入、
//! 也不解析英文 `reason`，拒绝分类直接取自 Engine 契约的 `SettingsRejection`。

use uc_engine::{
    NetworkSettingsPatch, NetworkSettingsSummary, OperationResult, SettingsPatch,
    SettingsRejection, SettingsUpdateOutcome, TrustedNetworkRejectionKind,
};

use crate::BindingError;

/// 仅局域网相关的网络设置。自定义中转另有接口，不在这里读写。
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct NetworkSettings {
    /// 只读：`false` 表示仅局域网已开启。移动端没有该开关，更新不能修改它。
    pub allow_relay_fallback: bool,
    /// 放行的 CGNAT/Tailscale 段（CIDR 文本）。修改后需 `recover_network` 或重启才生效。
    pub trusted_networks: Vec<String>,
    /// 固定 UDP 监听端口；`None` 表示随机端口。修改后需 `recover_network` 或重启才生效。
    pub listen_port: Option<u16>,
}

// 条目与端口属于批准明文保存的设置，但仍不进入调试输出。
impl std::fmt::Debug for NetworkSettings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NetworkSettings")
            .field("allow_relay_fallback", &self.allow_relay_fallback)
            .field("trusted_network_count", &self.trusted_networks.len())
            .field("listen_port_fixed", &self.listen_port.is_some())
            .finish()
    }
}

/// 网络设置更新。`None` 表示保持不变。
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct NetworkSettingsUpdate {
    /// `Some(列表)` 整体替换（条目去空白、空白条目丢弃），`Some([])` 清空。
    pub trusted_networks: Option<Vec<String>>,
    /// `Some(0)` 恢复随机端口，`Some(1..=65535)` 固定端口。
    pub listen_port: Option<u16>,
}

impl std::fmt::Debug for NetworkSettingsUpdate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NetworkSettingsUpdate")
            .field(
                "trusted_network_count",
                &self.trusted_networks.as_ref().map(Vec::len),
            )
            .field("listen_port_present", &self.listen_port.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum NetworkSettingsRejectionField {
    TrustedNetworks,
    CustomRelays,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum NetworkSettingsRejectionKind {
    /// 不是合法的 `地址/前缀`。
    InvalidCidr,
    /// 网段不完全位于私有地址空间内。
    OutsidePrivateSpace,
    /// 规范化后与前面的条目重复。
    Duplicate,
    /// 已保存的自定义中转 URL 不合法。
    InvalidRelayUrl,
}

/// 更新结果：全部保存，或整次拒绝（不保存任何字段）。拒绝不含用户输入的原文。
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum NetworkSettingsUpdateResult {
    Saved {
        settings: NetworkSettings,
    },
    Rejected {
        field: NetworkSettingsRejectionField,
        /// 被拒绝条目在提交列表中的位置（从 0 开始）；整字段被拒绝时为 `None`。
        index: Option<u32>,
        kind: NetworkSettingsRejectionKind,
    },
}

pub(super) fn update_patch(update: NetworkSettingsUpdate) -> SettingsPatch {
    SettingsPatch {
        network: Some(NetworkSettingsPatch {
            trusted_networks: update.trusted_networks,
            listen_port: update.listen_port,
            ..Default::default()
        }),
        ..Default::default()
    }
}

pub(super) fn map_network_settings(
    result: OperationResult,
) -> Result<NetworkSettings, BindingError> {
    match result {
        OperationResult::Settings(settings) => Ok(project(settings.network)),
        _ => Err(BindingError::UnexpectedResult),
    }
}

pub(super) fn map_network_settings_update(
    result: OperationResult,
) -> Result<NetworkSettingsUpdateResult, BindingError> {
    let OperationResult::SettingsUpdated(outcome) = result else {
        return Err(BindingError::UnexpectedResult);
    };
    match outcome {
        SettingsUpdateOutcome::Updated(settings) => Ok(NetworkSettingsUpdateResult::Saved {
            settings: project(settings.network),
        }),
        SettingsUpdateOutcome::Rejected { rejection, .. } => match rejection {
            SettingsRejection::TrustedNetwork { index, kind } => {
                Ok(NetworkSettingsUpdateResult::Rejected {
                    field: NetworkSettingsRejectionField::TrustedNetworks,
                    index: Some(index),
                    kind: match kind {
                        TrustedNetworkRejectionKind::InvalidFormat => {
                            NetworkSettingsRejectionKind::InvalidCidr
                        }
                        TrustedNetworkRejectionKind::OutsidePrivateSpace => {
                            NetworkSettingsRejectionKind::OutsidePrivateSpace
                        }
                        TrustedNetworkRejectionKind::Duplicate => {
                            NetworkSettingsRejectionKind::Duplicate
                        }
                    },
                })
            }
            SettingsRejection::CustomRelayUrl => Ok(NetworkSettingsUpdateResult::Rejected {
                field: NetworkSettingsRejectionField::CustomRelays,
                index: None,
                kind: NetworkSettingsRejectionKind::InvalidRelayUrl,
            }),
            // 补丁只含网络字段中的两项，其他字段的转换失败在这里不可能出现。
            SettingsRejection::Other => Err(BindingError::UnexpectedResult),
        },
    }
}

fn project(network: NetworkSettingsSummary) -> NetworkSettings {
    NetworkSettings {
        allow_relay_fallback: network.allow_relay_fallback,
        trusted_networks: network.trusted_networks,
        listen_port: network.listen_port,
    }
}
