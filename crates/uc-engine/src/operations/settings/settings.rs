//! Engine 设置的查询与局部更新。

use std::time::Duration;

use uc_application::facade::settings as app;
use uc_application::facade::AppFacade;

use crate::error_codes::*;
use crate::{
    CongestionControllerSummary, EngineError, EngineErrorCategory, FileSyncSettingsSummary,
    GeneralSettingsSummary, NetworkSettingsSummary, OperationResult, PairingSettingsSummary,
    RetentionPolicySummary, RetentionRulePatch, RetentionRuleSummary, RuleEvaluationSummary,
    SecuritySettingsSummary, SettingsContentTypes, SettingsContentTypesPatch, SettingsPatch,
    SettingsSummary, SettingsUpdateOutcome, SyncFrequencySummary, SyncSettingsSummary,
};

pub(crate) async fn execute_query_settings(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let settings = facade
        .settings()
        .await
        // discarded-source[contract-boundary]: the public error carries a stable code only, the owner records the failure classification
        .map_err(|_| internal_error(QUERY_SETTINGS_FAILED_CODE))?;
    Ok(OperationResult::Settings(Box::new(map_settings(settings))))
}

pub(crate) async fn execute_update_settings(
    facade: &AppFacade,
    patch: SettingsPatch,
) -> Result<OperationResult, EngineError> {
    let patch = match map_patch(patch) {
        Ok(patch) => patch,
        Err(reason) => {
            return Ok(OperationResult::SettingsUpdated(
                SettingsUpdateOutcome::Rejected { reason },
            ));
        }
    };
    match facade.update_settings(patch).await {
        Ok(settings) => Ok(OperationResult::SettingsUpdated(
            SettingsUpdateOutcome::Updated(Box::new(map_settings(settings))),
        )),
        Err(app::SettingsFacadeError::Invalid(reason)) => Ok(OperationResult::SettingsUpdated(
            SettingsUpdateOutcome::Rejected { reason },
        )),
        Err(_) => Err(internal_error(UPDATE_SETTINGS_FAILED_CODE)),
    }
}

/// 中继探测诊断文本按宿主契约原样透传（已确认的例外）：取下层来源的显示文本，
/// 与探测错误改为携带 source 之前交给宿主的文本逐字一致。

pub(super) fn map_settings(settings: app::SettingsView) -> SettingsSummary {
    SettingsSummary {
        schema_version: settings.schema_version,
        general: GeneralSettingsSummary {
            device_name: settings.general.device_name,
        },
        sync: SyncSettingsSummary {
            sync_enabled: settings.sync.sync_enabled,
            auto_sync_enabled: settings.sync.auto_sync_enabled,
            sync_frequency: map_sync_frequency(settings.sync.sync_frequency),
            content_types: map_content_types(settings.sync.content_types),
            sync_on_restore: settings.sync.sync_on_restore,
        },
        retention_policy: RetentionPolicySummary {
            enabled: settings.retention_policy.enabled,
            rules: settings
                .retention_policy
                .rules
                .into_iter()
                .map(map_retention_rule)
                .collect(),
            skip_pinned: settings.retention_policy.skip_pinned,
            evaluation: map_rule_evaluation(settings.retention_policy.evaluation),
        },
        security: SecuritySettingsSummary {
            encryption_enabled: settings.security.encryption_enabled,
            passphrase_configured: settings.security.passphrase_configured,
            auto_unlock_enabled: settings.security.auto_unlock_enabled,
        },
        pairing: PairingSettingsSummary {
            step_timeout_secs: settings.pairing.step_timeout.as_secs(),
            user_verification_timeout_secs: settings.pairing.user_verification_timeout.as_secs(),
            session_timeout_secs: settings.pairing.session_timeout.as_secs(),
            max_retries: settings.pairing.max_retries,
            protocol_version: settings.pairing.protocol_version,
        },
        file_sync: FileSyncSettingsSummary {
            file_sync_enabled: settings.file_sync.file_sync_enabled,
            small_file_threshold: settings.file_sync.small_file_threshold,
            max_file_size: settings.file_sync.max_file_size,
            file_cache_quota_per_device: settings.file_sync.file_cache_quota_per_device,
            file_retention_hours: settings.file_sync.file_retention_hours,
            file_auto_cleanup: settings.file_sync.file_auto_cleanup,
            max_file_set_total_bytes: settings.file_sync.max_file_set_total_bytes,
            max_file_set_member_count: settings.file_sync.max_file_set_member_count,
            auto_save_dir: settings.file_sync.auto_save_dir,
        },
        network: NetworkSettingsSummary {
            allow_relay_fallback: settings.network.allow_relay_fallback,
            allow_overlay_network_addrs: settings.network.allow_overlay_network_addrs,
            custom_relay_urls: settings.network.custom_relay_urls,
            congestion_controller: map_congestion_controller(
                settings.network.congestion_controller,
            ),
        },
    }
}

