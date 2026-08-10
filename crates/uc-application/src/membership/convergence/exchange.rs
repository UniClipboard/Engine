use async_trait::async_trait;
use tracing::{info, warn};

use super::*;

#[derive(Default)]
struct SponsorSeedDeliveryMetrics {
    seed_count: usize,
    device_metadata_bytes: usize,
    address_bytes: usize,
    security_update_count: usize,
    security_update_bytes: usize,
    largest_seed_bytes: usize,
    largest_security_update_bytes: usize,
}

impl SponsorSeedDeliveryMetrics {
    fn from_seeds(seeds: &[SponsorCandidateSeed]) -> Self {
        let mut metrics = Self {
            seed_count: seeds.len(),
            ..Self::default()
        };
        for seed in seeds {
            let device_metadata_bytes = seed
                .space_id
                .as_ref()
                .len()
                .saturating_add(seed.device_id.as_str().len())
                .saturating_add(seed.device_name_hint.len())
                .saturating_add(seed.identity_fingerprint_hint.as_display().len())
                .saturating_add(seed.source_device_id.as_str().len())
                .saturating_add(64);
            let security_update_bytes = seed
                .security_updates
                .iter()
                .map(|update| update.payload.len())
                .sum::<usize>();
            metrics.device_metadata_bytes = metrics
                .device_metadata_bytes
                .saturating_add(device_metadata_bytes);
            metrics.address_bytes = metrics
                .address_bytes
                .saturating_add(seed.transport_address_blob.len());
            metrics.security_update_count = metrics
                .security_update_count
                .saturating_add(seed.security_updates.len());
            metrics.security_update_bytes = metrics
                .security_update_bytes
                .saturating_add(security_update_bytes);
            metrics.largest_seed_bytes = metrics.largest_seed_bytes.max(
                device_metadata_bytes
                    .saturating_add(seed.transport_address_blob.len())
                    .saturating_add(
                        seed.security_updates
                            .iter()
                            .map(|update| update.payload.len().saturating_add(64))
                            .sum::<usize>(),
                    ),
            );
            metrics.largest_security_update_bytes = metrics.largest_security_update_bytes.max(
                seed.security_updates
                    .iter()
                    .map(|update| update.payload.len())
                    .max()
                    .unwrap_or(0),
            );
        }
        metrics
    }
}

fn split_sponsor_seed_events(
    space_id: &SpaceId,
    seeds: &[SponsorCandidateSeed],
) -> Result<Vec<Vec<MembershipEvent>>, MembershipConvergenceError> {
    let mut batches = Vec::new();
    let mut current = Vec::new();

    for seed in seeds.iter().cloned() {
        current.push(MembershipEvent::SponsorSeed(seed));
        let candidate = MembershipEventBatch {
            space_id: space_id.clone(),
            batch_id: [0; 32],
            events: current.clone(),
        };
        match candidate.validate_transfer_bounds() {
            Ok(()) => {}
            Err(MembershipGossipBoundsError::MessageTooLarge) if current.len() > 1 => {
                let event = current.pop().ok_or_else(|| {
                    MembershipConvergenceError::Relationship(
                        "membership delivery batch unexpectedly empty".into(),
                    )
                })?;
                batches.push(std::mem::take(&mut current));
                current.push(event);
                MembershipEventBatch {
                    space_id: space_id.clone(),
                    batch_id: [0; 32],
                    events: current.clone(),
                }
                .validate_transfer_bounds()?;
            }
            Err(error) => return Err(error.into()),
        }
    }

    if !current.is_empty() {
        batches.push(current);
    }
    Ok(batches)
}

impl MembershipConvergence {
    pub(super) async fn deliver_workspace_recovery(
        &self,
        space_id: &SpaceId,
    ) -> Result<(), MembershipConvergenceError> {
        let transport = self
            .workspace_recovery
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(transport) = transport else {
            return Ok(());
        };
        let mut updates = self
            .deps
            .applied_security_updates
            .list(space_id)
            .await
            .map_err(MembershipConvergenceError::from)?;
        updates.extend(
            self.deps
                .candidate_repo
                .list(space_id)
                .await?
                .into_iter()
                .flat_map(|candidate| candidate.security_updates().to_vec()),
        );
        updates.sort_by_key(|update| update.previous_epoch);
        updates.dedup_by_key(|update| update.digest);
        if updates.is_empty() {
            return Ok(());
        }
        let local_device_id = self.deps.device_identity.current_device_id();
        let members = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?;
        for member in members {
            if member.device_id == local_device_id {
                continue;
            }
            let _ = transport
                .deliver_recovery(&member.device_id, &updates)
                .await;
        }
        Ok(())
    }

