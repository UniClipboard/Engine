//! Shared identifiers and messages for the current membership history.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::DeviceId;

use super::versioned_membership_history::{MembershipHistoryPageV2, MembershipHistoryV2Ack};
use super::MemberInstanceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MembershipEventId([u8; 32]);

impl MembershipEventId {
    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    pub fn from_hex(value: &str) -> Option<Self> {
        if value.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            bytes[index] = u8::try_from((high << 4) | low).ok()?;
        }
        Some(Self(bytes))
    }
}

impl fmt::Display for MembershipEventId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0[..8] {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalDecision {
    Accept,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MembershipDecisionId([u8; 32]);

impl MembershipDecisionId {
    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipHistoryRelationship {
    Unknown,
    Consistent,
    UpgradeRequired,
    PendingRemovalDecision,
    Diverged,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRemovalFacts {
    pub removal_event_id: MembershipEventId,
    pub proposed_by_device_id: DeviceId,
    pub target_device_ids: Vec<DeviceId>,
    target_members: BTreeSet<MemberInstanceId>,
}

impl PendingRemovalFacts {
    pub fn new(
        removal_event_id: MembershipEventId,
        proposed_by_device_id: DeviceId,
        target_device_ids: Vec<DeviceId>,
        target_members: BTreeSet<MemberInstanceId>,
    ) -> Self {
        Self {
            removal_event_id,
            proposed_by_device_id,
            target_device_ids,
            target_members,
        }
    }

    pub fn includes_member(&self, member: MemberInstanceId) -> bool {
        self.target_members.contains(&member)
    }
}

/// Versioned reconciliation messages carried on the authenticated member channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MembershipHistoryMessage {
    HistoryPageV2(MembershipHistoryPageV2),
    AckV2(MembershipHistoryV2Ack),
}
