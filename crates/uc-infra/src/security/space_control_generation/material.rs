use std::collections::{BTreeMap, BTreeSet};

use chrono::{TimeZone as _, Utc};
use uc_application::deps::{
    AdmissionSpaceTransitionPreparationV2, LoadedMembershipLedger, PeerHistorySyncState,
    PeerReconciliationRecord,
};
use uc_core::membership::{
    AdmissionContentKeyCatalogV1, ContentKeyId, GroupEpoch, MembershipHistoryRelationship,
    ProtectionGroupId, SpaceKeyMaterial, SpaceKeyState, SpaceMember,
};
use uc_core::ports::PeerAddressRecord;
use uc_core::trusted_peer::TrustedPeer;

use super::{inconsistent, ActiveRuntimeManifestV3, SpaceControlGenerationError};
use crate::space::import_admission_content_key_catalog;

pub(super) struct PreparedAdmissionControl {
    space_id: uc_core::ids::SpaceId,
    security_material: SpaceKeyMaterial,
    membership_history: Vec<u8>,
    local_device_id: uc_core::ids::DeviceId,
    local_member_instance: uc_core::membership::MemberInstanceId,
    peer_reconciliation: BTreeMap<uc_core::ids::DeviceId, PeerReconciliationRecord>,
    members: Vec<SpaceMember>,
    trusted_peers: Vec<TrustedPeer>,
    peer_addresses: Vec<PeerAddressRecord>,
    credentials: Vec<u8>,
}

impl PreparedAdmissionControl {
    pub(super) fn try_from_input(
        input: &AdmissionSpaceTransitionPreparationV2,
        manifest: &ActiveRuntimeManifestV3,
    ) -> Result<Self, SpaceControlGenerationError> {
        input
            .target_security_commitment
            .validate()
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        let space_id = uc_core::ids::SpaceId::from_str(&input.target_space_id);
        if manifest.layout().space_id() != &space_id
            || input.target_security_commitment.attempt_id != *input.attempt_id.as_bytes()
            || input.target_security_commitment.lineage_id != input.target_space_id
            || input.target_membership_history.is_empty()
            || input.target_security_state.is_empty()
            || input.target_admission_credentials.is_empty()
        {
            return Err(inconsistent(anyhow::anyhow!(
                "admission control material is incomplete"
            )));
        }
        let catalog = AdmissionContentKeyCatalogV1::decode(&input.target_key_catalog)
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        if catalog.target_epoch != input.target_security_commitment.target_epoch
            || catalog.digest() != input.target_security_commitment.key_catalog_digest
        {
            return Err(inconsistent(anyhow::anyhow!(
                "admission content catalog does not match commitment"
            )));
        }
        let current_content_key_id = ContentKeyId::from_string(&catalog.current_content_key_id)
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        let protection_group_id = ProtectionGroupId::from_string(&input.target_protection_group_id)
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        let state = SpaceKeyState::ready_for_admission(
            space_id.clone(),
            GroupEpoch::new(catalog.target_epoch),
            current_content_key_id,
            protection_group_id,
        )
        .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        let key_catalog = import_admission_content_key_catalog(&catalog)
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        let mut security_material =
            SpaceKeyMaterial::new(state, input.target_security_state.clone(), key_catalog, 0);
        validate_updates(input)?;
        security_material.add_pending_group_updates(input.relayed_group_updates.clone(), 0);

        let timestamp = Utc
            .timestamp_millis_opt(0)
            .single()
            .ok_or_else(|| inconsistent(anyhow::anyhow!("control timestamp is invalid")))?;
        let mut members = Vec::with_capacity(input.target_relationships.len());
        let mut trusted_peers = Vec::new();
        let mut peer_addresses = Vec::new();
        let mut local_member_instance = None;
        let mut peer_reconciliation = BTreeMap::new();
        for facts in &input.target_relationships {
            members.push(SpaceMember {
                device_id: facts.device_id.clone(),
                device_name: facts.device_name.clone(),
                identity_fingerprint: facts.identity_fingerprint.clone(),
                joined_at: timestamp,
                sync_preferences: Default::default(),
            });
            if facts.device_id == input.local_device_id {
                if local_member_instance
                    .replace(facts.member_instance)
                    .is_some()
                {
                    return Err(inconsistent(anyhow::anyhow!(
                        "local relationship is duplicated"
                    )));
                }
            } else {
                trusted_peers.push(TrustedPeer {
                    local_device_id: input.local_device_id.clone(),
                    peer_device_id: facts.device_id.clone(),
                    peer_fingerprint: facts.identity_fingerprint.clone(),
                    trusted_at: timestamp,
                });
                peer_addresses.push(PeerAddressRecord {
                    device_id: facts.device_id.clone(),
                    addr_blob: facts.transport_address_blob.clone(),
                    observed_at: timestamp,
                });
                peer_reconciliation.insert(
                    facts.device_id.clone(),
                    PeerReconciliationRecord {
                        peer_device_id: facts.device_id.clone(),
                        relationship: MembershipHistoryRelationship::Consistent,
                        confirmed_position: None,
                        sync_state: PeerHistorySyncState::default(),
                        restricted_delivery: Vec::new(),
                        updated_at_ms: 0,
                    },
                );
            }
        }
        let local_member_instance = local_member_instance
            .ok_or_else(|| inconsistent(anyhow::anyhow!("local relationship is missing")))?;
        members.sort_by(|left, right| left.device_id.cmp(&right.device_id));
        trusted_peers.sort_by(|left, right| left.peer_device_id.cmp(&right.peer_device_id));
        peer_addresses.sort_by(|left, right| left.device_id.cmp(&right.device_id));
        Ok(Self {
            space_id,
            security_material,
            membership_history: input.target_membership_history.clone(),
            local_device_id: input.local_device_id.clone(),
            local_member_instance,
            peer_reconciliation,
            members,
            trusted_peers,
            peer_addresses,
            credentials: input.target_admission_credentials.clone(),
        })
    }

