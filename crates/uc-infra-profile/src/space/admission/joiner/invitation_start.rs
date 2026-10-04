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
use uc_observability_contract::diagnostics::connectivity::{observe_local_result, LocalWorkStep};
use uc_sync_protocol::full_invitation::decode_invitation_entry;

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
        observe_local_result(LocalWorkStep::JoinerPrepareInvitation, async {
            match decode_invitation_entry(
                input.invitation_code.as_str(),
                chrono::Utc::now().timestamp_millis(),
            ) {
                Ok(Some(_)) => return Ok(PreparedJoinerInvitation::Full),
                Ok(None) => {}
                Err(_) => return Err(PrepareJoinerInvitationError::invalid()),
            }

            let short_code = AdmissionShortInvitationCode::from_bytes(
                input.invitation_code.as_str().as_bytes().to_vec(),
            )
            .map_err(PrepareJoinerInvitationError::invalid_from)?;
            let context = postcard::to_stdvec(&JoinerStartContextV1 {
                format_version: 1,
                passphrase: input.passphrase.expose().as_bytes(),
                preserve_unreadable_history: input.preserve_unreadable_history,
            })
            .map_err(|error| {
                PrepareJoinerInvitationError::unavailable(anyhow::Error::new(error))
            })?;
            let start_context =
                AdmissionJoinerStartContext::from_bytes(context).map_err(|error| {
                    PrepareJoinerInvitationError::unavailable(anyhow::Error::new(error))
                })?;

            let prepared = PreparedJoinerInvitation::short(
                mint_admission_id(),
                mint_join_id(),
                start_context,
                short_code,
            );
            Ok(prepared)
        })
        .await
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

#[cfg(test)]
mod tests {
    use uc_core::crypto::domain::Passphrase;
    use uc_core::pairing::InvitationCode;
    use uc_sync_protocol::full_invitation::encode_full_invitation;

    use super::*;

    fn invitation_id() -> uc_core::membership::InvitationId {
        uc_core::membership::InvitationId::from_bytes([0x51; 32]).expect("valid invitation id")
    }

    #[tokio::test]
    async fn production_joiner_preparation_separates_full_and_short_entries() {
        let adapter = DefaultJoinerInvitationPreparation;
        let full = encode_full_invitation(invitation_id(), b"route", 1_900_000_000_000)
            .expect("valid full invitation");
        let prepared = adapter
            .prepare(&JoinSpaceInput {
                invitation_code: InvitationCode::new(full.as_str()),
                device_name: None,
                passphrase: Passphrase::new("secret-passphrase"),
                preserve_unreadable_history: false,
            })
            .await
            .expect("full invitation should prepare locally");
        assert!(matches!(prepared, PreparedJoinerInvitation::Full));

        let prepared = adapter
            .prepare(&JoinSpaceInput {
                invitation_code: InvitationCode::new("ABCD-1234"),
                device_name: None,
                passphrase: Passphrase::new("secret-passphrase"),
                preserve_unreadable_history: true,
            })
            .await
            .expect("short code should prepare a durable context");
        assert!(matches!(
            prepared,
            PreparedJoinerInvitation::Short {
                short_code,
                start_context,
                ..
            } if short_code.as_bytes() == b"ABCD-1234"
                && !start_context.as_bytes().is_empty()
                && !format!("{start_context:?}").contains("secret-passphrase")
        ));
    }
}
