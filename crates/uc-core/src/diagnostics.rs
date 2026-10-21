//! 宿主声明的日志档位：宿主进程实际安装的日志档位名称，Engine 只原样报告，不解释、不保存。
//!
//! 名称会出现在诊断状态与诊断导出清单中，因此只接受短小的固定字符集，任何自由文本都无法经此进入
//! 诊断输出。

/// 档位名称的字节上限。
const MAX_LABEL_LEN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("host log profile label must be 1-32 bytes of [a-z0-9_-]")]
pub struct HostLogProfileError;

/// 校验后的宿主日志档位名称。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostLogProfile(String);

impl HostLogProfile {
    pub fn new(label: impl Into<String>) -> Result<Self, HostLogProfileError> {
        let label = label.into();
        let valid = !label.is_empty()
            && label.len() <= MAX_LABEL_LEN
            && label.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            });
        if !valid {
            return Err(HostLogProfileError);
        }
        Ok(Self(label))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
