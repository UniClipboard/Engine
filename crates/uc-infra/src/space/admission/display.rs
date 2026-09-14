use async_trait::async_trait;
use uc_application::deps::{
    AdmissionDisplayStatus, LoadCurrentJoinStatusPort, PairingConfirmationObservation,
    PairingConfirmationStatus, PairingConfirmationTarget, QueryDeviceTrustError,
};
use uc_application::facade::{
    CurrentJoinStatus, JoinSpaceTerminationReason as ApplicationTerminationReason,
};
use uc_core::membership::{
    AdmissionSpaceTransitionResultV2, JoinerAdmission, SpaceAdmissionTerminationReason,
    SponsorAdmission, SponsorPairingConfirmationStatus, VersionedMembershipHistory,
};

use crate::db::ports::DbExecutor;
use crate::space::OpenMlsHistoricalSignatureVerifier;

use super::repository::codec::{into_anyhow, map_executor_error};
use super::repository::{SpaceAdmissionStateStoreError, SqliteSpaceAdmissionState};

#[async_trait]
impl<E: DbExecutor + Send + Sync> LoadCurrentJoinStatusPort for SqliteSpaceAdmissionState<E> {
    #[tracing::instrument(name = "space_admission.current_join_status.load", skip_all, err)]
    async fn load_current_join(&self) -> Result<Option<CurrentJoinStatus>, QueryDeviceTrustError> {
        let admission = self
            .executor
            .run(|conn| {
                let state = self.load_state_on(conn).map_err(into_anyhow)?;
                let Some(admission_id) = state.current_local_join_id.or(state.latest_local_join_id)
                else {
                    return Ok(None);
                };
                let stored = state
                    .records
                    .get(&admission_id)
                    .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                let record = self
                    .open_record(admission_id, stored)
                    .map_err(into_anyhow)?;
                let admission = JoinerAdmission::try_from_record(record)
                    .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                Ok(Some(admission))
            })
            .map_err(map_executor_error)
            .map_err(map_query_error)?;
        match admission {
            Some(admission) => self.project_current_join(admission).await.map(Some),
            None => Ok(None),
        }
    }

    #[tracing::instrument(name = "space_admission.display_status.load", skip_all, err)]
    async fn load_admission_display(
        &self,
        targets: &[PairingConfirmationTarget],
    ) -> Result<AdmissionDisplayStatus, QueryDeviceTrustError> {
        let (current_join, pairing_confirmations) =
            self.executor
                .run(|conn| {
                    let state = self.load_state_on(conn).map_err(into_anyhow)?;
                    let current_join = state
                        .current_local_join_id
                        .or(state.latest_local_join_id)
                        .map(|admission_id| {
                            let stored = state.records.get(&admission_id).ok_or_else(|| {
                                into_anyhow(SpaceAdmissionStateStoreError::Corrupt)
                            })?;
                            let record = self
                                .open_record(admission_id, stored)
                                .map_err(into_anyhow)?;
                            JoinerAdmission::try_from_record(record)
                                .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))
                        })
                        .transpose()?;
                    let mut confirmations = Vec::new();
                    for (admission_id, stored) in &state.records {
                        let aggregate = self
                            .open_record(*admission_id, stored)
                            .map_err(into_anyhow)?;
                        let Some(sponsor) = SponsorAdmission::try_from_record(aggregate) else {
                            continue;
                        };
                        let Some(summary) = sponsor.pairing_confirmation() else {
                            continue;
                        };
                        let target = PairingConfirmationTarget {
                            member_instance_id: summary.member_instance_id(),
                            add_event_id: summary.add_event_id(),
                        };
                        if targets.contains(&target) {
                            confirmations.push(PairingConfirmationObservation {
                                target,
                                status: map_pairing_confirmation_status(summary.status()),
                            });
                        }
                    }
                    Ok((current_join, confirmations))
                })
                .map_err(map_executor_error)
                .map_err(map_query_error)?;
        let current_join = match current_join {
            Some(admission) => Some(self.project_current_join(admission).await?),
            None => None,
        };
        Ok(AdmissionDisplayStatus {
            current_join,
            pairing_confirmations,
        })
    }
}

