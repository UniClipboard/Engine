//! Application-layer errors for `MemberRosterFacade`.

use thiserror::Error;

/// Failure modes of [`crate::facade::roster::MemberRosterFacade::list_with_presence`].
///
/// `PresencePort::current_state` 故意**不**在这里出现——它的签名是
/// `async fn current_state(...) -> ReachabilityState`(无 Result),
/// 表示"读缓存不可能失败"。若未来 adapter 想在缓存失效时回退到
/// `Unknown`,也应继续在 port 内部消化,保持本 error 小而稳定。
#[derive(Debug, Error)]
pub enum RosterError {
    /// 分布式成员移除在当前宿主上不可用(未组装)。
    #[error("distributed member removal is unavailable on this host")]
    MembershipReconciliationUnavailable,

    /// 成员状态存在，但当前加密会话尚未解锁。
    #[error("membership reconciliation is locked")]
    MembershipReconciliationLocked,

    /// The encrypted local membership state matches no supported format.
    #[error("membership reconciliation state is corrupt")]
    MembershipReconciliationCorrupt,

    /// 分布式成员移除流程失败。
    #[error("distributed member removal failed: {0}")]
    MemberRemoval(String),

    /// 成员移除请求的目标不合法，例如移除本机成员实例。
    #[error("member removal input is invalid")]
    MemberRemovalInvalidInput,

    /// 成员移除的目标不在当前因果成员视图中。
    #[error("member removal target was not found")]
    MemberRemovalTargetNotFound,

    /// `MemberRepositoryPort::list` 故障。消息面向日志;UI 上层一般
    /// 展示一句"无法加载成员列表"+ 原样 error 字符串调试。
    #[error("failed to list members: {0}")]
    MemberRepository(String),

    /// `LocalIdentityPort::get_current_fingerprint` 故障——adapter 在读
    /// 身份存储(keychain / 文件)时出错。区别于"还没创建身份"
    /// (那返回 `Ok(None)`,不是 error),此 variant 表示存储本身故障。
    #[error("failed to read local identity: {0}")]
    LocalIdentity(String),

    /// 目标成员不存在。
    #[error("member `{0}` not found")]
    NotFound(String),

    #[error("failed to bootstrap legacy space security: {0}")]
    GroupBootstrap(String),

    #[error("failed to query space protection: {0}")]
    SpaceProtection(String),

    /// 成员 roster 入口尚未接入。通常表示 daemon/CLI 组合阶段没有注入该能力。
    #[error("member roster facade unavailable")]
    Unavailable,
}
