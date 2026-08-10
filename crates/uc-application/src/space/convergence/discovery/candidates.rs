use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use tracing::warn;
use uc_core::ids::SpaceId;
use uc_core::membership::{
    CandidateEvent, CandidateFailure, CandidateMergeOutcome, MemberSyncPreferences,
    MembershipAttestationEndpointError, MembershipAttestationEndpointPort,
    MembershipAttestationError, RelayedSecurityUpdate, SpaceMember, SpaceMembershipCandidate,
    SponsorCandidateSeed, VerifiedMembershipPeer,
};
use uc_core::ports::PeerAddressRecord;
use uc_core::TrustedPeer;

use super::*;

impl MembershipConvergence {
    pub(super) async fn accept_sponsor_seed(
        &self,
        seed: SponsorCandidateSeed,
    ) -> Result<CandidateMergeOutcome, MembershipConvergenceError> {
        let now_ms = self.deps.clock.now_ms();
        self.deps
            .candidate_repo
            .purge_expired(&seed.space_id, now_ms)
            .await?;

        let formal_member = self
            .deps
            .member_repo
            .get(&seed.device_id)
            .await
            .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?;
        if let Some(member) = formal_member.as_ref() {
            if member.identity_fingerprint != seed.identity_fingerprint_hint {
                return Err(MembershipConvergenceError::VerificationRejected);
            }
        }

        let existing = self
            .deps
            .candidate_repo
            .get(&seed.space_id, &seed.device_id)
            .await?;
        if let Some(member) = formal_member {
            return match existing {
                Some(mut candidate) => {
                    if candidate.identity_fingerprint_hint() != &member.identity_fingerprint {
                        return Err(MembershipConvergenceError::VerificationRejected);
                    }
                    let (outcome, _) = candidate.apply(CandidateEvent::Seed(seed), now_ms)?;
                    let _ = candidate.apply(CandidateEvent::Admitted, now_ms)?;
                    self.deps.candidate_repo.save(&candidate).await?;
                    Ok(outcome)
                }
                None => {
                    SpaceMembershipCandidate::from_sponsor_seed(seed, now_ms)?;
                    Ok(CandidateMergeOutcome::Unchanged)
                }
            };
        }
        match existing {
            Some(mut candidate) => {
                let (outcome, effect) = candidate.apply(CandidateEvent::Seed(seed), now_ms)?;
                if effect.persist {
                    self.deps.candidate_repo.save(&candidate).await?;
                }
                if effect.wake_runtime {
                    self.wake.notify_one();
                }
                Ok(outcome)
            }
            None => {
                let candidate = SpaceMembershipCandidate::from_sponsor_seed(seed, now_ms)?;
                self.deps.candidate_repo.save(&candidate).await?;
                self.wake.notify_one();
                Ok(CandidateMergeOutcome::Updated)
            }
        }
    }

    pub(super) async fn load_pending(
        &self,
        space_id: &SpaceId,
    ) -> Result<Vec<SpaceMembershipCandidate>, MembershipConvergenceError> {
        let now_ms = self.deps.clock.now_ms();
        self.deps
            .candidate_repo
            .purge_expired(space_id, now_ms)
            .await?;
        Ok(self
            .deps
            .candidate_repo
            .list(space_id)
            .await?
            .into_iter()
            .filter(|candidate| is_pending(candidate.status()))
            .collect())
    }

    pub(super) async fn apply_relayed_security_updates(
        &self,
        space_id: &SpaceId,
        updates: &[RelayedSecurityUpdate],
    ) -> Result<u64, MembershipConvergenceError> {
        let mut state = self.deps.security_updates.current_state().await?;
        if &state.space_id != space_id {
            return Err(MembershipConvergenceError::VerificationRejected);
        }
        let mut epoch_advanced = false;
        for update in updates {
            let digest = self
                .deps
                .hash
                .hash_bytes(&update.payload)
                .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?;
            if digest.bytes != update.digest
                || update.next_epoch != update.previous_epoch.saturating_add(1)
            {
                return Err(MembershipConvergenceError::VerificationRejected);
            }
            if update.next_epoch <= state.group_epoch {
                continue;
            }
            if update.previous_epoch != state.group_epoch {
                return Err(MembershipConvergenceError::WaitingForUpdate);
            }
            let applied_epoch = self
                .deps
                .security_updates
                .apply_group_epoch_update(&update.payload)
                .await?;
            if applied_epoch != update.next_epoch {
                return Err(MembershipConvergenceError::VerificationRejected);
            }
            self.deps
                .applied_security_updates
                .save(space_id, update)
                .await
                .map_err(MembershipConvergenceError::from)?;
            state.group_epoch = applied_epoch;
            epoch_advanced = true;
        }
        if epoch_advanced {
            self.reawaken_waiting_for_update_candidates(space_id)
                .await?;
        }
        Ok(state.group_epoch)
    }

