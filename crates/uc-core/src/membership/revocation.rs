use std::{collections::HashSet, fmt};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::{DeviceId, SpaceId};
use crate::space_access::GroupAdmission;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ProtectionGroupId(String);

impl<'de> Deserialize<'de> for ProtectionGroupId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_string(value).map_err(serde::de::Error::custom)
    }
}

impl ProtectionGroupId {
    pub fn from_string(value: impl Into<String>) -> Result<Self, KeyEpochError> {
        let value = value.into();
        if value.is_empty() || value.len() > 128 || !value.is_ascii() {
            return Err(KeyEpochError::InvalidProtectionGroupId);
        }
        Ok(Self(value))
    }

    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AdmissionReplayId([u8; 32]);

impl AdmissionReplayId {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectionGroupAdmission {
    pub protection_group_id: ProtectionGroupId,
    pub admission: GroupAdmission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GroupEpoch(u64);

impl GroupEpoch {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Result<Self, KeyEpochError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(KeyEpochError::EpochOverflow)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContentKeyId(String);

impl ContentKeyId {
    pub fn from_string(value: impl Into<String>) -> Result<Self, KeyEpochError> {
        let value = value.into();
        if value.is_empty() || value.len() > 128 || !value.is_ascii() {
            return Err(KeyEpochError::InvalidContentKeyId);
        }
        Ok(Self(value))
    }

