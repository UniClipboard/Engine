use std::fmt;

use anyhow::Error;
use thiserror::Error;

/// 安全存储调用在到达具体存储前失败。
#[derive(Error)]
#[error("secure storage access failed")]
pub struct SecureStorageAccessFailure {
    #[source]
    source: Error,
}

impl SecureStorageAccessFailure {
    pub fn new(source: impl Into<Error>) -> Self {
        Self {
            source: source.into(),
        }
    }

    pub fn into_source(self) -> Error {
        self.source
    }
}

impl fmt::Debug for SecureStorageAccessFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

/// Secure storage errors.
///
/// 安全存储错误类型。
#[derive(Debug, Error)]
pub enum SecureStorageError {
    /// Secure storage is unavailable on this platform.
    ///
    /// 平台不支持或不可用。
    #[error("secure storage unavailable: {0}")]
    Unavailable(String),

    /// Access was denied by the platform (permissions/ACL).
    ///
    /// 平台权限或 ACL 拒绝访问。
    #[error("secure storage access denied: {0}")]
    PermissionDenied(String),

    /// Stored data is corrupt or invalid.
    ///
    /// 存储数据损坏或无效。
    #[error("secure storage data corrupt: {0}")]
    Corrupt(String),

    /// Other storage failures.
    ///
    /// 其它存储失败。
    #[error("secure storage failed: {0}")]
    Other(String),

    /// 调用在到达具体存储前失败。
    #[error(transparent)]
    AccessFailed(#[from] SecureStorageAccessFailure),
}

/// Secure storage port for key-value secrets.
///
/// 安全存储端口：用于存取敏感字节数据。
pub trait SecureStoragePort: Send + Sync {
    /// Get a value by key.
    ///
    /// 按 key 读取数据。
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError>;

    /// Set a value by key.
    ///
    /// 按 key 写入数据。
    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError>;

    /// Delete a value by key.
    ///
    /// 按 key 删除数据。
    fn delete(&self, key: &str) -> Result<(), SecureStorageError>;
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::io;

    use super::SecureStorageAccessFailure;

    #[test]
    fn access_failure_keeps_source_without_displaying_it() {
        let failure = SecureStorageAccessFailure::new(io::Error::other("private host payload"));
        assert!(failure
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .is_some());
        assert!(!format!("{failure:?}").contains("private"));
        assert!(!format!("{failure}").contains("private"));
    }
}