    async fn reawaken_waiting_for_update_candidates(
        &self,
        space_id: &SpaceId,
    ) -> Result<usize, MembershipConvergenceError> {
        let now_ms = self.deps.clock.now_ms();
        let candidates = self.deps.candidate_repo.list(space_id).await?;
        let mut reawakened = 0usize;
        for mut candidate in candidates {
            let (outcome, effect) =
                candidate.apply(CandidateEvent::SecurityMaterialApplied, now_ms)?;
            if outcome != CandidateMergeOutcome::Updated {
                continue;
            }
            if effect.persist {
                self.deps.candidate_repo.save(&candidate).await?;
            }
            if effect.wake_runtime {
                self.wake.notify_one();
            }
            reawakened = reawakened.saturating_add(1);
        }
        Ok(reawakened)
    }

    pub(super) async fn confirm_candidate(
        &self,
        space_id: &SpaceId,
        device_id: &uc_core::ids::DeviceId,
    ) -> Result<(), MembershipConvergenceError> {
        let _attempt = self.candidate_attempt_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut candidate = self
            .deps
            .candidate_repo
            .get(space_id, device_id)
            .await?
            .ok_or(MembershipConvergenceError::CandidateNotFound)?;
        candidate.apply(CandidateEvent::Confirming, now_ms)?;
        self.deps.candidate_repo.save(&candidate).await?;

        let verified = match self.deps.attestation.attest_candidate(&candidate).await {
            Ok(verified) => verified,
            Err(
                error @ (MembershipAttestationError::Offline
                | MembershipAttestationError::Transport),
            ) => {
                let failure = if matches!(error, MembershipAttestationError::Offline) {
                    CandidateFailure::PeerOffline
                } else {
                    CandidateFailure::Transport
                };
                candidate.apply(
                    CandidateEvent::AttestationFailed {
                        failure,
                        retry_at_ms: Some(next_candidate_retry_at(&candidate, now_ms)),
                    },
                    now_ms,
                )?;
                self.deps.candidate_repo.save(&candidate).await?;
                return Err(MembershipConvergenceError::PeerUnavailable);
            }
            Err(MembershipAttestationError::MissingSecurityUpdate) => {
                let local_epoch = self
                    .deps
                    .security_updates
                    .current_state()
                    .await
                    .map(|state| state.group_epoch)
                    .unwrap_or_default();
                warn!(
                    candidate_device_id = %candidate.device_id().as_str(),
                    local_group_epoch = local_epoch,
                    candidate_update_count = candidate.security_updates().len(),
                    retry_at_ms = next_candidate_retry_at(&candidate, now_ms),
                    error_kind = "membership_attestation_missing_security_update",
                    "candidate attestation deferred until the missing security update arrives"
                );
                candidate.apply(
                    CandidateEvent::AttestationFailed {
                        failure: CandidateFailure::MissingSecurityUpdate,
                        retry_at_ms: Some(next_candidate_retry_at(&candidate, now_ms)),
                    },
                    now_ms,
                )?;
                self.deps.candidate_repo.save(&candidate).await?;
                return Err(MembershipConvergenceError::WaitingForUpdate);
            }
            Err(MembershipAttestationError::VersionIncompatible) => {
                candidate.apply(
                    CandidateEvent::AttestationFailed {
                        failure: CandidateFailure::VersionIncompatible,
                        retry_at_ms: Some(next_candidate_retry_at(&candidate, now_ms)),
                    },
                    now_ms,
                )?;
                self.deps.candidate_repo.save(&candidate).await?;
                return Err(MembershipConvergenceError::PeerUnavailable);
            }
            Err(MembershipAttestationError::Rejected) => {
                candidate.apply(
                    CandidateEvent::AttestationFailed {
                        failure: CandidateFailure::InvalidProof,
                        retry_at_ms: None,
                    },
                    now_ms,
                )?;
                self.deps.candidate_repo.save(&candidate).await?;
                return Err(MembershipConvergenceError::VerificationRejected);
            }
        };

        let outcome = match candidate.apply(CandidateEvent::VerifiedPeer(verified.clone()), now_ms)
        {
            Ok((outcome, _)) => outcome,
            Err(_) => CandidateMergeOutcome::Unchanged,
        };
        if outcome != CandidateMergeOutcome::Updated {
            candidate.apply(
                CandidateEvent::AttestationFailed {
                    failure: CandidateFailure::InvalidProof,
                    retry_at_ms: None,
                },
                now_ms,
            )?;
            self.deps.candidate_repo.save(&candidate).await?;
            return Err(MembershipConvergenceError::VerificationRejected);
        }
        self.promote_verified_peer(&mut candidate, verified, now_ms)
            .await
    }

