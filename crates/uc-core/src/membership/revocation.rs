use std::{collections::HashSet, fmt};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::{DeviceId, SpaceId};

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
}

impl SpaceKeyState {
    pub fn legacy(space_id: SpaceId) -> Self {
        Self {
            space_id,
            epoch: GroupEpoch::new(0),
            current_content_key_id: ContentKeyId::legacy_v1(),
            mode: SpaceSecurityMode::Legacy,
        }
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

    pub fn mark_ready(&mut self, content_key_id: ContentKeyId) -> Result<(), KeyEpochError> {
        if self.mode != SpaceSecurityMode::Migrating {
            return Err(KeyEpochError::InvalidSpaceSecurityTransition {
                from: self.mode,
                to: SpaceSecurityMode::Ready,
            });
        }
        self.advance(content_key_id)?;
        self.mode = SpaceSecurityMode::Ready;
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
        pending_recipients: usize,
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
                pending_recipients, ..
            } => *pending_recipients,
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
    retained_recipients: Vec<DeviceId>,
    previous_epoch: GroupEpoch,
    next_epoch: GroupEpoch,
    status: RevocationStatus,
    created_at_ms: i64,
    updated_at_ms: i64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationOutboxMessage {
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

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawRevocationStage")]
pub struct RevocationStage {
    record: RevocationRecord,
    next_space_state: SpaceKeyState,
    group_state: Vec<u8>,
    key_catalog: Vec<u8>,
    outbox: Vec<RevocationOutboxMessage>,
}

#[derive(Deserialize)]
struct RawRevocationStage {
    record: RevocationRecord,
    next_space_state: SpaceKeyState,
    group_state: Vec<u8>,
    key_catalog: Vec<u8>,
    outbox: Vec<RevocationOutboxMessage>,
}

impl RevocationStage {
    pub fn new(
        record: RevocationRecord,
        next_space_state: SpaceKeyState,
        group_state: Vec<u8>,
        key_catalog: Vec<u8>,
        outbox: Vec<RevocationOutboxMessage>,
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
        Ok(Self {
            record,
            next_space_state,
            group_state,
            key_catalog,
            outbox,
        })
    }

    pub fn record(&self) -> &RevocationRecord {
        &self.record
    }

    pub fn next_space_state(&self) -> &SpaceKeyState {
        &self.next_space_state
    }

    pub fn group_state(&self) -> &[u8] {
        &self.group_state
    }

    pub fn key_catalog(&self) -> &[u8] {
        &self.key_catalog
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
            .find(|message| message.recipient() == recipient)
            .ok_or(KeyEpochError::RevocationRecipientNotFound)?;
        message.confirm(now_ms);
        Ok(())
    }

    pub fn all_recipients_confirmed(&self) -> bool {
        self.outbox
            .iter()
            .all(RevocationOutboxMessage::is_confirmed)
    }
}

impl TryFrom<RawRevocationStage> for RevocationStage {
    type Error = KeyEpochError;

    fn try_from(raw: RawRevocationStage) -> Result<Self, Self::Error> {
        let target_status = raw.record.status();
        let updated_at_ms = raw.record.updated_at_ms();
        let mut record = RevocationRecord::prepare_with_recipients(
            raw.record.revocation_id().clone(),
            raw.record.space_id().clone(),
            raw.record.target_device_id().clone(),
            raw.record.retained_recipients().to_vec(),
            raw.record.previous_epoch(),
            raw.record.created_at_ms(),
        )?;
        record.transition_to(RevocationStatus::Staged, updated_at_ms)?;
        let mut stage = Self::new(
            record,
            raw.next_space_state,
            raw.group_state,
            raw.key_catalog,
            raw.outbox,
        )?;
        match target_status {
            RevocationStatus::Staged => {}
            RevocationStatus::Activated => {
                stage.transition_to(RevocationStatus::Activated, updated_at_ms)?;
            }
            RevocationStatus::Distributing => {
                stage.transition_to(RevocationStatus::Activated, updated_at_ms)?;
                stage.transition_to(RevocationStatus::Distributing, updated_at_ms)?;
            }
            RevocationStatus::Complete => {
                stage.transition_to(RevocationStatus::Activated, updated_at_ms)?;
                stage.transition_to(RevocationStatus::Distributing, updated_at_ms)?;
                stage.transition_to(RevocationStatus::Complete, updated_at_ms)?;
            }
            RevocationStatus::RecoveryRequired => {
                stage.transition_to(RevocationStatus::RecoveryRequired, updated_at_ms)?;
            }
            RevocationStatus::Prepared => return Err(KeyEpochError::InvalidRevocationStage),
        }
        Ok(stage)
    }
}

impl fmt::Debug for RevocationStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RevocationStage")
            .field("revocation_id", self.record.revocation_id())
            .field("status", &self.record.status())
            .field("epoch", &self.next_space_state.epoch())
            .field("group_state_len", &self.group_state.len())
            .field("key_catalog_len", &self.key_catalog.len())
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
    updated_at_ms: i64,
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
            updated_at_ms,
        }
    }

    pub fn state(&self) -> &SpaceKeyState {
        &self.state
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

    pub fn with_pending_group_updates_from(mut self, previous: &Self) -> Self {
        self.pending_group_updates = previous.pending_group_updates.clone();
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
        self
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
            .field("updated_at_ms", &self.updated_at_ms)
            .finish()
    }
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

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum KeyEpochError {
    #[error("group epoch overflow")]
    EpochOverflow,

    #[error("invalid content key id")]
    InvalidContentKeyId,

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

    #[error("removed member cannot receive the staged revocation")]
    RemovedMemberInOutbox,

    #[error("revocation recipient not found")]
    RevocationRecipientNotFound,

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
