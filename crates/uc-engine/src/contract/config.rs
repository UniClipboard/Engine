use std::fmt;

use serde::{Deserialize, Serialize};
use uc_core::app_dirs::{HostConfigurationFileError, HostConfigurationFiles};
use uc_core::diagnostics::{HostLogProfile, HostLogProfileError};

const DEFAULT_PROFILE_ID: &str = "default";

/// 宿主数据根下的一个附加配置文件。
///
/// 全部声明文件随配置导出，并在导入后的下一次启动恢复到数据根下的同名位置；升级备份照常
/// 保留整个数据根。`reset` 只决定恢复出厂时由谁删除它。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostConfigurationFile {
    pub name: String,
    pub reset: HostConfigurationFileReset,
}

impl HostConfigurationFile {
    pub fn new(name: impl Into<String>, reset: HostConfigurationFileReset) -> Self {
        Self {
            name: name.into(),
            reset,
        }
    }
}

impl fmt::Debug for HostConfigurationFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostConfigurationFile")
            .field("name", &"[REDACTED]")
            .field("reset", &self.reset)
            .finish()
    }
}

/// 恢复出厂时声明文件的删除负责人。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostConfigurationFileReset {
    /// Engine 不删除。宿主在 `FactoryResetSpace` 成功后自行删除，并负责其重试与失败呈现。
    ByHost,
    /// Engine 在清除密钥之后与自身资料一起删除并核对不存在；中断后随重置续做。
    ByEngine,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineConfig {
    app_version: String,
    profile_id: String,
    #[serde(default)]
    portable_storage: bool,
    /// 宿主在数据根下拥有、随配置导出导入的附加配置文件；启动时校验。
    #[serde(default)]
    host_configuration_files: Vec<HostConfigurationFile>,
    /// 宿主进程实际安装的日志档位名称；诊断状态与导出清单原样报告，启动时校验。
    #[serde(default)]
    host_log_profile: Option<String>,
    #[cfg(feature = "dev-tools")]
    #[serde(skip)]
    rendezvous_base_url: Option<String>,
    #[cfg(feature = "dev-tools")]
    #[serde(skip)]
    test_relay_fallback: Option<bool>,
    #[cfg(feature = "dev-tools")]
    #[serde(skip)]
    test_iroh_bind_port: Option<u16>,
    #[cfg(feature = "dev-tools")]
    #[serde(skip)]
    test_legacy_group_updates: bool,
}

impl EngineConfig {
    pub fn new(app_version: impl Into<String>) -> Self {
        Self {
            app_version: app_version.into(),
            profile_id: DEFAULT_PROFILE_ID.to_string(),
            portable_storage: false,
            host_configuration_files: Vec::new(),
            host_log_profile: None,
            #[cfg(feature = "dev-tools")]
            rendezvous_base_url: None,
            #[cfg(feature = "dev-tools")]
            test_relay_fallback: None,
            #[cfg(feature = "dev-tools")]
            test_iroh_bind_port: None,
            #[cfg(feature = "dev-tools")]
            test_legacy_group_updates: false,
        }
    }

    pub fn with_profile_id(mut self, profile_id: impl Into<String>) -> Self {
        self.profile_id = profile_id.into();
        self
    }

    pub fn with_portable_storage(mut self, portable_storage: bool) -> Self {
        self.portable_storage = portable_storage;
        self
    }

    /// 只接受数据根下的一级 `.json` 文件名；不合规、重复或与 Engine 自有文件重名时
    /// `Engine::start` 以 `HOST_CONFIGURATION_FILES_INVALID_CODE` 拒绝启动。
    pub fn with_host_configuration_files(
        mut self,
        files: impl IntoIterator<Item = HostConfigurationFile>,
    ) -> Self {
        self.host_configuration_files = files.into_iter().collect();
        self
    }

    pub fn host_configuration_files(&self) -> &[HostConfigurationFile] {
        &self.host_configuration_files
    }

