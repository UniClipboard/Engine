use async_trait::async_trait;
use uc_core::membership::{
    AdmissionAttemptTimeline, AdmissionContinuationCredential,
    AdmissionEncryptedPasswordEquivalent, AdmissionPeerBinding, JoinerAdmissionTransition,
    SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SpaceAdmissionRoute, SponsorAdmissionTransition,
};

use super::AuthenticatedAdmissionReply;
use super::{
    AdmissionRecoveryTrigger, LoadedAdmissionRecovery, LoadedPendingAdmission,
    LoadedSponsorAbandonment, LoadedSponsorDeadline,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PendingAdmissionRecoveryStateError {
    #[error("pending admission recovery state is locked")]
    Locked,

    #[error("pending admission recovery state is unavailable")]
    Unavailable,

    #[error("pending admission recovery state changed")]
    StateChanged,

    #[error("pending admission recovery state is corrupt")]
    RecoveryRequired,
}

#[async_trait]
pub trait PendingAdmissionRecoveryStatePort: Send + Sync {
    async fn load(
        &self,
        trigger: AdmissionRecoveryTrigger,
        now_ms: i64,
    ) -> Result<LoadedAdmissionRecovery, PendingAdmissionRecoveryStateError>;

    /// Commits the replacement and every declared admission effect as one
    /// durable result. Returning success after saving only the replacement is
    /// invalid.
    async fn commit(
        &self,
        token: super::AdmissionRecoveryCommitToken,
        transition: JoinerAdmissionTransition,
    ) -> Result<LoadedPendingAdmission, PendingAdmissionRecoveryStateError>;

    async fn commit_sponsor_deadline(
        &self,
        _token: super::AdmissionRecoveryCommitToken,
        _transition: SponsorAdmissionTransition,
    ) -> Result<LoadedSponsorDeadline, PendingAdmissionRecoveryStateError> {
        Err(PendingAdmissionRecoveryStateError::Unavailable)
    }

    async fn commit_sponsor_abandonment(
        &self,
        _token: super::AdmissionRecoveryCommitToken,
        _transition: SponsorAdmissionTransition,
    ) -> Result<LoadedSponsorAbandonment, PendingAdmissionRecoveryStateError> {
        Err(PendingAdmissionRecoveryStateError::Unavailable)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SpaceAdmissionTransportError {
    #[error("space admission transport is temporarily unavailable")]
    Deferred,

    #[error("the invitation is unavailable")]
    InvitationUnavailable,

    #[error("space admission authentication was rejected")]
    AuthenticationRejected,

    #[error("the remote peer must be upgraded")]
    PeerUpgradeRequired,

    #[error("space admission protocol was rejected")]
    ProtocolRejected,

    #[error("space admission transport is unavailable")]
    Unavailable,
}

#[async_trait]
pub trait AuthenticatedAdmissionExchangePort: Send {
    /// 返回双方的身份绑定
    fn peer_binding(&self) -> AdmissionPeerBinding;
    /// 把后续连接凭据交给 Application 保存
    fn take_newly_established_continuation(&mut self) -> Option<AdmissionContinuationCredential>;
    /// 只交换一次业务消息，随后连接对象被消费
    async fn exchange(
        self: Box<Self>,
        request: &SpaceAdmissionEnvelopeV1,
    ) -> Result<AuthenticatedAdmissionReply, SpaceAdmissionTransportError>;
}

#[async_trait]
pub trait SpaceAdmissionTransportPort: Send + Sync {
    async fn establish_initial(
        &self,
        admission_id: SpaceAdmissionId,
        attempt_timeline: AdmissionAttemptTimeline,
        route: &SpaceAdmissionRoute,
        encrypted_password_equivalent: &AdmissionEncryptedPasswordEquivalent,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError>;

    async fn resume(
        &self,
        admission_id: SpaceAdmissionId,
        route: &SpaceAdmissionRoute,
        peer_binding: AdmissionPeerBinding,
        continuation_credential: &AdmissionContinuationCredential,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError>;
}
