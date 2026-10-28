use crate::ClipboardChangeOrigin;
use async_trait::async_trait;

/// 系统写入的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfWriteAttribution {
    Local,
    Remote,
}

/// 保存未完成写入和可证实的观察身份，并按身份确定来源。
///
/// 未完成写入尚无最终结果。未完成写入结束前，已提供有效身份的观察等待归因。
/// 默认实现不保存身份，所有观察均为本机复制。
#[async_trait]
pub trait SelfWriteLedgerPort: Send + Sync {
    /// 登记一个结果尚未确定的写入。
    async fn begin_identified_write(&self) {}

    /// 结束一个未完成写入，并保存可证实身份及其来源。
    ///
    /// `None` 表示没有可证实身份。每个未完成写入只能结束一次；此操作不保证幂等。
    async fn finish_identified_write(
        &self,
        _identity: Option<Vec<u8>>,
        _attribution: SelfWriteAttribution,
    ) {
    }

    /// 返回观察的来源。
    ///
    /// 已保存身份返回其对应来源。缺失、无效或未知身份返回本机复制。
    /// 返回结果不消费身份；重复观察可得到相同来源。
    async fn attribute_identity(&self, _identity: Option<&[u8]>) -> ClipboardChangeOrigin {
        ClipboardChangeOrigin::LocalCapture
    }
}
