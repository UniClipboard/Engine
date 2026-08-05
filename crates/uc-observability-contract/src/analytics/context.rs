use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// Operating-system family used in low-cardinality analytics properties.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    Macos,
    Windows,
    Linux,
    Ios,
    Android,
    Other,
}

/// Physical device class used in analytics properties.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    Mobile,
    Desktop,
}

/// Platform properties attached to every analytics event by the host sender.
///
/// This keeps platform details out of individual business events. Hosts provide
/// one immutable value at startup, then merge it with every event immediately
/// before delivery. Event-specific properties cannot replace these fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticsEventContext {
    pub os: Os,
    pub os_version: String,
    pub device_type: DeviceType,
    pub arch: String,
    pub app_channel: String,
}

impl AnalyticsEventContext {
    /// Returns the fixed wire properties shared by desktop and mobile senders.
    pub fn properties(&self) -> Map<String, Value> {
        let os = Value::String(
            match self.os {
                Os::Macos => "macos",
                Os::Windows => "windows",
                Os::Linux => "linux",
                Os::Ios => "ios",
                Os::Android => "android",
                Os::Other => "other",
            }
            .to_owned(),
        );
        let device_type = Value::String(
            match self.device_type {
                DeviceType::Mobile => "mobile",
                DeviceType::Desktop => "desktop",
            }
            .to_owned(),
        );
        [
            ("$os".to_owned(), os.clone()),
            ("os".to_owned(), os),
            ("os_version".to_owned(), json!(self.os_version)),
            ("$device_type".to_owned(), device_type),
            ("arch".to_owned(), json!(self.arch)),
            ("app_channel".to_owned(), json!(self.app_channel)),
        ]
        .into_iter()
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{AnalyticsEventContext, DeviceType, Os};

    #[test]
    fn platform_context_uses_the_shared_platform_property_names() {
        let context = AnalyticsEventContext {
            os: Os::Ios,
            os_version: "18.0".to_owned(),
            device_type: DeviceType::Mobile,
            arch: "arm64".to_owned(),
            app_channel: "development".to_owned(),
        };

        assert_eq!(context.properties().get("$os"), Some(&json!("ios")));
        assert_eq!(context.properties().get("os"), Some(&json!("ios")));
        assert_eq!(context.properties().get("os_version"), Some(&json!("18.0")));
        assert_eq!(
            context.properties().get("$device_type"),
            Some(&json!("mobile"))
        );
        assert_eq!(context.properties().get("arch"), Some(&json!("arm64")));
        assert_eq!(
            context.properties().get("app_channel"),
            Some(&json!("development"))
        );
    }
}
