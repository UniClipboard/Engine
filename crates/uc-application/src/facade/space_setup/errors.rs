//! Application-layer errors for the Slice 1 facade.

use std::net::IpAddr;

use thiserror::Error;

/// Failure modes of B1 `IssuePairingInvitationUseCase`.
///
/// Mirrors
/// [`uc_core::ports::pairing_invitation::InvitationError`] at the
/// application boundary, keeping the upstream-port variant names so UI
/// can branch on intent ("start network" vs. "retry later") without
/// having to import the infra-port enum.
#[derive(Debug, Error)]
pub enum IssuePairingInvitationError {
    #[error("membership reconciliation is still converging")]
    MembershipReconciliationInProgress,

    #[error("membership reconciliation requires recovery before admitting a device")]
    MembershipReconciliationRequired,

    #[error("membership reconciliation state is unavailable")]
    MembershipReconciliationUnavailable,

    /// Underlying network runtime has not been started. UI should surface
    /// "start network first" (A1/A2 completing auto-starts it, so this
    /// typically means startup failed earlier and the user needs to retry).
    #[error("network is not started")]
    NetworkNotStarted,

    /// Rendezvous service unreachable / transient failure. UI may offer a
    /// manual retry.
    #[error("pairing invitation service unavailable")]
    ServiceUnavailable,

    /// 调用方指定的本机地址当前不能用于配对邀请。
    #[error("requested address is not available: {0}")]
    AddressNotAvailable(IpAddr),

    /// Uncategorised adapter-side failure; message for logs only.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Failure modes of B2 `RedeemPairingInvitationUseCase` (joiner side).
///
/// 应用层把三类来源的失败统一成"下一步动作"导向:
///
/// * 本机参数/状态问题（`DeviceNameRequired`）→ UI 让用户补齐再试。
/// * 网络/凭证问题（`InvitationNotFound/Expired` / `PassphraseMismatch` /
///   `SponsorUnreachable` 等）→ UI 展示具体原因，用户决定改口令 /
///   重新要邀请 / 等对方上线再试。
/// * sponsor 主动拒绝（`SponsorRejectedInvitation` / `SponsorDeclined` /
///   `SponsorTimedOut` / `SponsorInternal`）→ 不是本机错，信息告知并让
///   用户重新开始。
#[derive(Debug, Error)]
pub enum RedeemPairingInvitationError {
    /// Rendezvous 服务端没有这条邀请（typo / 从未 issue / 已经被消费）。
    #[error("invitation not found")]
    InvitationNotFound,

    /// 邀请已过 TTL — 让用户重新找 sponsor 要一份。
    #[error("invitation has expired")]
    InvitationExpired,

    /// Sponsor 在线广告过 address，但连接没打通（NAT / relay / 对方掉线）。
    #[error("sponsor is not reachable")]
    SponsorUnreachable,

    /// Rendezvous 服务不可达。
    #[error("pairing invitation service unavailable")]
    ServiceUnavailable,
    /// The sponsor is reachable but only supports the previous pairing
    /// protocol. No local relationship state has been written.
    #[error("sponsor must upgrade before pairing")]
    SponsorUpgradeRequired,

    /// 口令错。覆盖两种来源:(a) 本机 `derive_master_key_for_proof` 解
    /// keyslot 失败；(b) sponsor 收到 proof 后 `verify_proof` 拒绝后发
    /// `Reject(PassphraseMismatch)`。两者语义相同 — UI 提示"再试一次
    /// 口令"。
    #[error("wrong passphrase")]
    PassphraseMismatch,

    /// Sponsor 发来的 keyslot 字节无法解析或版本不支持。属于数据/版本
    /// 故障，和 A2 `UnlockSpaceError::CorruptedKeyMaterial` 同义。
    #[error("space key material corrupted")]
    CorruptedKeyMaterial,

    /// 本机 `Settings.general.device_name` 为空且 command 里也没给 —
    /// UI 应该在进入 join flow 前先收集 device name（和 A1 一致）。
    #[error("device name is required but not provided")]
    DeviceNameRequired,

    #[error("unreadable history requires explicit confirmation")]
    UnreadableHistoryRequiresConfirmation,

    #[error("the previous local join cannot be superseded")]
    PreviousJoinCannotBeSuperseded,

    /// sponsor 收到 `JoinerRequest` 后 code 未命中任何 pending 邀请，回
    /// `Reject(InvitationMismatch)`。多半 race：code 在 sponsor 这边已
    /// 过期或被别的 joiner 消费。
    #[error("sponsor did not recognise the invitation code")]
    SponsorRejectedInvitation,
    #[error("sponsor cannot admit a device at this time")]
    SponsorAdmissionUnavailable,
    #[error("admission conflicts with the sponsor's current membership history")]
    SponsorAdmissionConflict,

    /// sponsor UI 明确拒绝本次配对（Slice 1 未暴露审批 UI，保留语义位）。
    #[error("sponsor declined the pairing request")]
    SponsorDeclined,

    /// sponsor 侧 TTL watchdog 先触发（P7g）— 对方还没看到本机的
    /// `ChallengeResponse`。UI 应提示"网络慢或 sponsor 没响应，重新试"。
    #[error("sponsor timed out the handshake")]
    SponsorTimedOut,

    /// sponsor 回 `Reject(Internal(..))` — 对方本地 persist / settings 出
    /// 问题。消息面向日志。
    #[error("sponsor internal error: {0}")]
    SponsorInternal(String),

    /// 本机等 sponsor 回消息时 TTL 耗尽（recv 超时）。
    #[error("pairing handshake timed out")]
    Timeout,

    /// 握手中途 transport 掉线（sponsor 关闭 stream / iroh connection
    /// 中断 / recv 收到 EOF）。
    #[error("connection lost mid-handshake")]
    ConnectionLost,

    /// 非预期消息、adapter 内部错、序列化等兜底。消息面向日志。
    #[error("internal error: {0}")]
    Internal(String),
}

/// Failure modes of [`crate::facade::space_setup::SpaceFacade::cancel_invitation`]
/// (Slice4 P3 T3.2).
#[derive(Debug, Error)]
pub enum CancelInvitationError {
    /// No in-flight invitation to cancel — the holder is empty. Maps
    /// to HTTP 409 Conflict at the daemon boundary so the UI can
    /// distinguish "nothing to cancel" from a transport error.
    #[error("no in-flight invitation to cancel")]
    NotIssued,

    /// Uncategorised infra / adapter failure.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Failure modes of [`crate::facade::space_setup::SpaceFacade::query_setup_state`]
/// (Slice4 P3 T3.2).
#[derive(Debug, Error)]
pub enum QuerySetupStateError {
    /// Failed to read the current Space identity or another setup projection.
    #[error("failed to read setup state: {0}")]
    StorageFailed(String),

    /// Uncategorised infra / adapter failure.
    #[error("internal error: {0}")]
    Internal(String),
}
