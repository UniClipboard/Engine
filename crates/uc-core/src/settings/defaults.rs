use std::time::Duration;

use super::model::*;

impl Default for GeneralSettings {
    /// Returns the default `GeneralSettings`: no saved device name.
    ///
    /// # Examples
    ///
    /// ```
    /// use uc_core::settings::model::GeneralSettings;
    ///
    /// let settings = GeneralSettings::default();
    /// assert!(settings.device_name.is_none());
    /// ```
    fn default() -> Self {
        Self { device_name: None }
    }
}

impl Default for ContentTypes {
    /// Returns default `ContentTypes` with all fields set to `true`.
    ///
    /// New devices sync everything by default. Users can then disable
    /// specific content types per device.
    ///
    /// # Examples
    ///
    /// ```
    /// use uc_core::settings::model::ContentTypes;
    ///
    /// let ct = ContentTypes::default();
    /// assert!(ct.text);
    /// assert!(ct.image);
    /// assert!(ct.link);
    /// assert!(ct.file);
    /// assert!(ct.code_snippet);
    /// assert!(ct.rich_text);
    /// ```
    fn default() -> Self {
        Self {
            text: true,
            image: true,
            link: true,
            file: true,
            code_snippet: true,
            rich_text: true,
        }
    }
}

impl Default for SyncSettings {
    /// Creates a `SyncSettings` populated with sensible defaults.
    ///
    /// The defaults enable global and automatic syncing, use realtime sync frequency, and
    /// include the default content types.
    ///
    /// # Examples
    ///
    /// ```
    /// use uc_core::settings::model::{SyncSettings, SyncFrequency};
    ///
    /// let s = SyncSettings::default();
    /// assert!(s.sync_enabled);
    /// assert!(s.auto_sync_enabled);
    /// assert_eq!(s.sync_frequency, SyncFrequency::Realtime);
    /// assert!(!s.sync_on_restore);
    /// ```
    fn default() -> Self {
        Self {
            sync_enabled: true,
            auto_sync_enabled: true,
            sync_frequency: SyncFrequency::Realtime,
            content_types: ContentTypes::default(),
            // Opt-in: a restore announces active content to peers only when
            // the user has explicitly enabled it.
            sync_on_restore: false,
        }
    }
}

impl Default for RetentionPolicy {
    /// Creates a `RetentionPolicy` populated with sensible defaults.
    ///
    /// The default policy is enabled, skips pinned items, evaluates rules using `AnyMatch`,
    /// and includes a single rule: keep items younger than 180 days. No count cap is applied
    /// by default (an absent `ByCount` rule means "unlimited").
    ///
    /// The 180-day window (rather than a shorter one) and the absence of a default count cap
    /// are deliberate: this policy previously existed but was never enforced, so installs may
    /// already hold history well past a stricter default. A conservative default avoids a
    /// surprise mass-delete the first time enforcement ships, mirroring the lesson from the
    /// file-cache quota's grandfathering baseline (issue #957).
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use uc_core::settings::model::{RetentionPolicy, RuleEvaluation, RetentionRule};
    ///
    /// let p = RetentionPolicy::default();
    /// assert!(p.enabled);
    /// assert!(p.skip_pinned);
    /// assert_eq!(p.evaluation, RuleEvaluation::AnyMatch);
    /// assert_eq!(p.rules.len(), 1);
    /// assert!(matches!(p.rules.get(0), Some(RetentionRule::ByAge { .. })));
    /// if let Some(RetentionRule::ByAge { max_age }) = p.rules.get(0) {
    ///     assert_eq!(*max_age, Duration::from_secs(60 * 60 * 24 * 180));
    /// }
    /// ```
    fn default() -> Self {
        Self {
            enabled: true,
            skip_pinned: true,
            evaluation: RuleEvaluation::AnyMatch,
            rules: vec![RetentionRule::ByAge {
                max_age: Duration::from_secs(60 * 60 * 24 * 180), // 180 days
            }],
        }
    }
}

impl Default for SecuritySettings {
    /// Creates default security settings with encryption disabled and no passphrase configured.
    ///
    /// The default has `encryption_enabled = false`, `passphrase_configured = false`,
    /// and `auto_unlock_enabled = false`.
    ///
    /// # Examples
    ///
    /// ```
    /// use uc_core::settings::model::SecuritySettings;
    ///
    /// let s = SecuritySettings::default();
    /// assert!(!s.encryption_enabled);
    /// assert!(!s.passphrase_configured);
    /// assert!(!s.auto_unlock_enabled);
    /// ```
    fn default() -> Self {
        Self {
            encryption_enabled: false,
            passphrase_configured: false,
            auto_unlock_enabled: false,
        }
    }
}

