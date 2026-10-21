//! 宿主声明的附加配置文件：只是数据根下的文件名及其重置负责人，Engine 不理解其内容。
//!
//! 配置导出与导入恢复处理全部声明文件；恢复出厂时 Engine 只删除重置负责人为 Engine 的文件，
//! 其余由宿主在 Engine 重置成功后自行处理。名字只能是数据根下的一级 `.json` 文件，不得与
//! Engine 自有文件重名，因此任何声明都无法指向数据根以外或 Engine 状态。

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// 单个文件名的字节上限。
const MAX_NAME_LEN: usize = 64;

/// 数据根下由 Engine 或宿主运行期持有、不可作为宿主配置声明的 `.json` 文件。
const RESERVED_NAMES: [&str; 6] = [
    "settings.json",
    "pending-import.json",
    "upgrade-cursor.json",
    "first-sync-state.json",
    "daemon-run.json",
    "daemon-last-exit.json",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HostConfigurationFileError {
    #[error("host configuration file name is not a plain data-root json file name")]
    InvalidName,
    #[error("host configuration file name is reserved by the engine")]
    Reserved,
    #[error("host configuration file name is declared more than once")]
    Duplicate,
}

/// 恢复出厂时由谁删除该文件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostConfigurationFileReset {
    /// Engine 不删除；宿主在 Engine 重置成功后自行删除或保留。
    ByHost,
    /// 与 Engine 资料一起在密钥清除之后删除，并随重置阶段重放。
    ByEngine,
}

/// 校验后的宿主配置文件声明，按名字排序，迭代顺序稳定。
#[derive(Clone, Default, PartialEq, Eq)]
pub struct HostConfigurationFiles {
    names: BTreeMap<String, HostConfigurationFileReset>,
}

impl HostConfigurationFiles {
    pub fn new<I, S>(declarations: I) -> Result<Self, HostConfigurationFileError>
    where
        I: IntoIterator<Item = (S, HostConfigurationFileReset)>,
        S: Into<String>,
    {
        let mut validated = BTreeMap::new();
        for (name, reset) in declarations {
            let name = name.into();
            validate_name(&name)?;
            if validated.insert(name, reset).is_some() {
                return Err(HostConfigurationFileError::Duplicate);
            }
        }
        Ok(Self { names: validated })
    }

    pub fn contains(&self, name: &str) -> bool {
        self.names.contains_key(name)
    }

    /// 全部声明文件名：配置导出与导入恢复的范围。
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.names.keys().map(String::as_str)
    }

    /// 恢复出厂时由 Engine 删除的文件在数据根下的路径。
    pub fn engine_reset_paths(&self, app_data_root: &Path) -> Vec<PathBuf> {
        self.names
            .iter()
            .filter(|(_, reset)| **reset == HostConfigurationFileReset::ByEngine)
            .map(|(name, _)| app_data_root.join(name))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

// 文件名可能透露宿主产品信息，调试输出只给数量。
impl fmt::Debug for HostConfigurationFiles {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostConfigurationFiles")
            .field("count", &self.names.len())
            .finish()
    }
}

fn validate_name(name: &str) -> Result<(), HostConfigurationFileError> {
    let bytes = name.as_bytes();
    let valid = !bytes.is_empty()
        && bytes.len() <= MAX_NAME_LEN
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
        && name.ends_with(".json")
        && name.len() > ".json".len()
        && !name.contains("..");
    if !valid {
        return Err(HostConfigurationFileError::InvalidName);
    }
    if RESERVED_NAMES.contains(&name) {
        return Err(HostConfigurationFileError::Reserved);
    }
    Ok(())
}