    pub(super) async fn refresh_local_announcement(
        &self,
    ) -> Result<DeviceAnnouncement, MembershipConvergenceError> {
        let material = self
            .deps
            .announcement_material
            .current_announcement_material()
            .await
            .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?;
        let state = self.deps.security_updates.current_state().await?;
        if material.space_id != state.space_id
            || material.device_id != self.deps.device_identity.current_device_id()
        {
            return Err(MembershipConvergenceError::VerificationRejected);
        }
        let derived_fingerprint = self
            .deps
            .fingerprint_factory
            .from_public_key(&material.transport_public_key)
            .map_err(|_| MembershipConvergenceError::VerificationRejected)?;
        if derived_fingerprint != material.identity_fingerprint {
            return Err(MembershipConvergenceError::VerificationRejected);
        }
        let now_ms = self.deps.clock.now_ms();
        let existing = self
            .deps
            .announcement_repo
            .get(&state.space_id, &material.device_id)
            .await?;
        if let Some(existing) = existing.as_ref() {
            let unchanged = existing.device_name == material.device_name
                && existing.identity_fingerprint == material.identity_fingerprint
                && existing.transport_public_key == material.transport_public_key
                && existing.transport_address_blob == material.transport_address_blob
                && existing.group_epoch == state.group_epoch;
            if unchanged
                && existing.expires_at_ms > now_ms.saturating_add(ANNOUNCEMENT_REFRESH_LEAD_MS)
            {
                return Ok(existing.clone());
            }
        }
        let sequence = existing
            .as_ref()
            .map(|announcement| announcement.sequence.saturating_add(1))
            .unwrap_or(1);
        let mut announcement = DeviceAnnouncement {
            space_id: state.space_id,
            device_id: material.device_id,
            device_name: material.device_name,
            identity_fingerprint: material.identity_fingerprint,
            transport_public_key: material.transport_public_key,
            transport_address_blob: material.transport_address_blob,
            sequence,
            group_epoch: state.group_epoch,
            expires_at_ms: now_ms.saturating_add(DIRECT_ATTESTATION_TTL_MS),
            content_digest: [0; 32],
            signature: Vec::new(),
        };
        announcement.content_digest = self
            .deps
            .hash
            .hash_bytes(&announcement.content_bytes())
            .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?
            .bytes;
        announcement.signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&announcement.signing_payload())
            .await
            .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?;
        MembershipEventBatch {
            space_id: announcement.space_id.clone(),
            batch_id: announcement.content_digest,
            events: vec![MembershipEvent::Announcement(announcement.clone())],
        }
        .validate_transfer_bounds()
        .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?;
        self.deps.announcement_repo.save(&announcement).await?;
        Ok(announcement)
    }

    #[cfg(test)]
    pub(super) async fn build_sponsor_seed_batch(
        &self,
        context: SponsorSeedBatchContext,
    ) -> Result<Vec<SponsorCandidateSeed>, MembershipConvergenceError> {
        self.build_sponsor_seed_batch_inner(&context).await
    }

    pub(super) async fn build_sponsor_seed_batch_inner(
        &self,
        context: &SponsorSeedBatchContext,
    ) -> Result<Vec<SponsorCandidateSeed>, MembershipConvergenceError> {
        let previous_epoch = context.group_epoch.checked_sub(1).ok_or_else(|| {
            MembershipConvergenceError::Relationship("invalid group epoch".into())
        })?;
        let now_ms = self.deps.clock.now_ms();
        let expires_at_ms = now_ms.saturating_add(DIRECT_ATTESTATION_TTL_MS);
        let mut members = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?;
        members.retain(|member| member.device_id != context.joiner_device_id);
        members.sort_by(|left, right| left.device_id.as_str().cmp(right.device_id.as_str()));

        let mut seeds = Vec::with_capacity(members.len());
        for member in members {
            let (transport_address_blob, address_observed_at_ms) = if member.device_id
                == context.sponsor_device_id
            {
                if context.sponsor_transport_address_blob.is_empty() {
                    continue;
                }
                (context.sponsor_transport_address_blob.clone(), now_ms)
            } else {
                let address = self
                    .deps
                    .peer_address_repo
                    .get(&member.device_id)
                    .await
                    .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?
                    .ok_or_else(|| {
                        MembershipConvergenceError::Relationship(
                            "current member address is unavailable".into(),
                        )
                    })?;
                (address.addr_blob, address.observed_at.timestamp_millis())
            };
            let security_updates = context
                .existing_member_updates
                .iter()
                .filter(|update| update.recipient() == &member.device_id)
                .map(|update| {
                    let digest = self
                        .deps
                        .hash
                        .hash_bytes(update.payload())
                        .map_err(|error| {
                            MembershipConvergenceError::Relationship(error.to_string())
                        })?;
                    Ok(RelayedSecurityUpdate {
                        previous_epoch,
                        next_epoch: context.group_epoch,
                        payload: update.payload().to_vec(),
                        digest: digest.bytes,
                    })
                })
                .collect::<Result<Vec<_>, MembershipConvergenceError>>()?;
            seeds.push(SponsorCandidateSeed {
                space_id: context.space_id.clone(),
                device_id: member.device_id,
                device_name_hint: member.device_name,
                identity_fingerprint_hint: member.identity_fingerprint,
                transport_address_blob,
                address_observed_at_ms,
                source_device_id: context.sponsor_device_id,
                security_updates,
                expires_at_ms,
            });
        }
        Ok(seeds)
    }

    pub(super) async fn prepare_sponsor_membership(
        &self,
        context: SponsorSeedBatchContext,
    ) -> Result<(), MembershipConvergenceError> {
        let seeds = self.build_sponsor_seed_batch_inner(&context).await?;
        let now_ms = self.deps.clock.now_ms();
        let expires_at_ms = now_ms.saturating_add(DIRECT_ATTESTATION_TTL_MS);

        let joiner_seeds = seeds
            .iter()
            .filter(|seed| seed.device_id != context.sponsor_device_id)
            .cloned()
            .collect::<Vec<_>>();
        let metrics = SponsorSeedDeliveryMetrics::from_seeds(&joiner_seeds);
        let batch_limit_bytes = MembershipEventBatch::max_transfer_bytes();
        let mut joiner_batches = Vec::new();
        for events in split_sponsor_seed_events(&context.space_id, &joiner_seeds)? {
            let batch_id_input = serde_json::to_vec(&(context.joiner_device_id.as_str(), &events))
                .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?;
            let batch_id = self
                .deps
                .hash
                .hash_bytes(&batch_id_input)
                .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?
                .bytes;
            let pending = PendingMembershipBatch::new(
                context.joiner_device_id.clone(),
                MembershipEventBatch {
                    space_id: context.space_id.clone(),
                    batch_id,
                    events,
                },
                now_ms,
            )?;
            joiner_batches.push(pending);
        }
        let joiner_batch_count = joiner_batches.len();
        let joiner_total_batch_bytes = joiner_batches
            .iter()
            .map(|pending| pending.batch().estimated_transfer_bytes())
            .sum::<usize>();
        let joiner_max_batch_bytes = joiner_batches
            .iter()
            .map(|pending| pending.batch().estimated_transfer_bytes())
            .max()
            .unwrap_or(0);
        for (batch_index, pending) in joiner_batches.iter().enumerate() {
            if let Err(error) = self.deps.outbox_repo.save(pending).await {
                warn!(
                    recipient_device_id = %context.joiner_device_id.as_str(),
                    batch_index,
                    batch_count = joiner_batch_count,
                    batch_event_count = pending.batch().events.len(),
                    batch_bytes = pending.batch().estimated_transfer_bytes(),
                    batch_limit_bytes,
                    error_kind = "membership_outbox_persist_failed",
                    retryable = false,
                    "membership delivery could not be queued for joiner"
                );
                return Err(error.into());
            }
            info!(
                recipient_device_id = %context.joiner_device_id.as_str(),
                batch_index,
                batch_count = joiner_batch_count,
                batch_event_count = pending.batch().events.len(),
                batch_bytes = pending.batch().estimated_transfer_bytes(),
                batch_limit_bytes,
                "membership delivery batch queued for joiner"
            );
        }

        let mut persisted_existing_member_batches = 0usize;
        let mut failed_existing_member_batches = 0usize;
        for recipient in seeds
            .iter()
            .filter(|seed| seed.device_id != context.sponsor_device_id)
        {
            let joiner_seed = SponsorCandidateSeed {
                space_id: context.space_id.clone(),
                device_id: context.joiner_device_id,
                device_name_hint: context.joiner_device_name.clone(),
                identity_fingerprint_hint: context.joiner_identity_fingerprint.clone(),
                transport_address_blob: context.joiner_transport_address_blob.clone(),
                address_observed_at_ms: now_ms,
                source_device_id: context.sponsor_device_id,
                security_updates: recipient.security_updates.clone(),
                expires_at_ms,
            };
            let event = MembershipEvent::SponsorSeed(joiner_seed);
            let batch_id_input = serde_json::to_vec(&(recipient.device_id.as_str(), &event))
                .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?;
            let batch_id = self
                .deps
                .hash
                .hash_bytes(&batch_id_input)
                .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?
                .bytes;
            let pending = PendingMembershipBatch::new(
                recipient.device_id,
                MembershipEventBatch {
                    space_id: context.space_id.clone(),
                    batch_id,
                    events: vec![event],
                },
                now_ms,
            )
            .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?;
            match self.deps.outbox_repo.save(&pending).await {
                Ok(()) => {
                    persisted_existing_member_batches =
                        persisted_existing_member_batches.saturating_add(1);
                }
                Err(_) => {
                    failed_existing_member_batches =
                        failed_existing_member_batches.saturating_add(1);
                }
            }
        }
        if failed_existing_member_batches > 0 {
            warn!(
                failed_existing_member_batches,
                recovery_via_membership_gossip = true,
                error_kind = "membership_outbox_persist_failed",
                retryable = false,
                "membership sponsor delivery could not be persisted"
            );
        }
        info!(
            recipient_device_id = %context.joiner_device_id.as_str(),
            existing_member_count = metrics.seed_count,
            delivery_batch_count = joiner_batch_count,
            delivery_total_batch_bytes = joiner_total_batch_bytes,
            delivery_max_batch_bytes = joiner_max_batch_bytes,
            delivery_batch_limit_bytes = batch_limit_bytes,
            device_metadata_bytes = metrics.device_metadata_bytes,
            address_bytes = metrics.address_bytes,
            security_update_count = metrics.security_update_count,
            security_update_bytes = metrics.security_update_bytes,
            largest_seed_bytes = metrics.largest_seed_bytes,
            largest_security_update_bytes = metrics.largest_security_update_bytes,
            persisted_existing_member_batches,
            failed_existing_member_batches,
            "membership delivery queued for joiner after pairing confirmation"
        );
        Ok(())
    }

    pub(super) fn notify_pending_delivery(&self) {
        self.wake.notify_one();
    }

    pub(super) async fn accept_verified_announcement(
        &self,
        announcement: DeviceAnnouncement,
    ) -> Result<CandidateMergeOutcome, MembershipConvergenceError> {
        let state = self.deps.security_updates.current_state().await?;
        if state.space_id != announcement.space_id || state.group_epoch != announcement.group_epoch
        {
            return Err(MembershipConvergenceError::VerificationRejected);
        }
        let digest = self
            .deps
            .hash
            .hash_bytes(&announcement.content_bytes())
            .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?;
        if digest.bytes != announcement.content_digest {
            return Err(MembershipConvergenceError::VerificationRejected);
        }
        let fingerprint = self
            .deps
            .fingerprint_factory
            .from_public_key(&announcement.transport_public_key)
            .map_err(|_| MembershipConvergenceError::VerificationRejected)?;
        if fingerprint != announcement.identity_fingerprint {
            return Err(MembershipConvergenceError::VerificationRejected);
        }
        let valid = self
            .deps
            .member_signatures
            .verify_current_member_payload(
                &announcement.device_id,
                &announcement.signing_payload(),
                &announcement.signature,
            )
            .await
            .map_err(|_| MembershipConvergenceError::VerificationRejected)?;
        if !valid {
            return Err(MembershipConvergenceError::VerificationRejected);
        }

        let now_ms = self.deps.clock.now_ms();
        let formal_member = self
            .deps
            .member_repo
            .get(&announcement.device_id)
            .await
            .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?;
        if let Some(member) = formal_member.as_ref() {
            if member.identity_fingerprint != announcement.identity_fingerprint {
                return Err(MembershipConvergenceError::VerificationRejected);
            }
        }
        let existing_announcement = self
            .deps
            .announcement_repo
            .get(&announcement.space_id, &announcement.device_id)
            .await?;
        if let Some(existing) = existing_announcement {
            if announcement.sequence < existing.sequence {
                return Ok(CandidateMergeOutcome::Stale);
            }
            if announcement.sequence == existing.sequence {
                if announcement.content_digest == existing.content_digest {
                    return Ok(CandidateMergeOutcome::Unchanged);
                }
                return Err(MembershipConvergenceError::VerificationRejected);
            }
        }

        let existing_candidate = self
            .deps
            .candidate_repo
            .get(&announcement.space_id, &announcement.device_id)
            .await?;
        if let Some(member) = formal_member {
            return match existing_candidate {
                Some(mut candidate) => {
                    if candidate.identity_fingerprint_hint() != &member.identity_fingerprint {
                        return Err(MembershipConvergenceError::VerificationRejected);
                    }
                    let (outcome, effect) = candidate.apply(
                        CandidateEvent::VerifiedAnnouncement(announcement.clone()),
                        now_ms,
                    )?;
                    candidate.apply(CandidateEvent::Admitted, now_ms)?;
                    if effect.persist {
                        self.deps.announcement_repo.save(&announcement).await?;
                    }
                    self.deps.candidate_repo.save(&candidate).await?;
                    Ok(outcome)
                }
                None => {
                    self.deps.announcement_repo.save(&announcement).await?;
                    Ok(CandidateMergeOutcome::Updated)
                }
            };
        }
        let (candidate, outcome, persist) = match existing_candidate {
            Some(mut candidate) => {
                let (outcome, effect) = candidate.apply(
                    CandidateEvent::VerifiedAnnouncement(announcement.clone()),
                    now_ms,
                )?;
                (candidate, outcome, effect.persist)
            }
            None => (
                SpaceMembershipCandidate::from_verified_announcement(announcement.clone(), now_ms)?,
                CandidateMergeOutcome::Updated,
                true,
            ),
        };
        if persist {
            self.deps.announcement_repo.save(&announcement).await?;
            self.deps.candidate_repo.save(&candidate).await?;
        }
        Ok(outcome)
    }

    pub(super) async fn request_for_digest(
        &self,
        digest: uc_core::membership::MembershipDigest,
    ) -> Result<MembershipGossipMessage, MembershipGossipEndpointError> {
        let state = self
            .deps
            .security_updates
            .current_state()
            .await
            .map_err(|_| MembershipGossipEndpointError::Persistence)?;
        if state.space_id != digest.space_id {
            warn!(
                digest_space_id = %digest.space_id.as_ref(),
                local_space_id = %state.space_id.as_ref(),
                error_kind = "membership_digest_space_mismatch",
                "membership digest rejected because the space does not match"
            );
            return Err(MembershipGossipEndpointError::Rejected);
        }
        let local = self
            .deps
            .announcement_repo
            .list(&digest.space_id)
            .await
            .map_err(|_| MembershipGossipEndpointError::Persistence)?;
        let mut requested = digest
            .announcements
            .into_iter()
            .filter(|remote| {
                local
                    .iter()
                    .find(|known| known.device_id == remote.device_id)
                    .map(|known| {
                        remote.sequence > known.sequence
                            || (remote.sequence == known.sequence
                                && remote.content_digest != known.content_digest)
                    })
                    .unwrap_or(true)
            })
            .map(|remote| remote.device_id)
            .collect::<Vec<_>>();
        requested.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        requested.dedup();
        Ok(MembershipGossipMessage::RequestMissing(
            uc_core::membership::MembershipRequestMissing {
                space_id: digest.space_id,
                announcement_devices: requested,
                security_updates_after_epoch: (digest.group_epoch > state.group_epoch)
                    .then_some(state.group_epoch),
            },
        ))
    }

    pub(super) async fn shared_device_page_for_request(
        &self,
        requester_device_id: &uc_core::ids::DeviceId,
        request: MembershipSharedDevicePageRequest,
    ) -> Result<MembershipGossipMessage, MembershipGossipEndpointError> {
        let state = self
            .deps
            .security_updates
            .current_state()
            .await
            .map_err(|_| MembershipGossipEndpointError::Persistence)?;
        if state.space_id != request.space_id {
            warn!(
                requester_device_id = %requester_device_id.as_str(),
                request_space_id = %request.space_id.as_ref(),
                local_space_id = %state.space_id.as_ref(),
                error_kind = "membership_shared_page_space_mismatch",
                "shared device page request rejected because the space does not match"
            );
            return Err(MembershipGossipEndpointError::Rejected);
        }
        if self
            .deps
            .member_repo
            .get(requester_device_id)
            .await
            .map_err(|_| MembershipGossipEndpointError::Persistence)?
            .is_none()
        {
            warn!(
                requester_device_id = %requester_device_id.as_str(),
                error_kind = "membership_shared_page_requester_not_member",
                "shared device page request rejected because the requester is not a member"
            );
            return Err(MembershipGossipEndpointError::Rejected);
        }

        let local_device_id = self.deps.device_identity.current_device_id();
        let mut members = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|_| MembershipGossipEndpointError::Persistence)?
            .into_iter()
            .filter(|member| {
                member.device_id != *requester_device_id && member.device_id != local_device_id
            })
            .filter(|member| {
                request
                    .after_device_id
                    .as_ref()
                    .map(|after| member.device_id.as_str() > after.as_str())
                    .unwrap_or(true)
            })
            .collect::<Vec<_>>();
        members.sort_by(|left, right| left.device_id.as_str().cmp(right.device_id.as_str()));

        let now_ms = self.deps.clock.now_ms();
        let expires_at_ms = now_ms.saturating_add(DIRECT_ATTESTATION_TTL_MS);
        let applied_updates = self
            .deps
            .applied_security_updates
            .list(&state.space_id)
            .await
            .map_err(|_| MembershipGossipEndpointError::Persistence)?;
        let mut seeds = Vec::new();
        let mut next_member_index = 0usize;
        while next_member_index < members.len() {
            let member = &members[next_member_index];
            let address = self
                .deps
                .peer_address_repo
                .get(&member.device_id)
                .await
                .map_err(|_| MembershipGossipEndpointError::Persistence)?
                .ok_or(MembershipGossipEndpointError::Persistence)?;
            let mut seed = SponsorCandidateSeed {
                space_id: state.space_id.clone(),
                device_id: member.device_id,
                device_name_hint: member.device_name.clone(),
                identity_fingerprint_hint: member.identity_fingerprint.clone(),
                transport_address_blob: address.addr_blob,
                address_observed_at_ms: address.observed_at.timestamp_millis(),
                source_device_id: local_device_id,
                security_updates: applied_updates.clone(),
                expires_at_ms,
            };
            if seed.validate_transfer_bounds().is_err() {
                seed.security_updates.clear();
                if seed.validate_transfer_bounds().is_err() {
                    return Err(MembershipGossipEndpointError::Rejected);
                }
            }
            seeds.push(seed);
            let tentative = MembershipSharedDevicePage {
                space_id: state.space_id.clone(),
                seeds: seeds.clone(),
                next_after_device_id: None,
            };
            match tentative.validate_transfer_bounds() {
                Ok(()) => next_member_index = next_member_index.saturating_add(1),
                Err(
                    MembershipGossipBoundsError::MessageTooLarge
                    | MembershipGossipBoundsError::TooManyDevices,
                ) if seeds.len() > 1 => {
                    seeds.pop();
                    break;
                }
                Err(
                    MembershipGossipBoundsError::MessageTooLarge
                    | MembershipGossipBoundsError::TooManyDevices,
                ) => {
                    if let Some(last) = seeds.last_mut() {
                        last.security_updates.clear();
                        let trimmed_page = MembershipSharedDevicePage {
                            space_id: state.space_id.clone(),
                            seeds: seeds.clone(),
                            next_after_device_id: None,
                        };
                        if trimmed_page.validate_transfer_bounds().is_ok() {
                            next_member_index = next_member_index.saturating_add(1);
                            continue;
                        }
                    }
                    return Err(MembershipGossipEndpointError::Rejected);
                }
                Err(_) => return Err(MembershipGossipEndpointError::Rejected),
            }
        }
        let next_after_device_id = (next_member_index < members.len())
            .then(|| seeds.last().map(|seed| seed.device_id))
            .flatten();
        let page = MembershipSharedDevicePage {
            space_id: state.space_id,
            seeds,
            next_after_device_id,
        };
        page.validate_transfer_bounds()
            .map_err(|_| MembershipGossipEndpointError::Rejected)?;
        Ok(MembershipGossipMessage::SharedDevicePage(page))
    }

    pub(super) async fn events_for_request(
        &self,
        request: uc_core::membership::MembershipRequestMissing,
    ) -> Result<MembershipGossipMessage, MembershipGossipEndpointError> {
        let state = self
            .deps
            .security_updates
            .current_state()
            .await
            .map_err(|_| MembershipGossipEndpointError::Persistence)?;
        if state.space_id != request.space_id {
            warn!(
                request_space_id = %request.space_id.as_ref(),
                local_space_id = %state.space_id.as_ref(),
                error_kind = "membership_event_request_space_mismatch",
                "membership event request rejected because the space does not match"
            );
            return Err(MembershipGossipEndpointError::Rejected);
        }
        let mut events = Vec::new();
        for device_id in request.announcement_devices {
            if let Some(announcement) = self
                .deps
                .announcement_repo
                .get(&request.space_id, &device_id)
                .await
                .map_err(|_| MembershipGossipEndpointError::Persistence)?
            {
                events.push(MembershipEvent::Announcement(announcement));
            }
        }
        if let Some(epoch) = request.security_updates_after_epoch {
            let mut applied = self
                .deps
                .applied_security_updates
                .list(&request.space_id)
                .await
                .map_err(|_| MembershipGossipEndpointError::Persistence)?;
            let mut from_candidates = self
                .deps
                .candidate_repo
                .list(&request.space_id)
                .await
                .map_err(|_| MembershipGossipEndpointError::Persistence)?
                .into_iter()
                .flat_map(|candidate| candidate.security_updates().to_vec())
                .collect::<Vec<_>>();
            applied.append(&mut from_candidates);
            let mut updates = applied
                .into_iter()
                .filter(|update| update.previous_epoch >= epoch)
                .collect::<Vec<_>>();
            updates.sort_by_key(|update| update.previous_epoch);
            updates.dedup_by_key(|update| update.digest);
            events.extend(updates.into_iter().map(MembershipEvent::SecurityUpdate));
        }
        let batch_input = serde_json::to_vec(&(&request.space_id, &events))
            .map_err(|_| MembershipGossipEndpointError::Persistence)?;
        let batch_id = self
            .deps
            .hash
            .hash_bytes(&batch_input)
            .map_err(|_| MembershipGossipEndpointError::Persistence)?
            .bytes;
        let batch = MembershipEventBatch {
            space_id: request.space_id,
            batch_id,
            events,
        };
        batch
            .validate_transfer_bounds()
            .map_err(|_| MembershipGossipEndpointError::Rejected)?;
        Ok(MembershipGossipMessage::EventBatch(batch))
    }

    pub(super) async fn deliver_pending(
        &self,
        space_id: &SpaceId,
        now_ms: i64,
    ) -> Result<usize, MembershipConvergenceError> {
        let pending = self.deps.outbox_repo.list_pending(space_id).await?;
        let mut delivered = 0usize;
        for mut item in pending
            .into_iter()
            .filter(|item| item.next_attempt_at_ms() <= now_ms)
        {
            let batch_event_count = item.batch().events.len();
            let batch_bytes = item.batch().estimated_transfer_bytes();
            let batch_limit_bytes = MembershipEventBatch::max_transfer_bytes();
            let delivery_attempt = item.attempt_count().saturating_add(1);
            info!(
                recipient_device_id = %item.recipient_device_id().as_str(),
                batch_event_count,
                batch_bytes,
                batch_limit_bytes,
                delivery_attempt,
                "membership delivery attempt started"
            );
            let response = self
                .deps
                .transport
                .exchange(
                    item.recipient_device_id(),
                    MembershipGossipMessage::EventBatch(item.batch().clone()),
                )
                .await;
            let acknowledged = matches!(
                response,
                Ok(MembershipGossipMessage::Ack(ref ack))
                    if ack.space_id == item.batch().space_id
                        && ack.batch_id == item.batch().batch_id
            );
            if acknowledged {
                let removed = self
                    .deps
                    .outbox_repo
                    .remove(
                        &item.batch().space_id,
                        item.recipient_device_id(),
                        &item.batch().batch_id,
                    )
                    .await?;
                if removed {
                    delivered = delivered.saturating_add(1);
                }
                info!(
                    recipient_device_id = %item.recipient_device_id().as_str(),
                    batch_event_count,
                    batch_bytes,
                    batch_limit_bytes,
                    delivery_attempt,
                    outbox_removed = removed,
                    "membership delivery acknowledged by recipient"
                );
            } else {
                let next_attempt_at_ms = next_membership_retry_at(&item, now_ms);
                let error_kind = match &response {
                    Ok(MembershipGossipMessage::Ack(_)) => "membership_ack_mismatch",
                    Ok(_) => "unexpected_membership_response",
                    Err(
                        uc_core::membership::MembershipGossipTransportError::VersionIncompatible,
                    ) => "version_incompatible",
                    Err(uc_core::membership::MembershipGossipTransportError::Offline) => "offline",
                    Err(uc_core::membership::MembershipGossipTransportError::Transport) => {
                        "transport"
                    }
                    Err(uc_core::membership::MembershipGossipTransportError::Rejected) => {
                        "rejected"
                    }
                };
                if matches!(
                    response,
                    Err(uc_core::membership::MembershipGossipTransportError::VersionIncompatible)
                ) {
                    item.mark_retry_after(
                        CandidateFailure::VersionIncompatible,
                        next_attempt_at_ms,
                        now_ms,
                    );
                } else {
                    item.mark_retry(next_attempt_at_ms, now_ms);
                }
                self.deps.outbox_repo.save(&item).await?;
                warn!(
                    recipient_device_id = %item.recipient_device_id().as_str(),
                    batch_event_count,
                    batch_bytes,
                    batch_limit_bytes,
                    delivery_attempt,
                    retry_count = item.attempt_count(),
                    next_attempt_at_ms,
                    error_kind,
                    retryable = true,
                    "membership delivery deferred for retry"
                );
            }
        }
        Ok(delivered)
    }

    pub(super) async fn synchronize_member(
        &self,
        recipient: &uc_core::ids::DeviceId,
    ) -> Result<(), MembershipConvergenceError> {
        self.refresh_local_announcement().await?;
        let state = self.deps.security_updates.current_state().await?;
        let mut announcements = self
            .deps
            .announcement_repo
            .list(&state.space_id)
            .await?
            .into_iter()
            .map(
                |announcement| uc_core::membership::MembershipAnnouncementVersion {
                    device_id: announcement.device_id,
                    sequence: announcement.sequence,
                    content_digest: announcement.content_digest,
                },
            )
            .collect::<Vec<_>>();
        announcements.sort_by(|left, right| left.device_id.as_str().cmp(right.device_id.as_str()));
        let mut known_updates = self
            .deps
            .applied_security_updates
            .list(&state.space_id)
            .await?
            .into_iter()
            .collect::<Vec<_>>();
        let mut candidate_updates = self
            .deps
            .candidate_repo
            .list(&state.space_id)
            .await?
            .into_iter()
            .flat_map(|candidate| candidate.security_updates().to_vec())
            .collect::<Vec<_>>();
        known_updates.append(&mut candidate_updates);
        let group_update_head_digest = known_updates
            .into_iter()
            .max_by_key(|update| update.next_epoch)
            .map(|update| update.digest);
        let response = self
            .deps
            .transport
            .exchange(
                recipient,
                MembershipGossipMessage::Digest(uc_core::membership::MembershipDigest {
                    space_id: state.space_id.clone(),
                    group_epoch: state.group_epoch,
                    group_update_head_digest,
                    announcements,
                }),
            )
            .await
            .map_err(map_gossip_transport_error)?;
        let MembershipGossipMessage::RequestMissing(request) = response else {
            return Err(MembershipConvergenceError::VerificationRejected);
        };
        if request.space_id != state.space_id {
            return Err(MembershipConvergenceError::VerificationRejected);
        }
        let events = self
            .events_for_request(request)
            .await
            .map_err(|error| match error {
                MembershipGossipEndpointError::Rejected => {
                    MembershipConvergenceError::VerificationRejected
                }
                MembershipGossipEndpointError::Persistence => {
                    MembershipConvergenceError::Relationship(
                        "membership event batch could not be built".into(),
                    )
                }
            })?;
        let MembershipGossipMessage::EventBatch(batch) = events else {
            return Err(MembershipConvergenceError::VerificationRejected);
        };
        let response = self
            .deps
            .transport
            .exchange(
                recipient,
                MembershipGossipMessage::EventBatch(batch.clone()),
            )
            .await
            .map_err(map_gossip_transport_error)?;
        match response {
            MembershipGossipMessage::Ack(ack)
                if ack.space_id == batch.space_id && ack.batch_id == batch.batch_id =>
            {
                Ok(())
            }
            _ => Err(MembershipConvergenceError::VerificationRejected),
        }
    }

    /// Pull missing security updates from `provider`.
    ///
    /// The local device asks the provider for every update after the local
    /// group epoch and applies whatever arrives, so a `WaitingForUpdate`
    /// candidate can resume without waiting for the provider to push on its
    /// own schedule. Returns the number of updates applied.
    pub(super) async fn pull_security_updates(
        &self,
        provider: &uc_core::ids::DeviceId,
    ) -> Result<usize, MembershipConvergenceError> {
        let state = self.deps.security_updates.current_state().await?;
        let request = uc_core::membership::MembershipRequestMissing {
            space_id: state.space_id.clone(),
            announcement_devices: Vec::new(),
            security_updates_after_epoch: Some(state.group_epoch),
        };
        let response = self
            .deps
            .transport
            .exchange(provider, MembershipGossipMessage::RequestMissing(request))
            .await
            .map_err(map_gossip_transport_error)?;
        let MembershipGossipMessage::EventBatch(batch) = response else {
            return Err(MembershipConvergenceError::VerificationRejected);
        };
        if batch.space_id != state.space_id {
            return Err(MembershipConvergenceError::VerificationRejected);
        }
        let mut applied = 0usize;
        for event in &batch.events {
            if let MembershipEvent::SecurityUpdate(update) = event {
                self.apply_relayed_security_updates(&batch.space_id, std::slice::from_ref(update))
                    .await?;
                applied = applied.saturating_add(1);
            }
        }
        Ok(applied)
    }
}

