use serde::{Deserialize, Serialize};

use super::AdmissionAttemptId;

pub const CROSS_SPACE_TRANSITION_FORMAT_V2: u16 = 2;
pub const FRESH_SPACE_TRANSITION_FORMAT_V1: u16 = 1;
pub const SAME_SPACE_TRANSITION_FORMAT_V1: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreshSpaceTransitionPhaseV1 {
    TargetStaged,
    ActivationStarted,
    TargetPromoted,
    CleanupPending,
}

impl FreshSpaceTransitionPhaseV1 {
    pub const fn rank(self) -> u8 {
        match self {
            Self::TargetStaged => 0,
            Self::ActivationStarted => 1,
            Self::TargetPromoted => 2,
            Self::CleanupPending => 3,
        }
    }

    const fn successor(self) -> Option<Self> {
        match self {
            Self::TargetStaged => Some(Self::ActivationStarted),
            Self::ActivationStarted => Some(Self::TargetPromoted),
            Self::TargetPromoted => Some(Self::CleanupPending),
            Self::CleanupPending => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshSpaceTransitionV1 {
    pub transition_format_version: u16,
    pub attempt_id: AdmissionAttemptId,
    pub target_space_id: String,
    pub target_generation: [u8; 16],
    pub target_keyslot_ref: Vec<u8>,
    pub target_workspace_ref: Vec<u8>,
    pub phase: FreshSpaceTransitionPhaseV1,
}

impl std::fmt::Debug for FreshSpaceTransitionV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FreshSpaceTransitionV1")
            .field("attempt_id", &self.attempt_id)
            .field("phase", &self.phase)
            .finish()
    }
}

impl FreshSpaceTransitionV1 {
    pub fn validate(&self) -> bool {
        self.transition_format_version == FRESH_SPACE_TRANSITION_FORMAT_V1
            && !self.target_space_id.is_empty()
            && !self.target_keyslot_ref.is_empty()
            && !self.target_workspace_ref.is_empty()
    }

