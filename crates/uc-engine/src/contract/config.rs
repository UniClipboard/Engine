use std::fmt;

use serde::{Deserialize, Serialize};

const DEFAULT_PROFILE_ID: &str = "default";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineConfig {
    app_version: String,
    profile_id: String,
    #[serde(default)]
    portable_storage: bool,
    #[cfg(feature = "dev-tools")]
    #[serde(skip)]
    rendezvous_base_url: Option<String>,
    #[cfg(feature = "dev-tools")]
    #[serde(skip)]
    test_relay_fallback: Option<bool>,
}

impl EngineConfig {
    pub fn new(app_version: impl Into<String>) -> Self {
        Self {
            app_version: app_version.into(),
            profile_id: DEFAULT_PROFILE_ID.to_string(),
            portable_storage: false,
            #[cfg(feature = "dev-tools")]
            rendezvous_base_url: None,
            #[cfg(feature = "dev-tools")]
            test_relay_fallback: None,
        }
    }

    pub fn with_profile_id(mut self, profile_id: impl Into<String>) -> Self {
        self.profile_id = profile_id.into();
        self
    }

    pub fn with_portable_storage(mut self, portable_storage: bool) -> Self {
        self.portable_storage = portable_storage;
        self
    }

    pub fn app_version(&self) -> &str {
        &self.app_version
    }

    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    pub fn uses_portable_storage(&self) -> bool {
        self.portable_storage
    }

    #[cfg(feature = "dev-tools")]
    pub fn with_rendezvous_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.rendezvous_base_url = Some(base_url.into());
        self
    }

    #[cfg(feature = "dev-tools")]
    pub fn with_test_relay_fallback(mut self, allow_relay_fallback: bool) -> Self {
        self.test_relay_fallback = Some(allow_relay_fallback);
        self
    }

    pub(crate) fn rendezvous_base_url_override(&self) -> Option<String> {
        #[cfg(feature = "dev-tools")]
        {
            return self.rendezvous_base_url.clone();
        }
        #[cfg(not(feature = "dev-tools"))]
        {
            None
        }
    }

    pub(crate) fn test_relay_fallback_override(&self) -> Option<bool> {
        #[cfg(feature = "dev-tools")]
        {
            return self.test_relay_fallback;
        }
        #[cfg(not(feature = "dev-tools"))]
        {
            None
        }
    }
}

impl fmt::Debug for EngineConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("EngineConfig");
        debug
            .field("app_version", &self.app_version)
            .field("profile_id", &"[REDACTED]")
            .field("portable_storage", &self.portable_storage);
        #[cfg(feature = "dev-tools")]
        debug.field(
            "has_rendezvous_override",
            &self.rendezvous_base_url.is_some(),
        );
        #[cfg(feature = "dev-tools")]
        debug.field(
            "has_test_relay_fallback_override",
            &self.test_relay_fallback.is_some(),
        );
        debug.finish()
    }
}
