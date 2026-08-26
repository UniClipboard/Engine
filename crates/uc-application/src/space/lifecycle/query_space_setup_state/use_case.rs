use std::sync::Arc;

use uc_core::ports::SettingsPort;

use super::{CurrentInvitation, QuerySetupStateError, SetupStateView};
use crate::space::admission::InMemoryPairingInvitationHolder;
use crate::space::lifecycle::CurrentSpaceIdentityPort;
use crate::space::membership::RePairingState;

/// 组合当前 Space 身份、待处理邀请、设备名称和重新配对要求，生成一次产品查询结果。
pub(crate) struct QuerySpaceSetupStateUseCase {
    current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
    invitation_holder: Arc<InMemoryPairingInvitationHolder>,
    settings: Arc<dyn SettingsPort>,
    re_pairing_state: Arc<RePairingState>,
}

impl QuerySpaceSetupStateUseCase {
    pub(crate) fn new(
        current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
        invitation_holder: Arc<InMemoryPairingInvitationHolder>,
        settings: Arc<dyn SettingsPort>,
        re_pairing_state: Arc<RePairingState>,
    ) -> Self {
        Self {
            current_space_identity,
            invitation_holder,
            settings,
            re_pairing_state,
        }
    }

    pub(crate) async fn execute(&self) -> Result<SetupStateView, QuerySetupStateError> {
        let current_space_id = self
            .current_space_identity
            .current_space_id()
            .await
            .map_err(|error| QuerySetupStateError::StorageFailed(error.to_string()))?;
        let current_invitation = self
            .invitation_holder
            .snapshot_earliest()
            .await
            .map(|(code, expires_at)| CurrentInvitation { code, expires_at });
        let settings = self
            .settings
            .load()
            .await
            .map_err(|error| QuerySetupStateError::StorageFailed(error.to_string()))?;
        let re_pairing_required = self
            .re_pairing_state
            .is_required()
            .await
            .map_err(|error| QuerySetupStateError::StorageFailed(error.to_string()))?;

        Ok(SetupStateView {
            has_completed: current_space_id.is_some(),
            space_id: current_space_id,
            current_invitation,
            device_name: settings.general.device_name,
            re_pairing_required,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use chrono::{Duration, TimeZone, Utc};
    use uc_core::ids::{DeviceId, SpaceId};
    use uc_core::pairing::invitation::PairingInvitation;
    use uc_core::pairing::InvitationCode;
    use uc_core::settings::model::Settings;

    use super::*;
    use crate::space::lifecycle::{CurrentSpaceIdentityError, CurrentSpaceIdentityPort};
    use crate::space::membership::{RePairingStateError, RePairingStateStorePort};

    struct FixedCurrentSpace(Option<SpaceId>);

    #[async_trait]
    impl CurrentSpaceIdentityPort for FixedCurrentSpace {
        async fn current_space_id(&self) -> Result<Option<SpaceId>, CurrentSpaceIdentityError> {
            Ok(self.0.clone())
        }
    }

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

    struct InMemoryRePairingState(Mutex<bool>);

    #[async_trait]
    impl RePairingStateStorePort for InMemoryRePairingState {
        async fn is_required(&self) -> Result<bool, RePairingStateError> {
            Ok(*self.0.lock().unwrap())
        }

        async fn set_required(&self, required: bool) -> Result<(), RePairingStateError> {
            *self.0.lock().unwrap() = required;
            Ok(())
        }
    }

    fn use_case(
        space_id: Option<SpaceId>,
        device_name: Option<&str>,
        re_pairing_required: bool,
    ) -> (
        QuerySpaceSetupStateUseCase,
        Arc<InMemoryPairingInvitationHolder>,
    ) {
        let holder = Arc::new(InMemoryPairingInvitationHolder::new());
        let mut settings = Settings::default();
        settings.general.device_name = device_name.map(str::to_owned);
        (
            QuerySpaceSetupStateUseCase::new(
                Arc::new(FixedCurrentSpace(space_id)),
                Arc::clone(&holder),
                Arc::new(InMemorySettings(Mutex::new(settings))),
                Arc::new(RePairingState::new(Arc::new(InMemoryRePairingState(
                    Mutex::new(re_pairing_required),
                )))),
            ),
            holder,
        )
    }

    #[tokio::test]
    async fn fresh_install_returns_empty_setup_state() {
        let (query, _) = use_case(None, None, false);

        let state = query.execute().await.unwrap();

        assert_eq!(
            state,
            SetupStateView {
                has_completed: false,
                space_id: None,
                current_invitation: None,
                device_name: None,
                re_pairing_required: false,
            }
        );
    }

    #[tokio::test]
    async fn completed_setup_returns_identity_name_and_re_pairing_requirement() {
        let space_id = SpaceId::from("space-a");
        let (query, _) = use_case(Some(space_id.clone()), Some("MacBook"), true);

        let state = query.execute().await.unwrap();

        assert!(state.has_completed);
        assert_eq!(state.space_id, Some(space_id));
        assert_eq!(state.device_name.as_deref(), Some("MacBook"));
        assert!(state.re_pairing_required);
    }

    #[tokio::test]
    async fn pending_invitation_is_included_in_setup_state() {
        let (query, holder) = use_case(None, None, false);
        let issued_at = Utc.with_ymd_and_hms(2026, 8, 23, 10, 0, 0).unwrap();
        let expires_at = issued_at + Duration::minutes(5);
        let (invitation, _) = PairingInvitation::issue(
            InvitationCode::new("ABCD-1234"),
            issued_at,
            expires_at,
            DeviceId::new("device-a"),
            0,
        );
        holder.insert(invitation).await;

        let state = query.execute().await.unwrap();

        assert_eq!(
            state.current_invitation,
            Some(CurrentInvitation {
                code: InvitationCode::new("ABCD-1234"),
                expires_at,
            })
        );
    }
}
