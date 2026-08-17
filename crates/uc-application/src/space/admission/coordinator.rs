use std::net::IpAddr;
use std::sync::Arc;

use crate::facade::settings::{GeneralSettingsPatch, SettingsFacade, SettingsPatch};
use crate::facade::space_setup::{
    CancelInvitationError, IssuePairingInvitationError, IssuePairingInvitationResult,
    PairingInvitationAddressCandidate, RedeemPairingInvitationError, RedeemPairingInvitationInput,
    RedeemPairingInvitationResult, SpaceFacade,
};

pub struct JoinSpaceInput {
    pub invitation_code: String,
    pub device_name: Option<String>,
    pub passphrase: String,
    pub preserve_unreadable_history: bool,
}

pub type JoinSpaceResult = RedeemPairingInvitationResult;

#[derive(Debug, thiserror::Error)]
pub enum JoinSpaceError {
    #[error("device name is required")]
    DeviceNameRequired,
    #[error("failed to save device name: {0}")]
    Settings(String),
    #[error(transparent)]
    Admission(#[from] RedeemPairingInvitationError),
}

pub(crate) struct SpaceAdmissionCoordinator {
    setup: Arc<SpaceFacade>,
    settings: Arc<SettingsFacade>,
}

impl SpaceAdmissionCoordinator {
    pub(crate) fn new(setup: Arc<SpaceFacade>, settings: Arc<SettingsFacade>) -> Self {
        Self { setup, settings }
    }

    pub(crate) async fn join_space(
        &self,
        input: JoinSpaceInput,
    ) -> Result<JoinSpaceResult, JoinSpaceError> {
        if let Some(device_name) = input.device_name {
            self.save_device_name(device_name).await?;
        }

        self.setup
            .redeem_pairing_invitation(RedeemPairingInvitationInput {
                code: input.invitation_code,
                passphrase: input.passphrase,
                preserve_unreadable_history: input.preserve_unreadable_history,
            })
            .await
            .map_err(JoinSpaceError::Admission)
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