    pub(super) fn ledger(
        &self,
        current_revision: u64,
    ) -> Result<LoadedMembershipLedger, SpaceControlGenerationError> {
        Ok(LoadedMembershipLedger {
            revision: current_revision
                .checked_add(1)
                .ok_or_else(|| inconsistent(anyhow::anyhow!("ledger revision overflow")))?,
            lineage_id: Some(self.space_id.as_ref().to_owned()),
            membership_history: Some(self.membership_history.clone()),
            local_device_id: Some(self.local_device_id.clone()),
            local_member_instance: Some(self.local_member_instance),
            local_join_active: true,
            peer_reconciliation: self.peer_reconciliation.clone(),
            history_sync_cursor: None,
            inbound_transfers: Default::default(),
            completed_inbound_transfers: Default::default(),
            pending_effects: Default::default(),
            membership_conflicts: Default::default(),
            membership_branch_transitions: Default::default(),
            consumed_membership_recovery_nonces: Default::default(),
            membership_branch_recovery_sessions: Default::default(),
        })
    }

    pub(super) fn space_id(&self) -> &uc_core::ids::SpaceId {
        &self.space_id
    }

    pub(super) fn security_material(&self) -> &SpaceKeyMaterial {
        &self.security_material
    }

    pub(super) fn members(&self) -> &[SpaceMember] {
        &self.members
    }

    pub(super) fn trusted_peers(&self) -> &[TrustedPeer] {
        &self.trusted_peers
    }

    pub(super) fn peer_addresses(&self) -> &[PeerAddressRecord] {
        &self.peer_addresses
    }

    pub(super) fn credentials(&self) -> &[u8] {
        &self.credentials
    }
}

fn validate_updates(
    input: &AdmissionSpaceTransitionPreparationV2,
) -> Result<(), SpaceControlGenerationError> {
    let mut relationship_devices = BTreeSet::new();
    for facts in &input.target_relationships {
        if !relationship_devices.insert(facts.device_id.clone()) {
            return Err(inconsistent(anyhow::anyhow!(
                "control relationship is duplicated"
            )));
        }
    }
    let mut update_ids = BTreeSet::new();
    let mut recipients = BTreeSet::new();
    if input.relayed_group_updates.iter().any(|update| {
        update.recipient() == &input.local_device_id
            || !relationship_devices.contains(update.recipient())
            || update.update_id().is_empty()
            || update.payload().is_empty()
            || !update_ids.insert(update.update_id())
            || !recipients.insert(update.recipient().clone())
    }) {
        return Err(inconsistent(anyhow::anyhow!(
            "control group updates are inconsistent"
        )));
    }
    Ok(())
}