    pub(super) async fn accept_verified_peer(
        &self,
        verified: VerifiedMembershipPeer,
    ) -> Result<(), MembershipConvergenceError> {
        let now_ms = self.deps.clock.now_ms();
        let existing = self
            .deps
            .candidate_repo
            .get(&verified.space_id, &verified.device_id)
            .await?;
        let mut candidate = match existing {
            Some(mut candidate) => {
                let (outcome, _) =
                    candidate.apply(CandidateEvent::VerifiedPeer(verified.clone()), now_ms)?;
                if outcome != CandidateMergeOutcome::Updated {
                    return Err(MembershipConvergenceError::VerificationRejected);
                }
                candidate.apply(CandidateEvent::Confirming, now_ms)?;
                candidate
            }
            None => SpaceMembershipCandidate::from_verified_peer(
                &verified,
                now_ms.saturating_add(DIRECT_ATTESTATION_TTL_MS),
                now_ms,
            )?,
        };
        self.deps.candidate_repo.save(&candidate).await?;
        self.promote_verified_peer(&mut candidate, verified, now_ms)
            .await
    }

    async fn promote_verified_peer(
        &self,
        candidate: &mut SpaceMembershipCandidate,
        verified: VerifiedMembershipPeer,
        now_ms: i64,
    ) -> Result<(), MembershipConvergenceError> {
        let observed_at = Utc
            .timestamp_millis_opt(now_ms)
            .single()
            .ok_or_else(|| MembershipConvergenceError::Relationship("invalid clock".into()))?;
        let address = PeerAddressRecord {
            device_id: verified.device_id,
            addr_blob: verified.transport_address_blob,
            observed_at,
        };
        let trusted_peer = TrustedPeer {
            local_device_id: self.deps.device_identity.current_device_id(),
            peer_device_id: verified.device_id,
            peer_fingerprint: verified.identity_fingerprint.clone(),
            trusted_at: observed_at,
        };
        let member = SpaceMember {
            device_id: verified.device_id,
            device_name: verified.device_name,
            identity_fingerprint: verified.identity_fingerprint,
            joined_at: observed_at,
            sync_preferences: MemberSyncPreferences::default(),
        };
        candidate.apply(CandidateEvent::Admitted, now_ms)?;
        self.deps
            .verified_peer_promotion
            .promote_verified_peer(&member, &trusted_peer, &address, candidate)
            .await
            .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl MembershipAttestationEndpointPort for MembershipConvergence {
    async fn apply_relayed_security_updates(
        &self,
        space_id: &SpaceId,
        updates: &[RelayedSecurityUpdate],
    ) -> Result<u64, MembershipAttestationEndpointError> {
        MembershipConvergence::apply_relayed_security_updates(self, space_id, updates)
            .await
            .map_err(|error| match error {
                MembershipConvergenceError::WaitingForUpdate => {
                    MembershipAttestationEndpointError::MissingSecurityUpdate
                }
                MembershipConvergenceError::VerificationRejected
                | MembershipConvergenceError::InvalidCandidate(_)
                | MembershipConvergenceError::CandidateNotFound => {
                    MembershipAttestationEndpointError::Rejected
                }
                _ => MembershipAttestationEndpointError::Persistence,
            })
    }

    async fn accept_verified_peer(
        &self,
        peer: VerifiedMembershipPeer,
    ) -> Result<(), MembershipAttestationEndpointError> {
        MembershipConvergence::accept_verified_peer(self, peer)
            .await
            .map_err(|error| match error {
                MembershipConvergenceError::VerificationRejected
                | MembershipConvergenceError::InvalidCandidate(_)
                | MembershipConvergenceError::CandidateNotFound => {
                    MembershipAttestationEndpointError::Rejected
                }
                _ => MembershipAttestationEndpointError::Persistence,
            })
    }
}

pub(super) fn next_candidate_retry_at(candidate: &SpaceMembershipCandidate, now_ms: i64) -> i64 {
    let multiplier = 1i64 << candidate.attempt_count().min(4);
    let base = INITIAL_RETRY_DELAY_MS
        .saturating_mul(multiplier)
        .min(MAX_RETRY_DELAY_MS);
    let jitter_window = (base / 5).max(1);
    let jitter_seed = candidate
        .device_id()
        .as_str()
        .bytes()
        .fold(u64::from(candidate.attempt_count()), |sum, byte| {
            sum.wrapping_mul(31).wrapping_add(u64::from(byte))
        });
    let jitter = (jitter_seed % jitter_window as u64) as i64;
    now_ms.saturating_add(base).saturating_add(jitter)
}
