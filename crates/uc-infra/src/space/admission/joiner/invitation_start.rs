use async_trait::async_trait;
use rand::RngCore;
use serde::Serialize;
use uc_application::deps::{
    PrepareJoinerInvitationError, PrepareJoinerInvitationPort, PreparedJoinerInvitation,
};
use uc_application::facade::JoinSpaceInput;
use uc_core::membership::{
    AdmissionJoinerStartContext, AdmissionShortInvitationCode, JoinId, SpaceAdmissionId,
};

use crate::space::decode_invitation_entry;

pub struct DefaultJoinerInvitationPreparation;

#[derive(Serialize)]
struct JoinerStartContextV1<'a> {
    format_version: u16,
    passphrase: &'a [u8],
    preserve_unreadable_history: bool,
}

#[async_trait]
impl PrepareJoinerInvitationPort for DefaultJoinerInvitationPreparation {
    async fn prepare(
        &self,
        input: &JoinSpaceInput,
    ) -> Result<PreparedJoinerInvitation, PrepareJoinerInvitationError> {
        match decode_invitation_entry(
            input.invitation_code.as_str(),
            chrono::Utc::now().timestamp_millis(),
        ) {
            Ok(Some(_)) => return Ok(PreparedJoinerInvitation::Full),
            Ok(None) => {}
            Err(_) => return Err(PrepareJoinerInvitationError::Invalid),
        }

        let short_code = AdmissionShortInvitationCode::from_bytes(
            input.invitation_code.as_str().as_bytes().to_vec(),
        )
        .map_err(|_| PrepareJoinerInvitationError::Invalid)?;
        let context = postcard::to_stdvec(&JoinerStartContextV1 {
            format_version: 1,
            passphrase: input.passphrase.expose().as_bytes(),
            preserve_unreadable_history: input.preserve_unreadable_history,
        })
        .map_err(|error| PrepareJoinerInvitationError::unavailable(anyhow::Error::new(error)))?;
        let start_context = AdmissionJoinerStartContext::from_bytes(context).map_err(|error| {
            PrepareJoinerInvitationError::unavailable(anyhow::Error::new(error))
        })?;

        Ok(PreparedJoinerInvitation::short(
            mint_admission_id(),
            mint_join_id(),
            start_context,
            short_code,
        ))
    }
}

fn mint_admission_id() -> SpaceAdmissionId {
    loop {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        if let Some(id) = SpaceAdmissionId::from_bytes(bytes) {
            return id;
        }
    }
}

fn mint_join_id() -> JoinId {
    loop {
        let mut bytes = [0u8; 16];
        rand::rng().fill_bytes(&mut bytes);
        if let Some(id) = JoinId::from_bytes(bytes) {
            return id;
        }
    }
}