impl Default for PairingSettings {
    /// Creates default pairing settings for handshake timers and retry behavior.
    ///
    /// Defaults are:
    /// - `step_timeout`: 30 seconds
    /// - `user_verification_timeout`: 120 seconds
    /// - `session_timeout`: 300 seconds
    /// - `max_retries`: 3
    /// - `protocol_version`: "1.0.0"
    fn default() -> Self {
        Self {
            step_timeout: Duration::from_secs(30),
            user_verification_timeout: Duration::from_secs(120),
            session_timeout: Duration::from_secs(300),
            max_retries: 3,
            protocol_version: "1.0.0".to_string(),
        }
    }
}

impl Default for FileSyncSettings {
    /// Returns default `FileSyncSettings` enabling file sync with sensible limits.
    ///
    /// Defaults:
    /// - `file_sync_enabled`: true
    /// - `small_file_threshold`: 10 MB (inline transfer threshold)
    /// - `max_file_size`: 5 GB
    /// - `max_file_set_total_bytes`: 1 GiB (whole-set cap; ADR-010)
    /// - `max_file_set_member_count`: 2000 (whole-set cap; ADR-010)
    /// - `file_cache_quota_per_device`: 500 MB
    /// - `file_retention_hours`: 24
    /// - `file_auto_cleanup`: true
    /// - `auto_save_dir`: None (inbound files stay in managed storage)
    fn default() -> Self {
        Self {
            file_sync_enabled: true,
            small_file_threshold: 10 * 1024 * 1024, // 10 MB
            max_file_size: 5 * 1024 * 1024 * 1024,  // 5 GB
            max_file_set_total_bytes: 1024 * 1024 * 1024, // 1 GiB
            max_file_set_member_count: 2000,
            file_cache_quota_per_device: 500 * 1024 * 1024, // 500 MB
            file_retention_hours: 24,
            file_auto_cleanup: true,
            auto_save_dir: None,
        }
    }
}

impl Default for NetworkSettings {
    /// 返回默认 `NetworkSettings`：直连失败时允许 iroh 回退到公共 relay
    /// （保持既有 v0.6.x 行为）。
    ///
    /// 默认值：
    /// - `allow_relay_fallback`: true
    /// - `allow_overlay_network_addrs`: false
    /// - `custom_relay_urls`: 空列表（使用产品内置 relay）
    /// - `bind_port`: `None`（随机端口）
    /// - `peer_addresses`: 空（没有用户配置的设备地址）
    ///
    // 默认 true = 允许 fallback。
    // 改成 false 会让所有跨网段老用户突然离线，属于 breaking change。
    // 修改默认值前请先 grep `LAN-only Mode` 文档与 changelog。
    fn default() -> Self {
        Self {
            allow_relay_fallback: true,
            allow_overlay_network_addrs: false,
            custom_relay_urls: Vec::new(),
            congestion_controller: CongestionController::default(),
            bind_port: None,
            peer_addresses: Default::default(),
        }
    }
}

impl Default for MobileSyncSettings {
    /// 默认全部关闭 / 未选定。开启移动端同步暴露 LAN 监听端口,必须由用户
    /// 在设置页显式开启 + 重启 daemon。详见
    /// `.context/mobile-sync/SPEC.md` §5 + §14.10。
    fn default() -> Self {
        Self {
            enabled: false,
            lan_listen_enabled: false,
            lan_advertise_ip: None,
            lan_advertise_base_url: None,
            lan_port: None,
        }
    }
}

