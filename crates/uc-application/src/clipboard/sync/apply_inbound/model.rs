//! 完整入站动作的输入与业务结果；不暴露物化或结算步骤。
use crate::clipboard::write::ClipboardWriteIntent;
use bytes::Bytes;
use uc_core::ids::{DeviceId, EntryId};

/// 一次入站落地由哪个流程负责登记激活，不改变内容保存与系统写入责任。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundActivationOwner {
    /// P2P 等入站流程登记发送者的激活。
    Inbound,
    /// 调用流程在落地后登记并传播激活。
    Caller,
}

/// 完整入站动作的调用输入；不依赖 facade 类型。
#[derive(Debug, Clone)]
pub struct ApplyInboundInput {
    pub from_device: DeviceId,
    pub snapshot_hash: String,
    pub plaintext: Bytes,
    /// 仅用于历史内容重新激活；新内容沿用 RemotePush 并抑制系统写入回声。
    pub resurface_intent: ClipboardWriteIntent,
    pub activation_owner: InboundActivationOwner,
}

/// 一次完整入站动作的结果；系统剪贴板后续写入仍遵循原有监督策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// 新内容已保存；系统写入可由原受监督后续动作执行，部分结果不写系统剪贴板。
    Applied { entry_id: EntryId },
    /// 已完整持有的历史内容重新激活，不重新下载或创建记录。
    Resurfaced {
        snapshot_hash: String,
        existing_entry_id: EntryId,
        /// 系统写入失败时不推进激活登记，调用方不得据此向其他设备传播激活。
        os_write_succeeded: bool,
    },
    /// 短窗口内重复或被丢弃的部分重投，不重新保存或写系统剪贴板。
    /// 重新复制历史内容的正常激活使用 Resurfaced。
    DuplicateSkipped {
        snapshot_hash: String,
        existing_entry_id: EntryId,
    },
    /// 原 V3 信封解码失败；保留既有结果合同，接收循环继续工作。
    DecodeFailed { reason: String },
}
