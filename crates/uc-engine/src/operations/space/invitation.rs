//! Shared pairing invitation implementation.
//!
//! The daemon uses this internal seam only while its remaining callers migrate
//! to `Engine`. Do not re-export it from the crate root.

use tracing::error;
use uc_application::facade::{AppFacade, IssuePairingInvitationError};
use uc_observability_contract::analytics::{AnalyticsFacade, Event, InvitationIssueErrorCategory};
use uc_observability_contract::error_source::io_error_kind;

use crate::error_codes::{
    INVITATION_DIRECTORY_INVALID_RESPONSE_CODE, INVITATION_DIRECTORY_REJECTED_CODE,
    INVITATION_DIRECTORY_TRANSPORT_FAILED_CODE, INVITATION_FAILED_CODE,
    INVITATION_INVALID_INPUT_CODE, INVITATION_INVALID_STATE_CODE,
    INVITATION_LOCAL_PUBLICATION_FAILED_CODE, INVITATION_NO_LOCAL_ADDRESS_CODE,
    INVITATION_RECONCILIATION_PENDING_CODE, INVITATION_RECOVERY_REQUIRED_CODE,
    INVITATION_UNAVAILABLE_CODE,
};
use crate::{EngineError, EngineErrorCategory, InvitationAvailability, OperationResult};

pub async fn execute_issue_invitation(
    facade: &AppFacade,
    analytics: &dyn AnalyticsFacade,
) -> Result<OperationResult, EngineError> {
    let invitation = match facade.issue_pairing_invitation().await {
        Ok(invitation) => invitation,
        Err(source) => {
            let error = map_issue_invitation_error(source);
            capture_invitation_failure(analytics, &error);
            return Err(error);
        }
    };
    Ok(OperationResult::InvitationIssued {
        invitation_code: invitation.code.as_str().to_string(),
        full_invitation: invitation.full_invitation.into_string(),
        expires_at_ms: invitation.expires_at.timestamp_millis(),
        availability: match invitation.availability {
            uc_application::facade::space_setup::InvitationAvailability::CrossNetwork => {
                InvitationAvailability::CrossNetwork
            }
            uc_application::facade::space_setup::InvitationAvailability::SameLocalNetwork => {
                InvitationAvailability::SameLocalNetwork
            }
        },
    })
}

fn capture_invitation_failure(analytics: &dyn AnalyticsFacade, error: &EngineError) {
    analytics.capture(Event::PairingInvitationFailed {
        error_code: error.code(),
        error_category: invitation_issue_error_category(error.category()),
        retryable: error.is_retryable(),
    });
}

fn invitation_issue_error_category(category: EngineErrorCategory) -> InvitationIssueErrorCategory {
    match category {
        EngineErrorCategory::InvalidInput => InvitationIssueErrorCategory::InvalidInput,
        EngineErrorCategory::InvalidState => InvitationIssueErrorCategory::InvalidState,
        EngineErrorCategory::Unauthorized => InvitationIssueErrorCategory::Unauthorized,
        EngineErrorCategory::NotFound => InvitationIssueErrorCategory::NotFound,
        EngineErrorCategory::Conflict => InvitationIssueErrorCategory::Conflict,
        EngineErrorCategory::Unavailable => InvitationIssueErrorCategory::Unavailable,
        EngineErrorCategory::DeadlineExceeded => InvitationIssueErrorCategory::DeadlineExceeded,
        EngineErrorCategory::Internal => InvitationIssueErrorCategory::Internal,
    }
}