    pub fn can_advance_to(&self, next: &Self) -> bool {
        self.validate()
            && next.validate()
            && self.phase.successor() == Some(next.phase)
            && self.attempt_id == next.attempt_id
            && self.target_space_id == next.target_space_id
            && self.target_generation == next.target_generation
            && self.target_keyslot_ref == next.target_keyslot_ref
            && self.target_workspace_ref == next.target_workspace_ref
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SameSpaceTransitionPhaseV1 {
    TargetStaged,
    ActivationStarted,
    TargetPromoted,
    CleanupPending,
}

impl SameSpaceTransitionPhaseV1 {
    pub const fn rank(self) -> u8 {
        match self {
            Self::TargetStaged => 0,
            Self::ActivationStarted => 1,
            Self::TargetPromoted => 2,
            Self::CleanupPending => 3,
        }
    }

    const fn successor(self) -> Option<Self> {
        match self {
            Self::TargetStaged => Some(Self::ActivationStarted),
            Self::ActivationStarted => Some(Self::TargetPromoted),
            Self::TargetPromoted => Some(Self::CleanupPending),
            Self::CleanupPending => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SameSpaceTransitionV1 {
    pub transition_format_version: u16,
    pub attempt_id: AdmissionAttemptId,
    pub target_space_id: String,
    pub source_generation: [u8; 16],
    pub target_generation: [u8; 16],
    pub target_keyslot_ref: Vec<u8>,
    pub target_workspace_ref: Vec<u8>,
    pub phase: SameSpaceTransitionPhaseV1,
}

impl std::fmt::Debug for SameSpaceTransitionV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SameSpaceTransitionV1")
            .field("attempt_id", &self.attempt_id)
            .field("phase", &self.phase)
            .finish()
    }
}

impl SameSpaceTransitionV1 {
    pub fn validate(&self) -> bool {
        self.transition_format_version == SAME_SPACE_TRANSITION_FORMAT_V1
            && !self.target_space_id.is_empty()
            && self.source_generation != self.target_generation
            && !self.target_keyslot_ref.is_empty()
            && !self.target_workspace_ref.is_empty()
    }

    pub fn can_advance_to(&self, next: &Self) -> bool {
        self.validate()
            && next.validate()
            && self.phase.successor() == Some(next.phase)
            && self.attempt_id == next.attempt_id
            && self.target_space_id == next.target_space_id
            && self.source_generation == next.source_generation
            && self.target_generation == next.target_generation
            && self.target_keyslot_ref == next.target_keyslot_ref
            && self.target_workspace_ref == next.target_workspace_ref
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossSpaceTransitionPhaseV2 {
    SourcePrepared,
    TargetStaged,
    ActivationStarted,
    SourceFinalized,
    DataRewrapped,
    TargetPromoted,
    CleanupPending,
}

impl CrossSpaceTransitionPhaseV2 {
    pub const fn rank(self) -> u8 {
        match self {
            Self::SourcePrepared => 0,
            Self::TargetStaged => 1,
            Self::ActivationStarted => 2,
            Self::SourceFinalized => 3,
            Self::DataRewrapped => 4,
            Self::TargetPromoted => 5,
            Self::CleanupPending => 6,
        }
    }

    pub const fn successor(self) -> Option<Self> {
        match self {
            Self::SourcePrepared => Some(Self::TargetStaged),
            Self::TargetStaged => Some(Self::ActivationStarted),
            Self::ActivationStarted => Some(Self::SourceFinalized),
            Self::SourceFinalized => Some(Self::DataRewrapped),
            Self::DataRewrapped => Some(Self::TargetPromoted),
            Self::TargetPromoted => Some(Self::CleanupPending),
            Self::CleanupPending => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossSpaceTransitionV2 {
    pub transition_format_version: u16,
    pub attempt_id: AdmissionAttemptId,
    pub source_space_id: String,
    pub source_generation: [u8; 16],
    pub source_backup_ref: Vec<u8>,
    pub source_backup_digest: [u8; 32],
    pub source_revision_at_backup: u64,
    pub target_space_id: String,
    pub target_generation: [u8; 16],
    pub target_keyslot_ref: Vec<u8>,
    pub target_workspace_ref: Vec<u8>,
    pub phase: CrossSpaceTransitionPhaseV2,
    pub final_source_revision: Option<u64>,
    pub final_manifest_digest: Option<[u8; 32]>,
    pub migrated_records: u64,
    pub preserved_unreadable_records: u64,
    #[serde(default)]
    pub preserve_unreadable_history: bool,
}

impl std::fmt::Debug for CrossSpaceTransitionV2 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CrossSpaceTransitionV2")
            .field("attempt_id", &self.attempt_id)
            .field("phase", &self.phase)
            .field("source_revision_at_backup", &self.source_revision_at_backup)
            .field("final_source_revision", &self.final_source_revision)
            .field("migrated_records", &self.migrated_records)
            .field(
                "preserved_unreadable_records",
                &self.preserved_unreadable_records,
            )
            .finish()
    }
}

impl CrossSpaceTransitionV2 {
    pub fn encode(&self) -> Option<Vec<u8>> {
        self.validate()
            .then(|| postcard::to_stdvec(self).ok())
            .flatten()
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let transition: Self = postcard::from_bytes(bytes).ok()?;
        transition.validate().then_some(transition)
    }

    pub fn validate(&self) -> bool {
        if self.transition_format_version != CROSS_SPACE_TRANSITION_FORMAT_V2
            || self.source_space_id.is_empty()
            || self.target_space_id.is_empty()
            || self.source_space_id == self.target_space_id
            || self.source_backup_ref.is_empty()
            || self.target_keyslot_ref.is_empty()
            || self.target_workspace_ref.is_empty()
        {
            return false;
        }
        let finalized = self.phase.rank() >= CrossSpaceTransitionPhaseV2::SourceFinalized.rank();
        finalized == (self.final_source_revision.is_some() && self.final_manifest_digest.is_some())
            && self
                .final_source_revision
                .is_none_or(|revision| revision >= self.source_revision_at_backup)
    }

    pub fn can_advance_to(&self, next: &Self) -> bool {
        let Some(successor) = self.phase.successor() else {
            return false;
        };
        self.validate()
            && next.validate()
            && next.phase == successor
            && self.attempt_id == next.attempt_id
            && self.source_space_id == next.source_space_id
            && self.source_generation == next.source_generation
            && self.source_backup_ref == next.source_backup_ref
            && self.source_backup_digest == next.source_backup_digest
            && self.source_revision_at_backup == next.source_revision_at_backup
            && self.target_space_id == next.target_space_id
            && self.target_generation == next.target_generation
            && self.target_keyslot_ref == next.target_keyslot_ref
            && self.target_workspace_ref == next.target_workspace_ref
            && next.migrated_records >= self.migrated_records
            && next.preserved_unreadable_records >= self.preserved_unreadable_records
            && self.preserve_unreadable_history == next.preserve_unreadable_history
            && (self.final_source_revision.is_none()
                || self.final_source_revision == next.final_source_revision)
            && (self.final_manifest_digest.is_none()
                || self.final_manifest_digest == next.final_manifest_digest)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossSpaceTransitionResultV2 {
    pub source_space_id: String,
    pub target_space_id: String,
    pub final_source_revision: u64,
    pub final_manifest_digest: [u8; 32],
    pub migrated_records: u64,
    pub preserved_unreadable_records: u64,
}

impl CrossSpaceTransitionResultV2 {
    pub fn encode(&self) -> Option<Vec<u8>> {
        postcard::to_stdvec(self).ok()
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        postcard::from_bytes(bytes).ok()
    }

    pub fn from_cleanup_pending(transition: &CrossSpaceTransitionV2) -> Option<Self> {
        if transition.phase != CrossSpaceTransitionPhaseV2::CleanupPending || !transition.validate()
        {
            return None;
        }
        Some(Self {
            source_space_id: transition.source_space_id.clone(),
            target_space_id: transition.target_space_id.clone(),
            final_source_revision: transition.final_source_revision?,
            final_manifest_digest: transition.final_manifest_digest?,
            migrated_records: transition.migrated_records,
            preserved_unreadable_records: transition.preserved_unreadable_records,
        })
    }

    pub fn matches_cleanup_pending(&self, transition: &CrossSpaceTransitionV2) -> bool {
        Self::from_cleanup_pending(transition).as_ref() == Some(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdmissionSpaceTransitionV2 {
    Fresh(FreshSpaceTransitionV1),
    SameSpace(SameSpaceTransitionV1),
    CrossSpace(CrossSpaceTransitionV2),
}

impl AdmissionSpaceTransitionV2 {
    pub fn encode(&self) -> Option<Vec<u8>> {
        self.validate()
            .then(|| postcard::to_stdvec(self).ok())
            .flatten()
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let transition: Self = postcard::from_bytes(bytes).ok()?;
        transition.validate().then_some(transition)
    }

    pub fn validate(&self) -> bool {
        match self {
            Self::Fresh(transition) => transition.validate(),
            Self::SameSpace(transition) => transition.validate(),
            Self::CrossSpace(transition) => transition.validate(),
        }
    }

    pub const fn attempt_id(&self) -> AdmissionAttemptId {
        match self {
            Self::Fresh(transition) => transition.attempt_id,
            Self::SameSpace(transition) => transition.attempt_id,
            Self::CrossSpace(transition) => transition.attempt_id,
        }
    }

    pub fn target_space_id(&self) -> &str {
        match self {
            Self::Fresh(transition) => &transition.target_space_id,
            Self::SameSpace(transition) => &transition.target_space_id,
            Self::CrossSpace(transition) => &transition.target_space_id,
        }
    }

    pub const fn phase_rank(&self) -> u8 {
        match self {
            Self::Fresh(transition) => transition.phase.rank(),
            Self::SameSpace(transition) => transition.phase.rank(),
            Self::CrossSpace(transition) => transition.phase.rank(),
        }
    }

    pub const fn activation_started_rank(&self) -> u8 {
        match self {
            Self::Fresh(_) => FreshSpaceTransitionPhaseV1::ActivationStarted.rank(),
            Self::SameSpace(_) => SameSpaceTransitionPhaseV1::ActivationStarted.rank(),
            Self::CrossSpace(_) => CrossSpaceTransitionPhaseV2::ActivationStarted.rank(),
        }
    }

    pub fn is_initial(&self) -> bool {
        match self {
            Self::Fresh(transition) => {
                transition.phase == FreshSpaceTransitionPhaseV1::TargetStaged
            }
            Self::SameSpace(transition) => {
                transition.phase == SameSpaceTransitionPhaseV1::TargetStaged
            }
            Self::CrossSpace(transition) => {
                transition.phase == CrossSpaceTransitionPhaseV2::TargetStaged
            }
        }
    }

    pub fn can_advance_to(&self, next: &Self) -> bool {
        match (self, next) {
            (Self::Fresh(current), Self::Fresh(next)) => current.can_advance_to(next),
            (Self::SameSpace(current), Self::SameSpace(next)) => current.can_advance_to(next),
            (Self::CrossSpace(current), Self::CrossSpace(next)) => current.can_advance_to(next),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdmissionSpaceTransitionResultV2 {
    Fresh { target_space_id: String },
    SameSpace { target_space_id: String },
    CrossSpace(CrossSpaceTransitionResultV2),
}

impl AdmissionSpaceTransitionResultV2 {
    pub fn encode(&self) -> Option<Vec<u8>> {
        postcard::to_stdvec(self).ok()
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        postcard::from_bytes(bytes).ok()
    }

    pub fn matches_cleanup_pending(&self, transition: &AdmissionSpaceTransitionV2) -> bool {
        match (self, transition) {
            (Self::Fresh { target_space_id }, AdmissionSpaceTransitionV2::Fresh(fresh)) => {
                fresh.validate()
                    && fresh.phase == FreshSpaceTransitionPhaseV1::CleanupPending
                    && target_space_id == &fresh.target_space_id
            }
            (Self::CrossSpace(result), AdmissionSpaceTransitionV2::CrossSpace(cross)) => {
                result.matches_cleanup_pending(cross)
            }
            (Self::SameSpace { target_space_id }, AdmissionSpaceTransitionV2::SameSpace(same)) => {
                same.validate()
                    && same.phase == SameSpaceTransitionPhaseV1::CleanupPending
                    && target_space_id == &same.target_space_id
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transition(phase: CrossSpaceTransitionPhaseV2) -> CrossSpaceTransitionV2 {
        let finalized = phase.rank() >= CrossSpaceTransitionPhaseV2::SourceFinalized.rank();
        CrossSpaceTransitionV2 {
            transition_format_version: CROSS_SPACE_TRANSITION_FORMAT_V2,
            attempt_id: AdmissionAttemptId::from_bytes([0x11; 32]),
            source_space_id: "source".to_owned(),
            source_generation: [0x12; 16],
            source_backup_ref: b"backup".to_vec(),
            source_backup_digest: [0x13; 32],
            source_revision_at_backup: 7,
            target_space_id: "target".to_owned(),
            target_generation: [0x14; 16],
            target_keyslot_ref: b"keyslot".to_vec(),
            target_workspace_ref: b"workspace".to_vec(),
            phase,
            final_source_revision: finalized.then_some(9),
            final_manifest_digest: finalized.then_some([0x15; 32]),
            migrated_records: 3,
            preserved_unreadable_records: 1,
            preserve_unreadable_history: false,
        }
    }

    #[test]
    fn transition_only_advances_one_phase_without_changing_identity() {
        let current = transition(CrossSpaceTransitionPhaseV2::TargetStaged);
        let next = transition(CrossSpaceTransitionPhaseV2::ActivationStarted);
        assert!(current.can_advance_to(&next));

        let skipped = transition(CrossSpaceTransitionPhaseV2::SourceFinalized);
        assert!(!current.can_advance_to(&skipped));

        let mut changed = next;
        changed.target_space_id = "other".to_owned();
        assert!(!current.can_advance_to(&changed));

        let cleanup = transition(CrossSpaceTransitionPhaseV2::CleanupPending);
        let mut replaced_result = cleanup.clone();
        replaced_result.migrated_records += 1;
        assert!(!cleanup.can_advance_to(&replaced_result));
    }

    #[test]
    fn unified_transition_and_result_round_trip() {
        let transition = AdmissionSpaceTransitionV2::CrossSpace(transition(
            CrossSpaceTransitionPhaseV2::CleanupPending,
        ));
        assert_eq!(
            AdmissionSpaceTransitionV2::decode(&transition.encode().unwrap()),
            Some(transition.clone())
        );
        let result = AdmissionSpaceTransitionResultV2::CrossSpace(
            CrossSpaceTransitionResultV2::from_cleanup_pending(match &transition {
                AdmissionSpaceTransitionV2::CrossSpace(cross) => cross,
                _ => unreachable!(),
            })
            .unwrap(),
        );
        assert_eq!(
            AdmissionSpaceTransitionResultV2::decode(&result.encode().unwrap()),
            Some(result)
        );
    }

    #[test]
    fn result_requires_a_valid_cleanup_pending_transition() {
        assert!(
            CrossSpaceTransitionResultV2::from_cleanup_pending(&transition(
                CrossSpaceTransitionPhaseV2::TargetPromoted
            ))
            .is_none()
        );
        let result = CrossSpaceTransitionResultV2::from_cleanup_pending(&transition(
            CrossSpaceTransitionPhaseV2::CleanupPending,
        ))
        .unwrap();
        assert_eq!(result.final_source_revision, 9);
        assert_eq!(result.migrated_records, 3);
    }
}
