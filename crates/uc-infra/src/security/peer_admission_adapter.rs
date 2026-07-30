use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    PeerAdmissionError, PeerAdmissionPort, RevocationRepositoryPort, SpaceSecurityMode,
};

use super::mls_group::{MlsClientState, MlsGroupEngine};
use super::session::InMemorySession;

/// Resolves current peer authorization from the active space protection state.
///
/// Legacy spaces retain roster-based authorization until an explicit bootstrap
/// migrates them. Once a space is Ready, only its current MLS group authorizes
/// inbound peers; a malformed Ready state fails closed.
pub struct MlsPeerAdmissionAdapter {
    session: Arc<InMemorySession>,
    repository: Arc<dyn RevocationRepositoryPort>,
}

impl MlsPeerAdmissionAdapter {
    pub fn new(
        session: Arc<InMemorySession>,
        repository: Arc<dyn RevocationRepositoryPort>,
    ) -> Self {
        Self {
            session,
            repository,
        }
    }
}

#[async_trait]
impl PeerAdmissionPort for MlsPeerAdmissionAdapter {
    async fn is_admitted(&self, device_id: &DeviceId) -> Result<bool, PeerAdmissionError> {
        let space_id = self
            .session
            .current_space_id()
            .map_err(|_| PeerAdmissionError::Unavailable)?;
        let material = self
            .repository
            .load_space_material(&space_id)
            .await
            .map_err(|_| PeerAdmissionError::Internal("failed to load space protection".into()))?;

        let Some(material) = material else {
            return Ok(true);
        };
        match material.state().mode() {
            SpaceSecurityMode::Legacy => Ok(true),
            SpaceSecurityMode::Migrating => Ok(false),
            SpaceSecurityMode::Ready if material.group_state().is_empty() => Ok(false),
            SpaceSecurityMode::Ready => MlsGroupEngine::contains_active_member(
                &MlsClientState::from_bytes(material.group_state().to_vec()),
                device_id.as_str().as_bytes(),
            )
            .map_err(|_| PeerAdmissionError::InvalidState),
        }
    }
}
