use std::net::IpAddr;
use std::sync::Arc;

use tokio::sync::broadcast;

use super::space_setup::{
    CancelInvitationError, IssuePairingInvitationError, IssuePairingInvitationResult,
    PairingInvitationAddressCandidate, PairingOutcome, SwitchSpaceError, SwitchSpaceInput,
    SwitchSpaceResult, UnreadableHistoryPolicy,
};
use super::{
    GeneralSettingsPatch, RedeemPairingInvitationError, RedeemPairingInvitationInput,
    RedeemPairingInvitationResult, SettingsFacade, SettingsPatch, SpaceFacade,
};

pub struct JoinSpaceInput {
    pub invitation_code: String,
    pub device_name: Option<String>,
    pub passphrase: String,
    pub preserve_unreadable_history: bool,
}

pub enum JoinSpaceResult {
    Fresh(RedeemPairingInvitationResult),
    Switched(SwitchSpaceResult),
}

#[derive(Debug, thiserror::Error)]
pub enum JoinSpaceError {
    #[error("device name is required")]
    DeviceNameRequired,
    #[error("failed to save device name: {0}")]
    Settings(String),
    #[error("failed to query setup state: {0}")]
    Setup(String),
    #[error(transparent)]
    Fresh(#[from] RedeemPairingInvitationError),
    #[error(transparent)]
    Switch(#[from] SwitchSpaceError),
    #[error("failed to restore space activities: {0}")]
    Activity(String),
}

pub(crate) struct SpaceTransitionCoordinator {
    setup: Arc<SpaceFacade>,
}

impl SpaceTransitionCoordinator {
    fn new(setup: Arc<SpaceFacade>) -> Self {
        Self { setup }
    }

    async fn switch_space(
        &self,
        invitation_code: String,
        passphrase: String,
        preserve_unreadable_history: bool,
    ) -> Result<SwitchSpaceResult, SwitchSpaceError> {
        self.setup
            .switch_space(SwitchSpaceInput {
                code: invitation_code,
                new_passphrase: passphrase,
                unreadable_history_policy: if preserve_unreadable_history {
                    UnreadableHistoryPolicy::PreserveAndContinue
                } else {
                    UnreadableHistoryPolicy::Reject
                },
            })
            .await
    }
}

pub(crate) struct SpaceAdmissionCoordinator {
    setup: Arc<SpaceFacade>,
    settings: Arc<SettingsFacade>,
    transition: SpaceTransitionCoordinator,
}

impl SpaceAdmissionCoordinator {
    pub(crate) fn new(setup: Arc<SpaceFacade>, settings: Arc<SettingsFacade>) -> Self {
        Self {
            transition: SpaceTransitionCoordinator::new(Arc::clone(&setup)),
            setup,
            settings,
        }
    }

    pub(crate) async fn join_space(
        &self,
        input: JoinSpaceInput,
    ) -> Result<JoinSpaceResult, JoinSpaceError> {
        if let Some(device_name) = input.device_name {
            self.save_device_name(device_name).await?;
        }

        let setup = self
            .setup
            .query_setup_state()
            .await
            .map_err(|error| JoinSpaceError::Setup(error.to_string()))?;
        if setup.has_completed {
            return self
                .transition
                .switch_space(
                    input.invitation_code,
                    input.passphrase,
                    input.preserve_unreadable_history,
                )
                .await
                .map(JoinSpaceResult::Switched)
                .map_err(JoinSpaceError::Switch);
        }

        self.setup
            .redeem_pairing_invitation(RedeemPairingInvitationInput {
                code: input.invitation_code,
                passphrase: input.passphrase,
            })
            .await
            .map(JoinSpaceResult::Fresh)
            .map_err(JoinSpaceError::Fresh)
    }

    pub(crate) async fn issue_invitation(
        &self,
    ) -> Result<IssuePairingInvitationResult, IssuePairingInvitationError> {
        self.setup.issue_pairing_invitation().await
    }

    pub(crate) async fn issue_invitation_for_address(
        &self,
        selected_ip: IpAddr,
    ) -> Result<IssuePairingInvitationResult, IssuePairingInvitationError> {
        self.setup
            .issue_pairing_invitation_for_address(selected_ip)
            .await
    }

    pub(crate) async fn list_invitation_addresses(
        &self,
    ) -> Result<Vec<PairingInvitationAddressCandidate>, IssuePairingInvitationError> {
        self.setup.list_pairing_invitation_addresses().await
    }

    pub(crate) async fn redeem_invitation(
        &self,
        input: RedeemPairingInvitationInput,
    ) -> Result<RedeemPairingInvitationResult, RedeemPairingInvitationError> {
        self.setup.redeem_pairing_invitation(input).await
    }

    pub(crate) async fn cancel_invitation(&self) -> Result<(), CancelInvitationError> {
        self.setup.cancel_invitation().await
    }

    pub(crate) async fn switch_space(
        &self,
        input: SwitchSpaceInput,
    ) -> Result<SwitchSpaceResult, SwitchSpaceError> {
        self.setup.switch_space(input).await
    }

    pub(crate) fn subscribe_pairing_completion(&self) -> broadcast::Receiver<PairingOutcome> {
        self.setup.subscribe_pairing_completion()
    }

    async fn save_device_name(&self, device_name: String) -> Result<(), JoinSpaceError> {
        let device_name = device_name.trim().to_owned();
        if device_name.is_empty() {
            return Err(JoinSpaceError::DeviceNameRequired);
        }
        let current = self
            .settings
            .get()
            .await
            .map_err(|error| JoinSpaceError::Settings(error.to_string()))?;
        if current.general.device_name.as_deref() == Some(device_name.as_str()) {
            return Ok(());
        }
        self.settings
            .update(SettingsPatch {
                general: Some(GeneralSettingsPatch {
                    device_name: Some(Some(device_name)),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .map_err(|error| JoinSpaceError::Settings(error.to_string()))?;
        Ok(())
    }
}