impl<E: DbExecutor + Send + Sync> SqliteSpaceAdmissionState<E> {
    async fn project_current_join(
        &self,
        admission: JoinerAdmission,
    ) -> Result<CurrentJoinStatus, QueryDeviceTrustError> {
        let join_id = *admission.join_id().as_bytes();
        let peer_upgrade_required = admission.peer_upgrade_required();
        if let Some(reason) = admission.termination_reason() {
            let reason = match reason {
                SpaceAdmissionTerminationReason::Cancelled => {
                    ApplicationTerminationReason::Cancelled
                }
                SpaceAdmissionTerminationReason::Expired => ApplicationTerminationReason::Expired,
                SpaceAdmissionTerminationReason::Superseded => {
                    ApplicationTerminationReason::Superseded
                }
            };
            return Ok(CurrentJoinStatus::Terminated { join_id, reason });
        }
        if let Some(reason) = admission.rejection_reason() {
            return Ok(CurrentJoinStatus::Rejected { join_id, reason });
        }
        if !admission.is_active() {
            return Ok(CurrentJoinStatus::Pending {
                join_id,
                target_space_id: None,
                sponsor_device_id: None,
                sponsor_identity_fingerprint: None,
                cancel_requested: admission.is_cancelling(),
                peer_upgrade_required,
            });
        }

        let ledger = self.membership.load().await?;
        let history = VersionedMembershipHistory::decode_persisted_v2(
            ledger
                .membership_history
                .as_deref()
                .ok_or(QueryDeviceTrustError::RecoveryRequired)?,
            &OpenMlsHistoricalSignatureVerifier,
        )
        .map_err(|_| QueryDeviceTrustError::RecoveryRequired)?;
        let local_member = ledger
            .local_member_instance
            .ok_or(QueryDeviceTrustError::RecoveryRequired)?;
        let local_facts = history
            .admission_facts_for(local_member)
            .ok_or(QueryDeviceTrustError::RecoveryRequired)?;
        let sponsor_member = history
            .admission_author_for(local_member)
            .ok_or(QueryDeviceTrustError::RecoveryRequired)?;
        let sponsor_facts = history
            .admission_facts_for(sponsor_member)
            .ok_or(QueryDeviceTrustError::RecoveryRequired)?;
        let transition = admission
            .active_transition_result()
            .and_then(|result| AdmissionSpaceTransitionResultV2::decode(result.as_bytes()));
        let (migrated_records, preserved_unreadable_records) = match transition {
            Some(AdmissionSpaceTransitionResultV2::CrossSpace(result)) => (
                Some(result.migrated_records),
                Some(result.preserved_unreadable_records),
            ),
            Some(AdmissionSpaceTransitionResultV2::SameSpace { .. }) => (Some(0), Some(0)),
            Some(
                AdmissionSpaceTransitionResultV2::CrossSpaceControl(_)
                | AdmissionSpaceTransitionResultV2::SameSpaceControl(_)
                | AdmissionSpaceTransitionResultV2::FreshControl(_),
            ) => (Some(0), Some(0)),
            Some(AdmissionSpaceTransitionResultV2::Fresh { .. }) | None => (None, None),
        };
        Ok(CurrentJoinStatus::Active {
            join_id,
            joined_space: uc_application::facade::JoinedSpace {
                sponsor_device_id: sponsor_facts.device_id.clone(),
                sponsor_identity_fingerprint: sponsor_facts.identity_fingerprint.clone(),
                space_id: history.lineage_id().to_owned(),
                self_device_id: local_facts.device_id.clone(),
                self_identity_fingerprint: local_facts.identity_fingerprint.clone(),
                migrated_records,
                preserved_unreadable_records,
            },
            peer_upgrade_required,
        })
    }
}

fn map_pairing_confirmation_status(
    status: SponsorPairingConfirmationStatus,
) -> PairingConfirmationStatus {
    match status {
        SponsorPairingConfirmationStatus::AwaitingPeerConfirmation => {
            PairingConfirmationStatus::AwaitingPeerConfirmation
        }
        SponsorPairingConfirmationStatus::Unconfirmed => PairingConfirmationStatus::Unconfirmed,
        SponsorPairingConfirmationStatus::Confirmed => PairingConfirmationStatus::Confirmed,
    }
}

fn map_query_error(error: SpaceAdmissionStateStoreError) -> QueryDeviceTrustError {
    QueryDeviceTrustError::Dependency {
        source: anyhow::Error::new(error),
    }
}
