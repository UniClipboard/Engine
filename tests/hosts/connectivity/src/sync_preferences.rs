#[cfg(not(feature = "automatic-sync-control"))]
use anyhow::bail;
use anyhow::{Context, Result};
use serde_json::Value;
use uc_engine::{Operation, SettingsPatch, SyncSettingsPatch};

/// 自动同步与全局同步拆分前的旧接口不能表达该请求，禁止映射为旧 auto_sync。
pub(super) fn operation(request: &Value) -> Result<Operation> {
    #[cfg(not(feature = "automatic-sync-control"))]
    if request.get("auto_sync_enabled").is_some() {
        bail!("automatic_sync_control_unavailable");
    }
    Ok(Operation::UpdateSettings(Box::new(SettingsPatch {
        sync: Some(SyncSettingsPatch {
            #[cfg(feature = "automatic-sync-control")]
            auto_sync_enabled: match request.get("auto_sync_enabled") {
                Some(value) => Some(value.as_bool().context("invalid auto sync preference")?),
                None => None,
            },
            sync_on_restore: Some(
                request["sync_on_restore"]
                    .as_bool()
                    .context("missing sync preference")?,
            ),
            ..Default::default()
        }),
        ..Default::default()
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn restore_preference_remains_available() {
        let Operation::UpdateSettings(patch) =
            operation(&json!({"sync_on_restore": false})).unwrap()
        else {
            panic!("settings operation expected");
        };
        assert_eq!(patch.sync.unwrap().sync_on_restore, Some(false));
    }

    #[test]
    fn automatic_sync_requires_the_declared_capability() {
        for enabled in [true, false] {
            let result = operation(&json!({"sync_on_restore": true, "auto_sync_enabled": enabled}));
            #[cfg(feature = "automatic-sync-control")]
            {
                let Operation::UpdateSettings(patch) = result.unwrap() else {
                    panic!("settings operation expected");
                };
                assert_eq!(patch.sync.unwrap().auto_sync_enabled, Some(enabled));
            }
            #[cfg(not(feature = "automatic-sync-control"))]
            assert!(result.is_err());
        }
    }

    #[test]
    fn host_advertises_automatic_control_only_when_supported() {
        let identity = crate::host_identity();
        let capabilities = identity["capabilities"].as_array().unwrap();
        assert!(capabilities.contains(&json!("sync_preference")));
        assert_eq!(
            capabilities.contains(&json!("automatic-sync-control")),
            cfg!(feature = "automatic-sync-control")
        );
    }

    #[test]
    fn malformed_preferences_are_not_silently_ignored() {
        for value in [Value::Null, json!("false"), json!(0)] {
            assert!(
                operation(&json!({"sync_on_restore": true, "auto_sync_enabled": value})).is_err()
            );
        }
        assert!(operation(&json!({"auto_sync_enabled": false})).is_err());
    }
}