    pub(crate) fn validated_host_configuration_files(
        &self,
    ) -> Result<HostConfigurationFiles, HostConfigurationFileError> {
        HostConfigurationFiles::new(self.host_configuration_files.iter().map(|file| {
            let reset = match file.reset {
                HostConfigurationFileReset::ByHost => {
                    uc_core::app_dirs::HostConfigurationFileReset::ByHost
                }
                HostConfigurationFileReset::ByEngine => {
                    uc_core::app_dirs::HostConfigurationFileReset::ByEngine
                }
            };
            (file.name.clone(), reset)
        }))
    }

    /// 声明宿主本次进程实际安装的日志档位，例如 `prod` 或 `debug`。Engine 不解释、不保存该名称，
    /// 也不据此改变自身日志；未声明时诊断报告为缺失。名称须为 1–32 字节的 `[a-z0-9_-]`，否则
    /// `Engine::start` 以 `HOST_LOG_PROFILE_INVALID_CODE` 拒绝启动。
    pub fn with_host_log_profile(mut self, label: impl Into<String>) -> Self {
        self.host_log_profile = Some(label.into());
        self
    }

    pub fn host_log_profile(&self) -> Option<&str> {
        self.host_log_profile.as_deref()
    }

    pub(crate) fn validated_host_log_profile(
        &self,
    ) -> Result<Option<HostLogProfile>, HostLogProfileError> {
        self.host_log_profile
            .as_deref()
            .map(HostLogProfile::new)
            .transpose()
    }

    pub fn app_version(&self) -> &str {
        &self.app_version
    }

    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    pub fn uses_portable_storage(&self) -> bool {
        self.portable_storage
    }

    #[cfg(feature = "dev-tools")]
    pub fn with_rendezvous_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.rendezvous_base_url = Some(base_url.into());
        self
    }

    #[cfg(feature = "dev-tools")]
    pub fn with_test_relay_fallback(mut self, allow_relay_fallback: bool) -> Self {
        self.test_relay_fallback = Some(allow_relay_fallback);
        self
    }

    #[cfg(feature = "dev-tools")]
    pub fn with_test_iroh_bind_port(mut self, port: u16) -> Self {
        self.test_iroh_bind_port = Some(port);
        self
    }

    /// 只用于真实旧版协议互通验收，生产仍优先协商 /2。
    #[cfg(feature = "dev-tools")]
    pub fn with_test_legacy_group_updates(mut self, enabled: bool) -> Self {
        self.test_legacy_group_updates = enabled;
        self
    }
    #[cfg(feature = "dev-tools")]
    pub(crate) fn uses_test_legacy_group_updates(&self) -> bool {
        self.test_legacy_group_updates
    }

    pub(crate) fn rendezvous_base_url_override(&self) -> Option<String> {
        #[cfg(feature = "dev-tools")]
        {
            return self.rendezvous_base_url.clone();
        }
        #[cfg(not(feature = "dev-tools"))]
        {
            None
        }
    }

    pub(crate) fn test_relay_fallback_override(&self) -> Option<bool> {
        #[cfg(feature = "dev-tools")]
        {
            return self.test_relay_fallback;
        }
        #[cfg(not(feature = "dev-tools"))]
        {
            None
        }
    }

    pub(crate) fn test_iroh_bind_port_override(&self) -> Option<u16> {
        #[cfg(feature = "dev-tools")]
        {
            return self.test_iroh_bind_port;
        }
        #[cfg(not(feature = "dev-tools"))]
        {
            None
        }
    }
}

impl fmt::Debug for EngineConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("EngineConfig");
        debug
            .field("app_version", &self.app_version)
            .field("profile_id", &"[REDACTED]")
            .field("portable_storage", &self.portable_storage)
            .field(
                "host_configuration_file_count",
                &self.host_configuration_files.len(),
            )
            .field("has_host_log_profile", &self.host_log_profile.is_some());
        #[cfg(feature = "dev-tools")]
        debug.field(
            "has_rendezvous_override",
            &self.rendezvous_base_url.is_some(),
        );
        #[cfg(feature = "dev-tools")]
        debug.field(
            "has_test_relay_fallback_override",
            &self.test_relay_fallback.is_some(),
        );
        #[cfg(feature = "dev-tools")]
        debug.field(
            "has_test_iroh_bind_port_override",
            &self.test_iroh_bind_port.is_some(),
        );
        debug.finish()
    }
}
