use async_trait::async_trait;
use serde::{Deserialize, Serialize};
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

/// 无法区分密钥不匹配与密文认证失败时使用同一类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionReadFailureCategory {
    CredentialMissing,
    AuthenticationMismatch,
    CurrentMetadataInvalid,
    LegacyFallbackInvalid,
    LegacyMigrationFailed,
    RecordRelationIncomplete,
    DerivedSummaryInvalid,
    GenerationMismatch,
    OtherStorageError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionRecoveryAction {
    RestoreCredential,
    ChooseBackup,
    RebuildDerivedState,
    ExportDiagnostics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionRecoveryStage {
    Credential,
    RepositoryMetadata,
    LegacyRepository,
    RepositoryRecord,
    RecoverySummary,
    Storage,
}

impl AdmissionReadFailureCategory {
    pub fn guidance(self) -> (AdmissionRecoveryStage, AdmissionRecoveryAction) {
        use AdmissionReadFailureCategory as Category;
        use AdmissionRecoveryAction as Action;
        use AdmissionRecoveryStage as Stage;
        match self {
            Category::CredentialMissing => (Stage::Credential, Action::RestoreCredential),
            Category::AuthenticationMismatch => (Stage::Credential, Action::ChooseBackup),
            Category::CurrentMetadataInvalid | Category::GenerationMismatch => {
                (Stage::RepositoryMetadata, Action::ChooseBackup)
            }
            Category::LegacyFallbackInvalid | Category::LegacyMigrationFailed => {
                (Stage::LegacyRepository, Action::ChooseBackup)
            }
            Category::RecordRelationIncomplete => (Stage::RepositoryRecord, Action::ChooseBackup),
            Category::DerivedSummaryInvalid => {
                (Stage::RecoverySummary, Action::RebuildDerivedState)
            }
            Category::OtherStorageError => (Stage::Storage, Action::ExportDiagnostics),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PendingAdmissionRecoveryStateError {
    #[error("pending admission state requires restricted recovery")]
    ReadFailure {
        category: AdmissionReadFailureCategory,
        #[source]
        source: anyhow::Error,
    },
    #[error("pending admission recovery state is locked")]
    Locked,

    #[error("pending admission recovery state is unavailable")]
    Unavailable,

    #[error("pending admission recovery state changed")]
    StateChanged,

    #[error("pending admission recovery state is corrupt")]
    RecoveryRequired,
}

impl PendingAdmissionRecoveryStateError {
    pub fn category(&self) -> AdmissionReadFailureCategory {
        match self {
            Self::ReadFailure { category, .. } => *category,
            Self::Locked | Self::Unavailable | Self::StateChanged | Self::RecoveryRequired => {
                AdmissionReadFailureCategory::OtherStorageError
            }
        }
    }
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
