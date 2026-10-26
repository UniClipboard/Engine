use std::collections::BTreeSet;
use std::time::Duration;

use uc_core::settings::model as core;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentTypesView {
    pub text: bool,
    pub image: bool,
    pub link: bool,
    pub file: bool,
    pub code_snippet: bool,
    pub rich_text: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentTypesPatch {
    pub text: Option<bool>,
    pub image: Option<bool>,
    pub link: Option<bool>,
    pub file: Option<bool>,
    pub code_snippet: Option<bool>,
    pub rich_text: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncFrequencyView {
    Realtime,
    Interval,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleEvaluationView {
    AnyMatch,
    AllMatch,
}

#[derive(Debug, Clone)]
pub enum RetentionRuleView {
    ByAge {
        max_age: Duration,
    },
    ByCount {
        max_items: usize,
    },
    ByContentType {
        content_type: ContentTypesView,
        max_age: Duration,
    },
    ByTotalSize {
        max_bytes: u64,
    },
    Sensitive {
        max_age: Duration,
    },
}

#[derive(Debug, Clone)]
pub struct GeneralSettingsView {
    pub device_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SyncSettingsView {
    pub sync_enabled: bool,
    pub auto_sync_enabled: bool,
    pub sync_frequency: SyncFrequencyView,
    pub content_types: ContentTypesView,
    pub sync_on_restore: bool,
}

#[derive(Debug, Clone)]
pub struct RetentionPolicyView {
    pub enabled: bool,
    pub rules: Vec<RetentionRuleView>,
    pub skip_pinned: bool,
    pub evaluation: RuleEvaluationView,
}

#[derive(Debug, Clone)]
pub struct SecuritySettingsView {
    pub encryption_enabled: bool,
    pub passphrase_configured: bool,
    pub auto_unlock_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct PairingSettingsView {
    pub step_timeout: Duration,
    pub user_verification_timeout: Duration,
    pub session_timeout: Duration,
    pub max_retries: u8,
    pub protocol_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSyncSettingsView {
    pub file_sync_enabled: bool,
    pub small_file_threshold: u64,
    pub max_file_size: u64,
    pub file_cache_quota_per_device: u64,
    pub file_retention_hours: u32,
    pub file_auto_cleanup: bool,
    pub max_file_set_total_bytes: u64,
    pub max_file_set_member_count: u64,
    /// Directory where inbound files are saved, or `None` for managed storage.
    pub auto_save_dir: Option<String>,
}

/// LAN-only Mode 业务字段镜像 —— 业务正向语义 `allow_relay_fallback`。
/// UI = "LAN-only Mode = ON" → 此字段 = false。
/// 取反唯一发生在 `uc-bootstrap/src/network_policy.rs`，本层只搬运。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkSettingsView {
    pub allow_relay_fallback: bool,
    pub allow_overlay_network_addrs: bool,
    pub custom_relay_urls: Vec<String>,
    pub congestion_controller: CongestionControllerView,
    /// 固定的 P2P UDP 端口；`None` 表示随机端口。
    pub bind_port: Option<u16>,
}

/// Mirror of `uc_core::settings::model::CongestionController` for the
/// application layer view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CongestionControllerView {
    Cubic,
    Bbr3,
}

#[derive(Debug, Clone)]
pub struct SettingsView {
    pub schema_version: u32,
    pub general: GeneralSettingsView,
    pub sync: SyncSettingsView,
    pub retention_policy: RetentionPolicyView,
    pub security: SecuritySettingsView,
    pub pairing: PairingSettingsView,
    pub file_sync: FileSyncSettingsView,
    pub network: NetworkSettingsView,
}

#[derive(Debug, Clone, Default)]
pub struct GeneralSettingsPatch {
    pub device_name: Option<Option<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct SyncSettingsPatch {
    pub sync_enabled: Option<bool>,
    pub auto_sync_enabled: Option<bool>,
    pub sync_frequency: Option<SyncFrequencyView>,
    pub content_types: Option<ContentTypesPatch>,
    pub sync_on_restore: Option<bool>,
}

#[derive(Debug, Clone)]
pub enum RetentionRulePatchValue {
    ByAge {
        max_age: Duration,
    },
    ByCount {
        max_items: usize,
    },
    ByContentType {
        content_type: ContentTypesView,
        max_age: Duration,
    },
    ByTotalSize {
        max_bytes: u64,
    },
    Sensitive {
        max_age: Duration,
    },
}

#[derive(Debug, Clone, Default)]
pub struct RetentionPolicyPatch {
    pub enabled: Option<bool>,
    pub rules: Option<Vec<RetentionRulePatchValue>>,
    pub skip_pinned: Option<bool>,
    pub evaluation: Option<RuleEvaluationView>,
}

#[derive(Debug, Clone, Default)]
pub struct SecuritySettingsPatch {
    pub encryption_enabled: Option<bool>,
    pub auto_unlock_enabled: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct PairingSettingsPatch {
    pub step_timeout: Option<Duration>,
    pub user_verification_timeout: Option<Duration>,
    pub session_timeout: Option<Duration>,
    pub max_retries: Option<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct FileSyncSettingsPatch {
    pub file_sync_enabled: Option<bool>,
    pub small_file_threshold: Option<u64>,
    pub max_file_size: Option<u64>,
    pub file_cache_quota_per_device: Option<u64>,
    pub file_retention_hours: Option<u32>,
    pub file_auto_cleanup: Option<bool>,
    pub max_file_set_total_bytes: Option<u64>,
    pub max_file_set_member_count: Option<u64>,
    /// `None` = leave unchanged. `Some("")` (blank) = clear (managed storage).
    /// `Some(dir)` = save inbound files to `dir`.
    pub auto_save_dir: Option<String>,
}

/// LAN-only Mode 字段 patch 镜像 —— `None` = 不修改。
#[derive(Debug, Clone, Default)]
pub struct NetworkSettingsPatch {
    pub allow_relay_fallback: Option<bool>,
    pub allow_overlay_network_addrs: Option<bool>,
    pub custom_relay_urls: Option<Vec<String>>,
    pub congestion_controller: Option<CongestionControllerView>,
    /// `None` = 不修改；`Some(0)` = 恢复随机端口；其他值 = 固定该端口。
    pub bind_port: Option<u16>,
}

#[derive(Debug, Clone, Default)]
pub struct SettingsPatch {
    pub general: Option<GeneralSettingsPatch>,
    pub sync: Option<SyncSettingsPatch>,
    pub retention_policy: Option<RetentionPolicyPatch>,
    pub security: Option<SecuritySettingsPatch>,
    pub pairing: Option<PairingSettingsPatch>,
    pub file_sync: Option<FileSyncSettingsPatch>,
    pub network: Option<NetworkSettingsPatch>,
}

impl From<core::CongestionController> for CongestionControllerView {
    fn from(value: core::CongestionController) -> Self {
        match value {
            core::CongestionController::Cubic => Self::Cubic,
            core::CongestionController::Bbr3 => Self::Bbr3,
        }
    }
}

impl From<CongestionControllerView> for core::CongestionController {
    fn from(value: CongestionControllerView) -> Self {
        match value {
            CongestionControllerView::Cubic => Self::Cubic,
            CongestionControllerView::Bbr3 => Self::Bbr3,
        }
    }
}

impl From<core::ContentTypes> for ContentTypesView {
    fn from(value: core::ContentTypes) -> Self {
        Self {
            text: value.text,
            image: value.image,
            link: value.link,
            file: value.file,
            code_snippet: value.code_snippet,
            rich_text: value.rich_text,
        }
    }
}

impl From<ContentTypesView> for core::ContentTypes {
    fn from(value: ContentTypesView) -> Self {
        Self {
            text: value.text,
            image: value.image,
            link: value.link,
            file: value.file,
            code_snippet: value.code_snippet,
            rich_text: value.rich_text,
        }
    }
}

impl From<core::SyncFrequency> for SyncFrequencyView {
    fn from(value: core::SyncFrequency) -> Self {
        match value {
            core::SyncFrequency::Realtime => Self::Realtime,
            core::SyncFrequency::Interval => Self::Interval,
        }
    }
}

impl From<SyncFrequencyView> for core::SyncFrequency {
    fn from(value: SyncFrequencyView) -> Self {
        match value {
            SyncFrequencyView::Realtime => Self::Realtime,
            SyncFrequencyView::Interval => Self::Interval,
        }
    }
}

impl From<core::RuleEvaluation> for RuleEvaluationView {
    fn from(value: core::RuleEvaluation) -> Self {
        match value {
            core::RuleEvaluation::AnyMatch => Self::AnyMatch,
            core::RuleEvaluation::AllMatch => Self::AllMatch,
        }
    }
}

impl From<RuleEvaluationView> for core::RuleEvaluation {
    fn from(value: RuleEvaluationView) -> Self {
        match value {
            RuleEvaluationView::AnyMatch => Self::AnyMatch,
            RuleEvaluationView::AllMatch => Self::AllMatch,
        }
    }
}

impl From<core::RetentionRule> for RetentionRuleView {
    fn from(value: core::RetentionRule) -> Self {
        match value {
            core::RetentionRule::ByAge { max_age } => Self::ByAge { max_age },
            core::RetentionRule::ByCount { max_items } => Self::ByCount { max_items },
            core::RetentionRule::ByContentType {
                content_type,
                max_age,
            } => Self::ByContentType {
                content_type: content_type.into(),
                max_age,
            },
            core::RetentionRule::ByTotalSize { max_bytes } => Self::ByTotalSize { max_bytes },
            core::RetentionRule::Sensitive { max_age } => Self::Sensitive { max_age },
        }
    }
}

impl From<RetentionRulePatchValue> for core::RetentionRule {
    fn from(value: RetentionRulePatchValue) -> Self {
        match value {
            RetentionRulePatchValue::ByAge { max_age } => Self::ByAge { max_age },
            RetentionRulePatchValue::ByCount { max_items } => Self::ByCount { max_items },
            RetentionRulePatchValue::ByContentType {
                content_type,
                max_age,
            } => Self::ByContentType {
                content_type: content_type.into(),
                max_age,
            },
            RetentionRulePatchValue::ByTotalSize { max_bytes } => Self::ByTotalSize { max_bytes },
            RetentionRulePatchValue::Sensitive { max_age } => Self::Sensitive { max_age },
        }
    }
}

impl From<core::Settings> for SettingsView {
    fn from(value: core::Settings) -> Self {
        Self {
            schema_version: value.schema_version,
            general: GeneralSettingsView {
                device_name: value.general.device_name,
            },
            sync: SyncSettingsView {
                sync_enabled: value.sync.sync_enabled,
                auto_sync_enabled: value.sync.auto_sync_enabled,
                sync_frequency: value.sync.sync_frequency.into(),
                content_types: value.sync.content_types.into(),
                sync_on_restore: value.sync.sync_on_restore,
            },
            retention_policy: RetentionPolicyView {
                enabled: value.retention_policy.enabled,
                rules: value
                    .retention_policy
                    .rules
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                skip_pinned: value.retention_policy.skip_pinned,
                evaluation: value.retention_policy.evaluation.into(),
            },
            security: SecuritySettingsView {
                encryption_enabled: value.security.encryption_enabled,
                passphrase_configured: value.security.passphrase_configured,
                auto_unlock_enabled: value.security.auto_unlock_enabled,
            },
            pairing: PairingSettingsView {
                step_timeout: value.pairing.step_timeout,
                user_verification_timeout: value.pairing.user_verification_timeout,
                session_timeout: value.pairing.session_timeout,
                max_retries: value.pairing.max_retries,
                protocol_version: value.pairing.protocol_version,
            },
            file_sync: FileSyncSettingsView {
                file_sync_enabled: value.file_sync.file_sync_enabled,
                small_file_threshold: value.file_sync.small_file_threshold,
                max_file_size: value.file_sync.max_file_size,
                file_cache_quota_per_device: value.file_sync.file_cache_quota_per_device,
                file_retention_hours: value.file_sync.file_retention_hours,
                file_auto_cleanup: value.file_sync.file_auto_cleanup,
                max_file_set_total_bytes: value.file_sync.max_file_set_total_bytes,
                max_file_set_member_count: value.file_sync.max_file_set_member_count,
                auto_save_dir: value.file_sync.auto_save_dir,
            },
            network: NetworkSettingsView {
                allow_relay_fallback: value.network.allow_relay_fallback,
                allow_overlay_network_addrs: value.network.allow_overlay_network_addrs,
                custom_relay_urls: value.network.custom_relay_urls,
                congestion_controller: value.network.congestion_controller.into(),
                bind_port: value.network.bind_port,
            },
        }
    }
}

pub(crate) fn apply_settings_patch(
    mut existing: core::Settings,
    patch: SettingsPatch,
) -> core::Settings {
    if let Some(general) = patch.general {
        if let Some(v) = general.device_name {
            existing.general.device_name = v;
        }
    }

    if let Some(sync) = patch.sync {
        if let Some(v) = sync.sync_enabled {
            existing.sync.sync_enabled = v;
        }
        if let Some(v) = sync.auto_sync_enabled {
            existing.sync.auto_sync_enabled = v;
        }
        if let Some(v) = sync.sync_frequency {
            existing.sync.sync_frequency = v.into();
        }
        if let Some(content_types) = sync.content_types {
            apply_content_types_patch(&mut existing.sync.content_types, content_types);
        }
        if let Some(v) = sync.sync_on_restore {
            existing.sync.sync_on_restore = v;
        }
    }

    if let Some(retention_policy) = patch.retention_policy {
        if let Some(v) = retention_policy.enabled {
            existing.retention_policy.enabled = v;
        }
        if let Some(v) = retention_policy.skip_pinned {
            existing.retention_policy.skip_pinned = v;
        }
        if let Some(v) = retention_policy.evaluation {
            existing.retention_policy.evaluation = v.into();
        }
        if let Some(rules) = retention_policy.rules {
            existing.retention_policy.rules = rules.into_iter().map(Into::into).collect();
        }
    }

    if let Some(security) = patch.security {
        if let Some(v) = security.encryption_enabled {
            existing.security.encryption_enabled = v;
        }
        if let Some(v) = security.auto_unlock_enabled {
            existing.security.auto_unlock_enabled = v;
        }
    }

    if let Some(pairing) = patch.pairing {
        if let Some(v) = pairing.step_timeout {
            existing.pairing.step_timeout = v;
        }
        if let Some(v) = pairing.user_verification_timeout {
            existing.pairing.user_verification_timeout = v;
        }
        if let Some(v) = pairing.session_timeout {
            existing.pairing.session_timeout = v;
        }
        if let Some(v) = pairing.max_retries {
            existing.pairing.max_retries = v;
        }
    }

    if let Some(file_sync) = patch.file_sync {
        if let Some(v) = file_sync.file_sync_enabled {
            existing.file_sync.file_sync_enabled = v;
        }
        if let Some(v) = file_sync.small_file_threshold {
            existing.file_sync.small_file_threshold = v;
        }
        if let Some(v) = file_sync.max_file_size {
            existing.file_sync.max_file_size = v;
        }
        if let Some(v) = file_sync.file_cache_quota_per_device {
            existing.file_sync.file_cache_quota_per_device = v;
        }
        if let Some(v) = file_sync.file_retention_hours {
            existing.file_sync.file_retention_hours = v;
        }
        if let Some(v) = file_sync.file_auto_cleanup {
            existing.file_sync.file_auto_cleanup = v;
        }
        if let Some(v) = file_sync.max_file_set_total_bytes {
            existing.file_sync.max_file_set_total_bytes = v;
        }
        if let Some(v) = file_sync.max_file_set_member_count {
            existing.file_sync.max_file_set_member_count = v;
        }
        if let Some(dir) = file_sync.auto_save_dir {
            // Blank clears the setting (managed storage); a path sets it.
            // Store the trimmed value so the persisted setting is canonical
            // (no leading/trailing whitespace round-tripping to the UI).
            let trimmed = dir.trim();
            existing.file_sync.auto_save_dir = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
        }
    }

    if let Some(network) = patch.network {
        if let Some(v) = network.allow_relay_fallback {
            existing.network.allow_relay_fallback = v;
        }
        if let Some(v) = network.allow_overlay_network_addrs {
            existing.network.allow_overlay_network_addrs = v;
        }
        if let Some(v) = network.custom_relay_urls {
            existing.network.custom_relay_urls = normalize_relay_urls(v);
        }
        if let Some(v) = network.congestion_controller {
            existing.network.congestion_controller = v.into();
        }
        if let Some(port) = network.bind_port {
            // 0 是“随机端口”哨兵，保存为未设置，使持久值保持规范形态。
            existing.network.bind_port = (port != 0).then_some(port);
        }
    }

    existing
}

fn normalize_relay_urls(urls: Vec<String>) -> Vec<String> {
    urls.into_iter()
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
        .collect()
}

pub(crate) fn validate_settings(settings: &core::Settings) -> Result<(), String> {
    validate_custom_relay_urls(&settings.network.custom_relay_urls)
}

fn validate_custom_relay_urls(urls: &[String]) -> Result<(), String> {
    let mut canonical_urls = BTreeSet::new();
    for raw in urls {
        let url = url::Url::parse(raw)
            .map_err(|err| format!("invalid custom relay URL `{raw}`: {err}"))?;
        let scheme = url.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(format!(
                "invalid custom relay URL `{raw}`: scheme must be http or https"
            ));
        }
        if url.host_str().is_none() {
            return Err(format!(
                "invalid custom relay URL `{raw}`: host is required"
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(format!(
                "invalid custom relay URL `{raw}`: URL must not include credentials"
            ));
        }
        if !canonical_urls.insert(url.to_string()) {
            return Err(format!("duplicate custom relay URL `{raw}`"));
        }
    }
    Ok(())
}

fn apply_content_types_patch(target: &mut core::ContentTypes, patch: ContentTypesPatch) {
    if let Some(v) = patch.text {
        target.text = v;
    }
    if let Some(v) = patch.image {
        target.image = v;
    }
    if let Some(v) = patch.link {
        target.link = v;
    }
    if let Some(v) = patch.file {
        target.file = v;
    }
    if let Some(v) = patch.code_snippet {
        target.code_snippet = v;
    }
    if let Some(v) = patch.rich_text {
        target.rich_text = v;
    }
}

#[cfg(test)]
mod network_settings_apply_patch_tests {
    use super::*;
    use uc_core::settings::model::{NetworkSettings, Settings};

    fn baseline_with_network(allow: bool) -> Settings {
        let mut s = Settings::default();
        s.network = NetworkSettings {
            allow_relay_fallback: allow,
            allow_overlay_network_addrs: false,
            custom_relay_urls: Vec::new(),
            congestion_controller: Default::default(),
            bind_port: None,
        };
        s
    }

    fn baseline_with_overlay(allow_overlay: bool) -> Settings {
        let mut s = Settings::default();
        s.network = NetworkSettings {
            allow_relay_fallback: true,
            allow_overlay_network_addrs: allow_overlay,
            custom_relay_urls: Vec::new(),
            congestion_controller: Default::default(),
            bind_port: None,
        };
        s
    }

    /// 固定端口 patch：未带字段不修改，0 恢复随机端口，其他值固定该端口。
    #[test]
    fn bind_port_patch_sets_clears_and_keeps() {
        let mut existing = baseline_with_network(true);
        existing.network.bind_port = Some(51821);
        let patch = |bind_port| SettingsPatch {
            network: Some(NetworkSettingsPatch {
                bind_port,
                ..Default::default()
            }),
            ..Default::default()
        };

        let kept = apply_settings_patch(existing.clone(), patch(None));
        assert_eq!(kept.network.bind_port, Some(51821));
        let changed = apply_settings_patch(existing.clone(), patch(Some(40000)));
        assert_eq!(changed.network.bind_port, Some(40000));
        let cleared = apply_settings_patch(existing, patch(Some(0)));
        assert_eq!(cleared.network.bind_port, None);
        assert_eq!(SettingsView::from(changed).network.bind_port, Some(40000));
    }

    /// NETSET-02 #2 硬约束：旧客户端 PUT 不带 `network` 段时，
    /// 不抹掉已存在 `network` 字段。
    #[test]
    fn apply_patch_with_no_network_section_keeps_existing() {
        let existing = baseline_with_network(false);
        let patch = SettingsPatch::default(); // patch.network = None
        let result = apply_settings_patch(existing, patch);
        assert!(
            !result.network.allow_relay_fallback,
            "patch.network=None must keep existing false"
        );
    }

    /// 嵌套 None — patch.network = Some 但内部字段 None — 仍不抹掉。
    #[test]
    fn apply_patch_with_empty_network_section_keeps_existing() {
        let existing = baseline_with_network(true);
        let patch = SettingsPatch {
            network: Some(NetworkSettingsPatch::default()),
            ..Default::default()
        };
        let result = apply_settings_patch(existing, patch);
        assert!(
            result.network.allow_relay_fallback,
            "inner None must keep existing true"
        );
    }

    /// 显式 Some(false) 写入生效。
    #[test]
    fn apply_patch_with_explicit_false_writes_through() {
        let existing = baseline_with_network(true);
        let patch = SettingsPatch {
            network: Some(NetworkSettingsPatch {
                allow_relay_fallback: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = apply_settings_patch(existing, patch);
        assert!(!result.network.allow_relay_fallback);
    }

    /// 显式 Some(true) 写入生效（双向覆盖）。
    #[test]
    fn apply_patch_with_explicit_true_writes_through() {
        let existing = baseline_with_network(false);
        let patch = SettingsPatch {
            network: Some(NetworkSettingsPatch {
                allow_relay_fallback: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = apply_settings_patch(existing, patch);
        assert!(result.network.allow_relay_fallback);
    }

    /// From<core::Settings> 透明搬运（不取反）。
    #[test]
    fn from_core_settings_passes_through_business_semantics() {
        let mut s = Settings::default();
        s.network.allow_relay_fallback = false;
        let view: SettingsView = s.into();
        assert!(
            !view.network.allow_relay_fallback,
            "view 必须保留业务正向语义，不取反"
        );
    }

    /// allow_overlay_network_addrs：patch 缺字段时不抹掉已存在值。
    #[test]
    fn apply_patch_with_no_overlay_field_keeps_existing() {
        let existing = baseline_with_overlay(true);
        let patch = SettingsPatch {
            network: Some(NetworkSettingsPatch::default()),
            ..Default::default()
        };
        let result = apply_settings_patch(existing, patch);
        assert!(
            result.network.allow_overlay_network_addrs,
            "inner None must keep existing true"
        );
    }

    /// allow_overlay_network_addrs：显式 Some(true) 写入生效。
    #[test]
    fn apply_patch_with_overlay_explicit_true_writes_through() {
        let existing = baseline_with_overlay(false);
        let patch = SettingsPatch {
            network: Some(NetworkSettingsPatch {
                allow_overlay_network_addrs: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = apply_settings_patch(existing, patch);
        assert!(result.network.allow_overlay_network_addrs);
    }

    /// allow_overlay_network_addrs：显式 Some(false) 双向覆盖。
    #[test]
    fn apply_patch_with_overlay_explicit_false_writes_through() {
        let existing = baseline_with_overlay(true);
        let patch = SettingsPatch {
            network: Some(NetworkSettingsPatch {
                allow_overlay_network_addrs: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = apply_settings_patch(existing, patch);
        assert!(!result.network.allow_overlay_network_addrs);
    }

    /// View 透明搬运 allow_overlay_network_addrs。
    #[test]
    fn from_core_settings_passes_through_overlay_field() {
        let mut s = Settings::default();
        s.network.allow_overlay_network_addrs = true;
        let view: SettingsView = s.into();
        assert!(view.network.allow_overlay_network_addrs);
    }

    /// custom_relay_urls：patch 缺字段时不抹掉已存在列表。
    #[test]
    fn apply_patch_with_no_custom_relay_urls_keeps_existing() {
        let mut existing = baseline_with_network(true);
        existing.network.custom_relay_urls = vec!["https://relay.example.com.".to_string()];
        let patch = SettingsPatch {
            network: Some(NetworkSettingsPatch::default()),
            ..Default::default()
        };
        let result = apply_settings_patch(existing, patch);
        assert_eq!(
            result.network.custom_relay_urls,
            vec!["https://relay.example.com.".to_string()]
        );
    }

    /// custom_relay_urls：显式 Some(list) 会 trim，并过滤空行。
    #[test]
    fn apply_patch_with_custom_relay_urls_normalizes_values() {
        let existing = baseline_with_network(true);
        let patch = SettingsPatch {
            network: Some(NetworkSettingsPatch {
                custom_relay_urls: Some(vec![
                    " https://relay-a.example.com. ".to_string(),
                    "".to_string(),
                    "https://relay-b.example.com.".to_string(),
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = apply_settings_patch(existing, patch);
        assert_eq!(
            result.network.custom_relay_urls,
            vec![
                "https://relay-a.example.com.".to_string(),
                "https://relay-b.example.com.".to_string()
            ]
        );
    }

    /// custom_relay_urls：View 透明搬运，不在 application 层转换成 iroh 类型。
    #[test]
    fn from_core_settings_passes_through_custom_relay_urls() {
        let mut s = Settings::default();
        s.network.custom_relay_urls = vec!["https://relay.example.com.".to_string()];
        let view: SettingsView = s.into();
        assert_eq!(
            view.network.custom_relay_urls,
            vec!["https://relay.example.com.".to_string()]
        );
    }

    /// custom_relay_urls：只接受 HTTP(S) relay URL。
    #[test]
    fn validate_custom_relay_urls_rejects_invalid_scheme() {
        let urls = vec!["ftp://relay.example.com".to_string()];
        let err = validate_custom_relay_urls(&urls).expect_err("invalid scheme");
        assert!(err.contains("scheme must be http or https"));
    }

    #[test]
    fn validate_custom_relay_urls_rejects_embedded_credentials() {
        let urls = vec!["https://login:password@relay.example.com".to_string()];
        let err = validate_custom_relay_urls(&urls).expect_err("embedded credentials");
        assert!(err.contains("must not include credentials"));
    }

    #[test]
    fn validate_custom_relay_urls_rejects_canonical_duplicates() {
        let urls = vec![
            "https://relay.example.com".to_string(),
            "https://relay.example.com/".to_string(),
        ];
        let err = validate_custom_relay_urls(&urls).expect_err("duplicate relay URL");
        assert!(err.contains("duplicate"));
    }

    /// custom_relay_urls：合法 HTTP(S) URL 通过校验。
    #[test]
    fn validate_custom_relay_urls_accepts_http_urls() {
        let urls = vec![
            "https://relay-a.example.com.".to_string(),
            "http://127.0.0.1:3340".to_string(),
        ];
        validate_custom_relay_urls(&urls).expect("valid relay urls");
    }
}

#[cfg(test)]
mod file_sync_auto_save_dir_apply_patch_tests {
    use super::*;
    use uc_core::settings::model::Settings;

    fn patch_auto_save_dir(value: Option<&str>) -> SettingsPatch {
        SettingsPatch {
            file_sync: Some(FileSyncSettingsPatch {
                auto_save_dir: value.map(|s| s.to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn sync_patch_preserves_global_and_automatic_choices_independently() {
        let global_disabled = apply_settings_patch(
            Settings::default(),
            SettingsPatch {
                sync: Some(SyncSettingsPatch {
                    sync_enabled: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert!(!global_disabled.sync.sync_enabled);
        assert!(global_disabled.sync.auto_sync_enabled);

        let automatic_disabled = apply_settings_patch(
            Settings::default(),
            SettingsPatch {
                sync: Some(SyncSettingsPatch {
                    auto_sync_enabled: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert!(automatic_disabled.sync.sync_enabled);
        assert!(!automatic_disabled.sync.auto_sync_enabled);
    }

    /// Absent field (patch = None) leaves an existing directory untouched.
    #[test]
    fn absent_auto_save_dir_keeps_existing() {
        let mut existing = Settings::default();
        existing.file_sync.auto_save_dir = Some("/keep".to_string());
        let patch = patch_auto_save_dir(None);
        let result = apply_settings_patch(existing, patch);
        assert_eq!(result.file_sync.auto_save_dir.as_deref(), Some("/keep"));
    }

    /// A non-empty path sets the directory.
    #[test]
    fn sets_auto_save_dir() {
        let existing = Settings::default();
        let patch = patch_auto_save_dir(Some("/Users/me/Downloads"));
        let result = apply_settings_patch(existing, patch);
        assert_eq!(
            result.file_sync.auto_save_dir.as_deref(),
            Some("/Users/me/Downloads")
        );
    }

    #[test]
    fn file_set_caps_round_trip_through_patch_and_view() {
        let existing = Settings::default();
        let result = apply_settings_patch(
            existing,
            SettingsPatch {
                file_sync: Some(FileSyncSettingsPatch {
                    max_file_set_total_bytes: Some(123),
                    max_file_set_member_count: Some(7),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert_eq!(result.file_sync.max_file_set_total_bytes, 123);
        assert_eq!(result.file_sync.max_file_set_member_count, 7);

        let view = SettingsView::from(result);
        assert_eq!(view.file_sync.max_file_set_total_bytes, 123);
        assert_eq!(view.file_sync.max_file_set_member_count, 7);
    }

    /// A blank string clears the directory (back to managed storage).
    #[test]
    fn blank_auto_save_dir_clears() {
        let mut existing = Settings::default();
        existing.file_sync.auto_save_dir = Some("/old".to_string());
        let patch = patch_auto_save_dir(Some("   "));
        let result = apply_settings_patch(existing, patch);
        assert_eq!(result.file_sync.auto_save_dir, None);
    }
}
