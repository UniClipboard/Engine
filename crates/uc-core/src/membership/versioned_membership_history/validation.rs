//! 历史授权校验与成员快照重建。

use super::{
    HistoricalMembershipSignatureError, HistoricalMembershipSignatureVerifier, MemberInstanceId,
    MembershipActivationBaselineV2, MembershipCredential, MembershipEventId, MembershipEventV2,
    MembershipHistorySnapshot, MembershipHistoryV2Error, MembershipOperationV2,
    VersionedMembershipHistory,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

impl VersionedMembershipHistory {
    pub fn expected_resulting_members_digest(
        &self,
        parent_event_id: Option<MembershipEventId>,
        operation: &MembershipOperationV2,
    ) -> Result<[u8; 32], MembershipHistoryV2Error> {
        let mut members = match parent_event_id {
            Some(parent) => self
                .snapshots
                .get(&parent)
                .ok_or(MembershipHistoryV2Error::UnknownParent)?
                .members
                .clone(),
            None if self.known_head.is_none() => BTreeSet::new(),
            None => return Err(MembershipHistoryV2Error::InvalidGenesis),
        };
        apply_membership_operation(&mut members, operation)?;
        Ok(members_digest(&members))
    }

    pub(super) fn validate_parent_and_author(
        &self,
        event: &MembershipEventV2,
    ) -> Result<(MembershipHistorySnapshot, MembershipCredential), MembershipHistoryV2Error> {
        match event.parent_event_id {
            None => {
                if self.known_head.is_some() || event.parent_depth != 0 {
                    return Err(MembershipHistoryV2Error::InvalidGenesis);
                }
                let MembershipOperationV2::AddDevice { admission } = &event.operation else {
                    return Err(MembershipHistoryV2Error::InvalidGenesis);
                };
                if event.author_member_instance_id != admission.facts.member_instance
                    || event.author_credential_id != admission.membership_credential.credential_id
                    || event.author_signature_algorithm_version
                        != admission.membership_credential.signature_algorithm_version
                {
                    return Err(MembershipHistoryV2Error::InvalidGenesis);
                }
                Ok((
                    MembershipHistorySnapshot {
                        members: BTreeSet::new(),
                        active_members: BTreeSet::new(),
                    },
                    admission.membership_credential.clone(),
                ))
            }
            Some(parent_id) => {
                let parent_depth = self
                    .depth(parent_id)
                    .ok_or(MembershipHistoryV2Error::UnknownParent)?;
                if event.parent_depth != parent_depth.saturating_add(1) {
                    return Err(MembershipHistoryV2Error::InvalidParentDepth);
                }
                let parent_snapshot = self
                    .snapshots
                    .get(&parent_id)
                    .ok_or(MembershipHistoryV2Error::UnknownParent)?;
                if !parent_snapshot
                    .members
                    .contains(&event.author_member_instance_id)
                {
                    return Err(MembershipHistoryV2Error::UnauthorizedAuthor);
                }
                if !parent_snapshot
                    .active_members
                    .contains(&event.author_member_instance_id)
                {
                    return Err(MembershipHistoryV2Error::AwaitingActivationReceipt);
                }
                let credential = self
                    .credentials
                    .get(&event.author_member_instance_id)
                    .ok_or(MembershipHistoryV2Error::InvalidCredential)?;
                if credential.credential_id != event.author_credential_id
                    || credential.signature_algorithm_version
                        != event.author_signature_algorithm_version
                {
                    return Err(MembershipHistoryV2Error::InvalidCredential);
                }
                Ok((parent_snapshot.clone(), credential.clone()))
            }
        }
    }

    pub(super) fn verify_signature(
        &self,
        event: &MembershipEventV2,
        credential: &MembershipCredential,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<(), MembershipHistoryV2Error> {
        credential.validate()?;
        verify_signature(
            verifier,
            credential,
            &event.signing_payload(),
            &event.signature,
        )
    }

    pub(super) fn validate_operation(
        &self,
        event: &MembershipEventV2,
        parent_snapshot: &MembershipHistorySnapshot,
    ) -> Result<(), MembershipHistoryV2Error> {
        match &event.operation {
            MembershipOperationV2::AddDevice { admission } => {
                admission.membership_credential.validate()?;
                if admission.facts.member_instance
                    != admission
                        .membership_credential
                        .member_instance_id(&admission.facts.device_id)
                    || parent_snapshot
                        .members
                        .contains(&admission.facts.member_instance)
                {
                    return Err(MembershipHistoryV2Error::InvalidCredential);
                }
                Ok(())
            }
            MembershipOperationV2::RemoveDevice { member } => {
                if *member == event.author_member_instance_id
                    || !parent_snapshot.members.contains(member)
                {
                    return Err(MembershipHistoryV2Error::InvalidOperation);
                }
                Ok(())
            }
        }
    }

    pub(super) fn rebuild_snapshots(&mut self) -> Result<(), MembershipHistoryV2Error> {
        let mut ordered_events = self.events.values().cloned().collect::<Vec<_>>();
        ordered_events.sort_by_key(|event| (event.parent_depth, event.event_id()));
        let mut snapshots = BTreeMap::new();
        if let Some(baseline) = &self.activation_baseline {
            let (head, _) = baseline.head_and_depth();
            let members = baseline.current_member_ids();
            snapshots.insert(
                head,
                MembershipHistorySnapshot {
                    members: members.clone(),
                    active_members: members,
                },
            );
        }
        for event in ordered_events {
            let mut snapshot = match event.parent_event_id {
                Some(parent) => snapshots
                    .get(&parent)
                    .cloned()
                    .ok_or(MembershipHistoryV2Error::UnknownParent)?,
                None => MembershipHistorySnapshot {
                    members: BTreeSet::new(),
                    active_members: BTreeSet::new(),
                },
            };
            match &event.operation {
                MembershipOperationV2::AddDevice { admission } => {
                    snapshot.members.insert(admission.facts.member_instance);
                    if event.parent_event_id.is_none()
                        || self.activation_receipts.contains_key(&event.event_id())
                    {
                        snapshot
                            .active_members
                            .insert(admission.facts.member_instance);
                    }
                }
                MembershipOperationV2::RemoveDevice { member } => {
                    snapshot.members.remove(member);
                    snapshot.active_members.remove(member);
                }
            }
            snapshots.insert(event.event_id(), snapshot);
        }
        self.snapshots = snapshots;
        Ok(())
    }

    pub(super) fn verify_activation_baseline_signatures(
        &self,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<(), MembershipHistoryV2Error> {
        let Some(MembershipActivationBaselineV2::Established {
            current_members, ..
        }) = &self.activation_baseline
        else {
            return Ok(());
        };
        for (facts, credential) in current_members {
            verify_signature(
                verifier,
                credential,
                &facts.signing_payload(),
                &facts.identity_signature,
            )?;
        }
        Ok(())
    }
}
pub(super) fn verify_signature(
    verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    credential: &MembershipCredential,
    payload: &[u8],
    signature: &[u8],
) -> Result<(), MembershipHistoryV2Error> {
    match verifier.verify(
        credential.signature_algorithm_version,
        &credential.public_key,
        payload,
        signature,
    ) {
        Ok(true) => Ok(()),
        Ok(false) | Err(HistoricalMembershipSignatureError::VerificationFailed) => {
            Err(MembershipHistoryV2Error::InvalidSignature)
        }
        Err(HistoricalMembershipSignatureError::UnsupportedAlgorithm) => {
            Err(MembershipHistoryV2Error::UpgradeRequired)
        }
    }
}

pub(super) fn apply_membership_operation(
    members: &mut BTreeSet<MemberInstanceId>,
    operation: &MembershipOperationV2,
) -> Result<(), MembershipHistoryV2Error> {
    match operation {
        MembershipOperationV2::AddDevice { admission } => {
            if !members.insert(admission.facts.member_instance) {
                return Err(MembershipHistoryV2Error::InvalidOperation);
            }
        }
        MembershipOperationV2::RemoveDevice { member } => {
            if !members.remove(member) {
                return Err(MembershipHistoryV2Error::InvalidOperation);
            }
        }
    }
    Ok(())
}

pub(super) fn members_digest(members: &BTreeSet<MemberInstanceId>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/membership-members/v2\0");
    hasher.update((members.len() as u64).to_be_bytes());
    for member in members {
        hasher.update(member.as_bytes());
    }
    hasher.finalize().into()
}
