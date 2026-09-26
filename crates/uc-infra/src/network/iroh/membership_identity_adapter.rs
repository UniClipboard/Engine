use std::sync::Arc;

use async_trait::async_trait;
use iroh::Endpoint;
use uc_core::membership::{
    CurrentMembershipAnnouncementMaterial, CurrentMembershipAnnouncementPort,
    CurrentMembershipIdentity, CurrentMembershipIdentityError, CurrentMembershipIdentityPort,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::{DeviceIdentityPort, SettingsPort};

use crate::space::InMemorySession;

use super::persistable_addr::to_persistable_addr;

/// 本机当前成员身份与公告资料：活动 Space、设备名、由 iroh 端点公钥派生的身份指纹和传输地址。
pub struct IrohMembershipIdentityAdapter {
    endpoint: Arc<Endpoint>,
    session: Arc<InMemorySession>,
    device_identity: Arc<dyn DeviceIdentityPort>,
    settings: Arc<dyn SettingsPort>,
    fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
}

impl IrohMembershipIdentityAdapter {
    pub(crate) fn new(
        endpoint: Arc<Endpoint>,
        session: Arc<InMemorySession>,
        device_identity: Arc<dyn DeviceIdentityPort>,
        settings: Arc<dyn SettingsPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    ) -> Self {
        Self {
            endpoint,
            session,
            device_identity,
            settings,
            fingerprint_factory,
        }
    }
}

#[async_trait]
impl CurrentMembershipIdentityPort for IrohMembershipIdentityAdapter {
    async fn current_membership_identity(
        &self,
    ) -> Result<CurrentMembershipIdentity, CurrentMembershipIdentityError> {
        let space_id = self
            .session
            .current_space_id()
            .map_err(CurrentMembershipIdentityError::unavailable_from)?;
        let settings = self.settings.load().await.map_err(|error| {
            CurrentMembershipIdentityError::LoadFailed {
                source: Some(error.context("load settings").into()),
            }
        })?;
        let device_name = settings
            .general
            .device_name
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(CurrentMembershipIdentityError::unavailable)?;
        let identity_fingerprint = self
            .fingerprint_factory
            .from_public_key(self.endpoint.id().as_bytes())
            .map_err(|error| CurrentMembershipIdentityError::LoadFailed {
                source: Some(error.context("derive local fingerprint").into()),
            })?;

        Ok(CurrentMembershipIdentity {
            space_id,
            device_id: self.device_identity.current_device_id(),
            device_name,
            identity_fingerprint,
        })
    }
}

#[async_trait]
impl CurrentMembershipAnnouncementPort for IrohMembershipIdentityAdapter {
    async fn current_announcement_material(
        &self,
    ) -> Result<CurrentMembershipAnnouncementMaterial, CurrentMembershipIdentityError> {
        let identity = self.current_membership_identity().await?;
        let transport_address_blob =
            postcard::to_stdvec(&to_persistable_addr(self.endpoint.addr()))
                .map_err(CurrentMembershipIdentityError::load_failed_from)?;
        Ok(CurrentMembershipAnnouncementMaterial {
            space_id: identity.space_id,
            device_id: identity.device_id,
            device_name: identity.device_name,
            identity_fingerprint: identity.identity_fingerprint,
            transport_public_key: self.endpoint.id().as_bytes().to_vec(),
            transport_address_blob,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use iroh::{Endpoint, RelayMode, SecretKey};
    use uc_core::ids::{DeviceId, SpaceId};
    use uc_core::membership::{
        CurrentMembershipAnnouncementPort, CurrentMembershipIdentityError,
        CurrentMembershipIdentityPort,
    };
    use uc_core::ports::security::IdentityFingerprintFactoryPort;
    use uc_core::ports::{DeviceIdentityPort, SettingsPort};
    use uc_core::settings::model::Settings;

    use super::IrohMembershipIdentityAdapter;
    use crate::security::{MasterKey, Sha256IdentityFingerprintFactory};
    use crate::space::InMemorySession;

    struct FixedDeviceIdentity(DeviceId);

    impl DeviceIdentityPort for FixedDeviceIdentity {
        fn current_device_id(&self) -> DeviceId {
            self.0
        }
    }

    struct FixedSettings(Settings);

    #[async_trait]
    impl SettingsPort for FixedSettings {
        async fn load(&self) -> anyhow::Result<Settings> {
            Ok(self.0.clone())
        }

        async fn save(&self, _settings: &Settings) -> anyhow::Result<()> {
            Ok(())
        }
    }

    async fn endpoint(seed: [u8; 32]) -> Arc<Endpoint> {
        Arc::new(
            Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(SecretKey::from_bytes(&seed))
                .relay_mode(RelayMode::Disabled)
                .bind()
                .await
                .unwrap(),
        )
    }

    fn membership_identity_adapter(
        endpoint: Arc<Endpoint>,
        session: Arc<InMemorySession>,
        device_name: Option<&str>,
    ) -> IrohMembershipIdentityAdapter {
        let mut settings = Settings::default();
        settings.general.device_name = device_name.map(str::to_owned);
        IrohMembershipIdentityAdapter::new(
            endpoint,
            session,
            Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            Arc::new(FixedSettings(settings)),
            Arc::new(Sha256IdentityFingerprintFactory),
        )
    }

    fn ready_session(key_byte: u8) -> Arc<InMemorySession> {
        let session = Arc::new(InMemorySession::new());
        session.set_master_key_for_space(
            SpaceId::from("space-a"),
            MasterKey::from_bytes(&[key_byte; 32]).unwrap(),
        );
        session
    }

    #[tokio::test]
    async fn current_membership_identity_is_unavailable_while_space_is_locked() {
        let endpoint = endpoint([0x41; 32]).await;
        let adapter = membership_identity_adapter(
            Arc::clone(&endpoint),
            Arc::new(InMemorySession::new()),
            Some("Device A"),
        );

        let result = adapter.current_membership_identity().await;

        assert!(matches!(
            result,
            Err(CurrentMembershipIdentityError::Unavailable { .. })
        ));
        endpoint.close().await;
    }

    #[tokio::test]
    async fn current_membership_identity_requires_a_device_name() {
        let endpoint = endpoint([0x42; 32]).await;
        let adapter = membership_identity_adapter(Arc::clone(&endpoint), ready_session(0x51), None);

        let result = adapter.current_membership_identity().await;

        assert!(matches!(
            result,
            Err(CurrentMembershipIdentityError::Unavailable { .. })
        ));
        endpoint.close().await;
    }

    #[tokio::test]
    async fn current_membership_identity_uses_the_active_space_and_endpoint_key() {
        let endpoint = endpoint([0x43; 32]).await;
        let adapter = membership_identity_adapter(
            Arc::clone(&endpoint),
            ready_session(0x52),
            Some("Device A"),
        );

        let identity = adapter.current_membership_identity().await.unwrap();

        assert_eq!(identity.space_id, SpaceId::from("space-a"));
        assert_eq!(identity.device_id, DeviceId::new("device-a"));
        assert_eq!(identity.device_name, "Device A");
        assert_eq!(
            identity.identity_fingerprint,
            Sha256IdentityFingerprintFactory
                .from_public_key(endpoint.id().as_bytes())
                .unwrap()
        );
        endpoint.close().await;
    }

    #[tokio::test]
    async fn announcement_material_carries_the_identity_and_endpoint_transport_key() {
        let endpoint = endpoint([0x44; 32]).await;
        let adapter = membership_identity_adapter(
            Arc::clone(&endpoint),
            ready_session(0x53),
            Some("Device A"),
        );

        let identity = adapter.current_membership_identity().await.unwrap();
        let material = adapter.current_announcement_material().await.unwrap();

        assert_eq!(material.space_id, identity.space_id);
        assert_eq!(material.device_id, identity.device_id);
        assert_eq!(material.device_name, identity.device_name);
        assert_eq!(material.identity_fingerprint, identity.identity_fingerprint);
        assert_eq!(
            material.transport_public_key,
            endpoint.id().as_bytes().to_vec()
        );
        assert!(!material.transport_address_blob.is_empty());
        endpoint.close().await;
    }
}
