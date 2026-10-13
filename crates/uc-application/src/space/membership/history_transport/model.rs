use futures::future::BoxFuture;
use uc_core::membership::{MembershipHistoryExchangeError, MembershipHistoryMessage};
use uc_observability_contract::diagnostics::ObservationContext;

pub struct HistoryIncoming {
    pub message: MembershipHistoryMessage,
    /// 解码只保存载体；完整接收端确认身份之后才进入这个关联。
    pub observation: ObservationContext,
}
pub type HistoryWork =
    BoxFuture<'static, Result<MembershipHistoryMessage, MembershipHistoryExchangeError>>;

pub struct HistoryReply {
    pub message: MembershipHistoryMessage,
    /// 对端明确的连接能力，只能限制保留；不能授予成员资格。
    pub reusable: bool,
}
