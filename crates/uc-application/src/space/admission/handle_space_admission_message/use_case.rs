use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use super::{
    AuthenticatedSpaceAdmissionMessage, HandleSpaceAdmissionMessageError,
    HandleSpaceAdmissionMessagePort, PrepareSpaceAdmissionMessagePort,
    PreparedSpaceAdmissionMessage, SpaceAdmissionPreparationContext,
};
use crate::space::admission::invitation::InMemoryPairingInvitationHolder;
use crate::space::membership_ledger::{MembershipLedger, MembershipLedgerError};
use crate::space::remove_space_member::WakeSpaceMembershipMaintenancePort;
use uc_core::ports::ClockPort;

pub(crate) struct HandleSpaceAdmissionMessageUseCase {
    ledger: Arc<MembershipLedger>,
    preparation: Arc<dyn PrepareSpaceAdmissionMessagePort>,
    maintenance: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    invitations: Arc<InMemoryPairingInvitationHolder>,
    clock: Arc<dyn ClockPort>,
    execution_lock: tokio::sync::Mutex<()>,
}

#[async_trait::async_trait]
impl HandleSpaceAdmissionMessagePort for HandleSpaceAdmissionMessageUseCase {
    async fn handle_space_admission_message(
        &self,
        message: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<Vec<u8>, HandleSpaceAdmissionMessageError> {
        self.execute(message).await
    }
}

impl HandleSpaceAdmissionMessageUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        preparation: Arc<dyn PrepareSpaceAdmissionMessagePort>,
        maintenance: Arc<dyn WakeSpaceMembershipMaintenancePort>,
        invitations: Arc<InMemoryPairingInvitationHolder>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            ledger,
            preparation,
            maintenance,
            invitations,
            clock,
            execution_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) async fn execute(
        &self,
        message: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<Vec<u8>, HandleSpaceAdmissionMessageError> {
        let _guard = self.execution_lock.lock().await;
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        let history = snapshot
            .record()
            .membership_history_v2
            .clone()
            .ok_or(HandleSpaceAdmissionMessageError::RecoveryRequired)?;
        let current_record = snapshot
            .record()
            .admission_records
            .get(message.attempt_id.as_bytes())
            .cloned();
        let invitation = if current_record.is_none() {
            let code = message
                .invitation_code
                .as_ref()
                .ok_or(HandleSpaceAdmissionMessageError::Invalid)?;
            let now = DateTime::<Utc>::from_timestamp_millis(self.clock.now_ms())
                .ok_or(HandleSpaceAdmissionMessageError::Unavailable)?;
            let invitation = self
                .invitations
                .inspect_matching(code, now)
                .await
                .map_err(|_| HandleSpaceAdmissionMessageError::Invalid)?;
            if invitation.admission_generation() != snapshot.record().revision {
                return Err(HandleSpaceAdmissionMessageError::StateChanged);
            }
            Some((
                code.clone(),
                now,
                invitation_digest(code.as_str()),
                invitation.admission_generation(),
            ))
        } else {
            None
        };
        let context = SpaceAdmissionPreparationContext {
            revision: snapshot.record().revision,
            invitation_generation: invitation.as_ref().map(|(_, _, _, generation)| *generation),
            membership_history_v2: history.clone(),
            current_record,
        };
        match self.preparation.prepare(&message, &context).await? {
            PreparedSpaceAdmissionMessage::NoChange { reply } => Ok(reply),
            PreparedSpaceAdmissionMessage::Commit(commit) => {
                if commit.record.record_id != message.attempt_id
                    || commit.relationship.peer_device_id != message.source_device_id
                    || !commit
                        .effect
                        .affected_device_ids
                        .contains(&message.source_device_id)
                {
                    return Err(HandleSpaceAdmissionMessageError::Invalid);
                }
                let reply = commit.reply.clone();
                let expected_record_version = commit.expected_record_version;
                let invitation = if expected_record_version.is_none() {
                    let invitation = invitation.ok_or(HandleSpaceAdmissionMessageError::Invalid)?;
                    if commit.invitation_generation != Some(invitation.3) {
                        return Err(HandleSpaceAdmissionMessageError::StateChanged);
                    }
                    Some(invitation)
                } else {
                    if commit.invitation_generation.is_some() {
                        return Err(HandleSpaceAdmissionMessageError::Invalid);
                    }
                    None
                };
                self.ledger
                    .commit_inbound_admission(
                        context.revision,
                        history,
                        expected_record_version,
                        commit.record,
                        commit.membership_history_v2,
                        commit.relationship,
                        commit.effect,
                        invitation.as_ref().map(|(_, _, digest, _)| *digest),
                    )
                    .await
                    .map_err(map_ledger_error)?;
                if let Some((code, now, _, _)) = invitation {
                    self.invitations
                        .take_matching(&code, now)
                        .await
                        .map_err(|_| HandleSpaceAdmissionMessageError::RecoveryRequired)?;
                }
                self.maintenance.wake();
                Ok(reply)
            }
        }
    }
}

fn invitation_digest(code: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/admission-invitation-claim/v1\0");
    hasher.update((code.len() as u64).to_be_bytes());
    hasher.update(code.as_bytes());
    hasher.finalize().into()
}

fn map_ledger_error(error: MembershipLedgerError) -> HandleSpaceAdmissionMessageError {
    match error {
        MembershipLedgerError::Locked => HandleSpaceAdmissionMessageError::Locked,
        MembershipLedgerError::Conflict => HandleSpaceAdmissionMessageError::StateChanged,
        MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
            HandleSpaceAdmissionMessageError::RecoveryRequired
        }
        MembershipLedgerError::Unavailable => HandleSpaceAdmissionMessageError::Unavailable,
    }
}
