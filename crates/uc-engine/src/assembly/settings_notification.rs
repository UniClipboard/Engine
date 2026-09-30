//! 设置变更通知装饰器：在任何一次成功保存之后，把“哪些分区变了”交给宿主事件出口。
//!
//! 装饰的是既有完整的 `SettingsPort`，只观察保存的输入与结果，不编排业务步骤。
//! 所有写入路径（设置更新、中继配置、调试开关、建立或加入空间时的设备名）共享同一个
//! 通知出口，因此不会漏掉只经过其他流程的写入。通知不携带设置值。

use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ports::{
    HostEvent, HostEventEmitterPort, SettingsHostEvent, SettingsPort, SettingsSection,
};
use uc_core::settings::model::Settings;

pub(crate) struct NotifyingSettings {
    inner: Arc<dyn SettingsPort>,
    emitter: Arc<dyn HostEventEmitterPort>,
}

impl NotifyingSettings {
    pub(crate) fn new(
        inner: Arc<dyn SettingsPort>,
        emitter: Arc<dyn HostEventEmitterPort>,
    ) -> Self {
        Self { inner, emitter }
    }
}

#[async_trait]
impl SettingsPort for NotifyingSettings {
    async fn load(&self) -> anyhow::Result<Settings> {
        self.inner.load().await
    }

    async fn save(&self, settings: &Settings) -> anyhow::Result<()> {
        // 保存前的旧值只用于比较；读取失败时无法确定差异，保守地通知所有分区。
        let before = self.inner.load().await.ok();
        self.inner.save(settings).await?;
        let sections = match before {
            Some(before) => changed_sections(&before, settings),
            None => ALL_SECTIONS.to_vec(),
        };
        if !sections.is_empty() {
            if let Err(error) = self
                .emitter
                .emit(HostEvent::Settings(SettingsHostEvent::Changed { sections }))
            {
                // 设置已经保存成功；通知失败不能反向让保存失败。宿主仍可在下次查询时读到新值。
                tracing::warn!(
                    error_kind = "settings_change_notification",
                    io_error_kind = uc_observability_contract::error_source::io_error_kind(&error),
                    "settings change notification was not delivered"
                );
            }
        }
        Ok(())
    }
}

const ALL_SECTIONS: [SettingsSection; 10] = [
    SettingsSection::General,
    SettingsSection::Sync,
    SettingsSection::RetentionPolicy,
    SettingsSection::Security,
    SettingsSection::Pairing,
    SettingsSection::KeyboardShortcuts,
    SettingsSection::FileSync,
    SettingsSection::Network,
    SettingsSection::MobileSync,
    SettingsSection::QuickPanel,
];

/// 逐分区比较两份设置；`schema_version` 不属于任何分区，不单独通知。
fn changed_sections(before: &Settings, after: &Settings) -> Vec<SettingsSection> {
    fn differs<T: PartialEq>(before: &T, after: &T) -> bool {
        before != after
    }
    let mut changed = Vec::new();
    let mut push = |section: SettingsSection, differs: bool| {
        if differs {
            changed.push(section);
        }
    };
    push(
        SettingsSection::General,
        differs(&before.general, &after.general),
    );
    push(SettingsSection::Sync, differs(&before.sync, &after.sync));
    push(
        SettingsSection::RetentionPolicy,
        differs(&before.retention_policy, &after.retention_policy),
    );
    push(
        SettingsSection::Security,
        differs(&before.security, &after.security),
    );
    push(
        SettingsSection::Pairing,
        differs(&before.pairing, &after.pairing),
    );
    push(
        SettingsSection::KeyboardShortcuts,
        differs(&before.keyboard_shortcuts, &after.keyboard_shortcuts),
    );
    push(
        SettingsSection::FileSync,
        differs(&before.file_sync, &after.file_sync),
    );
    push(
        SettingsSection::Network,
        differs(&before.network, &after.network),
    );
    push(
        SettingsSection::MobileSync,
        differs(&before.mobile_sync, &after.mobile_sync),
    );
    push(
        SettingsSection::QuickPanel,
        differs(&before.quick_panel, &after.quick_panel),
    );
    changed
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use uc_core::ports::EmitError;

    use super::*;

    #[derive(Default)]
    struct MemorySettings(Mutex<Option<Settings>>, Mutex<bool>);

    #[async_trait]
    impl SettingsPort for MemorySettings {
        async fn load(&self) -> anyhow::Result<Settings> {
            let stored = self.0.lock().unwrap().clone();
            Ok(stored.unwrap_or_default())
        }

        async fn save(&self, settings: &Settings) -> anyhow::Result<()> {
            if *self.1.lock().unwrap() {
                anyhow::bail!("disk full");
            }
            *self.0.lock().unwrap() = Some(settings.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingEmitter {
        events: Mutex<Vec<Vec<SettingsSection>>>,
        fail: Mutex<bool>,
    }

    impl HostEventEmitterPort for RecordingEmitter {
        fn emit(&self, event: HostEvent) -> Result<(), EmitError> {
            if *self.fail.lock().unwrap() {
                return Err(EmitError::Failed("closed".into()));
            }
            if let HostEvent::Settings(SettingsHostEvent::Changed { sections }) = event {
                self.events.lock().unwrap().push(sections);
            }
            Ok(())
        }
    }

    fn fixture() -> (
        NotifyingSettings,
        Arc<MemorySettings>,
        Arc<RecordingEmitter>,
    ) {
        let inner = Arc::new(MemorySettings::default());
        let emitter = Arc::new(RecordingEmitter::default());
        (
            NotifyingSettings::new(inner.clone(), emitter.clone()),
            inner,
            emitter,
        )
    }

    #[tokio::test]
    async fn only_the_sections_that_changed_are_reported() {
        let (settings, _inner, emitter) = fixture();
        let mut next = settings.load().await.unwrap();
        next.general.language = Some("zh-CN".into());
        next.quick_panel.enabled = !next.quick_panel.enabled;
        settings.save(&next).await.unwrap();

        assert_eq!(
            emitter.events.lock().unwrap().as_slice(),
            [vec![SettingsSection::General, SettingsSection::QuickPanel]]
        );
    }

    #[tokio::test]
    async fn saving_identical_settings_emits_nothing() {
        let (settings, _inner, emitter) = fixture();
        let current = settings.load().await.unwrap();
        settings.save(&current).await.unwrap();
        assert!(emitter.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn failed_save_emits_nothing_and_keeps_the_error() {
        let (settings, inner, emitter) = fixture();
        *inner.1.lock().unwrap() = true;
        let mut next = settings.load().await.unwrap();
        next.general.language = Some("fr".into());
        assert!(settings.save(&next).await.is_err());
        assert!(emitter.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn undeliverable_notification_does_not_fail_a_successful_save() {
        let (settings, inner, emitter) = fixture();
        *emitter.fail.lock().unwrap() = true;
        let mut next = settings.load().await.unwrap();
        next.general.language = Some("de".into());
        settings.save(&next).await.unwrap();
        assert_eq!(
            inner.load().await.unwrap().general.language.as_deref(),
            Some("de")
        );
    }

    #[tokio::test]
    async fn events_never_carry_setting_values() {
        let (settings, _inner, emitter) = fixture();
        let mut next = settings.load().await.unwrap();
        next.general.device_name = Some("private device name".into());
        settings.save(&next).await.unwrap();
        let debug = format!("{:?}", emitter.events.lock().unwrap());
        assert!(!debug.contains("private device name"));
    }
}