fn map_issue_invitation_error(error: IssuePairingInvitationError) -> EngineError {
    match error {
        IssuePairingInvitationError::NetworkNotStarted => EngineError::new(
            INVITATION_INVALID_STATE_CODE,
            EngineErrorCategory::InvalidState,
            true,
        ),
        IssuePairingInvitationError::NoPublishableAddress { .. } => EngineError::new(
            INVITATION_NO_LOCAL_ADDRESS_CODE,
            EngineErrorCategory::Unavailable,
            true,
        ),
        IssuePairingInvitationError::LocalPublicationFailed { .. } => EngineError::new(
            INVITATION_LOCAL_PUBLICATION_FAILED_CODE,
            EngineErrorCategory::Unavailable,
            true,
        ),
        IssuePairingInvitationError::DirectoryTransportFailed { .. } => EngineError::new(
            INVITATION_DIRECTORY_TRANSPORT_FAILED_CODE,
            EngineErrorCategory::Unavailable,
            true,
        ),
        IssuePairingInvitationError::DirectoryRejected { .. } => EngineError::new(
            INVITATION_DIRECTORY_REJECTED_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        IssuePairingInvitationError::DirectoryInvalidResponse { .. } => EngineError::new(
            INVITATION_DIRECTORY_INVALID_RESPONSE_CODE,
            EngineErrorCategory::Unavailable,
            true,
        ),
        IssuePairingInvitationError::MembershipReconciliationInProgress => EngineError::new(
            INVITATION_RECONCILIATION_PENDING_CODE,
            EngineErrorCategory::InvalidState,
            true,
        ),
        IssuePairingInvitationError::MembershipReconciliationRequired => EngineError::new(
            INVITATION_RECOVERY_REQUIRED_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        IssuePairingInvitationError::MembershipReconciliationUnavailable => EngineError::new(
            INVITATION_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            true,
        ),
        IssuePairingInvitationError::AddressNotAvailable(_) => EngineError::new(
            INVITATION_INVALID_INPUT_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        ),
        IssuePairingInvitationError::ServiceUnavailable => EngineError::new(
            INVITATION_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            true,
        ),
        IssuePairingInvitationError::PassphraseChangeRecovery { .. } => {
            error!(
                error_kind = "invitation_recovery_required",
                io_error_kind = io_error_kind(&error),
                "issue invitation requires passphrase change recovery"
            );
            EngineError::new(
                INVITATION_RECOVERY_REQUIRED_CODE,
                EngineErrorCategory::InvalidState,
                true,
            )
        }
        IssuePairingInvitationError::Internal(_) => {
            error!(
                error_kind = "issue_invitation",
                io_error_kind = io_error_kind(&error),
                "issue invitation failed"
            );
            EngineError::new(INVITATION_FAILED_CODE, EngineErrorCategory::Internal, false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    use uc_observability_contract::analytics::{
        AnalyticsPort, DefaultAnalyticsFacade, NoopAnalyticsIdentity,
    };

    #[derive(Default)]
    struct RecordingAnalyticsSink {
        events: Mutex<Vec<Event>>,
    }

    impl AnalyticsPort for RecordingAnalyticsSink {
        fn capture(&self, event: Event) {
            self.events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event);
        }
    }

    fn recording_analytics() -> (Arc<RecordingAnalyticsSink>, DefaultAnalyticsFacade) {
        let sink = Arc::new(RecordingAnalyticsSink::default());
        let facade = DefaultAnalyticsFacade::new(
            sink.clone() as Arc<dyn AnalyticsPort>,
            Arc::new(NoopAnalyticsIdentity),
        );
        (sink, facade)
    }

    fn recorded_events(sink: &RecordingAnalyticsSink) -> Vec<Event> {
        sink.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    #[test]
    fn invitation_state_failures_keep_distinct_reasons() {
        for (error, code, retryable) in [
            (IssuePairingInvitationError::NetworkNotStarted, 1221, true),
            (
                IssuePairingInvitationError::MembershipReconciliationInProgress,
                1225,
                true,
            ),
            (
                IssuePairingInvitationError::MembershipReconciliationRequired,
                1226,
                false,
            ),
        ] {
            let error = map_issue_invitation_error(error);
            assert_eq!(error.code(), code);
            assert_eq!(error.category(), EngineErrorCategory::InvalidState);
            assert_eq!(error.is_retryable(), retryable);
        }
    }

    #[test]
    fn invitation_publication_failures_keep_stable_public_codes() {
        let cases = [
            (
                IssuePairingInvitationError::NoPublishableAddress {
                    source: anyhow::anyhow!("private address detail"),
                },
                1227,
                EngineErrorCategory::Unavailable,
                true,
            ),
            (
                IssuePairingInvitationError::LocalPublicationFailed {
                    source: anyhow::anyhow!("private socket detail"),
                },
                1228,
                EngineErrorCategory::Unavailable,
                true,
            ),
            (
                IssuePairingInvitationError::DirectoryTransportFailed {
                    source: anyhow::anyhow!("private transport detail"),
                },
                1229,
                EngineErrorCategory::Unavailable,
                true,
            ),
            (
                IssuePairingInvitationError::DirectoryRejected {
                    source: anyhow::anyhow!("private response detail"),
                },
                1230,
                EngineErrorCategory::InvalidState,
                false,
            ),
            (
                IssuePairingInvitationError::DirectoryInvalidResponse {
                    source: anyhow::anyhow!("private response body"),
                },
                1231,
                EngineErrorCategory::Unavailable,
                true,
            ),
        ];

        for (source, code, category, retryable) in cases {
            let error = map_issue_invitation_error(source);
            assert_eq!(error.code(), code);
            assert_eq!(error.category(), category);
            assert_eq!(error.is_retryable(), retryable);
            let public = error.to_string();
            assert!(!public.contains("private"));
        }
    }

    #[test]
    fn invitation_failure_event_uses_only_public_error_fields() {
        let (sink, analytics) = recording_analytics();
        let error = map_issue_invitation_error(IssuePairingInvitationError::DirectoryRejected {
            source: anyhow::anyhow!(
                "sensitive invitation address device space response and system error"
            ),
        });

        capture_invitation_failure(&analytics, &error);

        assert_eq!(
            recorded_events(&sink),
            vec![Event::PairingInvitationFailed {
                error_code: 1230,
                error_category: InvitationIssueErrorCategory::InvalidState,
                retryable: false,
            }]
        );
        let properties = recorded_events(&sink)[0].properties();
        assert_eq!(properties.len(), 3);
        assert!(!properties.values().any(|value| value
            .as_str()
            .is_some_and(|value| value.contains("sensitive"))));
    }

    #[test]
    fn unexpected_internal_failure_records_one_terminal_failure() {
        let (sink, analytics) = recording_analytics();
        let error = map_issue_invitation_error(IssuePairingInvitationError::Internal(
            anyhow::anyhow!("private internal detail"),
        ));

        capture_invitation_failure(&analytics, &error);

        assert_eq!(
            recorded_events(&sink),
            vec![Event::PairingInvitationFailed {
                error_code: INVITATION_FAILED_CODE,
                error_category: InvitationIssueErrorCategory::Internal,
                retryable: false,
            }]
        );
    }
}
