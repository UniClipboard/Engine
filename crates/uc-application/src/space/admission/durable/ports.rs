use async_trait::async_trait;

use super::error::{AdmissionAttemptRepositoryError, AdmissionOutboxDeliveryError};
use uc_core::membership::{
    AdmissionAttemptId, AdmissionAttemptV1, AdmissionOutboxMessageV1, AdmissionProfileMetadataV1,
    AdmissionRejectionReasonV1, AdmissionTerminalResultV1, TerminalAdmissionAttemptV1,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalJoinStartMutationV1 {
    Create {
        replacement: AdmissionAttemptV1,
    },
    Supersede {
        expected_previous_attempt_id: AdmissionAttemptId,
        expected_previous_record_version: u64,
        previous_terminal: AdmissionAttemptV1,
        replacement: AdmissionAttemptV1,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentLocalJoinProjectionV1 {
    pub device_trust_revision: u64,
    pub attempt_id: AdmissionAttemptId,
    pub join_id: [u8; 16],
    pub local_join_ordinal: u64,
    pub terminal_result: Option<AdmissionTerminalResultV1>,
    pub rejection_reason: Option<AdmissionRejectionReasonV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvitationConsumeDeliveryResultV1 {
    Consumed,
    NotFound,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionOutboxDeliveryResultV1 {
    Deferred,
    Persisted(uc_core::membership::AdmissionInboxRecordV1),
    InvitationConsume(InvitationConsumeDeliveryResultV1),
    Rejected(AdmissionOutboxMessageV1),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionOutboxDeliveryRouteV1 {
    Invitation(Vec<u8>),
    Continuation(Vec<u8>),
}

#[async_trait]
pub trait AdmissionAttemptRepositoryPort: Send + Sync {
    async fn reset_admission_profile(
        &self,
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError> {
        Err(AdmissionAttemptRepositoryError::Repository(
            "device management reset storage is unavailable".to_owned(),
        ))
    }

    async fn commit_local_join_start(
        &self,
        _mutation: LocalJoinStartMutationV1,
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError> {
        Err(AdmissionAttemptRepositoryError::Repository(
            "local join start storage is unavailable".to_owned(),
        ))
    }

    async fn create(
        &self,
        attempt: &AdmissionAttemptV1,
        consumed_invitation_digest: Option<[u8; 32]>,
        initial_membership_history_v2: Option<&[u8]>,
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError>;

    async fn load(
        &self,
        attempt_id: AdmissionAttemptId,
    ) -> Result<Option<AdmissionAttemptV1>, AdmissionAttemptRepositoryError>;

    async fn save_completion_recovery_challenge(
        &self,
        _attempt_id: AdmissionAttemptId,
        _challenge: &[u8],
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError> {
        Err(AdmissionAttemptRepositoryError::Repository(
            "completion recovery challenge storage is unavailable".to_owned(),
        ))
    }

    async fn load_completion_recovery_challenge(
        &self,
        _attempt_id: AdmissionAttemptId,
    ) -> Result<Option<Vec<u8>>, AdmissionAttemptRepositoryError> {
        Err(AdmissionAttemptRepositoryError::Repository(
            "completion recovery challenge storage is unavailable".to_owned(),
        ))
    }

    async fn create_completion_helper(
        &self,
        _attempt: &AdmissionAttemptV1,
        _expected_challenge: &[u8],
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError> {
        Err(AdmissionAttemptRepositoryError::Repository(
            "completion helper storage is unavailable".to_owned(),
        ))
    }

    async fn compare_and_advance(
        &self,
        attempt_id: AdmissionAttemptId,
        expected_record_version: u64,
        next: &AdmissionAttemptV1,
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError>;

    async fn compare_and_advance_with_membership_history_v2(
        &self,
        attempt_id: AdmissionAttemptId,
        expected_record_version: u64,
        next: &AdmissionAttemptV1,
        expected_membership_history_v2: Option<&[u8]>,
        membership_history_v2: &[u8],
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError>;

    async fn scan_recoverable(
        &self,
    ) -> Result<Vec<AdmissionAttemptV1>, AdmissionAttemptRepositoryError>;

    async fn compact_terminal(
        &self,
        attempt_id: AdmissionAttemptId,
        expected_record_version: u64,
    ) -> Result<TerminalAdmissionAttemptV1, AdmissionAttemptRepositoryError>;

    async fn load_terminal(
        &self,
        attempt_id: AdmissionAttemptId,
    ) -> Result<Option<TerminalAdmissionAttemptV1>, AdmissionAttemptRepositoryError>;

    async fn profile_metadata(
        &self,
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError>;

    async fn project_current_local_join(
        &self,
    ) -> Result<Option<CurrentLocalJoinProjectionV1>, AdmissionAttemptRepositoryError>;

    async fn advance_projection_floor(
        &self,
        expected_device_trust_revision: u64,
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError>;
}

#[async_trait]
pub trait AdmissionOutboxDeliveryPort: Send + Sync {
    async fn deliver(
        &self,
        attempt_id: AdmissionAttemptId,
        message: &AdmissionOutboxMessageV1,
        route: Option<&AdmissionOutboxDeliveryRouteV1>,
    ) -> Result<AdmissionOutboxDeliveryResultV1, AdmissionOutboxDeliveryError>;
}
