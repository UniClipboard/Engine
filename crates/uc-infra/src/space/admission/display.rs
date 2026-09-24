use async_trait::async_trait;
use uc_application::deps::{
    AdmissionDisplayStatus, LoadCurrentJoinStatusPort, PairingConfirmationObservation,
    PairingConfirmationStatus, PairingConfirmationTarget, QueryDeviceTrustError,
};
use uc_application::facade::{
    CurrentJoinStatus, InboundPairing, InboundPairingStatus, JoinSpaceAttentionReason,
    JoinSpaceAttentionRecovery, JoinSpaceTerminationReason as ApplicationTerminationReason,
    PendingInboundMember,
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
        let (current_join, inbound_pairings, pairing_confirmations) = self
            .executor
            .run(|conn| {
                let state = self.load_state_on(conn).map_err(into_anyhow)?;
                let current_join = state
                    .current_local_join_id
                    .or(state.latest_local_join_id)
                    .map(|admission_id| {
                        let stored = state
                            .records
                            .get(&admission_id)
                            .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                        let record = self
                            .open_record(admission_id, stored)
                            .map_err(into_anyhow)?;
                        JoinerAdmission::try_from_record(record)
                            .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))
                    })
                    .transpose()?;
                let mut confirmations = Vec::new();
                let mut inbound_pairings = Vec::new();
                for (admission_id, stored) in &state.records {
                    let aggregate = self
                        .open_record(*admission_id, stored)
                        .map_err(into_anyhow)?;
                    let Some(sponsor) = SponsorAdmission::try_from_record(aggregate) else {
                        continue;
                    };
                    if let Some(preparation) = sponsor.sponsor_settlement_preparation() {
                        let history = VersionedMembershipHistory::decode_persisted_v2(
                            preparation.committed_history().as_bytes(),
                            &OpenMlsHistoricalSignatureVerifier,
                        )
                        .map_err(|_| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                        let facts = history
                            .admission_facts_for(
                                preparation.activation_receipt().joiner_member_instance_id,
                            )
                            .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                        let status = if sponsor.expires_at_ms().is_none() {
                            InboundPairingStatus::NeedsAttention
                        } else {
                            match sponsor
                                .pairing_confirmation()
                                .map(|summary| summary.status())
                            {
                                Some(
                                    SponsorPairingConfirmationStatus::AwaitingPeerConfirmation,
                                ) => InboundPairingStatus::AwaitingConfirmation,
                                Some(SponsorPairingConfirmationStatus::Unconfirmed) => {
                                    InboundPairingStatus::ConfirmationMissed
                                }
                                Some(SponsorPairingConfirmationStatus::Confirmed) | None => {
                                    return Err(into_anyhow(
                                        SpaceAdmissionStateStoreError::Corrupt,
                                    ));
                                }
                            }
                        };
                        inbound_pairings.push(InboundPairing {
                            pairing_id: *admission_id,
                            device_id: Some(facts.device_id.clone()),
                            display_name: Some(facts.device_name.clone()),
                            status,
                        });
                    } else if sponsor.is_failed() {
                        inbound_pairings.push(InboundPairing {
                            pairing_id: *admission_id,
                            device_id: None,
                            display_name: None,
                            status: InboundPairingStatus::Failed,
                        });
                    }
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
                inbound_pairings.sort_by_key(|pairing| pairing.pairing_id);
                Ok((current_join, inbound_pairings, confirmations))
            })
            .map_err(map_executor_error)
            .map_err(map_query_error)?;
        let current_join = match current_join {
            Some(admission) => Some(self.project_current_join(admission).await?),
            None => None,
        };
        let mut active_candidates = inbound_pairings.iter().filter(|pairing| {
            !matches!(pairing.status, InboundPairingStatus::Failed)
                && pairing.device_id.is_some()
                && pairing.display_name.is_some()
        });
        let only_active_candidate = active_candidates
            .next()
            .filter(|_| active_candidates.next().is_none());
        let pending_inbound_member = only_active_candidate.and_then(|pairing| {
            Some(PendingInboundMember {
                device_id: pairing.device_id.clone()?,
                display_name: pairing.display_name.clone()?,
            })
        });
        Ok(AdmissionDisplayStatus {
            current_join,
            inbound_pairings,
            pending_inbound_member,
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
        if admission.needs_attention() {
            return Ok(CurrentJoinStatus::NeedsAttention {
                join_id,
                reason: JoinSpaceAttentionReason::OutcomeCannotBeProven,
                recovery: JoinSpaceAttentionRecovery::PreserveDataAndContactSupport,
                next_retry_at_ms: None,
            });
        }
        if let Some(reason) = admission.rejection_reason() {
            return Ok(CurrentJoinStatus::Rejected { join_id, reason });
        }
        if let Some(reason) = admission.termination_reason() {
            let reason = match reason {
                SpaceAdmissionTerminationReason::Cancelled => {
                    ApplicationTerminationReason::Cancelled
                }
                SpaceAdmissionTerminationReason::Expired => ApplicationTerminationReason::Expired,
                SpaceAdmissionTerminationReason::Superseded => {
                    ApplicationTerminationReason::Superseded
                }
                SpaceAdmissionTerminationReason::ActivationRejected => {
                    return Ok(CurrentJoinStatus::Rejected {
                        join_id,
                        reason: uc_core::membership::SpaceAdmissionRejectionReason::HistoryConflict,
                    });
                }
                SpaceAdmissionTerminationReason::CompletionRejected => {
                    return Ok(CurrentJoinStatus::Rejected {
                        join_id,
                        reason:
                            uc_core::membership::SpaceAdmissionRejectionReason::CompletionInvalid,
                    });
                }
                SpaceAdmissionTerminationReason::MembershipHistoryRejected => {
                    return Ok(CurrentJoinStatus::Rejected {
                        join_id,
                        reason: uc_core::membership::SpaceAdmissionRejectionReason::MembershipHistoryInvalid,
                    });
                }
                SpaceAdmissionTerminationReason::SecurityMaterialRejected => {
                    return Ok(CurrentJoinStatus::Rejected {
                        join_id,
                        reason: uc_core::membership::SpaceAdmissionRejectionReason::SecurityMaterialInvalid,
                    });
                }
                SpaceAdmissionTerminationReason::RelationshipRejected => {
                    return Ok(CurrentJoinStatus::Rejected {
                        join_id,
                        reason:
                            uc_core::membership::SpaceAdmissionRejectionReason::RelationshipConflict,
                    });
                }
                SpaceAdmissionTerminationReason::IdentityRejected => {
                    return Ok(CurrentJoinStatus::Rejected {
                        join_id,
                        reason:
                            uc_core::membership::SpaceAdmissionRejectionReason::IdentityConflict,
                    });
                }
                SpaceAdmissionTerminationReason::ActivationStateRejected => {
                    return Ok(CurrentJoinStatus::Rejected {
                        join_id,
                        reason: uc_core::membership::SpaceAdmissionRejectionReason::ActivationStateInvalid,
                    });
                }
            };
            return Ok(CurrentJoinStatus::Terminated { join_id, reason });
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

        let record = self.membership.load().await?;
        let uc_application::deps::MembershipRecord::Space(space) = record else {
            return Err(QueryDeviceTrustError::RecoveryRequired);
        };
        let history = &space.ledger.history;
        let local_member = space.ledger.local_member;
        let local_facts = history
            .admission_facts_for(local_member)
            .ok_or(QueryDeviceTrustError::RecoveryRequired)?;
        let sponsor_member = history
            .admission_author_for(local_member)
            .ok_or(QueryDeviceTrustError::RecoveryRequired)?;
        let sponsor_facts = history
            .admission_facts_for(sponsor_member)
            .ok_or(QueryDeviceTrustError::RecoveryRequired)?;
        if !admission.is_active_settled() {
            return Ok(CurrentJoinStatus::Processing {
                join_id,
                target_space_id: history.lineage_id().to_owned(),
                sponsor_device_id: sponsor_facts.device_id.clone(),
                sponsor_identity_fingerprint: sponsor_facts.identity_fingerprint.clone(),
                peer_upgrade_required,
            });
        }
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use sha2::{Digest as _, Sha256};
    use uc_application::deps::{
        AdmissionRecoveryTrigger, AdmissionSecurityTransitionInput,
        PendingAdmissionRecoveryStatePort, SpaceWorkMode,
    };
    use uc_core::ids::DeviceId;
    use uc_core::membership::{
        AdmissionActivatedSecurityState, AdmissionActivationReceipt, AdmissionAppliedV1,
        AdmissionAttemptContractV2, AdmissionBaseSnapshot, AdmissionCandidateV1,
        AdmissionChangeFacts, AdmissionCommitV1, AdmissionCompleteV1, AdmissionCompletionV1,
        AdmissionContinuationCredential, AdmissionContinuationRoute, AdmissionInvitationClaim,
        AdmissionKeyPackage, AdmissionMessageId, AdmissionMlsCommit, AdmissionMlsWelcome,
        AdmissionPeerBinding, AdmissionPreparedV1, AdmissionRecoveryPublicKey, AdmissionRole,
        AdmissionSealedRecoveryMaterial, AdmissionSealedSecurityState,
        AdmissionSignedMembershipHistory, AdmissionStagedSecurityState,
        BaseMembershipHistoryPosition, InvitationId, MemberInstanceId, MembershipCredential,
        MembershipOperationV2, PreparedAdmissionProofV1, SpaceAdmissionBodyV1,
        SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SponsorAdmission, UnreadableHistoryPolicy,
        VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
    };
    use uc_core::ports::{SecureStorageError, SecureStoragePort};
    use uc_core::security::IdentityFingerprint;

    use super::*;
    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::pool::init_db_pool;
    use crate::security::{ActiveSpaceGenerationManifestStore, AdmissionKeyManager};
    use crate::space::security::mls_group::MlsGroupEngine;
    use crate::space::AdmissionSecurityTransitionAdapter;

    use super::super::repository::fresh_test_repository_state;

    #[derive(Default)]
    struct MemoryStorage(Mutex<HashMap<String, Vec<u8>>>);

    impl SecureStoragePort for MemoryStorage {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }

        fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
            self.0
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.to_vec());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }

    struct Fixture {
        _directory: tempfile::TempDir,
        database: std::path::PathBuf,
        storage: Arc<MemoryStorage>,
        store: SqliteSpaceAdmissionState<DieselSqliteExecutor>,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let database = directory.path().join("profile.sqlite");
            let storage = Arc::new(MemoryStorage::default());
            let store = Self::open_store(&directory, &database, Arc::clone(&storage));
            Self {
                _directory: directory,
                database,
                storage,
                store,
            }
        }

        fn reopen(&self) -> SqliteSpaceAdmissionState<DieselSqliteExecutor> {
            Self::open_store(&self._directory, &self.database, Arc::clone(&self.storage))
        }

        fn open_store(
            directory: &tempfile::TempDir,
            database: &std::path::Path,
            storage: Arc<MemoryStorage>,
        ) -> SqliteSpaceAdmissionState<DieselSqliteExecutor> {
            let keys = Arc::new(AdmissionKeyManager::new(storage, [0x31; 16]));
            SqliteSpaceAdmissionState::new(
                DieselSqliteExecutor::new(init_db_pool(database.to_str().unwrap()).unwrap()),
                Arc::clone(&keys),
                Arc::new(ActiveSpaceGenerationManifestStore::new(
                    directory.path().join("vault"),
                    keys,
                )),
                Arc::new(
                    crate::space::membership_record::test_support::UnavailableMembershipRecords,
                ),
            )
        }

        fn save(&self, records: &[SponsorAdmission]) {
            self.store
                .executor
                .run(|connection| {
                    let mut state = fresh_test_repository_state([0x31; 16]);
                    for record in records {
                        state.records.insert(
                            *record.admission_id().as_bytes(),
                            self.store.seal_new_record(record).unwrap(),
                        );
                    }
                    self.store.save_state_on(connection, &state).unwrap();
                    Ok(())
                })
                .unwrap();
        }

        fn pending_sponsor_count(&self) -> usize {
            self.store
                .executor
                .run(|connection| {
                    let state = self.store.load_state_on(connection).unwrap();
                    Ok(state
                        .records
                        .iter()
                        .filter(|(admission_id, stored)| {
                            self.store
                                .open_record(**admission_id, stored)
                                .ok()
                                .and_then(SponsorAdmission::try_from_record)
                                .is_some_and(|record| {
                                    record.sponsor_settlement_preparation().is_some()
                                })
                        })
                        .count())
                })
                .unwrap()
        }
    }

    #[tokio::test]
    async fn two_unsettled_sponsor_attempts_remain_independently_queryable_after_restart() {
        let fixture = Fixture::new();
        let first = sponsor_waiting_for_complete_ack(0x41);
        let second = sponsor_waiting_for_complete_ack(0x51);

        fixture.save(std::slice::from_ref(&first));
        let one_pending = fixture
            .store
            .load_admission_display(&[])
            .await
            .expect("one pending inbound member remains queryable");
        assert!(one_pending.pending_inbound_member.is_some());

        fixture.save(&[first, second]);
        assert_eq!(fixture.pending_sponsor_count(), 2);
        let display = fixture
            .store
            .load_admission_display(&[])
            .await
            .expect("two unfinished pairings remain independently queryable");
        assert_eq!(display.inbound_pairings.len(), 2);

        let reopened = fixture.reopen();
        let display_after_restart = reopened
            .load_admission_display(&[])
            .await
            .expect("restart preserves independently queryable pairings");
        assert_eq!(
            display_after_restart.inbound_pairings,
            display.inbound_pairings
        );
        let recovery = PendingAdmissionRecoveryStatePort::load(
            &reopened,
            AdmissionRecoveryTrigger::Periodic,
            301_000,
        )
        .await
        .expect("the two sponsor deadlines remain readable after restart");
        let (_, deadlines, abandonments, _, confirmation_pending, needs_attention) =
            recovery.into_parts();
        assert_eq!(deadlines.len(), 2);
        assert!(abandonments.is_empty());
        assert!(!confirmation_pending);
        assert!(!needs_attention);
        for deadline in deadlines {
            let (record, token) = deadline.into_parts();
            let transition = record
                .terminate_if_expired(301_000)
                .unwrap()
                .expect("the uncommitted candidate deadline is due");
            PendingAdmissionRecoveryStatePort::commit_sponsor_deadline(
                &reopened, token, transition,
            )
            .await
            .expect("the unconfirmed result persists");
        }

        let after_deadline = PendingAdmissionRecoveryStatePort::load(
            &fixture.reopen(),
            AdmissionRecoveryTrigger::Startup,
            301_000,
        )
        .await
        .expect("the deadline result reopens");
        assert!(!after_deadline.pairing_in_progress());
        assert_eq!(after_deadline.len(), 0);
        let expired = fixture
            .reopen()
            .load_admission_display(&[])
            .await
            .expect("expired candidates remain independently visible as failed");
        assert_eq!(expired.inbound_pairings.len(), 2);
        assert!(expired.inbound_pairings.iter().all(|pairing| {
            pairing.status == InboundPairingStatus::Failed
                && pairing.device_id.is_none()
                && pairing.display_name.is_none()
        }));
    }

    #[tokio::test]
    async fn legacy_unconfirmed_candidates_are_reindexed_and_ended_in_place() {
        let fixture = Fixture::new();
        let first = sponsor_waiting_for_complete_ack(0x61)
            .mark_confirmation_unconfirmed(301_000)
            .unwrap()
            .expect("legacy confirmation deadline is due")
            .into_replacement();
        let second = sponsor_waiting_for_complete_ack(0x71)
            .mark_confirmation_unconfirmed(301_000)
            .unwrap()
            .expect("legacy confirmation deadline is due")
            .into_replacement();
        fixture.save(&[first, second]);

        let before = fixture
            .reopen()
            .load_admission_display(&[])
            .await
            .expect("legacy candidates remain queryable before recovery");
        assert_eq!(before.inbound_pairings.len(), 2);
        assert!(before
            .inbound_pairings
            .iter()
            .all(|pairing| pairing.status == InboundPairingStatus::ConfirmationMissed));

        let recovery = PendingAdmissionRecoveryStatePort::load(
            &fixture.reopen(),
            AdmissionRecoveryTrigger::Startup,
            301_000,
        )
        .await
        .expect("legacy recovery summaries rebuild from encrypted records");
        let (_, deadlines, abandonments, _, _, needs_attention) = recovery.into_parts();
        assert_eq!(deadlines.len(), 2);
        assert!(abandonments.is_empty());
        assert!(!needs_attention);
        for deadline in deadlines {
            let (record, token) = deadline.into_parts();
            let transition = record
                .terminate_if_expired(301_000)
                .unwrap()
                .expect("legacy unconfirmed candidate is still uncommitted");
            PendingAdmissionRecoveryStatePort::commit_sponsor_deadline(
                &fixture.store,
                token,
                transition,
            )
            .await
            .expect("legacy candidate ends in place");
        }

        let after = fixture
            .reopen()
            .load_admission_display(&[])
            .await
            .expect("existing devices remain queryable after migration");
        assert_eq!(after.inbound_pairings.len(), 2);
        assert!(after
            .inbound_pairings
            .iter()
            .all(|pairing| pairing.status == InboundPairingStatus::Failed));
    }

    #[tokio::test]
    async fn legacy_candidate_without_commit_proof_is_preserved_for_attention() {
        let fixture = Fixture::new();
        let ambiguous = sponsor_waiting_for_complete_ack_with_contract(0x21, false);
        fixture.save(std::slice::from_ref(&ambiguous));

        let display = fixture
            .reopen()
            .load_admission_display(&[])
            .await
            .expect("ambiguous candidate remains queryable");
        assert_eq!(display.inbound_pairings.len(), 1);
        assert_eq!(
            display.inbound_pairings[0].status,
            InboundPairingStatus::NeedsAttention
        );

        let recovery = PendingAdmissionRecoveryStatePort::load(
            &fixture.reopen(),
            AdmissionRecoveryTrigger::Startup,
            1_000_000,
        )
        .await
        .expect("ambiguous candidate keeps a stable recovery result");
        assert_eq!(recovery.work_mode(), SpaceWorkMode::NeedsAttention);
        assert_eq!(fixture.pending_sponsor_count(), 1);
    }

    fn sponsor_waiting_for_complete_ack(seed: u8) -> SponsorAdmission {
        sponsor_waiting_for_complete_ack_with_contract(seed, true)
    }

    /// 邀请方已接受加入申请、尚未回 Candidate 的记录。
    fn sponsor_waiting_for_candidate(seed: u8) -> SponsorAdmission {
        sponsor_accepted_with_contract(seed, true)
    }

    fn sponsor_accepted_with_contract(seed: u8, bounded: bool) -> SponsorAdmission {
        let admission_id = SpaceAdmissionId::from_bytes([seed; 32]).unwrap();
        let join_request = join_request(admission_id, seed);
        let join_request_evidence = join_request.evidence([seed.wrapping_add(1); 32]).unwrap();
        let peer_binding = AdmissionPeerBinding::new(
            uc_core::membership::AdmissionChannelPeerId::from_bytes([seed.wrapping_add(2); 32])
                .unwrap(),
            uc_core::membership::AdmissionChannelPeerId::from_bytes([seed.wrapping_add(3); 32])
                .unwrap(),
        )
        .unwrap();
        let contract = AdmissionAttemptContractV2::start(
            admission_id,
            InvitationId::from_bytes([seed.wrapping_add(4); 32]).unwrap(),
            peer_binding.remote_peer_id(),
            peer_binding.local_peer_id(),
            1_000,
        )
        .unwrap();
        let invitation_claim =
            AdmissionInvitationClaim::from_bytes(vec![seed.wrapping_add(5); 32]).unwrap();
        let base_snapshot =
            AdmissionBaseSnapshot::from_bytes(vec![seed.wrapping_add(6); 32]).unwrap();
        let continuation =
            AdmissionContinuationCredential::from_bytes(vec![seed.wrapping_add(7); 64]).unwrap();
        if bounded {
            SponsorAdmission::accept_join_request_with_contract(
                admission_id,
                invitation_claim,
                join_request,
                join_request_evidence,
                base_snapshot,
                peer_binding,
                continuation,
                contract,
            )
        } else {
            SponsorAdmission::accept_join_request(
                admission_id,
                invitation_claim,
                join_request,
                join_request_evidence,
                base_snapshot,
                peer_binding,
                continuation,
            )
        }
        .unwrap()
        .into_replacement()
    }

    fn sponsor_waiting_for_complete_ack_with_contract(seed: u8, bounded: bool) -> SponsorAdmission {
        let admission_id = SpaceAdmissionId::from_bytes([seed; 32]).unwrap();
        let (candidate, committed_history, joiner_member) = valid_candidate(admission_id, seed);
        let join_request_id = join_request(admission_id, seed).header().message_id();
        let accepted = sponsor_accepted_with_contract(seed, bounded);
        let candidate_reply = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Sponsor,
            0,
            AdmissionMessageId::from_bytes([seed.wrapping_add(8); 32]).unwrap(),
            Some(join_request_id),
            SpaceAdmissionBodyV1::Candidate(copy_candidate(&candidate)),
        )
        .unwrap();
        let candidate_state = accepted
            .fix_candidate(
                candidate_reply,
                AdmissionStagedSecurityState::from_bytes(vec![seed.wrapping_add(9); 64]).unwrap(),
            )
            .unwrap()
            .into_replacement();
        let candidate_message_id = candidate_state
            .current_exact_reply()
            .unwrap()
            .header()
            .message_id();
        let prepared_id = AdmissionMessageId::from_bytes([seed.wrapping_add(10); 32]).unwrap();
        let prepared = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Joiner,
            1,
            prepared_id,
            Some(candidate_message_id),
            SpaceAdmissionBodyV1::Prepared(AdmissionPreparedV1::new(
                PreparedAdmissionProofV1::new(
                    *admission_id.as_bytes(),
                    candidate.security_commitment().lineage_id.clone(),
                    candidate
                        .security_commitment()
                        .base_history_position
                        .clone(),
                    candidate.candidate_event().event_id(),
                    candidate.candidate_event().resulting_members_digest,
                    candidate.security_commitment().security_commitment_id,
                    joiner_member,
                    match &candidate.candidate_event().operation {
                        MembershipOperationV2::AddDevice { admission } => {
                            admission.membership_credential.credential_id
                        }
                        _ => unreachable!(),
                    },
                    vec![seed.wrapping_add(11); 64],
                ),
            )),
        )
        .unwrap();
        let commit_id = AdmissionMessageId::from_bytes([seed.wrapping_add(12); 32]).unwrap();
        let commit_reply = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Sponsor,
            1,
            commit_id,
            Some(prepared_id),
            SpaceAdmissionBodyV1::Commit(AdmissionCommitV1::new(
                copy_candidate(&candidate),
                AdmissionSignedMembershipHistory::from_bytes(committed_history.clone()).unwrap(),
                AdmissionSealedRecoveryMaterial::from_bytes(vec![seed.wrapping_add(13); 64])
                    .unwrap(),
            )),
        )
        .unwrap();
        let committed = candidate_state
            .commit_prepared(
                prepared,
                [seed.wrapping_add(14); 32],
                AdmissionSignedMembershipHistory::from_bytes(committed_history).unwrap(),
                AdmissionSealedSecurityState::from_bytes(vec![seed.wrapping_add(15); 64]).unwrap(),
                commit_reply,
            )
            .unwrap()
            .into_replacement();
        let receipt = AdmissionActivationReceipt::new(
            1,
            *admission_id.as_bytes(),
            candidate.candidate_event().event_id(),
            candidate.candidate_event().resulting_members_digest,
            candidate.security_commitment().security_commitment_id,
            joiner_member,
            vec![seed.wrapping_add(16); 64],
        );
        let applied_id = AdmissionMessageId::from_bytes([seed.wrapping_add(17); 32]).unwrap();
        let applied = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Joiner,
            2,
            applied_id,
            Some(commit_id),
            SpaceAdmissionBodyV1::Applied(AdmissionAppliedV1::new(receipt.clone())),
        )
        .unwrap();
        let complete = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Sponsor,
            2,
            AdmissionMessageId::from_bytes([seed.wrapping_add(18); 32]).unwrap(),
            Some(applied_id),
            SpaceAdmissionBodyV1::Complete(AdmissionCompleteV1::new(AdmissionCompletionV1::new(
                *admission_id.as_bytes(),
                receipt.event_id,
                [seed.wrapping_add(19); 32],
                receipt.installed_security_commitment_id,
                MemberInstanceId::from_bytes([seed.wrapping_add(20); 32]),
                MembershipCredential::new(
                    ED25519_SIGNATURE_ALGORITHM_V1,
                    vec![seed.wrapping_add(21); 32],
                )
                .credential_id,
                BaseMembershipHistoryPosition {
                    event_id: Some(receipt.event_id),
                    depth: 1,
                    history_digest: [seed.wrapping_add(22); 32],
                },
                vec![seed.wrapping_add(23); 64],
            ))),
        )
        .unwrap();
        let waiting = committed
            .complete_applied(
                applied,
                [seed.wrapping_add(24); 32],
                AdmissionActivatedSecurityState::from_bytes(vec![seed.wrapping_add(25); 64])
                    .unwrap(),
                complete,
            )
            .unwrap();
        assert!(waiting.effects().is_empty());
        waiting.into_replacement()
    }

    fn valid_candidate(
        admission_id: SpaceAdmissionId,
        seed: u8,
    ) -> (AdmissionCandidateV1, Vec<u8>, MemberInstanceId) {
        let sponsor_state =
            MlsGroupEngine::create_sponsor(b"space-a", &[seed]).expect("sponsor MLS state");
        let sponsor_public = MlsGroupEngine::signing_public_key(&sponsor_state).unwrap();
        let sponsor_credential =
            MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, sponsor_public);
        let sponsor_device = DeviceId::new(format!("sponsor-{seed}"));
        let mut sponsor_facts = identity_facts(&sponsor_device, &sponsor_credential);
        sponsor_facts.identity_signature =
            MlsGroupEngine::sign_member_payload(&sponsor_state, &sponsor_facts.signing_payload())
                .unwrap();
        let mut history = VersionedMembershipHistory::new_single_member_root(
            "space-a".to_owned(),
            sponsor_facts.clone(),
            sponsor_credential.clone(),
        )
        .unwrap();
        let base_history = history.encode_persisted_v2().unwrap();

        let pending = MlsGroupEngine::prepare_join(b"joiner-device").unwrap();
        let joiner_public = MlsGroupEngine::signing_public_key(&pending.client_state).unwrap();
        let joiner_credential =
            MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, joiner_public);
        let joiner_device = DeviceId::new("joiner-device");
        let mut joiner_facts = identity_facts(&joiner_device, &joiner_credential);
        joiner_facts.identity_signature = MlsGroupEngine::sign_pending_member_payload(
            &pending.client_state,
            &joiner_facts.signing_payload(),
        )
        .unwrap();
        let joiner_member = joiner_facts.member_instance;
        let recovery_public = [seed.wrapping_add(26); 32];
        let draft = history
            .create_unsigned_local_admission_event(
                sponsor_facts.member_instance,
                &sponsor_credential,
                joiner_facts,
                joiner_credential,
                Sha256::digest(recovery_public).into(),
                [seed.wrapping_add(27); 16],
            )
            .unwrap();
        let input = AdmissionSecurityTransitionInput {
            attempt_id: *admission_id.as_bytes(),
            base_history_position: history.current_position().unwrap(),
            candidate_core_digest: draft
                .admission_candidate_core_digest(*admission_id.as_bytes(), &pending.key_package)
                .unwrap(),
            key_catalog_digest: [seed.wrapping_add(28); 32],
            admission_bundle_digest: [seed.wrapping_add(29); 32],
        };
        let prepared_security = AdmissionSecurityTransitionAdapter::prepare_sponsor(
            sponsor_state.as_bytes(),
            joiner_device.as_str().as_bytes(),
            &pending.key_package,
            &input,
        )
        .unwrap();
        let mut event = history
            .finalize_unsigned_local_admission_event(
                draft,
                &pending.key_package,
                &prepared_security.public_commitment,
            )
            .unwrap();
        event.signature =
            MlsGroupEngine::sign_member_payload(&sponsor_state, &event.signing_payload()).unwrap();
        history
            .verify_and_receive_event(event.clone(), &OpenMlsHistoricalSignatureVerifier)
            .unwrap();
        let committed_history = history.encode_persisted_v2().unwrap();
        let candidate = AdmissionCandidateV1::new(
            AdmissionSignedMembershipHistory::from_bytes(base_history).unwrap(),
            event,
            prepared_security.public_commitment,
            AdmissionMlsCommit::from_bytes(prepared_security.commit).unwrap(),
            AdmissionMlsWelcome::from_bytes(prepared_security.welcome).unwrap(),
            AdmissionContinuationRoute::from_bytes(vec![seed.wrapping_add(30); 32]).unwrap(),
        )
        .unwrap();
        (candidate, committed_history, joiner_member)
    }

    fn copy_candidate(candidate: &AdmissionCandidateV1) -> AdmissionCandidateV1 {
        AdmissionCandidateV1::new(
            AdmissionSignedMembershipHistory::from_bytes(
                candidate.base_membership_history().as_bytes().to_vec(),
            )
            .unwrap(),
            candidate.candidate_event().clone(),
            candidate.security_commitment().clone(),
            AdmissionMlsCommit::from_bytes(candidate.mls_commit().as_bytes().to_vec()).unwrap(),
            AdmissionMlsWelcome::from_bytes(candidate.mls_welcome().as_bytes().to_vec()).unwrap(),
            AdmissionContinuationRoute::from_bytes(
                candidate.continuation_route().as_bytes().to_vec(),
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn join_request(admission_id: SpaceAdmissionId, seed: u8) -> SpaceAdmissionEnvelopeV1 {
        let device_id = DeviceId::new("joiner-device");
        let credential = MembershipCredential::new(
            ED25519_SIGNATURE_ALGORITHM_V1,
            vec![seed.wrapping_add(31); 32],
        );
        let facts = identity_facts(&device_id, &credential);
        let identity_signature = facts.identity_signature.clone();
        SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Joiner,
            0,
            AdmissionMessageId::from_bytes([seed.wrapping_add(32); 32]).unwrap(),
            None,
            SpaceAdmissionBodyV1::JoinRequest(
                uc_core::membership::AdmissionJoinRequestV1::new(
                    InvitationId::from_bytes([seed.wrapping_add(4); 32]).unwrap(),
                    device_id,
                    facts,
                    credential,
                    AdmissionKeyPackage::from_bytes(vec![seed.wrapping_add(33); 48]).unwrap(),
                    AdmissionRecoveryPublicKey::from_bytes([seed.wrapping_add(34); 32]).unwrap(),
                    uc_core::membership::AdmissionIdentitySignature::from_bytes(identity_signature)
                        .unwrap(),
                    UnreadableHistoryPolicy::Discard,
                )
                .unwrap(),
            ),
        )
        .unwrap()
    }

    fn identity_facts(
        device_id: &DeviceId,
        credential: &MembershipCredential,
    ) -> AdmissionChangeFacts {
        AdmissionChangeFacts {
            member_instance: credential.member_instance_id(device_id),
            device_id: device_id.clone(),
            device_name: "Device".to_owned(),
            identity_fingerprint: IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
                .unwrap(),
            transport_public_key: vec![0x71; 32],
            transport_address_blob: vec![0x72; 32],
            identity_signature: vec![0x73; 64],
        }
    }

    /// 未到期的邀请方配对同样占用运行资格：正式提交前普通维护不得插队。
    #[tokio::test]
    async fn an_unfinished_sponsor_pairing_holds_pairing_open_before_its_deadline() {
        let fixture = Fixture::new();
        fixture.save(&[sponsor_waiting_for_candidate(0x61)]);

        let before_deadline = PendingAdmissionRecoveryStatePort::load(
            &fixture.reopen(),
            AdmissionRecoveryTrigger::Periodic,
            1_000,
        )
        .await
        .expect("an unfinished sponsor record stays readable before its deadline");

        assert!(before_deadline.pairing_in_progress());
        // 未到期的记录不加载记录体，只作为运行资格事实。
        assert_eq!(before_deadline.len(), 0);
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
