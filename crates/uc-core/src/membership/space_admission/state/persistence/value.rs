use super::*;

pub(super) fn decode_join_id(bytes: [u8; 16]) -> Result<JoinId, SpaceAdmissionPersistenceError> {
    JoinId::from_bytes(bytes).ok_or(SpaceAdmissionPersistenceError::InvalidState)
}

pub(super) fn decode_message_id(
    bytes: [u8; 32],
) -> Result<AdmissionMessageId, SpaceAdmissionPersistenceError> {
    AdmissionMessageId::from_bytes(bytes).ok_or(SpaceAdmissionPersistenceError::InvalidState)
}

pub(super) fn decode_continuation_credential(
    bytes: Vec<u8>,
) -> Result<AdmissionContinuationCredential, SpaceAdmissionPersistenceError> {
    AdmissionContinuationCredential::from_bytes(bytes)
        .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)
}

pub(super) fn decode_staged_target(
    bytes: Vec<u8>,
) -> Result<AdmissionStagedTarget, SpaceAdmissionPersistenceError> {
    AdmissionStagedTarget::from_bytes(bytes)
        .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)
}

pub(super) fn validate_state_envelope(
    envelope: &SpaceAdmissionEnvelopeV1,
    admission_id: SpaceAdmissionId,
    kind: SpaceAdmissionMessageKind,
) -> Result<(), SpaceAdmissionPersistenceError> {
    if envelope.header().admission_id() != admission_id || envelope.kind() != kind {
        return Err(SpaceAdmissionPersistenceError::InvalidState);
    }
    Ok(())
}

pub(super) fn validate_activation_receipt(
    receipt: &AdmissionActivationReceipt,
    admission_id: SpaceAdmissionId,
) -> Result<(), SpaceAdmissionPersistenceError> {
    if receipt.receipt_format_version != ADMISSION_ACTIVATION_RECEIPT_FORMAT_V1
        || receipt.attempt_id != *admission_id.as_bytes()
    {
        return Err(SpaceAdmissionPersistenceError::InvalidState);
    }
    Ok(())
}

pub(super) const fn encode_rejection_reason(reason: SpaceAdmissionRejectionReason) -> u8 {
    match reason {
        SpaceAdmissionRejectionReason::InvitationUnavailable => 1,
        SpaceAdmissionRejectionReason::AuthenticationRejected => 2,
        SpaceAdmissionRejectionReason::IdentityConflict => 3,
        SpaceAdmissionRejectionReason::BaseHistoryChanged => 4,
        SpaceAdmissionRejectionReason::JoinerHistoryAhead => 5,
        SpaceAdmissionRejectionReason::HistoryConflict => 6,
        SpaceAdmissionRejectionReason::PeerUpgradeRequired => 7,
        SpaceAdmissionRejectionReason::Cancelled => 8,
        SpaceAdmissionRejectionReason::RemovedBeforeActivation => 9,
    }
}

pub(super) fn decode_rejection_reason(
    value: u8,
) -> Result<SpaceAdmissionRejectionReason, SpaceAdmissionPersistenceError> {
    match value {
        1 => Ok(SpaceAdmissionRejectionReason::InvitationUnavailable),
        2 => Ok(SpaceAdmissionRejectionReason::AuthenticationRejected),
        3 => Ok(SpaceAdmissionRejectionReason::IdentityConflict),
        4 => Ok(SpaceAdmissionRejectionReason::BaseHistoryChanged),
        5 => Ok(SpaceAdmissionRejectionReason::JoinerHistoryAhead),
        6 => Ok(SpaceAdmissionRejectionReason::HistoryConflict),
        7 => Ok(SpaceAdmissionRejectionReason::PeerUpgradeRequired),
        8 => Ok(SpaceAdmissionRejectionReason::Cancelled),
        9 => Ok(SpaceAdmissionRejectionReason::RemovedBeforeActivation),
        _ => Err(SpaceAdmissionPersistenceError::InvalidState),
    }
}

pub(super) const fn encode_recovery_category(category: AdmissionRecoveryCategory) -> u8 {
    match category {
        AdmissionRecoveryCategory::ProtocolConflict => 1,
        AdmissionRecoveryCategory::CorruptState => 2,
        AdmissionRecoveryCategory::MissingKey => 3,
        AdmissionRecoveryCategory::CounterOverflow => 4,
        AdmissionRecoveryCategory::SpaceActivation => 5,
    }
}

pub(super) fn decode_recovery_category(
    value: u8,
) -> Result<AdmissionRecoveryCategory, SpaceAdmissionPersistenceError> {
    match value {
        1 => Ok(AdmissionRecoveryCategory::ProtocolConflict),
        2 => Ok(AdmissionRecoveryCategory::CorruptState),
        3 => Ok(AdmissionRecoveryCategory::MissingKey),
        4 => Ok(AdmissionRecoveryCategory::CounterOverflow),
        5 => Ok(AdmissionRecoveryCategory::SpaceActivation),
        _ => Err(SpaceAdmissionPersistenceError::InvalidState),
    }
}

pub(super) const fn encode_unreadable_history_policy(policy: UnreadableHistoryPolicy) -> u8 {
    match policy {
        UnreadableHistoryPolicy::Discard => 1,
        UnreadableHistoryPolicy::Preserve => 2,
    }
}

pub(super) fn decode_unreadable_history_policy(
    value: u8,
) -> Result<UnreadableHistoryPolicy, SpaceAdmissionPersistenceError> {
    match value {
        1 => Ok(UnreadableHistoryPolicy::Discard),
        2 => Ok(UnreadableHistoryPolicy::Preserve),
        _ => Err(SpaceAdmissionPersistenceError::InvalidState),
    }
}
