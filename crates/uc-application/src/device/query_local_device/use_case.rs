use std::sync::Arc;

use uc_core::ports::{DeviceIdentityPort, SettingsPort};

use super::LocalDeviceInfo;

const DEFAULT_DEVICE_NAME: &str = "Uniclipboard Device";

pub struct QueryLocalDeviceUseCase {
    device_identity: Arc<dyn DeviceIdentityPort>,
    settings: Arc<dyn SettingsPort>,
}

impl QueryLocalDeviceUseCase {
    pub fn new(
        device_identity: Arc<dyn DeviceIdentityPort>,
        settings: Arc<dyn SettingsPort>,
    ) -> Self {
        Self {
            device_identity,
            settings,
        }
    }

    pub async fn execute(&self) -> LocalDeviceInfo {
        let device_name = match self.settings.load().await {
            Ok(settings) => normalize_device_name(settings.general.device_name),
            Err(_) => {
                tracing::warn!("local device settings unavailable; using fallback device name");
                DEFAULT_DEVICE_NAME.to_string()
            }
        };

        LocalDeviceInfo {
            device_id: self.device_identity.current_device_id().to_string(),
            device_name,
        }
    }
}

fn normalize_device_name(value: Option<String>) -> String {
    let name = value.unwrap_or_default();
    let trimmed = name.trim();
    if trimmed.is_empty() {
        DEFAULT_DEVICE_NAME.to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use uc_core::ids::DeviceId;
    use uc_core::settings::model::Settings;

    use super::*;

    struct StaticDeviceIdentity;

    impl DeviceIdentityPort for StaticDeviceIdentity {
        fn current_device_id(&self) -> DeviceId {
            DeviceId::new("dev-1")
        }
    }

    struct InMemorySettings {
        settings: Mutex<Settings>,
        fail_load: bool,
    }

    #[async_trait]
    impl SettingsPort for InMemorySettings {
        async fn load(&self) -> anyhow::Result<Settings> {
            if self.fail_load {
                anyhow::bail!("settings unavailable");
            }
            Ok(self.settings.lock().unwrap().clone())
        }

        async fn save(&self, settings: &Settings) -> anyhow::Result<()> {
            *self.settings.lock().unwrap() = settings.clone();
            Ok(())
        }
    }

    fn use_case(device_name: Option<String>, fail_load: bool) -> QueryLocalDeviceUseCase {
        let mut settings = Settings::default();
        settings.general.device_name = device_name;
        QueryLocalDeviceUseCase::new(
            Arc::new(StaticDeviceIdentity),
            Arc::new(InMemorySettings {
                settings: Mutex::new(settings),
                fail_load,
            }),
        )
    }

    #[tokio::test]
    async fn execute_returns_trimmed_settings_name() {
        let info = use_case(Some("  MacBook  ".to_string()), false)
            .execute()
            .await;

        assert_eq!(info.device_id, "dev-1");
        assert_eq!(info.device_name, "MacBook");
    }

    #[tokio::test]
    async fn execute_uses_fallback_when_name_is_blank_or_settings_fail() {
        let blank = use_case(Some("  ".to_string()), false).execute().await;
        let failed = use_case(Some("Ignored".to_string()), true).execute().await;

        assert_eq!(blank.device_name, DEFAULT_DEVICE_NAME);
        assert_eq!(failed.device_name, DEFAULT_DEVICE_NAME);
    }
}