impl Default for Settings {
    /// Constructs a Settings instance populated with the current schema version and sensible nested defaults.
    ///
    /// The created `Settings` uses `CURRENT_SCHEMA_VERSION` for `schema_version` and the `Default` implementations
    /// of the nested settings types for `general`, `sync`, `retention_policy`, `security`, and `pairing`.
    ///
    /// # Examples
    ///
    /// ```
    /// use uc_core::settings::model::{Settings, CURRENT_SCHEMA_VERSION};
    ///
    /// let settings = Settings::default();
    /// assert_eq!(settings.schema_version, CURRENT_SCHEMA_VERSION);
    /// // Nested defaults are available:
    /// let _ = settings.general;
    /// let _ = settings.sync;
    /// let _ = settings.retention_policy;
    /// let _ = settings.security;
    /// let _ = settings.pairing;
    /// ```
    ///
    /// # Returns
    ///
    /// `Settings` initialized with `CURRENT_SCHEMA_VERSION` and default values for `general`, `sync`, `retention_policy`, `security`, and `pairing`.
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            general: GeneralSettings::default(),
            sync: SyncSettings::default(),
            retention_policy: RetentionPolicy::default(),
            security: SecuritySettings::default(),
            pairing: PairingSettings::default(),
            file_sync: FileSyncSettings::default(),
            network: NetworkSettings::default(),
            mobile_sync: MobileSyncSettings::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::settings::model::{
        ContentTypes, FileSyncSettings, NetworkSettings, Settings, SyncFrequency, SyncSettings,
        CURRENT_SCHEMA_VERSION,
    };

    /// Pitfall 2 防御：默认值必须为 true（允许 fallback），保护老用户
    /// 跨网段同步行为。改 false = breaking change。
    #[test]
    fn network_settings_default_allows_relay_fallback() {
        let n = NetworkSettings::default();
        assert!(
            n.allow_relay_fallback,
            "NetworkSettings::default().allow_relay_fallback MUST be true (Pitfall 2)"
        );
    }

    /// Pitfall 2 防御：顶层 Settings::default 把 network 装配进去，
    /// 字段值与子结构 default 保持一致。
    #[test]
    fn settings_default_includes_network_with_fallback_allowed() {
        let s = Settings::default();
        assert!(s.network.allow_relay_fallback);
        assert_eq!(s.schema_version, CURRENT_SCHEMA_VERSION);
    }

    /// NETSET-02 success criterion #2：缺 `network` 段的旧 settings.json
    /// 反序列化必须回填默认值 true。
    #[test]
    fn old_settings_json_without_network_section_falls_back_to_default() {
        // 模拟 v0.6.x 时代写出的 settings.json 片段（无 network 字段）
        let json = r#"{}"#;
        let s: Settings = serde_json::from_str(json).expect("parse minimal settings");
        assert!(
            s.network.allow_relay_fallback,
            "missing network section MUST default to true"
        );
        // A versionless JSON blob must NOT read back as CURRENT_SCHEMA_VERSION
        // (that would make it look already-migrated and skip the migration
        // chain); it defaults to the oldest known baseline instead.
        assert_eq!(s.schema_version, 1);
    }

    /// 显式 false 的 JSON 必须保留 false 语义（确认未误取反）。
    #[test]
    fn explicit_allow_relay_fallback_false_is_preserved() {
        let json = r#"{ "network": { "allow_relay_fallback": false } }"#;
        let s: Settings = serde_json::from_str(json).expect("parse explicit false");
        assert!(!s.network.allow_relay_fallback);
    }

    /// 反向断言：显式 true 也保留 true 语义。
    #[test]
    fn explicit_allow_relay_fallback_true_is_preserved() {
        let json = r#"{ "network": { "allow_relay_fallback": true } }"#;
        let s: Settings = serde_json::from_str(json).expect("parse explicit true");
        assert!(s.network.allow_relay_fallback);
    }

