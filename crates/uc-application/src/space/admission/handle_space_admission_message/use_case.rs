use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use super::error::{AcceptAdmissionError, LoadMemberAdmissionError};
use super::ports::{ConsumedInvitation, InboundAdmissionStatePort};
use super::{
    AuthenticatedSpaceAdmissionMessage, HandleSpaceAdmissionMessageError,
    HandleSpaceAdmissionMessagePort, PrepareSpaceAdmissionMessagePort,
    PreparedMemberAdmissionActivation, PreparedSpaceAdmissionMessage,
};
use crate::space::admission::invitation::InMemoryPairingInvitationHolder;
use crate::space::membership::WakeSpaceMembershipMaintenancePort;
use uc_core::ports::ClockPort;

pub(crate) struct HandleSpaceAdmissionMessageUseCase {
    preparation: Arc<dyn PrepareSpaceAdmissionMessagePort>,
    admission_state: Arc<dyn InboundAdmissionStatePort>,
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
        preparation: Arc<dyn PrepareSpaceAdmissionMessagePort>,
        admission_state: Arc<dyn InboundAdmissionStatePort>,
        maintenance: Arc<dyn WakeSpaceMembershipMaintenancePort>,
        invitations: Arc<InMemoryPairingInvitationHolder>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            preparation,
            admission_state,
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
        let state = self
            .admission_state
            .load(message.record_id)
            .await
            .map_err(map_load_member_admission_error)?;
        let invitation_required = !state.has_current_record();
        let invitation = if invitation_required {
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
            if invitation.admission_generation() != state.required_invitation_generation() {
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

        let context =
            state.preparation_context(invitation.as_ref().map(|(_, _, _, generation)| *generation));
        match self.preparation.prepare(&message, &context).await? {
            PreparedSpaceAdmissionMessage::NoChange { reply } => Ok(reply),
            PreparedSpaceAdmissionMessage::Commit(commit) => {
                if commit.record.record_id != message.record_id
                    || commit.relationship.peer_device_id != message.source_device_id
                    || !commit
                        .effect
                        .affected_device_ids
                        .contains(&message.source_device_id)
                {
                    return Err(HandleSpaceAdmissionMessageError::Invalid);
                }
                let reply = commit.reply.clone();
                let invitation = if invitation_required {
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
                self.admission_state
                    .accept(
                        state.into_commit_token(),
                        PreparedMemberAdmissionActivation::new(
                            commit.record,
                            commit.membership_history_v2,
                            commit.relationship,
                            commit.effect,
                        ),
                        invitation
                            .as_ref()
                            .map(|(_, _, digest, _)| ConsumedInvitation::new(*digest)),
                    )
                    .await
                    .map_err(map_accept_admission_error)?;
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

fn map_load_member_admission_error(
    error: LoadMemberAdmissionError,
) -> HandleSpaceAdmissionMessageError {
    match error {
        LoadMemberAdmissionError::Locked => HandleSpaceAdmissionMessageError::Locked,
        LoadMemberAdmissionError::RecoveryRequired => {
            HandleSpaceAdmissionMessageError::RecoveryRequired
        }
        LoadMemberAdmissionError::Unavailable => HandleSpaceAdmissionMessageError::Unavailable,
    }
}

fn map_accept_admission_error(error: AcceptAdmissionError) -> HandleSpaceAdmissionMessageError {
    match error {
        AcceptAdmissionError::Locked => HandleSpaceAdmissionMessageError::Locked,
        AcceptAdmissionError::StateChanged => HandleSpaceAdmissionMessageError::StateChanged,
        AcceptAdmissionError::RecoveryRequired => {
            HandleSpaceAdmissionMessageError::RecoveryRequired
        }
        AcceptAdmissionError::Unavailable => HandleSpaceAdmissionMessageError::Unavailable,
    }
}
