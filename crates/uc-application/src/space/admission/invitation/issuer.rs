use std::sync::Arc;

use chrono::{DateTime, Utc};
use tracing::warn;
use uc_core::membership::MembershipAdmissionDecision;
use uc_core::pairing::invitation::PairingInvitation;
use uc_core::ports::pairing_invitation::{CodeOrigin, InvitationError, IssuedInvitation};
use uc_core::ports::{ClockPort, DeviceIdentityPort};
use uc_observability_contract::analytics::{
    AnalyticsFacade, Event, InvitationCodeSource, PairingMethod,
};

use crate::facade::space_setup::{
    InvitationAvailability, IssuePairingInvitationError, IssuePairingInvitationResult,
};
use crate::space::admission::invitation::InMemoryPairingInvitationHolder;
use crate::space::query_membership_admission::QueryMembershipAdmissionPort;

pub(crate) struct PairingInvitationIssuer {
    device_identity: Arc<dyn DeviceIdentityPort>,
    clock: Arc<dyn ClockPort>,
    holder: Arc<InMemoryPairingInvitationHolder>,
    analytics: Arc<dyn AnalyticsFacade>,
    membership_admission: Arc<dyn QueryMembershipAdmissionPort>,
}

impl PairingInvitationIssuer {
    pub(crate) fn new(
        device_identity: Arc<dyn DeviceIdentityPort>,
        clock: Arc<dyn ClockPort>,
        holder: Arc<InMemoryPairingInvitationHolder>,
        analytics: Arc<dyn AnalyticsFacade>,
        membership_admission: Arc<dyn QueryMembershipAdmissionPort>,
    ) -> Self {
        Self {
            device_identity,
            clock,
            holder,
            analytics,
            membership_admission,
        }
    }

    pub(crate) async fn begin(&self) -> Result<u64, IssuePairingInvitationError> {
        self.analytics.capture(Event::PairingStarted {
            method: PairingMethod::Code,
        });
        let snapshot = self
            .membership_admission
            .query_membership_admission(None)
            .await
            .map_err(|error| IssuePairingInvitationError::Internal(error.to_string()))?;
        if snapshot.decision != MembershipAdmissionDecision::Allowed {
            return Err(map_membership_admission_decision(snapshot.decision));
        }
        Ok(snapshot.current_generation)
    }

    pub(crate) async fn finish(
        &self,
        issued: IssuedInvitation,
        admission_generation: u64,
    ) -> Result<IssuePairingInvitationResult, IssuePairingInvitationError> {
        let (code_source, lan_only_mode, availability) = match issued.code_origin {
            CodeOrigin::DirectoryIssued => (
                InvitationCodeSource::DirectoryIssued,
                false,
                InvitationAvailability::CrossNetwork,
            ),
            CodeOrigin::LocallyMintedLanOnly => (
                InvitationCodeSource::LocallyMinted,
                true,
                InvitationAvailability::SameLocalNetwork,
            ),
            CodeOrigin::LocallyMintedDirectoryUnreachable => (
                InvitationCodeSource::LocallyMinted,
                false,
                InvitationAvailability::SameLocalNetwork,
            ),
        };
        self.analytics.capture(Event::PairingInvitationIssued {
            code_source,
            lan_only_mode,
        });

        let issued_at = self.now_utc()?;
        let device_id = self.device_identity.current_device_id();
        let (invitation, _) = PairingInvitation::issue(
            issued.code.clone(),
            issued_at,
            issued.expires_at,
            device_id,
            admission_generation,
        );
        self.holder.insert(invitation).await;

        Ok(IssuePairingInvitationResult {
            code: issued.code,
            expires_at: issued.expires_at,
            availability,
        })
    }

    fn now_utc(&self) -> Result<DateTime<Utc>, IssuePairingInvitationError> {
        let ms = self.clock.now_ms();
        DateTime::<Utc>::from_timestamp_millis(ms).ok_or_else(|| {
            warn!(ms, "clock returned a timestamp outside chrono's range");
            IssuePairingInvitationError::Internal("clock returned invalid timestamp".into())
        })
    }
}

fn map_membership_admission_decision(
    decision: MembershipAdmissionDecision,
) -> IssuePairingInvitationError {
    match decision {
        MembershipAdmissionDecision::Allowed => IssuePairingInvitationError::Internal(
            "membership admission gate returned an incomplete allow result".into(),
        ),
        MembershipAdmissionDecision::AwaitingConvergence => {
            IssuePairingInvitationError::MembershipReconciliationInProgress
        }
        MembershipAdmissionDecision::RecoveryRequired => {
            IssuePairingInvitationError::MembershipReconciliationRequired
        }
        MembershipAdmissionDecision::SupersededInvitation
        | MembershipAdmissionDecision::Unavailable => {
            IssuePairingInvitationError::MembershipReconciliationUnavailable
        }
    }
}

pub(crate) fn map_invitation_error(error: InvitationError) -> IssuePairingInvitationError {
    match error {
        InvitationError::NetworkNotStarted => IssuePairingInvitationError::NetworkNotStarted,
        InvitationError::ServiceUnavailable => IssuePairingInvitationError::ServiceUnavailable,
        InvitationError::AddressNotAvailable(ip) => {
            IssuePairingInvitationError::AddressNotAvailable(ip)
        }
        InvitationError::Internal(message) => IssuePairingInvitationError::Internal(message),
    }
}