    /// 固定端口默认关闭：缺字段的旧 settings.json 与默认值都使用随机端口，
    /// 显式端口原样读回，写出后再读回保持不变。
    #[test]
    fn bind_port_defaults_to_ephemeral_and_round_trips() {
        assert_eq!(NetworkSettings::default().bind_port, None);
        let old: Settings =
            serde_json::from_str(r#"{ "network": { "allow_relay_fallback": false } }"#)
                .expect("parse settings without bind_port");
        assert_eq!(old.network.bind_port, None);

        let pinned: Settings = serde_json::from_str(r#"{ "network": { "bind_port": 51821 } }"#)
            .expect("parse settings with bind_port");
        assert_eq!(pinned.network.bind_port, Some(51821));
        let written = serde_json::to_string(&pinned).expect("serialize");
        let reread: Settings = serde_json::from_str(&written).expect("reparse");
        assert_eq!(reread.network.bind_port, Some(51821));
    }

    /// 用户配置的设备地址默认为空：缺字段的旧 settings.json 读回为空，显式条目原样往返，
    /// `Debug` 不泄露主机与端口。
    #[test]
    fn peer_addresses_default_to_empty_round_trip_and_redact_debug() {
        assert!(NetworkSettings::default().peer_addresses.is_empty());
        let old: Settings =
            serde_json::from_str(r#"{ "network": { "bind_port": 51821 } }"#).expect("parse old");
        assert!(old.network.peer_addresses.is_empty());

        let configured: Settings = serde_json::from_str(
            r#"{ "network": { "peer_addresses": { "device-a": { "host": "laptop.wg", "port": 51821 } } } }"#,
        )
        .expect("parse configured");
        let entry = &configured.network.peer_addresses["device-a"];
        assert_eq!((entry.host.as_str(), entry.port), ("laptop.wg", 51821));
        let written = serde_json::to_string(&configured).expect("serialize");
        let reread: Settings = serde_json::from_str(&written).expect("reparse");
        assert_eq!(
            reread.network.peer_addresses,
            configured.network.peer_addresses
        );
        let debug = format!("{:?}", configured.network);
        assert!(!debug.contains("laptop.wg"));
        assert!(!debug.contains("51821"));
    }

    /// 默认值：默认过滤 overlay 网络地址，保持 v0.6.x 起的现行行为。
    #[test]
    fn network_settings_default_filters_overlay_addrs() {
        let n = NetworkSettings::default();
        assert!(
            !n.allow_overlay_network_addrs,
            "NetworkSettings::default().allow_overlay_network_addrs MUST be false"
        );
    }

    /// 老 settings.json 缺 `allow_overlay_network_addrs` 字段时回填默认 false。
    #[test]
    fn old_settings_json_without_overlay_field_falls_back_to_default() {
        let json = r#"{ "network": { "allow_relay_fallback": true } }"#;
        let s: Settings = serde_json::from_str(json).expect("parse old network section");
        assert!(
            !s.network.allow_overlay_network_addrs,
            "missing allow_overlay_network_addrs MUST default to false"
        );
    }

    /// 显式 true 必须保留（专业用户主动开启）。
    #[test]
    fn explicit_allow_overlay_network_addrs_true_is_preserved() {
        let json = r#"{ "network": { "allow_relay_fallback": true, "allow_overlay_network_addrs": true } }"#;
        let s: Settings = serde_json::from_str(json).expect("parse explicit overlay true");
        assert!(s.network.allow_overlay_network_addrs);
    }

    /// 显式 false 必须保留（双向覆盖）。
    #[test]
    fn explicit_allow_overlay_network_addrs_false_is_preserved() {
        let json = r#"{ "network": { "allow_relay_fallback": true, "allow_overlay_network_addrs": false } }"#;
        let s: Settings = serde_json::from_str(json).expect("parse explicit overlay false");
        assert!(!s.network.allow_overlay_network_addrs);
    }

    /// 默认值：空列表继续使用 iroh 默认中继，不改变老用户行为。
    #[test]
    fn network_settings_default_uses_default_relays() {
        let n = NetworkSettings::default();
        assert!(
            n.custom_relay_urls.is_empty(),
            "NetworkSettings::default().custom_relay_urls MUST be empty"
        );
    }

    /// 老 settings.json 缺 `custom_relay_urls` 字段时回填空列表。
    #[test]
    fn old_settings_json_without_custom_relay_urls_falls_back_to_default() {
        let json = r#"{ "network": { "allow_relay_fallback": true, "allow_overlay_network_addrs": false } }"#;
        let s: Settings = serde_json::from_str(json).expect("parse old network section");
        assert!(s.network.custom_relay_urls.is_empty());
    }

    /// 显式配置的自定义 relay URL 必须保留原始字符串，具体 iroh 解析在
    /// bootstrap/infra 边界统一完成。
    #[test]
    fn explicit_custom_relay_urls_are_preserved() {
        let json = r#"{
            "network": {
                "allow_relay_fallback": true,
                "custom_relay_urls": ["https://relay.example.com."]
            }
        }"#;
        let s: Settings = serde_json::from_str(json).expect("parse custom relay urls");
        assert_eq!(
            s.network.custom_relay_urls,
            vec!["https://relay.example.com.".to_string()]
        );
    }

    // ============================================================
    // Issue #581 回归测试：旧版本 settings.json 在升级后被新版本读取时,
    // 缺失字段必须自动回退到 `Default::default()` 的业务默认值,
    // 反序列化不得失败。
    //
    // 核心场景:`file_sync` 段在 fed7ada2 之前没有 `small_file_threshold`
    // 字段,daemon 启动时反序列化整个 Settings 直接报错。
    // ============================================================

    /// Issue #581 直接复现:`file_sync` 段缺 `small_file_threshold`,
    /// 必须能反序列化并回退到默认 10 MB。
    #[test]
    fn file_sync_missing_small_file_threshold_falls_back_to_default() {
        let json = r#"{
            "file_sync": {
                "file_sync_enabled": true,
                "max_file_size": 5368709120,
                "file_cache_quota_per_device": 524288000,
                "file_retention_hours": 24,
                "file_auto_cleanup": true
            }
        }"#;
        let s: Settings = serde_json::from_str(json).expect("must not error on missing field");
        assert_eq!(s.file_sync.small_file_threshold, 10 * 1024 * 1024);
        assert!(s.file_sync.file_sync_enabled);
        assert_eq!(s.file_sync.max_file_size, 5 * 1024 * 1024 * 1024);
    }

    /// `file_sync` 段为空对象时,所有字段都回退到默认值。
    #[test]
    fn file_sync_empty_object_falls_back_to_full_default() {
        let json = r#"{ "file_sync": {} }"#;
        let s: Settings = serde_json::from_str(json).expect("empty file_sync must parse");
        let expected = FileSyncSettings::default();
        assert_eq!(s.file_sync, expected);
    }

    /// ADR-010 whole-set caps: a settings file predating the caps (no
    /// `max_file_set_*` keys) must deserialize and fall back to the 1 GiB /
    /// 2000 defaults, so upgraded users get the guard without editing config.
    #[test]
    fn file_sync_missing_file_set_caps_fall_back_to_defaults() {
        let json = r#"{
            "file_sync": {
                "file_sync_enabled": true,
                "small_file_threshold": 10485760,
                "max_file_size": 5368709120,
                "file_cache_quota_per_device": 524288000,
                "file_retention_hours": 24,
                "file_auto_cleanup": true
            }
        }"#;
        let s: Settings = serde_json::from_str(json).expect("must parse without file-set caps");
        assert_eq!(s.file_sync.max_file_set_total_bytes, 1024 * 1024 * 1024);
        assert_eq!(s.file_sync.max_file_set_member_count, 2000);
    }

