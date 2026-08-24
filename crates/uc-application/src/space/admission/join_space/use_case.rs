use std::sync::Arc;

use tracing::{info, warn};
use uc_core::crypto::domain::Passphrase;
use uc_core::pairing::InvitationCode;
use uc_core::ports::{PresencePort, SettingsPort};

use super::{JoinSpaceError, JoinSpaceInput, JoinSpaceResult};
use crate::facade::space_setup::commands::RedeemPairingInvitationCommand;
use crate::space::admission::joiner::RedeemPairingInvitationUseCase;
use crate::space::admission::query_space_join_status::QuerySpaceJoinStatusUseCase;
use crate::space::admission::{CurrentJoinStatus, SpaceAdmission};
use crate::space::connectivity::reachability::EnsureReachableAllUseCase;

pub(crate) struct JoinSpaceUseCase {
    settings: Arc<dyn SettingsPort>,
    redeem_invitation: Arc<RedeemPairingInvitationUseCase>,
    presence: Arc<dyn PresencePort>,
    ensure_reachable_all: Arc<EnsureReachableAllUseCase>,
    query_join_status: QuerySpaceJoinStatusUseCase,
}

impl JoinSpaceUseCase {
    pub(crate) fn new(
        settings: Arc<dyn SettingsPort>,
        redeem_invitation: Arc<RedeemPairingInvitationUseCase>,
        presence: Arc<dyn PresencePort>,
        ensure_reachable_all: Arc<EnsureReachableAllUseCase>,
        admission: Arc<SpaceAdmission>,
    ) -> Self {
        let query_join_status = QuerySpaceJoinStatusUseCase::new(Arc::clone(
            &admission.membership.deps.admission_attempts,
        ));
        Self {
            settings,
            redeem_invitation,
            presence,
            ensure_reachable_all,
            query_join_status,
        }
    }

    pub(crate) async fn execute(
        &self,
        input: JoinSpaceInput,
    ) -> Result<JoinSpaceResult, JoinSpaceError> {
        persist_device_name(self.settings.as_ref(), input.device_name).await?;
        self.prime_presence().await;
        let outcome = self
            .redeem_invitation
            .execute(RedeemPairingInvitationCommand {
                code: InvitationCode::new(input.invitation_code),
                passphrase: Passphrase::new(input.passphrase),
                preserve_unreadable_history: input.preserve_unreadable_history,
            })
            .await
            .map_err(JoinSpaceError::Admission)?;
        let status = self
            .query_join_status
            .execute()
            .await
            .map_err(|error| JoinSpaceError::SavedState(error.to_string()))?
            .ok_or_else(|| {
                JoinSpaceError::SavedState("successful join was not persisted".to_owned())
            })?;
        ensure_join_result_matches_transition(&status, outcome.requires_session_transition)?;
        Ok(JoinSpaceResult {
            status,
            requires_session_transition: outcome.requires_session_transition,
        })
    }

    async fn prime_presence(&self) {
        self.presence.activate().await;
        match self.ensure_reachable_all.execute().await {
            Ok(report) => info!(
                total = report.total,
                online = report.online,
                offline = report.offline,
                errors = report.errors.len(),
                "presence primed before joining Space"
            ),
            Err(error) => warn!(
                error = %error,
                "presence prime failed before joining Space; admission dial will report the actionable failure"
            ),
        }
    }
}

fn ensure_join_result_matches_transition(
    status: &CurrentJoinStatus,
    requires_session_transition: bool,
) -> Result<(), JoinSpaceError> {
    match (status, requires_session_transition) {
        (CurrentJoinStatus::Active { .. }, false) | (CurrentJoinStatus::Pending { .. }, true) => {
            Ok(())
        }
        _ => Err(JoinSpaceError::SavedState(
            "saved join state does not match its session transition".to_owned(),
        )),
    }
}

async fn persist_device_name(
    settings: &dyn SettingsPort,
    device_name: Option<String>,
) -> Result<(), JoinSpaceError> {
    let Some(device_name) = device_name else {
        return Ok(());
    };
    let device_name = device_name.trim().to_owned();
    if device_name.is_empty() {
        return Err(JoinSpaceError::DeviceNameRequired);
    }

    let mut current = settings
        .load()
        .await
        .map_err(|error| JoinSpaceError::Settings(error.to_string()))?;
    if current.general.device_name.as_deref() == Some(device_name.as_str()) {
        return Ok(());
    }
    current.general.device_name = Some(device_name);
    settings
        .save(&current)
        .await
        .map_err(|error| JoinSpaceError::Settings(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use uc_core::settings::model::Settings;

    use super::*;

    struct InMemorySettings(Mutex<Settings>);

    #[async_trait]
    impl SettingsPort for InMemorySettings {
        async fn load(&self) -> anyhow::Result<Settings> {
            Ok(self.0.lock().unwrap().clone())
        }

        async fn save(&self, settings: &Settings) -> anyhow::Result<()> {
            *self.0.lock().unwrap() = settings.clone();
            Ok(())
        }
    }

    #[tokio::test]
    async fn empty_device_name_is_rejected() {
        let settings = InMemorySettings(Mutex::new(Settings::default()));

        let error = persist_device_name(&settings, Some("  ".to_owned()))
            .await
            .unwrap_err();

        assert!(matches!(error, JoinSpaceError::DeviceNameRequired));
    }

    #[tokio::test]
    async fn device_name_is_trimmed_and_saved_without_overwriting_other_settings() {
        let mut initial = Settings::default();
        initial.general.device_name = Some("Old".to_owned());
        let settings = InMemorySettings(Mutex::new(initial));

        persist_device_name(&settings, Some("  New Device  ".to_owned()))
            .await
            .unwrap();

        assert_eq!(
            settings.0.lock().unwrap().general.device_name.as_deref(),
            Some("New Device")
        );
    }

    #[test]
    fn active_join_does_not_require_session_transition() {
        let status = CurrentJoinStatus::Active {
            join_id: [7; 16],
            joined_space: crate::space::admission::JoinedSpace {
                sponsor_device_id: uc_core::DeviceId::new("sponsor"),
                sponsor_identity_fingerprint:
                    uc_core::security::IdentityFingerprint::from_raw_string("ABCDEFGHIJKLMNOP")
                        .unwrap(),
                space_id: "space".to_owned(),
                self_device_id: uc_core::DeviceId::new("self"),
                self_identity_fingerprint: uc_core::security::IdentityFingerprint::from_raw_string(
                    "QRSTUVWXYZ234567",
                )
                .unwrap(),
                migrated_records: None,
                preserved_unreadable_records: None,
            },
        };

        assert!(ensure_join_result_matches_transition(&status, false).is_ok());
        assert!(ensure_join_result_matches_transition(&status, true).is_err());
    }

    #[test]
    fn pending_join_requires_session_transition() {
        let status = CurrentJoinStatus::Pending {
            join_id: [7; 16],
            target_space_id: Some("space".to_owned()),
            sponsor_device_id: None,
            sponsor_identity_fingerprint: None,
            cancel_requested: false,
        };

        assert!(ensure_join_result_matches_transition(&status, true).is_ok());
        assert!(ensure_join_result_matches_transition(&status, false).is_err());
    }
}
