use crate::error::anyhow_error_constructor;
use uc_core::membership::{AdmissionReplayError, SpaceAdmissionAggregateError};

use super::super::{
    PrepareSponsorCandidateError, PrepareSponsorCommitError, PrepareSponsorCompleteError,
    PrepareSponsorSettledError, SponsorAdmissionStateError,
};

#[derive(Debug, thiserror::Error)]
pub enum HandleAuthenticatedSpaceAdmissionMessageError {
    #[error("the authenticated admission message is invalid")]
    Invalid {
        #[source]
        source: anyhow::Error,
    },
    #[error("the authenticated admission message conflicts with saved state")]
    Conflict {
        #[source]
        source: anyhow::Error,
    },
    #[error("the authenticated admission message is out of order")]
    OutOfOrder {
        #[source]
        source: anyhow::Error,
    },
    #[error("space admission state is locked")]
    Locked {
        #[source]
        source: anyhow::Error,
    },
    #[error("space admission state changed")]
    StateChanged {
        #[source]
        source: anyhow::Error,
    },
    #[error("space admission requires recovery")]
    RecoveryRequired {
        #[source]
        source: anyhow::Error,
    },
    #[error("space admission is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

impl HandleAuthenticatedSpaceAdmissionMessageError {
    anyhow_error_constructor!(pub invalid, Invalid);
    anyhow_error_constructor!(pub conflict, Conflict);
    anyhow_error_constructor!(pub out_of_order, OutOfOrder);
    anyhow_error_constructor!(pub locked, Locked);
    anyhow_error_constructor!(pub state_changed, StateChanged);
    anyhow_error_constructor!(pub recovery_required, RecoveryRequired);
    anyhow_error_constructor!(pub unavailable, Unavailable);
}

impl From<AdmissionReplayError> for HandleAuthenticatedSpaceAdmissionMessageError {
    fn from(error: AdmissionReplayError) -> Self {
        match error {
            AdmissionReplayError::Conflict => Self::conflict(error),
            AdmissionReplayError::OutOfOrder => Self::out_of_order(error),
        }
    }
}

impl From<SpaceAdmissionAggregateError> for HandleAuthenticatedSpaceAdmissionMessageError {
    fn from(error: SpaceAdmissionAggregateError) -> Self {
        Self::invalid(error)
    }
}

impl From<SponsorAdmissionStateError> for HandleAuthenticatedSpaceAdmissionMessageError {
    fn from(error: SponsorAdmissionStateError) -> Self {
        match error {
            SponsorAdmissionStateError::Locked { .. } => Self::locked(error),
            SponsorAdmissionStateError::StateChanged { .. } => Self::state_changed(error),
            SponsorAdmissionStateError::RecoveryRequired { .. } => Self::recovery_required(error),
            SponsorAdmissionStateError::Unavailable { .. } => Self::unavailable(error),
        }
    }
}

impl From<PrepareSponsorCandidateError> for HandleAuthenticatedSpaceAdmissionMessageError {
    fn from(error: PrepareSponsorCandidateError) -> Self {
        match error {
            PrepareSponsorCandidateError::Invalid { .. } => Self::invalid(error),
            PrepareSponsorCandidateError::Unavailable { .. } => Self::unavailable(error),
        }
    }
}

impl From<PrepareSponsorCommitError> for HandleAuthenticatedSpaceAdmissionMessageError {
    fn from(error: PrepareSponsorCommitError) -> Self {
        match error {
            PrepareSponsorCommitError::Invalid { .. } => Self::invalid(error),
            PrepareSponsorCommitError::Unavailable { .. } => Self::unavailable(error),
        }
    }
}

impl From<PrepareSponsorCompleteError> for HandleAuthenticatedSpaceAdmissionMessageError {
    fn from(error: PrepareSponsorCompleteError) -> Self {
        match error {
            PrepareSponsorCompleteError::Invalid { .. } => Self::invalid(error),
            PrepareSponsorCompleteError::Unavailable { .. } => Self::unavailable(error),
        }
    }
}

impl From<PrepareSponsorSettledError> for HandleAuthenticatedSpaceAdmissionMessageError {
    fn from(error: PrepareSponsorSettledError) -> Self {
        match error {
            PrepareSponsorSettledError::Invalid { .. } => Self::invalid(error),
            PrepareSponsorSettledError::Unavailable { .. } => Self::unavailable(error),
        }
    }
}