#[async_trait]
impl PairingMembershipConvergencePort for MembershipConvergence {
    async fn prepare_sponsor_membership(
        &self,
        context: SponsorSeedBatchContext,
    ) -> Result<(), MembershipConvergenceError> {
        MembershipConvergence::prepare_sponsor_membership(self, context).await
    }

    fn notify_pending_delivery(&self) {
        MembershipConvergence::notify_pending_delivery(self);
    }
}

#[async_trait]
impl MembershipGossipEndpointPort for MembershipConvergence {
    async fn handle_message(
        &self,
        source_device_id: &uc_core::ids::DeviceId,
        message: MembershipGossipMessage,
    ) -> Result<MembershipGossipMessage, MembershipGossipEndpointError> {
        message
            .validate_transfer_bounds()
            .map_err(|_| MembershipGossipEndpointError::Rejected)?;
        match message {
            MembershipGossipMessage::EventBatch(batch) => {
                let state = self
                    .deps
                    .security_updates
                    .current_state()
                    .await
                    .map_err(|_| MembershipGossipEndpointError::Persistence)?;
                if state.space_id != batch.space_id {
                    return Err(MembershipGossipEndpointError::Rejected);
                }
                for event in &batch.events {
                    match event {
                        MembershipEvent::SponsorSeed(seed)
                            if &seed.source_device_id == source_device_id
                                && seed.space_id == batch.space_id =>
                        {
                            self.accept_sponsor_seed(seed.clone())
                                .await
                                .map_err(|error| match error {
                                    MembershipConvergenceError::InvalidCandidate(_)
                                    | MembershipConvergenceError::VerificationRejected => {
                                        MembershipGossipEndpointError::Rejected
                                    }
                                    _ => MembershipGossipEndpointError::Persistence,
                                })?;
                        }
                        MembershipEvent::SecurityUpdate(update) => {
                            let applied = self
                                .apply_relayed_security_updates(
                                    &batch.space_id,
                                    std::slice::from_ref(update),
                                )
                                .await
                                .map_err(|error| match error {
                                    MembershipConvergenceError::VerificationRejected
                                    | MembershipConvergenceError::WaitingForUpdate => {
                                        warn!(
                                            source_device_id = %source_device_id.as_str(),
                                            update_next_epoch = update.next_epoch,
                                            error_kind = "membership_security_update_rejected",
                                            error_reason = %error,
                                            "relayed security update rejected"
                                        );
                                        MembershipGossipEndpointError::Rejected
                                    }
                                    _ => MembershipGossipEndpointError::Persistence,
                                })?;
                            if applied == update.next_epoch {
                                info!(
                                    source_device_id = %source_device_id.as_str(),
                                    applied_group_epoch = applied,
                                    "relayed security update applied"
                                );
                            }
                        }
                        MembershipEvent::Announcement(announcement) => {
                            self.accept_verified_announcement(announcement.clone())
                                .await
                                .map_err(|error| match error {
                                    MembershipConvergenceError::VerificationRejected
                                    | MembershipConvergenceError::InvalidCandidate(_) => {
                                        MembershipGossipEndpointError::Rejected
                                    }
                                    _ => MembershipGossipEndpointError::Persistence,
                                })?;
                        }
                        MembershipEvent::SponsorSeed(_) => {
                            return Err(MembershipGossipEndpointError::Rejected);
                        }
                    }
                }
                Ok(MembershipGossipMessage::Ack(
                    uc_core::membership::MembershipAck {
                        space_id: batch.space_id,
                        batch_id: batch.batch_id,
                    },
                ))
            }
            MembershipGossipMessage::Digest(digest) => self.request_for_digest(digest).await,
            MembershipGossipMessage::RequestMissing(request) => {
                self.events_for_request(request).await
            }
            MembershipGossipMessage::RequestSharedDevicePage(request) => {
                self.shared_device_page_for_request(source_device_id, request)
                    .await
            }
            MembershipGossipMessage::SharedDevicePage(_) | MembershipGossipMessage::Ack(_) => {
                Err(MembershipGossipEndpointError::Rejected)
            }
        }
    }
}

fn map_gossip_transport_error(
    error: uc_core::membership::MembershipGossipTransportError,
) -> MembershipConvergenceError {
    match error {
        uc_core::membership::MembershipGossipTransportError::Offline
        | uc_core::membership::MembershipGossipTransportError::Transport => {
            MembershipConvergenceError::PeerUnavailable
        }
        uc_core::membership::MembershipGossipTransportError::Rejected
        | uc_core::membership::MembershipGossipTransportError::VersionIncompatible => {
            MembershipConvergenceError::VerificationRejected
        }
    }
}

fn next_membership_retry_at(pending: &PendingMembershipBatch, now_ms: i64) -> i64 {
    let multiplier = 1i64 << pending.attempt_count().min(4);
    let base = INITIAL_RETRY_DELAY_MS
        .saturating_mul(multiplier)
        .min(MAX_RETRY_DELAY_MS);
    let jitter_window = (base / 5).max(1);
    let jitter_seed =
        u16::from_be_bytes([pending.batch().batch_id[0], pending.batch().batch_id[1]]);
    let jitter = i64::from(jitter_seed) % jitter_window;
    now_ms.saturating_add(base).saturating_add(jitter)
}