pub(super) fn map_patch(patch: SettingsPatch) -> Result<app::SettingsPatch, String> {
    Ok(app::SettingsPatch {
        general: patch.general.map(|value| app::GeneralSettingsPatch {
            device_name: value.device_name,
        }),
        sync: patch.sync.map(|value| app::SyncSettingsPatch {
            sync_enabled: value.sync_enabled,
            auto_sync_enabled: value.auto_sync_enabled,
            sync_frequency: value.sync_frequency.map(unmap_sync_frequency),
            content_types: value.content_types.map(unmap_content_types_patch),
            sync_on_restore: value.sync_on_restore,
        }),
        retention_policy: patch
            .retention_policy
            .map(|value| -> Result<_, String> {
                Ok(app::RetentionPolicyPatch {
                    enabled: value.enabled,
                    rules: value
                        .rules
                        .map(|rules| {
                            rules
                                .into_iter()
                                .map(unmap_retention_rule)
                                .collect::<Result<Vec<_>, _>>()
                        })
                        .transpose()?,
                    skip_pinned: value.skip_pinned,
                    evaluation: value.evaluation.map(unmap_rule_evaluation),
                })
            })
            .transpose()?,
        security: patch.security.map(|value| app::SecuritySettingsPatch {
            encryption_enabled: value.encryption_enabled,
            auto_unlock_enabled: value.auto_unlock_enabled,
        }),
        pairing: patch.pairing.map(|value| app::PairingSettingsPatch {
            step_timeout: value.step_timeout_secs.map(Duration::from_secs),
            user_verification_timeout: value
                .user_verification_timeout_secs
                .map(Duration::from_secs),
            session_timeout: value.session_timeout_secs.map(Duration::from_secs),
            max_retries: value.max_retries,
        }),
        file_sync: patch.file_sync.map(|value| app::FileSyncSettingsPatch {
            file_sync_enabled: value.file_sync_enabled,
            small_file_threshold: value.small_file_threshold,
            max_file_size: value.max_file_size,
            file_cache_quota_per_device: value.file_cache_quota_per_device,
            file_retention_hours: value.file_retention_hours,
            file_auto_cleanup: value.file_auto_cleanup,
            max_file_set_total_bytes: value.max_file_set_total_bytes,
            max_file_set_member_count: value.max_file_set_member_count,
            auto_save_dir: value.auto_save_dir,
        }),
        network: patch.network.map(|value| app::NetworkSettingsPatch {
            allow_relay_fallback: value.allow_relay_fallback,
            allow_overlay_network_addrs: value.allow_overlay_network_addrs,
            custom_relay_urls: value.custom_relay_urls,
            congestion_controller: value.congestion_controller.map(unmap_congestion_controller),
        }),
    })
}

fn map_sync_frequency(value: app::SyncFrequencyView) -> SyncFrequencySummary {
    match value {
        app::SyncFrequencyView::Realtime => SyncFrequencySummary::Realtime,
        app::SyncFrequencyView::Interval => SyncFrequencySummary::Interval,
    }
}

fn unmap_sync_frequency(value: SyncFrequencySummary) -> app::SyncFrequencyView {
    match value {
        SyncFrequencySummary::Realtime => app::SyncFrequencyView::Realtime,
        SyncFrequencySummary::Interval => app::SyncFrequencyView::Interval,
    }
}

fn map_rule_evaluation(value: app::RuleEvaluationView) -> RuleEvaluationSummary {
    match value {
        app::RuleEvaluationView::AnyMatch => RuleEvaluationSummary::AnyMatch,
        app::RuleEvaluationView::AllMatch => RuleEvaluationSummary::AllMatch,
    }
}

fn unmap_rule_evaluation(value: RuleEvaluationSummary) -> app::RuleEvaluationView {
    match value {
        RuleEvaluationSummary::AnyMatch => app::RuleEvaluationView::AnyMatch,
        RuleEvaluationSummary::AllMatch => app::RuleEvaluationView::AllMatch,
    }
}

fn map_content_types(value: app::ContentTypesView) -> SettingsContentTypes {
    SettingsContentTypes {
        text: value.text,
        image: value.image,
        link: value.link,
        file: value.file,
        code_snippet: value.code_snippet,
        rich_text: value.rich_text,
    }
}

