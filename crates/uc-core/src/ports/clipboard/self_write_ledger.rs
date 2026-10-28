use crate::ClipboardChangeOrigin;
use async_trait::async_trait;

/// 系统写入的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfWriteAttribution {
    Local,
    Remote,
}

/// 管理系统写入及宿主确认的观察身份。
#[async_trait]
pub trait SelfWriteLedgerPort: Send + Sync {
    /// 在宿主写入开始前登记未完成动作。
    async fn begin_identified_write(&self) {}

    /// 完成写入。失败或无法证明身份时传入 None。
    async fn finish_identified_write(
        &self,
        _identity: Option<Vec<u8>>,
        _attribution: SelfWriteAttribution,
    ) {
    }

    /// 等待进行中的写入完成，只按可靠身份归因。
    async fn attribute_identity(&self, _identity: Option<&[u8]>) -> ClipboardChangeOrigin {
        ClipboardChangeOrigin::LocalCapture
    }
}