    /// Persisted caps round-trip through serde.
    #[test]
    fn file_sync_file_set_caps_round_trip() {
        let json = r#"{ "file_sync": { "max_file_set_total_bytes": 42, "max_file_set_member_count": 7 } }"#;
        let s: Settings = serde_json::from_str(json).expect("must parse file-set caps");
        assert_eq!(s.file_sync.max_file_set_total_bytes, 42);
        assert_eq!(s.file_sync.max_file_set_member_count, 7);
    }

    /// Pre-existing settings without `auto_save_dir` deserialize to `None`
    /// (feature off), keeping the managed-storage behavior for old users.
    #[test]
    fn file_sync_missing_auto_save_dir_falls_back_to_none() {
        let json = r#"{
            "file_sync": {
                "file_sync_enabled": true,
                "small_file_threshold": 10485760,
                "max_file_size": 5368709120,
                "file_cache_quota_per_device": 524288000,
                "file_retention_hours": 24,
                "file_auto_cleanup": true
            }
        }"#;
        let s: Settings = serde_json::from_str(json).expect("must parse without auto_save_dir");
        assert_eq!(s.file_sync.auto_save_dir, None);
    }

    /// A persisted `auto_save_dir` round-trips through serde.
    #[test]
    fn file_sync_auto_save_dir_round_trips() {
        let json = r#"{ "file_sync": { "auto_save_dir": "/Users/me/Downloads" } }"#;
        let s: Settings = serde_json::from_str(json).expect("must parse auto_save_dir");
        assert_eq!(
            s.file_sync.auto_save_dir.as_deref(),
            Some("/Users/me/Downloads")
        );
    }

    /// `general` 段缺多个字段时仍能解析,缺失字段全部回退。
    #[test]
    fn general_partial_object_fills_missing_fields() {
        let json = r#"{ "general": { "device_name": "Desk" } }"#;
        let s: Settings = serde_json::from_str(json).expect("partial general must parse");
        assert_eq!(s.general.device_name.as_deref(), Some("Desk"));
    }

    /// `sync` 段缺开关与 `content_types` 时均回退默认。
    #[test]
    fn sync_missing_fields_fall_back_to_default() {
        let json = r#"{ "sync": { "sync_frequency": "interval" } }"#;
        let s: Settings = serde_json::from_str(json).expect("partial sync must parse");
        assert_eq!(s.sync.sync_frequency, SyncFrequency::Interval);
        assert!(s.sync.sync_enabled);
        assert!(s.sync.auto_sync_enabled);
        assert_eq!(s.sync.content_types, ContentTypes::default());
    }

    #[test]
    fn sync_settings_persist_independent_global_and_automatic_choices() {
        let persisted =
            serde_json::to_value(SyncSettings::default()).expect("serialize sync settings");

        assert_eq!(persisted["sync_enabled"], true);
        assert_eq!(persisted["auto_sync_enabled"], true);
        assert!(persisted.get("auto_sync").is_none());
    }

    #[test]
    fn v0191_auto_sync_choice_maps_to_automatic_sync_setting() {
        let json = r#"{
            "schema_version": 3,
            "sync": {
                "auto_sync": false,
                "sync_frequency": "interval",
                "sync_on_restore": true
            }
        }"#;

        let settings: Settings = serde_json::from_str(json).expect("parse v0.19.1 settings");

        assert!(settings.sync.sync_enabled);
        assert!(!settings.sync.auto_sync_enabled);
        assert_eq!(settings.sync.sync_frequency, SyncFrequency::Interval);
        assert!(settings.sync.sync_on_restore);
    }

    /// `security` 段缺所有字段时回退默认,且未来加新字段不会再 break 启动。
    #[test]
    fn security_empty_object_falls_back_to_default() {
        let json = r#"{ "security": {} }"#;
        let s: Settings = serde_json::from_str(json).expect("empty security must parse");
        assert!(!s.security.encryption_enabled);
        assert!(!s.security.passphrase_configured);
        assert!(!s.security.auto_unlock_enabled);
    }

    /// `pairing` 段缺字段时回退到 `PairingSettings::default()` —— `serde_with`
    /// 装饰器与 struct 级 `#[serde(default)]` 协同工作。
    #[test]
    fn pairing_partial_object_fills_missing_fields() {
        let json = r#"{ "pairing": { "max_retries": 7 } }"#;
        let s: Settings = serde_json::from_str(json).expect("partial pairing must parse");
        assert_eq!(s.pairing.max_retries, 7);
        assert_eq!(s.pairing.protocol_version, "1.0.0");
        assert_eq!(s.pairing.step_timeout, std::time::Duration::from_secs(30));
    }

    /// `mobile_sync` 段缺 `lan_advertise_ip` / `lan_advertise_base_url` /
    /// `lan_port` 时回退 None。
    #[test]
    fn mobile_sync_missing_optional_fields_fall_back_to_none() {
        let json = r#"{ "mobile_sync": { "enabled": true } }"#;
        let s: Settings = serde_json::from_str(json).expect("partial mobile_sync must parse");
        assert!(s.mobile_sync.enabled);
        assert!(!s.mobile_sync.lan_listen_enabled);
        assert!(s.mobile_sync.lan_advertise_ip.is_none());
        assert!(s.mobile_sync.lan_advertise_base_url.is_none());
        assert!(s.mobile_sync.lan_port.is_none());
    }

    /// 显式字段值不被 `#[serde(default)]` 误覆盖。
    #[test]
    fn explicit_file_sync_values_are_preserved() {
        let json = r#"{
            "file_sync": {
                "file_sync_enabled": false,
                "small_file_threshold": 1024,
                "max_file_size": 2048,
                "file_cache_quota_per_device": 4096,
                "file_retention_hours": 1,
                "file_auto_cleanup": false
            }
        }"#;
        let s: Settings = serde_json::from_str(json).expect("explicit file_sync must parse");
        assert!(!s.file_sync.file_sync_enabled);
        assert_eq!(s.file_sync.small_file_threshold, 1024);
        assert_eq!(s.file_sync.max_file_size, 2048);
        assert_eq!(s.file_sync.file_cache_quota_per_device, 4096);
        assert_eq!(s.file_sync.file_retention_hours, 1);
        assert!(!s.file_sync.file_auto_cleanup);
    }
}