fn unmap_content_types(value: SettingsContentTypes) -> app::ContentTypesView {
    app::ContentTypesView {
        text: value.text,
        image: value.image,
        link: value.link,
        file: value.file,
        code_snippet: value.code_snippet,
        rich_text: value.rich_text,
    }
}

fn unmap_content_types_patch(value: SettingsContentTypesPatch) -> app::ContentTypesPatch {
    app::ContentTypesPatch {
        text: value.text,
        image: value.image,
        link: value.link,
        file: value.file,
        code_snippet: value.code_snippet,
        rich_text: value.rich_text,
    }
}

fn map_retention_rule(value: app::RetentionRuleView) -> RetentionRuleSummary {
    match value {
        app::RetentionRuleView::ByAge { max_age } => RetentionRuleSummary::ByAge {
            max_age_secs: max_age.as_secs(),
        },
        app::RetentionRuleView::ByCount { max_items } => RetentionRuleSummary::ByCount {
            max_items: max_items as u64,
        },
        app::RetentionRuleView::ByContentType {
            content_type,
            max_age,
        } => RetentionRuleSummary::ByContentType {
            content_type: map_content_types(content_type),
            max_age_secs: max_age.as_secs(),
        },
        app::RetentionRuleView::ByTotalSize { max_bytes } => {
            RetentionRuleSummary::ByTotalSize { max_bytes }
        }
        app::RetentionRuleView::Sensitive { max_age } => RetentionRuleSummary::Sensitive {
            max_age_secs: max_age.as_secs(),
        },
    }
}

fn unmap_retention_rule(value: RetentionRulePatch) -> Result<app::RetentionRulePatchValue, String> {
    Ok(match value {
        RetentionRulePatch::ByAge { max_age_secs } => app::RetentionRulePatchValue::ByAge {
            max_age: Duration::from_secs(max_age_secs),
        },
        RetentionRulePatch::ByCount { max_items } => app::RetentionRulePatchValue::ByCount {
            max_items: usize::try_from(max_items)
                // discarded-source[int-conversion]: `TryFromIntError`: the target classification already expresses the range or length mismatch
                .map_err(|_| "retention count exceeds this platform's limit".to_string())?,
        },
        RetentionRulePatch::ByContentType {
            content_type,
            max_age_secs,
        } => app::RetentionRulePatchValue::ByContentType {
            content_type: unmap_content_types(content_type),
            max_age: Duration::from_secs(max_age_secs),
        },
        RetentionRulePatch::ByTotalSize { max_bytes } => {
            app::RetentionRulePatchValue::ByTotalSize { max_bytes }
        }
        RetentionRulePatch::Sensitive { max_age_secs } => app::RetentionRulePatchValue::Sensitive {
            max_age: Duration::from_secs(max_age_secs),
        },
    })
}

fn map_congestion_controller(value: app::CongestionControllerView) -> CongestionControllerSummary {
    match value {
        app::CongestionControllerView::Cubic => CongestionControllerSummary::Cubic,
        app::CongestionControllerView::Bbr3 => CongestionControllerSummary::Bbr3,
    }
}

fn unmap_congestion_controller(
    value: CongestionControllerSummary,
) -> app::CongestionControllerView {
    match value {
        CongestionControllerSummary::Cubic => app::CongestionControllerView::Cubic,
        CongestionControllerSummary::Bbr3 => app::CongestionControllerView::Bbr3,
    }
}

pub(super) fn internal_error(code: u32) -> EngineError {
    EngineError::new(code, EngineErrorCategory::Internal, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]

    fn settings_patch_preserves_clear_and_unchanged_states() {
        let patch = SettingsPatch {
            general: Some(crate::GeneralSettingsPatch {
                device_name: Some(None),
            }),

            ..Default::default()
        };

        let mapped = map_patch(patch).expect("valid patch");

        let general = mapped.general.expect("general patch");

        assert_eq!(general.device_name, Some(None));
    }

    #[test]

    fn oversized_retention_count_is_a_business_rejection() {
        if usize::BITS >= 64 {
            return;
        }

        let patch = SettingsPatch {
            retention_policy: Some(crate::RetentionPolicyPatch {
                rules: Some(vec![RetentionRulePatch::ByCount {
                    max_items: u64::MAX,
                }]),

                ..Default::default()
            }),

            ..Default::default()
        };

        assert!(map_patch(patch).is_err());
    }
}