    pub fn legacy_v1() -> Self {
        Self("legacy-v1".to_owned())
    }

    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContentKeyPurpose {
    Content,
    Transport,
    Search,
}

impl ContentKeyPurpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::Transport => "transport",
            Self::Search => "search",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpaceSecurityMode {
    Legacy,
    Migrating,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceKeyState {
    space_id: SpaceId,
    epoch: GroupEpoch,
    current_content_key_id: ContentKeyId,
    mode: SpaceSecurityMode,
    #[serde(default)]
    protection_group_id: Option<ProtectionGroupId>,
}

impl SpaceKeyState {
    pub fn legacy(space_id: SpaceId) -> Self {
        Self {
            space_id,
            epoch: GroupEpoch::new(0),
            current_content_key_id: ContentKeyId::legacy_v1(),
            mode: SpaceSecurityMode::Legacy,
            protection_group_id: None,
        }
    }

    pub fn ready_for_admission(
        space_id: SpaceId,
        epoch: GroupEpoch,
        current_content_key_id: ContentKeyId,
        protection_group_id: ProtectionGroupId,
    ) -> Result<Self, KeyEpochError> {
        if epoch.value() == 0 || current_content_key_id == ContentKeyId::legacy_v1() {
            return Err(KeyEpochError::SpaceNotReady);
        }
        Ok(Self {
            space_id,
            epoch,
            current_content_key_id,
            mode: SpaceSecurityMode::Ready,
            protection_group_id: Some(protection_group_id),
        })
    }

    pub fn mark_migrating(&mut self) -> Result<(), KeyEpochError> {
        match self.mode {
            SpaceSecurityMode::Legacy => {
                self.mode = SpaceSecurityMode::Migrating;
                Ok(())
            }
            SpaceSecurityMode::Migrating => Ok(()),
            SpaceSecurityMode::Ready => Err(KeyEpochError::InvalidSpaceSecurityTransition {
                from: self.mode,
                to: SpaceSecurityMode::Migrating,
            }),
        }
    }

    pub fn mark_ready(
        &mut self,
        content_key_id: ContentKeyId,
        protection_group_id: ProtectionGroupId,
    ) -> Result<(), KeyEpochError> {
        if self.mode != SpaceSecurityMode::Migrating {
            return Err(KeyEpochError::InvalidSpaceSecurityTransition {
                from: self.mode,
                to: SpaceSecurityMode::Ready,
            });
        }
        self.advance(content_key_id)?;
        self.mode = SpaceSecurityMode::Ready;
        self.protection_group_id = Some(protection_group_id);
        Ok(())
    }

    pub fn rotate(&mut self, content_key_id: ContentKeyId) -> Result<(), KeyEpochError> {
        if self.mode != SpaceSecurityMode::Ready {
            return Err(KeyEpochError::SpaceNotReady);
        }
        self.advance(content_key_id)
    }

    fn advance(&mut self, content_key_id: ContentKeyId) -> Result<(), KeyEpochError> {
        if content_key_id == self.current_content_key_id {
            return Err(KeyEpochError::ContentKeyReuse);
        }
        self.epoch = self.epoch.next()?;
        self.current_content_key_id = content_key_id;
        Ok(())
    }

    pub fn space_id(&self) -> &SpaceId {
        &self.space_id
    }

    pub const fn epoch(&self) -> GroupEpoch {
        self.epoch
    }

    pub fn current_content_key_id(&self) -> &ContentKeyId {
        &self.current_content_key_id
    }

    pub const fn mode(&self) -> SpaceSecurityMode {
        self.mode
    }

    pub fn protection_group_id(&self) -> Option<&ProtectionGroupId> {
        self.protection_group_id.as_ref()
    }

    pub fn backfill_protection_group_id(
        &mut self,
        protection_group_id: ProtectionGroupId,
    ) -> Result<bool, KeyEpochError> {
        if self.mode != SpaceSecurityMode::Ready {
            return Err(KeyEpochError::SpaceNotReady);
        }
        if self.protection_group_id.is_some() {
            return Ok(false);
        }
        self.protection_group_id = Some(protection_group_id);
        Ok(true)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RevocationId(String);

impl RevocationId {
    pub fn from_string(value: impl Into<String>) -> Result<Self, KeyEpochError> {
        let value = value.into();
        if value.is_empty() || value.len() > 128 || !value.is_ascii() {
            return Err(KeyEpochError::InvalidRevocationId);
        }
        Ok(Self(value))
    }

    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RevocationStatus {
    Prepared,
    Staged,
    Activated,
    Distributing,
    Complete,
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupRevocationResult {
    LocalOnly,
    Reliable {
        revocation_id: RevocationId,
        status: RevocationStatus,
        removed_device_ids: Vec<DeviceId>,
        pending_recipient_device_ids: Vec<DeviceId>,
        updated_at_ms: i64,
    },
}

impl GroupRevocationResult {
    pub const fn status(&self) -> Option<RevocationStatus> {
        match self {
            Self::LocalOnly => None,
            Self::Reliable { status, .. } => Some(*status),
        }
    }

    pub fn revocation_id(&self) -> Option<&RevocationId> {
        match self {
            Self::LocalOnly => None,
            Self::Reliable { revocation_id, .. } => Some(revocation_id),
        }
    }

    pub const fn pending_recipients(&self) -> usize {
        match self {
            Self::LocalOnly => 0,
            Self::Reliable {
                pending_recipient_device_ids,
                ..
            } => pending_recipient_device_ids.len(),
        }
    }

    pub fn removed_device_ids(&self) -> &[DeviceId] {
        match self {
            Self::LocalOnly => &[],
            Self::Reliable {
                removed_device_ids, ..
            } => removed_device_ids,
        }
    }

    pub fn pending_recipient_device_ids(&self) -> &[DeviceId] {
        match self {
            Self::LocalOnly => &[],
            Self::Reliable {
                pending_recipient_device_ids,
                ..
            } => pending_recipient_device_ids,
        }
    }

    pub const fn updated_at_ms(&self) -> i64 {
        match self {
            Self::LocalOnly => 0,
            Self::Reliable { updated_at_ms, .. } => *updated_at_ms,
        }
    }
}

impl RevocationStatus {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Complete | Self::RecoveryRequired)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawRevocationRecord")]
pub struct RevocationRecord {
    revocation_id: RevocationId,
    space_id: SpaceId,
    target_device_id: DeviceId,
    #[serde(default)]
    permanently_lost_device_ids: Vec<DeviceId>,
    #[serde(default)]
    retained_recipients: Vec<DeviceId>,
    previous_epoch: GroupEpoch,
    next_epoch: GroupEpoch,
    status: RevocationStatus,
    created_at_ms: i64,
    updated_at_ms: i64,
}

#[derive(Deserialize)]
struct RawRevocationRecord {
    revocation_id: RevocationId,
    space_id: SpaceId,
    target_device_id: DeviceId,
    #[serde(default)]
    permanently_lost_device_ids: Vec<DeviceId>,
    #[serde(default)]
    retained_recipients: Vec<DeviceId>,
    previous_epoch: GroupEpoch,
    next_epoch: GroupEpoch,
    status: RevocationStatus,
    created_at_ms: i64,
    updated_at_ms: i64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationOutboxMessage {
    #[serde(default)]
    generation: u64,
    recipient: DeviceId,
    payload: Vec<u8>,
    #[serde(default)]
    confirmed_at_ms: Option<i64>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingGroupUpdate {
    update_id: String,
    revocation_id: Option<RevocationId>,
    recipient: DeviceId,
    payload: Vec<u8>,
}

impl PendingGroupUpdate {
    pub fn new(revocation_id: RevocationId, recipient: DeviceId, payload: Vec<u8>) -> Self {
        Self {
            update_id: format!(
                "revocation:{}:{}",
                revocation_id.as_str(),
                recipient.as_str()
            ),
            revocation_id: Some(revocation_id),
            recipient,
            payload,
        }
    }

    pub fn persistent(recipient: DeviceId, payload: Vec<u8>) -> Self {
        Self {
            update_id: uuid::Uuid::new_v4().to_string(),
            revocation_id: None,
            recipient,
            payload,
        }
    }

    pub fn for_admission(attempt_id: [u8; 32], recipient: DeviceId, payload: Vec<u8>) -> Self {
        let attempt_id = attempt_id
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Self {
            update_id: format!("admission:{attempt_id}:{}", recipient.as_str()),
            revocation_id: None,
            recipient,
            payload,
        }
    }

    pub fn for_generation(
        revocation_id: RevocationId,
        generation: GroupEpoch,
        recipient: DeviceId,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            update_id: format!(
                "revocation:{}:{}:{}",
                revocation_id.as_str(),
                generation.value(),
                recipient.as_str()
            ),
            revocation_id: Some(revocation_id),
            recipient,
            payload,
        }
    }

    pub fn update_id(&self) -> &str {
        &self.update_id
    }

    pub fn revocation_id(&self) -> Option<&RevocationId> {
        self.revocation_id.as_ref()
    }

    pub fn recipient(&self) -> &DeviceId {
        &self.recipient
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

impl fmt::Debug for PendingGroupUpdate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingGroupUpdate")
            .field("update_id", &self.update_id)
            .field("has_revocation", &self.revocation_id.is_some())
            .field("recipient", &self.recipient)
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

impl RevocationOutboxMessage {
    pub fn new(recipient: DeviceId, payload: Vec<u8>) -> Self {
        Self {
            generation: 0,
            recipient,
            payload,
            confirmed_at_ms: None,
        }
    }

    pub fn recipient(&self) -> &DeviceId {
        &self.recipient
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub const fn is_confirmed(&self) -> bool {
        self.confirmed_at_ms.is_some()
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    fn assign_generation(&mut self, generation: GroupEpoch) {
        self.generation = generation.value();
    }

    fn confirm(&mut self, now_ms: i64) {
        self.confirmed_at_ms.get_or_insert(now_ms);
    }
}

impl fmt::Debug for RevocationOutboxMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RevocationOutboxMessage")
            .field("recipient", &self.recipient)
            .field("payload_len", &self.payload.len())
            .field("confirmed", &self.is_confirmed())
            .finish()
    }
}

const REVOCATION_STAGE_VERSION: u8 = 2;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationGeneration {
    previous_epoch: GroupEpoch,
    next_space_state: SpaceKeyState,
    group_state: Vec<u8>,
    key_catalog: Vec<u8>,
}

impl RevocationGeneration {
    fn new(
        previous_epoch: GroupEpoch,
        next_space_state: SpaceKeyState,
        group_state: Vec<u8>,
        key_catalog: Vec<u8>,
    ) -> Result<Self, KeyEpochError> {
        if previous_epoch.next()? != next_space_state.epoch() {
            return Err(KeyEpochError::InvalidRevocationStage);
        }
        Ok(Self {
            previous_epoch,
            next_space_state,
            group_state,
            key_catalog,
        })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawRevocationStage")]
pub struct RevocationStage {
    version: u8,
    record: RevocationRecord,
    generations: Vec<RevocationGeneration>,
    outbox: Vec<RevocationOutboxMessage>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawRevocationStage {
    Current {
        version: u8,
        record: RevocationRecord,
        generations: Vec<RevocationGeneration>,
        outbox: Vec<RevocationOutboxMessage>,
    },
    Legacy {
        record: RevocationRecord,
        next_space_state: SpaceKeyState,
        group_state: Vec<u8>,
        key_catalog: Vec<u8>,
        outbox: Vec<RevocationOutboxMessage>,
    },
}

impl RevocationStage {
    pub fn new(
        record: RevocationRecord,
        next_space_state: SpaceKeyState,
        group_state: Vec<u8>,
        key_catalog: Vec<u8>,
        mut outbox: Vec<RevocationOutboxMessage>,
    ) -> Result<Self, KeyEpochError> {
        if record.status() != RevocationStatus::Staged
            || record.space_id() != next_space_state.space_id()
            || record.next_epoch() != next_space_state.epoch()
        {
            return Err(KeyEpochError::InvalidRevocationStage);
        }
        if outbox
            .iter()
            .any(|message| message.recipient() == record.target_device_id())
        {
            return Err(KeyEpochError::RemovedMemberInOutbox);
        }
        outbox
            .iter_mut()
            .for_each(|message| message.assign_generation(record.next_epoch()));
        let generation = RevocationGeneration::new(
            record.previous_epoch(),
            next_space_state,
            group_state,
            key_catalog,
        )?;
        Ok(Self {
            version: REVOCATION_STAGE_VERSION,
            record,
            generations: vec![generation],
            outbox,
        })
    }

    pub fn record(&self) -> &RevocationRecord {
        &self.record
    }

    pub fn next_space_state(&self) -> &SpaceKeyState {
        &self.current_generation().next_space_state
    }

    pub fn group_state(&self) -> &[u8] {
        &self.current_generation().group_state
    }

    pub fn key_catalog(&self) -> &[u8] {
        &self.current_generation().key_catalog
    }

    pub fn outbox(&self) -> &[RevocationOutboxMessage] {
        &self.outbox
    }

    pub fn transition_to(
        &mut self,
        status: RevocationStatus,
        now_ms: i64,
    ) -> Result<(), KeyEpochError> {
        self.record.transition_to(status, now_ms)
    }

    pub fn acknowledge_recipient(
        &mut self,
        recipient: &DeviceId,
        now_ms: i64,
    ) -> Result<(), KeyEpochError> {
        let message = self
            .outbox
            .iter_mut()
            .find(|message| message.recipient() == recipient && !message.is_confirmed())
            .ok_or(KeyEpochError::RevocationRecipientNotFound)?;
        message.confirm(now_ms);
        Ok(())
    }

    pub fn all_recipients_confirmed(&self) -> bool {
        self.outbox
            .iter()
            .all(RevocationOutboxMessage::is_confirmed)
    }

    pub fn generation_count(&self) -> usize {
        self.generations.len()
    }

    fn current_generation(&self) -> &RevocationGeneration {
        match self.generations.last() {
            Some(generation) => generation,
            None => unreachable!("revocation stage generation invariant"),
        }
    }

    pub fn removed_device_ids(&self) -> Vec<DeviceId> {
        self.record.removed_device_ids()
    }

    pub fn pending_recipient_device_ids(&self) -> Vec<DeviceId> {
        let mut seen = HashSet::new();
        self.outbox
            .iter()
            .filter(|message| !message.is_confirmed())
            .map(|message| message.recipient().clone())
            .filter(|recipient| seen.insert(recipient.clone()))
            .collect()
    }

    pub fn finish_absent_recipients(
        &mut self,
        permanently_lost_device_ids: &[DeviceId],
        now_ms: i64,
    ) -> Result<(), KeyEpochError> {
        if self.record.status() != RevocationStatus::Distributing {
            return Err(KeyEpochError::PermanentLossRecipientNotPending);
        }
        let pending = self.pending_recipient_device_ids();
        let mut unique = HashSet::new();
        if permanently_lost_device_ids.is_empty()
            || permanently_lost_device_ids
                .iter()
                .any(|device_id| !unique.insert(device_id.clone()) || !pending.contains(device_id))
        {
            return Err(KeyEpochError::PermanentLossRecipientNotPending);
        }
        for device_id in permanently_lost_device_ids {
            self.record.finish_absent_recipient(device_id, now_ms)?;
            self.outbox
                .retain(|message| message.recipient() != device_id);
        }
        if self.all_recipients_confirmed() {
            self.record
                .transition_to(RevocationStatus::Complete, now_ms)?;
        }
        Ok(())
    }

    pub fn append_recovery_generation(
        &mut self,
        permanently_lost_device_id: &DeviceId,
        next_space_state: SpaceKeyState,
        group_state: Vec<u8>,
        key_catalog: Vec<u8>,
        mut outbox: Vec<RevocationOutboxMessage>,
        now_ms: i64,
    ) -> Result<(), KeyEpochError> {
        if self.record.status() != RevocationStatus::Distributing
            || !self
                .pending_recipient_device_ids()
                .contains(permanently_lost_device_id)
        {
            return Err(KeyEpochError::PermanentLossRecipientNotPending);
        }
        if outbox
            .iter()
            .any(|message| message.recipient() == permanently_lost_device_id)
        {
            return Err(KeyEpochError::RemovedMemberInOutbox);
        }
        let mut next_record = self.record.clone();
        next_record.advance_for_permanent_loss(permanently_lost_device_id, now_ms)?;
        if next_space_state.space_id() != next_record.space_id()
            || next_space_state.epoch() != next_record.next_epoch()
        {
            return Err(KeyEpochError::InvalidRevocationStage);
        }
        let generation = RevocationGeneration::new(
            next_record.previous_epoch(),
            next_space_state,
            group_state,
            key_catalog,
        )?;
        let mut next_outbox = self.outbox.clone();
        next_outbox.retain(|message| message.recipient() != permanently_lost_device_id);
        outbox
            .iter_mut()
            .for_each(|message| message.assign_generation(next_record.next_epoch()));
        next_outbox.extend(outbox);
        if next_outbox
            .iter()
            .all(RevocationOutboxMessage::is_confirmed)
        {
            next_record.transition_to(RevocationStatus::Complete, now_ms)?;
        }
        self.record = next_record;
        self.outbox = next_outbox;
        self.generations.push(generation);
        Ok(())
    }
}

impl TryFrom<RawRevocationStage> for RevocationStage {
    type Error = KeyEpochError;

    fn try_from(raw: RawRevocationStage) -> Result<Self, Self::Error> {
        let mut stage = match raw {
            RawRevocationStage::Current {
                version,
                record,
                generations,
                outbox,
            } => Self {
                version,
                record,
                generations,
                outbox,
            },
            RawRevocationStage::Legacy {
                record,
                next_space_state,
                group_state,
                key_catalog,
                mut outbox,
            } => {
                let generation = RevocationGeneration::new(
                    record.previous_epoch(),
                    next_space_state,
                    group_state,
                    key_catalog,
                )?;
                outbox
                    .iter_mut()
                    .for_each(|message| message.assign_generation(record.next_epoch()));
                Self {
                    version: REVOCATION_STAGE_VERSION,
                    record,
                    generations: vec![generation],
                    outbox,
                }
            }
        };
        if stage.version != REVOCATION_STAGE_VERSION || stage.generations.is_empty() {
            return Err(KeyEpochError::InvalidRevocationStage);
        }
        let mut expected_previous = stage.generations[0].previous_epoch;
        for generation in &stage.generations {
            if generation.previous_epoch != expected_previous
                || generation.next_space_state.space_id() != stage.record.space_id()
                || generation.next_space_state.epoch() != expected_previous.next()?
            {
                return Err(KeyEpochError::InvalidRevocationStage);
            }
            expected_previous = generation.next_space_state.epoch();
        }
        let Some(last) = stage.generations.last() else {
            return Err(KeyEpochError::InvalidRevocationStage);
        };
        if last.previous_epoch != stage.record.previous_epoch()
            || last.next_space_state.epoch() != stage.record.next_epoch()
        {
            return Err(KeyEpochError::InvalidRevocationStage);
        }
        let removed = stage.record.removed_device_ids();
        if stage
            .outbox
            .iter()
            .any(|message| removed.contains(message.recipient()))
        {
            return Err(KeyEpochError::RemovedMemberInOutbox);
        }
        stage
            .outbox
            .sort_by_key(RevocationOutboxMessage::generation);
        Ok(stage)
    }
}

impl fmt::Debug for RevocationStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RevocationStage")
            .field("revocation_id", self.record.revocation_id())
            .field("status", &self.record.status())
            .field("generation_count", &self.generations.len())
            .field("epoch", &self.next_space_state().epoch())
            .field("group_state_len", &self.group_state().len())
            .field("key_catalog_len", &self.key_catalog().len())
            .field("outbox_count", &self.outbox.len())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceKeyMaterial {
    state: SpaceKeyState,
    group_state: Vec<u8>,
    key_catalog: Vec<u8>,
    #[serde(default)]
    pending_group_updates: Vec<PendingGroupUpdate>,
    #[serde(default)]
    pending_group_admission_replays: Vec<PendingGroupAdmissionReplay>,
    updated_at_ms: i64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PendingGroupAdmissionReplay {
    recipient: DeviceId,
    replay_id: AdmissionReplayId,
    admission: ProtectionGroupAdmission,
}

impl SpaceKeyMaterial {
    pub fn new(
        state: SpaceKeyState,
        group_state: Vec<u8>,
        key_catalog: Vec<u8>,
        updated_at_ms: i64,
    ) -> Self {
        Self {
            state,
            group_state,
            key_catalog,
            pending_group_updates: Vec::new(),
            pending_group_admission_replays: Vec::new(),
            updated_at_ms,
        }
    }

    pub fn state(&self) -> &SpaceKeyState {
        &self.state
    }

    pub fn backfill_protection_group_id(
        &mut self,
        protection_group_id: ProtectionGroupId,
    ) -> Result<bool, KeyEpochError> {
        self.state.backfill_protection_group_id(protection_group_id)
    }

    pub fn group_state(&self) -> &[u8] {
        &self.group_state
    }

    pub fn key_catalog(&self) -> &[u8] {
        &self.key_catalog
    }

    pub fn pending_group_updates(&self) -> &[PendingGroupUpdate] {
        &self.pending_group_updates
    }

    pub fn add_pending_group_updates(
        &mut self,
        updates: impl IntoIterator<Item = PendingGroupUpdate>,
        now_ms: i64,
    ) {
        self.pending_group_updates.extend(updates);
        self.updated_at_ms = now_ms;
    }

    pub fn acknowledge_group_update(&mut self, update_id: &str, now_ms: i64) -> bool {
        let before = self.pending_group_updates.len();
        self.pending_group_updates
            .retain(|update| update.update_id() != update_id);
        let removed = self.pending_group_updates.len() != before;
        if removed {
            self.updated_at_ms = now_ms;
        }
        removed
    }

    /// 将本轮未送达的欠账移到队尾，使后续轮次不会被同一批离线设备饿饿。
    pub fn defer_group_update(&mut self, update_id: &str, now_ms: i64) -> bool {
        let Some(index) = self
            .pending_group_updates
            .iter()
            .position(|update| update.update_id() == update_id)
        else {
            return false;
        };
        let update = self.pending_group_updates.remove(index);
        self.pending_group_updates.push(update);
        self.updated_at_ms = now_ms;
        true
    }

    pub fn with_pending_group_updates_from(mut self, previous: &Self) -> Self {
        self.pending_group_updates = previous.pending_group_updates.clone();
        self.pending_group_admission_replays = previous.pending_group_admission_replays.clone();
        self
    }

    pub fn with_pending_group_updates_from_excluding(
        mut self,
        previous: &Self,
        excluded_recipient: &DeviceId,
    ) -> Self {
        self.pending_group_updates = previous
            .pending_group_updates
            .iter()
            .filter(|update| update.recipient() != excluded_recipient)
            .cloned()
            .collect();
        self.pending_group_admission_replays = previous
            .pending_group_admission_replays
            .iter()
            .filter(|admission| &admission.recipient != excluded_recipient)
            .cloned()
            .collect();
        self
    }

    pub fn with_pending_group_updates_from_excluding_many(
        mut self,
        previous: &Self,
        excluded_recipients: &[DeviceId],
    ) -> Self {
        self.pending_group_updates = previous
            .pending_group_updates
            .iter()
            .filter(|update| !excluded_recipients.contains(update.recipient()))
            .cloned()
            .collect();
        self.pending_group_admission_replays = previous
            .pending_group_admission_replays
            .iter()
            .filter(|admission| !excluded_recipients.contains(&admission.recipient))
            .cloned()
            .collect();
        self
    }

    pub fn cache_group_admission(
        &mut self,
        recipient: DeviceId,
        replay_id: AdmissionReplayId,
        admission: ProtectionGroupAdmission,
        now_ms: i64,
    ) {
        self.pending_group_admission_replays
            .retain(|cached| cached.recipient != recipient);
        self.pending_group_admission_replays
            .push(PendingGroupAdmissionReplay {
                recipient,
                replay_id,
                admission,
            });
        self.updated_at_ms = now_ms;
    }

    pub fn cached_group_admission(
        &self,
        recipient: &DeviceId,
        replay_id: AdmissionReplayId,
    ) -> Option<&ProtectionGroupAdmission> {
        self.pending_group_admission_replays
            .iter()
            .find(|cached| cached.recipient == *recipient && cached.replay_id == replay_id)
            .map(|cached| &cached.admission)
    }

    pub const fn updated_at_ms(&self) -> i64 {
        self.updated_at_ms
    }
}

impl fmt::Debug for SpaceKeyMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpaceKeyMaterial")
            .field("space_id", self.state.space_id())
            .field("epoch", &self.state.epoch())
            .field("mode", &self.state.mode())
            .field("group_state_len", &self.group_state.len())
            .field("key_catalog_len", &self.key_catalog.len())
            .field(
                "pending_group_update_count",
                &self.pending_group_updates.len(),
            )
            .field(
                "pending_group_admission_replay_count",
                &self.pending_group_admission_replays.len(),
            )
            .field("updated_at_ms", &self.updated_at_ms)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedRevocationResolution {
    TargetAbsent(SpaceKeyMaterial),
    TargetPresent {
        current_material: SpaceKeyMaterial,
        stage: RevocationStage,
    },
    RecoveryRequired(Option<SpaceKeyMaterial>),
}

impl RevocationRecord {
    pub fn prepare(
        revocation_id: RevocationId,
        space_id: SpaceId,
        target_device_id: DeviceId,
        previous_epoch: GroupEpoch,
        now_ms: i64,
    ) -> Result<Self, KeyEpochError> {
        Self::prepare_with_recipients(
            revocation_id,
            space_id,
            target_device_id,
            Vec::new(),
            previous_epoch,
            now_ms,
        )
    }

    pub fn prepare_with_recipients(
        revocation_id: RevocationId,
        space_id: SpaceId,
        target_device_id: DeviceId,
        retained_recipients: Vec<DeviceId>,
        previous_epoch: GroupEpoch,
        now_ms: i64,
    ) -> Result<Self, KeyEpochError> {
        if retained_recipients
            .iter()
            .any(|recipient| recipient == &target_device_id)
        {
            return Err(KeyEpochError::RemovedMemberInOutbox);
        }
        let mut seen = HashSet::new();
        let retained_recipients = retained_recipients
            .into_iter()
            .filter(|recipient| seen.insert(recipient.clone()))
            .collect();
        Ok(Self {
            revocation_id,
            space_id,
            target_device_id,
            permanently_lost_device_ids: Vec::new(),
            retained_recipients,
            previous_epoch,
            next_epoch: previous_epoch.next()?,
            status: RevocationStatus::Prepared,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
    }

    pub fn transition_to(
        &mut self,
        next: RevocationStatus,
        now_ms: i64,
    ) -> Result<(), KeyEpochError> {
        if self.status == next {
            self.updated_at_ms = now_ms;
            return Ok(());
        }
        let valid = matches!(
            (self.status, next),
            (RevocationStatus::Prepared, RevocationStatus::Staged)
                | (RevocationStatus::Prepared, RevocationStatus::Complete)
                | (RevocationStatus::Staged, RevocationStatus::Activated)
                | (RevocationStatus::Activated, RevocationStatus::Distributing)
                | (RevocationStatus::Distributing, RevocationStatus::Complete)
                | (
                    RevocationStatus::Prepared
                        | RevocationStatus::Staged
                        | RevocationStatus::Activated
                        | RevocationStatus::Distributing,
                    RevocationStatus::RecoveryRequired
                )
        );
        if !valid {
            return Err(KeyEpochError::InvalidRevocationTransition {
                from: self.status,
                to: next,
            });
        }
        self.status = next;
        self.updated_at_ms = now_ms;
        Ok(())
    }

    pub fn rebase_prepared(
        &mut self,
        current_epoch: GroupEpoch,
        now_ms: i64,
    ) -> Result<(), KeyEpochError> {
        if self.status != RevocationStatus::Prepared || current_epoch < self.previous_epoch {
            return Err(KeyEpochError::InvalidRevocationRecord);
        }
        self.previous_epoch = current_epoch;
        self.next_epoch = current_epoch.next()?;
        self.updated_at_ms = now_ms;
        Ok(())
    }

    pub fn revocation_id(&self) -> &RevocationId {
        &self.revocation_id
    }

    pub fn space_id(&self) -> &SpaceId {
        &self.space_id
    }

    pub fn target_device_id(&self) -> &DeviceId {
        &self.target_device_id
    }

    pub fn retained_recipients(&self) -> &[DeviceId] {
        &self.retained_recipients
    }

    pub fn permanently_lost_device_ids(&self) -> &[DeviceId] {
        &self.permanently_lost_device_ids
    }

    pub fn removed_device_ids(&self) -> Vec<DeviceId> {
        std::iter::once(self.target_device_id.clone())
            .chain(self.permanently_lost_device_ids.iter().cloned())
            .collect()
    }

    fn advance_for_permanent_loss(
        &mut self,
        device_id: &DeviceId,
        now_ms: i64,
    ) -> Result<(), KeyEpochError> {
        if self.status != RevocationStatus::Distributing
            || !self.retained_recipients.contains(device_id)
            || self.permanently_lost_device_ids.contains(device_id)
        {
            return Err(KeyEpochError::PermanentLossRecipientNotPending);
        }
        self.retained_recipients
            .retain(|recipient| recipient != device_id);
        self.permanently_lost_device_ids.push(device_id.clone());
        self.previous_epoch = self.next_epoch;
        self.next_epoch = self.previous_epoch.next()?;
        self.updated_at_ms = now_ms;
        Ok(())
    }

    fn finish_absent_recipient(
        &mut self,
        device_id: &DeviceId,
        now_ms: i64,
    ) -> Result<(), KeyEpochError> {
        if self.status != RevocationStatus::Distributing
            || !self.retained_recipients.contains(device_id)
            || self.permanently_lost_device_ids.contains(device_id)
        {
            return Err(KeyEpochError::PermanentLossRecipientNotPending);
        }
        self.retained_recipients
            .retain(|recipient| recipient != device_id);
        self.permanently_lost_device_ids.push(device_id.clone());
        self.updated_at_ms = now_ms;
        Ok(())
    }

    pub const fn previous_epoch(&self) -> GroupEpoch {
        self.previous_epoch
    }

    pub const fn next_epoch(&self) -> GroupEpoch {
        self.next_epoch
    }

    pub const fn status(&self) -> RevocationStatus {
        self.status
    }

    pub const fn created_at_ms(&self) -> i64 {
        self.created_at_ms
    }

    pub const fn updated_at_ms(&self) -> i64 {
        self.updated_at_ms
    }
}

impl TryFrom<RawRevocationRecord> for RevocationRecord {
    type Error = KeyEpochError;

    fn try_from(raw: RawRevocationRecord) -> Result<Self, Self::Error> {
        if raw.next_epoch != raw.previous_epoch.next()? {
            return Err(KeyEpochError::InvalidRevocationRecord);
        }
        let mut record = Self::prepare_with_recipients(
            raw.revocation_id,
            raw.space_id,
            raw.target_device_id,
            raw.retained_recipients,
            raw.previous_epoch,
            raw.created_at_ms,
        )?;
        let mut seen = HashSet::new();
        if raw.permanently_lost_device_ids.iter().any(|device_id| {
            device_id == record.target_device_id()
                || record.retained_recipients().contains(device_id)
                || !seen.insert(device_id.clone())
        }) {
            return Err(KeyEpochError::InvalidRevocationRecord);
        }
        record.permanently_lost_device_ids = raw.permanently_lost_device_ids;
        match raw.status {
            RevocationStatus::Prepared => {
                record.transition_to(RevocationStatus::Prepared, raw.updated_at_ms)?;
            }
            RevocationStatus::Staged => {
                record.transition_to(RevocationStatus::Staged, raw.updated_at_ms)?;
            }
            RevocationStatus::Activated => {
                record.transition_to(RevocationStatus::Staged, raw.updated_at_ms)?;
                record.transition_to(RevocationStatus::Activated, raw.updated_at_ms)?;
            }
            RevocationStatus::Distributing => {
                record.transition_to(RevocationStatus::Staged, raw.updated_at_ms)?;
                record.transition_to(RevocationStatus::Activated, raw.updated_at_ms)?;
                record.transition_to(RevocationStatus::Distributing, raw.updated_at_ms)?;
            }
            RevocationStatus::Complete => {
                record.transition_to(RevocationStatus::Staged, raw.updated_at_ms)?;
                record.transition_to(RevocationStatus::Activated, raw.updated_at_ms)?;
                record.transition_to(RevocationStatus::Distributing, raw.updated_at_ms)?;
                record.transition_to(RevocationStatus::Complete, raw.updated_at_ms)?;
            }
            RevocationStatus::RecoveryRequired => {
                record.transition_to(RevocationStatus::RecoveryRequired, raw.updated_at_ms)?;
            }
        }
        Ok(record)
    }
}

#[derive(Debug, Error)]
pub enum KeyEpochError {
    #[error("group epoch overflow")]
    EpochOverflow,

    #[error("invalid content key id")]
    InvalidContentKeyId,

    #[error("invalid protection group id")]
    InvalidProtectionGroupId,

    #[error("content key id cannot be reused")]
    ContentKeyReuse,

    #[error("space is not ready for key rotation")]
    SpaceNotReady,

    #[error("invalid space security transition from {from:?} to {to:?}")]
    InvalidSpaceSecurityTransition {
        from: SpaceSecurityMode,
        to: SpaceSecurityMode,
    },

    #[error("invalid staged revocation payload")]
    InvalidRevocationStage,

    #[error("invalid persisted revocation record")]
    InvalidRevocationRecord,

    #[error("persisted security state could not be decrypted")]
    DecryptionFailed,

    #[error("persisted security state failed integrity validation")]
    PersistedStateIntegrityFailed,

    #[error("current space security state could not be installed")]
    SecurityState {
        #[source]
        source: anyhow::Error,
    },

    #[error("removed member cannot receive the staged revocation")]
    RemovedMemberInOutbox,

    #[error("revocation recipient not found")]
    RevocationRecipientNotFound,

    #[error("permanently lost device is not waiting for revocation")]
    PermanentLossRecipientNotPending,

    #[error("invalid revocation id")]
    InvalidRevocationId,

    #[error("invalid revocation transition from {from:?} to {to:?}")]
    InvalidRevocationTransition {
        from: RevocationStatus,
        to: RevocationStatus,
    },

    #[error("key epoch repository failure: {0}")]
    Repository(String),
}

#[cfg(test)]
mod protection_group_id_tests {
    use super::*;

    #[test]
    fn deserialization_rejects_invalid_protection_group_ids() {
        for value in [
            "\"\"".to_owned(),
            format!("\"{}\"", "a".repeat(129)),
            "\"空间\"".to_owned(),
        ] {
            let error = serde_json::from_str::<ProtectionGroupId>(&value).unwrap_err();
            assert!(error.to_string().contains("invalid protection group id"));
        }
    }

    #[test]
    fn deserialization_accepts_a_valid_protection_group_id() {
        let id = serde_json::from_str::<ProtectionGroupId>("\"group-1\"").unwrap();
        assert_eq!(id.as_str(), "group-1");
    }
}
