# 运行期观测清单

> 由 `scripts/architecture/check-observability-privacy.mjs --write-inventory` 从生产 Rust 源生成。
> 该文件只描述最后一次运行生成命令时的代码事实，不是新增埋点的授权清单。

## stable remote

共 9 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:310` | `span` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:655` | `span` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:670` | `span` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:889` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:899` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:955` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:966` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:1009` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:1028` | `event` | - |

## local operational

共 6 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `observability.health` | `crates/uc-observability-contract/src/diagnostics/mod.rs:556` | `event` | - |
| `observability.health` | `crates/uc-observability-contract/src/diagnostics/mod.rs:571` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:78` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:90` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:107` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:124` | `event` | - |

## local debug

共 1251 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:43` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:52` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:59` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:66` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:72` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:74` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:81` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:88` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:95` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:102` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:109` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:116` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:604` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:264` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:423` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:503` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:595` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:596` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:597` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:690` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:700` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:709` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:712` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:789` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:330` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:334` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:341` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:391` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:451` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:498` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:544` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:769` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:778` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:809` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:882` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:890` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:897` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1022` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1072` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1207` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1481` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:133` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:138` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:177` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:200` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:227` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:234` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:239` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:267` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:287` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:293` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:346` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:354` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:369` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:373` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:381` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:392` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:415` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:431` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:441` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:492` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:545` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:566` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:728` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:757` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:792` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:73` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:78` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:115` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:125` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:153` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:74` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:86` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:107` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:116` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:125` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:139` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:146` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:178` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:185` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:190` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:203` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:209` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:216` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:220` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:306` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:313` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:330` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:338` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:371` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:399` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:472` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:489` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:526` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:105` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:114` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:132` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:163` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:181` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:189` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:102` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:105` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:165` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:171` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:181` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:60` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:66` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:90` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:110` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:119` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:125` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:142` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/toggle_favorite.rs:41` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/toggle_favorite.rs:60` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/toggle_favorite.rs:68` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/toggle_favorite.rs:70` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:121` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:124` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:154` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:172` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:183` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:194` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:202` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:210` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:259` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:271` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:320` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:328` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/local.rs:196` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:221` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:266` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:297` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:306` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:322` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:331` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:359` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:437` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:670` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:679` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:781` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:948` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:994` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:92` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:116` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:123` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:143` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:156` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:183` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/resource/mod.rs:61` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/resource/mod.rs:89` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/resource/mod.rs:140` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:137` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:148` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:231` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:324` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:329` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:350` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:389` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:408` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_selection.rs:118` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_selection.rs:167` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_selection.rs:173` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:234` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:241` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:247` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:261` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:284` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:295` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:306` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:312` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:316` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:324` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:367` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:387` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:415` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:424` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:430` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:434` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:446` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:450` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:454` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:478` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:541` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:556` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:563` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:44` | `debug` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:74` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:81` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:105` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:99` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:109` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:158` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:181` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:185` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:197` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:114` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:122` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:139` | `info` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:162` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:171` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:79` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:86` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:127` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:132` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:97` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:111` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:115` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:152` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:156` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:162` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:182` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:193` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:201` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:205` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:230` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:234` | `debug` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:393` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:399` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:406` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:522` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:529` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:535` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:552` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:565` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:848` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:902` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:913` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:936` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1009` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1155` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1190` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1211` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1262` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1267` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1308` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1502` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1508` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1875` | `record` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1897` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1901` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:2316` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:2413` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:2416` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:2597` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:437` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:440` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:443` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:469` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:766` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:787` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:803` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:822` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:859` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:863` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:870` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:908` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:931` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:936` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:982` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1123` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1180` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1199` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1231` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1335` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1526` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1542` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1555` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1559` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1568` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1659` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1665` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:62` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:80` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:98` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:116` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:157` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:192` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:244` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/header.rs:64` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/mod.rs:475` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/per_peer.rs:87` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/per_peer.rs:174` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/per_peer.rs:187` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs:125` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs:132` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs:141` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs:145` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/existing_local_entry_delivery.rs:153` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/get_entry_delivery_view.rs:189` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/get_entry_delivery_view.rs:291` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/outbound_plan.rs:72` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:69` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:86` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:94` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:102` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:121` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/resend_entry.rs:217` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:60` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:69` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:76` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:85` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:89` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:123` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:205` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:227` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:261` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:319` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:326` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:333` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:339` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:110` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:123` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:196` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:213` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:225` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:238` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:265` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:283` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:296` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:310` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:327` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:345` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:437` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/active_register.rs:78` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/active_register.rs:89` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:142` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:169` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:191` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:257` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:310` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:359` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/write/mobile_consumability.rs:30` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/mobile_consumability.rs:59` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/restore_broadcast.rs:50` | `trace` | message-body |
| `<module>` | `crates/uc-application/src/device/query_local_device/use_case.rs:29` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:177` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:198` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:217` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:231` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:247` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/facade/clipboard/cancel_entry_receive.rs:91` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:332` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:392` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:409` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:427` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:110` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:155` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:186` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:203` | `instrument` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:218` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:239` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:257` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:296` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:354` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:362` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:373` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:376` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:381` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:387` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:392` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:400` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:413` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:419` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:431` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:447` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:461` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:481` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:484` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:536` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:546` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:564` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:579` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:593` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:622` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:650` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:663` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:677` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:683` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:688` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:762` | `debug` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:774` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:781` | `debug` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:795` | `debug` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:810` | `debug` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:882` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:893` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:895` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:900` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:902` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:913` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:917` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:933` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:941` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:949` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:956` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:959` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/live_index/mod.rs:111` | `debug` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/live_index/mod.rs:126` | `debug` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/live_index/mod.rs:152` | `debug` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/search/query.rs:18` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/search/query.rs:33` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/settings/config_migration/facade.rs:77` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/config_migration/facade.rs:100` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/config_migration/facade.rs:119` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/diagnostics.rs:61` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/diagnostics.rs:76` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/diagnostics.rs:100` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/diagnostics.rs:110` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:169` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:192` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:202` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:210` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/settings/facade.rs:214` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:231` | `info` | message-body |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:49` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:59` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:64` | `info` | message-body |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:83` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:104` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:110` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:125` | `info` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/acknowledge.rs:39` | `info` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:82` | `debug` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:92` | `debug` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:102` | `debug` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:110` | `debug` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:123` | `debug` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:135` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/space/admission/invitation/cancel/use_case.rs:53` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/admission/invitation/issue_for_address/use_case.rs:25` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/admission/invitation/issue/use_case.rs:48` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/admission/invitation/issuer.rs:110` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/admission/invitation/query_addresses/use_case.rs:19` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/admission/invitation/query_addresses/use_case.rs:28` | `record` | - |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/runtime.rs:192` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/runtime.rs:416` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:417` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:438` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:457` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:470` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:486` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:494` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:685` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:705` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:727` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:802` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:805` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:105` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:152` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:170` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:186` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:210` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:294` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/unlock_space/use_case.rs:40` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/unlock_space/use_case.rs:63` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/unlock_space/use_case.rs:76` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/unlock_space/use_case.rs:106` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/space/membership/decide_device_trust_change/use_case.rs:91` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/decide_device_trust_change/use_case.rs:148` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/decide_device_trust_change/use_case.rs:194` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:97` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:183` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:187` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:195` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:215` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:234` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:251` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:254` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:359` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:423` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:480` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:512` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:110` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:113` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:118` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:124` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:132` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:141` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:156` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:177` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:190` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:193` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:197` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:202` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:207` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:213` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/ledger/repository.rs:414` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/restricted_delivery.rs:46` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/projection.rs:129` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_admission/use_case.rs:38` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_admission/use_case.rs:43` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_admission/use_case.rs:56` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_admission/use_case.rs:60` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_admission/use_case.rs:64` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/use_case.rs:43` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/use_case.rs:43` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/use_case.rs:54` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/use_case.rs:69` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/use_case.rs:74` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/use_case.rs:122` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/use_case.rs:179` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/use_case.rs:203` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/use_case.rs:236` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/use_case.rs:238` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_diagnostics.rs:91` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_member_roster.rs:70` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/reconcile_history_evidence/use_case.rs:46` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/reconcile_history_evidence/use_case.rs:74` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/reconcile_history_evidence/use_case.rs:81` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/reconcile_history_evidence/use_case.rs:116` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/reconcile_history_evidence/use_case.rs:125` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/reconcile_history_evidence/use_case.rs:142` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/reconcile_history_evidence/use_case.rs:152` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/reconcile_history_evidence/use_case.rs:166` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/reconcile_history_evidence/use_case.rs:174` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/reconcile_history_evidence/use_case.rs:179` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/reconcile_history_evidence/use_case.rs:190` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/issuer.rs:66` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/issuer.rs:84` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/issuer.rs:89` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/issuer.rs:154` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:88` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:101` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:107` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:113` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:144` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:397` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:401` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/remove_space_member/use_case.rs:98` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/remove_space_member/use_case.rs:143` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/resolve_conflict/use_case.rs:46` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/resolve_conflict/use_case.rs:51` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/resolve_conflict/use_case.rs:60` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/resolve_conflict/use_case.rs:110` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/resolve_conflict/use_case.rs:156` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/resolve_conflict/use_case.rs:258` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/resolve_conflict/use_case.rs:269` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/use_case.rs:143` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/use_case.rs:167` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/use_case.rs:172` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/use_case.rs:250` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/use_case.rs:253` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/use_case.rs:272` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/use_case.rs:326` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/use_case.rs:420` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/use_case.rs:444` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/use_case.rs:457` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/use_case.rs:537` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/use_case.rs:540` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/use_case.rs:633` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/use_case.rs:804` | `record` | - |
| `<module>` | `crates/uc-application/src/support/host_event_bus.rs:70` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/support/host_event_bus.rs:93` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:16` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:68` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:84` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:115` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:169` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:357` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:362` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:404` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:449` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:460` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:488` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:623` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:631` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:659` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:778` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:786` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:822` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:956` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/publish_blob.rs:138` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/publish_blob.rs:203` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:189` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:205` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:214` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:236` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:248` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:268` | `info_span` | - |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:284` | `info_span` | - |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:289` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:295` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:299` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:410` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:421` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:441` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:453` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:175` | `instrument` | - |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:227` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:261` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:316` | `error` | message-body |
| `bootstrap.network` | `crates/uc-engine/src/assembly/facade.rs:81` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/assembly/host.rs:260` | `warn` | message-body |
| `settings.network` | `crates/uc-engine/src/assembly/lifecycle.rs:53` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/lifecycle.rs:96` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/assembly/lifecycle.rs:107` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:81` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:91` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:128` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:136` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:151` | `warn` | raw-error, message-body |
| `settings.network` | `crates/uc-engine/src/assembly/network.rs:177` | `info` | sensitive-field, message-body |
| `settings.network` | `crates/uc-engine/src/assembly/network.rs:198` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:206` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/assembly/observability/storage_upgrade.rs:37` | `record` | - |
| `<module>` | `crates/uc-engine/src/assembly/observability/storage_upgrade.rs:41` | `record` | - |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:153` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:167` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:179` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:187` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:190` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:198` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine.rs:134` | `instrument` | - |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine.rs:279` | `debug` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine.rs:424` | `instrument` | - |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine.rs:813` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/wire/mod.rs:371` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/wire/mod.rs:402` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/operations/clipboard/capture.rs:22` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/clipboard/query_active.rs:11` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/clipboard/restore.rs:56` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:39` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:54` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:75` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:599` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/peer_connections.rs:67` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/peer_connections.rs:77` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-engine/src/operations/history/delivery.rs:93` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/history.rs:189` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/receive.rs:153` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/receive.rs:160` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/resend.rs:68` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/resend.rs:76` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/resource.rs:68` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/search.rs:177` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/history/search.rs:265` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/config_migration.rs:141` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/diagnostics.rs:79` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/encryption.rs:14` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/encryption.rs:26` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/encryption.rs:40` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/storage.rs:47` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/cancel_invitation.rs:26` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/space/create_space.rs:58` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/space/device_group_choice.rs:19` | `debug` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/device_group_choice.rs:95` | `debug` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/encryption_passphrase.rs:47` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/space/encryption_passphrase.rs:55` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/space/factory_reset.rs:31` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/space/invitation.rs:71` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/space/invitation.rs:79` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/space/join_space.rs:92` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/space/reset_space.rs:18` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/space/session_recovery.rs:58` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/space/setup_state.rs:22` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/space/setup_state.rs:51` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/unlock.rs:54` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/dispatch.rs:539` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/dispatch.rs:549` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/dispatch.rs:767` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:35` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:45` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:50` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:123` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:148` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:114` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:165` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:170` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:320` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:337` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:345` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:352` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:379` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:459` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/mod.rs:343` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/mod.rs:374` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/mod.rs:391` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:108` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:120` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:128` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:846` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:999` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:1007` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:1019` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:1029` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:1031` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:1037` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:1042` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:1192` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/task_shutdown.rs:51` | `record` | - |
| `<module>` | `crates/uc-engine/src/runtime/task_shutdown.rs:52` | `record` | - |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:56` | `debug` | message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:60` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:68` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:75` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:124` | `debug` | message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:128` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:136` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:143` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:55` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:77` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:88` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:122` | `debug` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:155` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:177` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:206` | `debug` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:238` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:250` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/filesystem_store.rs:134` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/filesystem_store.rs:148` | `debug` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/blob/filesystem_store.rs:167` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/filesystem_store.rs:186` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:122` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:140` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:158` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:187` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:194` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:206` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:211` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:216` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:234` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:240` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:243` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:270` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:282` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:314` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:336` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:347` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:354` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:358` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:384` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:390` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:393` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:413` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:430` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:107` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:122` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:141` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:142` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:160` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:162` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/broadcasting_advance.rs:39` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:216` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:225` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:286` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:307` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:315` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/chunked_transfer.rs:439` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/clipboard/chunked_transfer.rs:455` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/clipboard/durable_spool_queue.rs:73` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/normalizer.rs:95` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/normalizer.rs:116` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/normalizer.rs:137` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:45` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:62` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:70` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:84` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:92` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:102` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:109` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:114` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:125` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:165` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_janitor.rs:67` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_janitor.rs:74` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_janitor.rs:83` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_janitor.rs:94` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:182` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:190` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:200` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:249` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:426` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:432` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:59` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:64` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:78` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:89` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:99` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:104` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:114` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:68` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:86` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:108` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:114` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:120` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:126` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:135` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:380` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:382` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:403` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:409` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:525` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:532` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:554` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:560` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:596` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:598` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:237` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:242` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:255` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:265` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:273` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:282` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:310` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/db/pool.rs:102` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/db/pool.rs:109` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/db/pool.rs:116` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/db/pool.rs:142` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/db/pool.rs:405` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/db/pool.rs:408` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/db/repositories/active_clipboard_register_repo.rs:139` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/active_clipboard_register_repo.rs:265` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/db/repositories/active_clipboard_register_repo.rs:286` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/db/repositories/active_clipboard_register_repo.rs:328` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/active_clipboard_register_repo.rs:362` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/blob_repo.rs:46` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/blob_repo.rs:64` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs:72` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs:96` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs:144` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs:169` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs:192` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs:230` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs:259` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs:299` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_event_repo.rs:87` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_event_repo.rs:128` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_event_repo.rs:166` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_selection_repo.rs:154` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_availability_repo.rs:37` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_delivery_repo.rs:109` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_delivery_repo.rs:139` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_file_set_repo.rs:477` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_file_set_repo.rs:509` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_replace_repo.rs:175` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/file_transfer_repo.rs:86` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/file_transfer_repo.rs:136` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/db/repositories/migration_repo.rs:49` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/migration_repo.rs:76` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/migration_repo.rs:105` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/migration_repo.rs:130` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/migration_repo.rs:145` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/migration_repo.rs:174` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/migration_repo.rs:203` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/migration_repo.rs:225` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/legacy_bootstrap.rs:198` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/legacy_bootstrap.rs:273` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:199` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:283` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:343` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:403` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:479` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:604` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:615` | `record` | - |
| `<module>` | `crates/uc-infra/src/file_transfer/privacy_maintenance.rs:68` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/file_transfer/privacy_maintenance.rs:92` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/file_transfer/projection/sqlite.rs:129` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:69` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:103` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:111` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:117` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:123` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:127` | `debug` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/fs/hidden_path.rs:65` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/fs/hidden_path.rs:72` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:39` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:58` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:87` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:112` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:119` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:150` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:136` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:142` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:228` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:240` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:242` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:274` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:288` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:334` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:402` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:435` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:451` | `debug` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:459` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/dispatch_adapter.rs:63` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/dispatch_adapter.rs:75` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/dispatch_adapter.rs:104` | `debug` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:71` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:83` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:95` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:127` | `debug` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:169` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:122` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:130` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:142` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:157` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:175` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:187` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:194` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:201` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:213` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:221` | `debug` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:244` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:257` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:268` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:133` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:143` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:155` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:169` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:183` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:201` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:223` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:236` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:247` | `warn` | raw-error, message-body |
| `iroh.addr_filter` | `crates/uc-infra/src/network/iroh/addr_filter.rs:134` | `warn` | raw-error, message-body |
| `iroh.addr_filter` | `crates/uc-infra/src/network/iroh/addr_filter.rs:155` | `info` | message-body |
| `iroh.addr_filter` | `crates/uc-infra/src/network/iroh/addr_filter.rs:168` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:184` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:214` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:224` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:269` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:279` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:293` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:324` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:380` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:389` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:405` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:416` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:427` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:439` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:452` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:461` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:491` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:520` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:531` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:570` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:595` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:609` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:623` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:648` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:666` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:683` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:709` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:725` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs:159` | `debug` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs:196` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs:251` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs:328` | `debug` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:157` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:163` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:180` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:192` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:230` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:249` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:315` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:318` | `debug` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:322` | `debug` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:338` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:351` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:362` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/connection_channel_adapter.rs:79` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/connection_channel_adapter.rs:83` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/group_update_adapter.rs:91` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/group_update_adapter.rs:165` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/group_update_adapter.rs:172` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:110` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:118` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:122` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:130` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:134` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs:243` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs:376` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs:514` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs:518` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs:596` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs:807` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs:821` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_branch_recovery_adapter.rs:318` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_history_exchange_adapter.rs:89` | `warn` | message-body |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:203` | `warn` | raw-error, message-body |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:243` | `warn` | raw-error, message-body |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:402` | `info` | message-body |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:435` | `warn` | message-body |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:480` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:285` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:298` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:305` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:318` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:321` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:327` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:459` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:494` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:678` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:872` | `instrument` | - |
| `iroh.addr_filter` | `crates/uc-infra/src/network/iroh/node.rs:888` | `info` | message-body |
| `iroh.address_lookup` | `crates/uc-infra/src/network/iroh/node.rs:947` | `info` | message-body |
| `iroh.bind` | `crates/uc-infra/src/network/iroh/node.rs:968` | `info` | message-body |
| `iroh.bind` | `crates/uc-infra/src/network/iroh/node.rs:983` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:1069` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:1595` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:1599` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:1612` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:210` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:221` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:239` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:248` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:282` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:311` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:404` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:406` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:412` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:429` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:618` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:697` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:778` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:858` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:897` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:906` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:915` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:920` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/relay_probe.rs:87` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/network/iroh/relay_probe.rs:148` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/relay_probe.rs:223` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/relay_probe.rs:258` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/runtime_consts.rs:22` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/runtime_consts.rs:40` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/runtime_consts.rs:53` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission/server.rs:256` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission/server.rs:260` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission/server.rs:264` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission/server.rs:270` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission/server.rs:274` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:166` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:172` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:179` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:193` | `debug` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:211` | `trace` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:218` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:222` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:247` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:284` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:294` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:307` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:317` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/pairing/invitation_resolver.rs:44` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_publisher.rs:117` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_publisher.rs:154` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_publisher.rs:198` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_resolver.rs:85` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_resolver.rs:179` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_resolver.rs:185` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_resolver.rs:189` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:138` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:167` | `debug` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:193` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:215` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:224` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:160` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:173` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:206` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:218` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:231` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:314` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:457` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:466` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:478` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:483` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:493` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:507` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/search/rows.rs:166` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/search/rows.rs:175` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:361` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:395` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:939` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1011` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1185` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1202` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1205` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1208` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1454` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1494` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1504` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1526` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1536` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1651` | `debug` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1680` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1686` | `debug` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1707` | `instrument` | raw-error, implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1967` | `instrument` | raw-error, implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1985` | `instrument` | raw-error, implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2019` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2029` | `instrument` | raw-error, implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2066` | `instrument` | raw-error, implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2099` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2109` | `instrument` | raw-error, implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2160` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2192` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2196` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/security/admission_proof.rs:103` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/security/admission_proof.rs:131` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/security/admission_proof.rs:150` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/security/admission_proof.rs:165` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/security/admission_proof.rs:172` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/security/content_protection/blob_store.rs:56` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/security/content_protection/blob_store.rs:59` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/security/content_protection/blob_store.rs:100` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/security/content_protection/blob_store.rs:111` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/security/decrypting_clipboard_event_repo.rs:63` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/security/decrypting_representation_repo.rs:70` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/security/decrypting_representation_repo.rs:107` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/security/decrypting_representation_repo.rs:127` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/security/decrypting_representation_repo.rs:210` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/security/encrypted_blob_store.rs:207` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/security/encrypted_blob_store.rs:267` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/security/encrypted_blob_store.rs:270` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/security/encrypted_blob_store.rs:327` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/security/encrypting_clipboard_event_writer.rs:60` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/security/encrypting_clipboard_event_writer.rs:88` | `debug` | sensitive-field, message-body |
| `uc_infra::security::profile_storage_upgrade` | `crates/uc-infra/src/security/profile_storage_upgrade/diagnostics.rs:54` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/settings/migration.rs:54` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/admission/display.rs:22` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/display.rs:51` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/activation_state.rs:19` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/activation_state.rs:47` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/cancellation.rs:35` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/cancellation.rs:70` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/start_state.rs:17` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/start_state.rs:60` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/recovery/pending_state.rs:22` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/recovery/pending_state.rs:73` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:133` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:147` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:174` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:178` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:237` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:242` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:258` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/state.rs:31` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/state.rs:121` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:374` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:380` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:393` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:404` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1476` | `record` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1499` | `record` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1650` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1653` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1658` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1661` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1679` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1682` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1700` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1705` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1710` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1714` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1719` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1725` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1727` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1730` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1743` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1746` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1764` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1766` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1769` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1772` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1780` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1784` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1789` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1802` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1804` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1807` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1810` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1818` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1822` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1825` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1830` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1840` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1844` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1856` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1858` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1865` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1886` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1888` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1896` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1901` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1904` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1909` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1913` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1916` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1920` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1932` | `info` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1950` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1960` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1964` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1976` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1988` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1990` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1997` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2000` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2007` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2011` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2022` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2026` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2036` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2050` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2061` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2078` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2080` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2083` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2092` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2095` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2105` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2117` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2124` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2130` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2132` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2135` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2148` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2161` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2164` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2169` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2177` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2181` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2185` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2189` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2193` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2930` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2945` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2954` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:439` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:445` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:465` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:469` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:473` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/session.rs:242` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/space/security/session.rs:254` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/session.rs:727` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/space/security/session.rs:731` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/time/timer.rs:45` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/time/timer.rs:53` | `debug` | message-body |
| `uc.connectivity` | `crates/uc-observability-contract/src/diagnostics/connectivity.rs:172` | `event` | sensitive-field |
| `uc.connectivity` | `crates/uc-observability-contract/src/diagnostics/connectivity.rs:175` | `event` | sensitive-field |
| `uc.connectivity` | `crates/uc-observability-contract/src/diagnostics/connectivity.rs:178` | `event` | sensitive-field |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/membership_recovery.rs:92` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:80` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:200` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:204` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:853` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:860` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:867` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:870` | `record` | - |
| `uc.local_diagnostic` | `crates/uc-observability-contract/src/diagnostics/profile_upgrade_backup.rs:10` | `event` | - |

## product analytics

共 2 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `<module>` | `crates/uc-observability-contract/src/analytics/facade.rs:174` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-observability-contract/src/analytics/facade.rs:200` | `warn` | raw-error, message-body |

## delete

共 0 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |

## 输出边界

- 系统日志、本地文件和远程发送默认拒绝 `local debug` 与 `delete`。
- `stable remote` 只能由诊断契约和 Engine 完整能力装饰器产生，并接受固定字段检查。
- `product analytics` 使用独立合同，不共享诊断身份、流程号或发送路径。
- 风险标记用于安排后续清理；被默认拒绝的历史调用点不等于允许输出。
