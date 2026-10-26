//! 本地接收事实及旧进度帧使用的粗粒度状态。
use crate::file_transfer::FileTransferCancellationReason;

/// 一帧出站进度上报的语义状态。
///
/// 适配器端会把这个值映射到 wire 状态字节;应用层关心四种粗粒度
/// 业务状态:三种终态(Completed / Failed / Cancelled)与一种活跃态
/// (InProgress)。`Cancelled` 与 `Failed` 在领域上是不同语义——前者
/// 是预期内的用户/系统主动放弃,后者是非预期的错误;sender 端 UI 据
/// 此区分中性"已取消"展示与告警"失败"展示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundProgressStatus {
    /// 进行中:`bytes_transferred` 是当前累计已收字节,`total_bytes`
    /// 在已知时透传。
    InProgress,
    /// 接收端已经把整个 blob 拉完,`bytes_transferred` 等于最终大小。
    Completed,
    /// 接收端 fetch 失败,sender 端可据此把 transferring 状态切为 failed。
    Failed,
    /// 接收端主动取消了 fetch(用户点取消、超时 sweep、被新内容替换等),
    /// sender 端据此把 transferring 状态切为 cancelled。`reason` 区分
    /// 取消的子类别,与领域层 [`FileTransferCancellationReason`] 1:1 对应。
    Cancelled {
        reason: FileTransferCancellationReason,
    },
}
