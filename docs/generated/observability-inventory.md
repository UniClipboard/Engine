# 运行期观测清单

> 由 `scripts/architecture/check-observability-privacy.mjs --write-inventory` 从生产 Rust 源生成。
> 该文件只描述最后一次运行生成命令时的代码事实，不是新增埋点的授权清单。

## stable remote

共 9 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:310` | `span` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:666` | `span` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:681` | `span` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:900` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:910` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:966` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:977` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:1020` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:1039` | `event` | - |

## local operational

共 6 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `observability.health` | `crates/uc-observability-contract/src/diagnostics/mod.rs:567` | `event` | - |
| `observability.health` | `crates/uc-observability-contract/src/diagnostics/mod.rs:582` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:78` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:90` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:107` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:124` | `event` | - |

## local debug

共 1107 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:52` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:61` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:68` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:75` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:81` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:83` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:90` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:97` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:104` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:111` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:118` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:125` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:679` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/lifecycle.rs:116` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:253` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:335` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:345` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:354` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:357` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:439` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:331` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:335` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:342` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:392` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:452` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:495` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:541` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:766` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:776` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:807` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:880` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:892` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:899` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1025` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1075` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1210` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1414` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1496` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:134` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:139` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:178` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:205` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:232` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:239` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:244` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:272` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:293` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:299` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:353` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:361` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:375` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:379` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:388` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:400` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:427` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:443` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:454` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:505` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:559` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:580` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:740` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:770` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:806` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:74` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:79` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:116` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:127` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:155` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:75` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:87` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:108` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:117` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:126` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:140` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:148` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:180` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:187` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:193` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:206` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:212` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:220` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:224` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:307` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:314` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:332` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:340` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:374` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:403` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:477` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:495` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:533` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:104` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:113` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:131` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:168` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:187` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:199` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:103` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:106` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:166` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:172` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:183` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:61` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:67` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:91` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:111` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:120` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:126` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:143` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/toggle_favorite.rs:42` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/toggle_favorite.rs:61` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/toggle_favorite.rs:70` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/toggle_favorite.rs:72` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:139` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:142` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:172` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:190` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:201` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:212` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:220` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:228` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:277` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:289` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:338` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:346` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/local.rs:205` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:225` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:273` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:307` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:320` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:343` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:352` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:380` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:458` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:702` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:711` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:814` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:994` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:1043` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:93` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:117` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:125` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:146` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:159` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:186` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/resource/mod.rs:61` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/resource/mod.rs:90` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/resource/mod.rs:147` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:138` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:149` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:232` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:326` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:331` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:353` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:392` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:411` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_selection.rs:119` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_selection.rs:168` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_selection.rs:174` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:235` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:247` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:253` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:267` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:290` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:301` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:312` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:322` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:326` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:334` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:377` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:401` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:430` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:439` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:445` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:449` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:461` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:465` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:469` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:497` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:555` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:571` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:578` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:45` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:76` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:87` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:115` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:101` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:116` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:166` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:189` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:193` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:209` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:115` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:123` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:140` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:164` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:173` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:81` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:93` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:135` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:144` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:98` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:112` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:154` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:198` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:206` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:237` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:394` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:404` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:415` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:531` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:542` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:548` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:565` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:578` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:865` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:919` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:930` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:954` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1027` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1173` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1208` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1230` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1281` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1286` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1327` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1519` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1525` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1893` | `record` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1915` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1923` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:2344` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:2441` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:2444` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:2625` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:451` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:454` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:457` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:483` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:817` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:838` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:854` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:874` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:911` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:915` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:923` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:965` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:993` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:998` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1044` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1183` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1252` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1271` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1303` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1407` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1598` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1613` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1626` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1630` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1639` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1730` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1736` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:70` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:88` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:106` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:124` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:171` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:252` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:301` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/header.rs:65` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/mod.rs:484` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/mod.rs:516` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/per_peer.rs:88` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/per_peer.rs:176` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/per_peer.rs:190` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs:127` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs:134` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs:143` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs:147` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/existing_local_entry_delivery.rs:152` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/get_entry_delivery_view.rs:196` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/get_entry_delivery_view.rs:306` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/outbound_plan.rs:73` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:69` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:86` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:94` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:102` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:121` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/resend_entry.rs:217` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:61` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:70` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:77` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:86` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:90` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:128` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:210` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:233` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:268` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:326` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:333` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:340` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:346` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:135` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:95` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:116` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:149` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:212` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:233` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:249` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:271` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:293` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:308` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:399` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:419` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/active_register.rs:78` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/active_register.rs:89` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:156` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:183` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:205` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:280` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:340` | `error` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:389` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/write/mobile_consumability.rs:31` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/mobile_consumability.rs:61` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/restore_broadcast.rs:50` | `trace` | message-body |
| `<module>` | `crates/uc-application/src/device/query_local_device/use_case.rs:29` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:178` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:199` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:218` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:232` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:248` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/facade/clipboard/cancel_entry_receive.rs:105` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:354` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:414` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:431` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:449` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:110` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:155` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:186` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:203` | `instrument` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:235` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:243` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:262` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:265` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:270` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:276` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:293` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:308` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:322` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:332` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:345` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:361` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:376` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:397` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:400` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:450` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:467` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:493` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:508` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:530` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:567` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:599` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:612` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:631` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:637` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:642` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:716` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:729` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:736` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:751` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:767` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:849` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:867` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:869` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:878` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:880` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:891` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:895` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:911` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:919` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:927` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:934` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:937` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/live_index/mod.rs:112` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/live_index/mod.rs:128` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/live_index/mod.rs:155` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/query.rs:18` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/search/query.rs:33` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/search/task_scope.rs:213` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/settings/config_migration/facade.rs:77` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/config_migration/facade.rs:101` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/config_migration/facade.rs:120` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/diagnostics.rs:62` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/diagnostics.rs:77` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/diagnostics.rs:101` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/diagnostics.rs:111` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:191` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:214` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:224` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:232` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/settings/facade.rs:236` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:253` | `info` | message-body |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:50` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:62` | `info` | message-body |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:81` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:104` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:111` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:127` | `info` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/acknowledge.rs:38` | `info` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:82` | `debug` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:92` | `debug` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:102` | `debug` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:110` | `debug` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:123` | `debug` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:135` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/admission/invitation/cancel/use_case.rs:53` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/admission/invitation/issue_for_address/use_case.rs:25` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/admission/invitation/issue/use_case.rs:48` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/admission/invitation/issuer.rs:109` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/admission/invitation/query_addresses/use_case.rs:19` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/admission/invitation/query_addresses/use_case.rs:28` | `record` | - |
| `<module>` | `crates/uc-application/src/space/admission/protocol/joiner/activate_complete/execute.rs:170` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/runtime.rs:194` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/runtime.rs:442` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:427` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:442` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:457` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:470` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:486` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:494` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:682` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:702` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:724` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:109` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:156` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:172` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:188` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:212` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:301` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/session/recovery.rs:124` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/unlock_space/use_case.rs:45` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/unlock_space/use_case.rs:72` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/unlock_space/use_case.rs:85` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:95` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:182` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:217` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:322` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/owner.rs:149` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/dependency.rs:32` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/query_member_roster.rs:70` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:403` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:407` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker.rs:193` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker.rs:211` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker.rs:218` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker.rs:420` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/effects.rs:69` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:202` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:271` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:302` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:311` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:383` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:386` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/support/host_event_bus.rs:71` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/support/host_event_bus.rs:98` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:16` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:69` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:90` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:126` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:180` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:360` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:365` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:410` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:455` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:467` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:503` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:644` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:652` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:680` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:805` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:813` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:849` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:983` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/publish_blob.rs:153` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/publish_blob.rs:218` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:194` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:212` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:221` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:246` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:259` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:280` | `info_span` | - |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:296` | `info_span` | - |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:306` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:310` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:426` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:438` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:459` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:475` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/timeout_runtime.rs:34` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:175` | `instrument` | - |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:227` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:261` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:316` | `error` | message-body |
| `bootstrap.network` | `crates/uc-engine/src/assembly/facade.rs:82` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/host.rs:263` | `warn` | message-body |
| `settings.network` | `crates/uc-engine/src/assembly/lifecycle.rs:56` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/lifecycle.rs:100` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/lifecycle.rs:112` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:82` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:93` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:131` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:139` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:155` | `warn` | message-body |
| `settings.network` | `crates/uc-engine/src/assembly/network.rs:181` | `info` | sensitive-field, message-body |
| `settings.network` | `crates/uc-engine/src/assembly/network.rs:202` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:210` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/observability/storage_upgrade.rs:37` | `record` | - |
| `<module>` | `crates/uc-engine/src/assembly/observability/storage_upgrade.rs:41` | `record` | - |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:156` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:174` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:186` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:194` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:201` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:209` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine.rs:119` | `instrument` | - |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine.rs:153` | `instrument` | - |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine.rs:544` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine/outbound_progress.rs:166` | `debug` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/wire/mod.rs:374` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/wire/mod.rs:405` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:400` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:427` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:432` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:437` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:448` | `record` | - |
| `<module>` | `crates/uc-engine/src/operations/clipboard/capture.rs:22` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/clipboard/query_active.rs:12` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/clipboard/restore.rs:56` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:35` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:50` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:71` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:599` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/peer_connections.rs:68` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/peer_connections.rs:78` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-engine/src/operations/history/delivery.rs:93` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/history.rs:190` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/receive.rs:153` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/receive.rs:160` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/resend.rs:68` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/resend.rs:76` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/resource.rs:68` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/search.rs:178` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/search.rs:266` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/config_migration.rs:143` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/diagnostics.rs:83` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/encryption.rs:15` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/encryption.rs:31` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/encryption.rs:49` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/storage.rs:47` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/cancel_invitation.rs:27` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/create_space.rs:59` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/device_group_choice.rs:19` | `debug` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/device_group_choice.rs:96` | `debug` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/encryption_passphrase.rs:48` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/encryption_passphrase.rs:60` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/factory_reset.rs:32` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/invitation.rs:127` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/invitation.rs:139` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/join_space.rs:99` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/reset_space.rs:19` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/session_recovery.rs:62` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/setup_state.rs:22` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/space/setup_state.rs:51` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/unlock.rs:55` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/dispatch.rs:577` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/dispatch.rs:588` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:40` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:54` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:63` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:148` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:173` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:118` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:174` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:183` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:339` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:360` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:372` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:383` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:414` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:498` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/mod.rs:376` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/mod.rs:411` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/mod.rs:450` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/profile_recovery.rs:167` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:125` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:143` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:155` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:814` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:831` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:847` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:1190` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor/shutdown.rs:26` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor/shutdown.rs:29` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor/shutdown.rs:44` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor/shutdown.rs:55` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor/shutdown.rs:66` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/shutdown.rs:70` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/task_shutdown.rs:54` | `record` | - |
| `<module>` | `crates/uc-engine/src/runtime/task_shutdown.rs:55` | `record` | - |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:52` | `debug` | message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:56` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:64` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:71` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:118` | `debug` | message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:122` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:130` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:137` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:56` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:78` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:89` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:123` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:157` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:180` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:209` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:242` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:251` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/filesystem_store.rs:135` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/filesystem_store.rs:149` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/filesystem_store.rs:169` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/filesystem_store.rs:188` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:150` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:168` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:190` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:220` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:227` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:239` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:244` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:249` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:267` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:273` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:276` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:304` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:316` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:349` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:372` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:384` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:391` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:414` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:420` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:423` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:444` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:462` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:111` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:126` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:148` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:172` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:174` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/broadcasting_advance.rs:39` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:216` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:225` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:286` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:307` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:315` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/clipboard/chunked_transfer.rs:484` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/clipboard/chunked_transfer.rs:506` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/clipboard/durable_spool_queue.rs:73` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/normalizer.rs:95` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/normalizer.rs:116` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/normalizer.rs:137` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:46` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:67` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:85` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:95` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:102` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:107` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:118` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:160` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_janitor.rs:68` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_janitor.rs:75` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_janitor.rs:84` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_janitor.rs:96` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:170` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:182` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:195` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:244` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:413` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:419` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:60` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:65` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:79` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:91` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:102` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:107` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:118` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:69` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:87` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:110` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:116` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:122` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:128` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:138` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:407` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:409` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:431` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:438` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:571` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:578` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:600` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:606` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:643` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:645` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:274` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:279` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:292` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:302` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:310` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:320` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:353` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/db/pool.rs:146` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/db/pool.rs:418` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/db/pool.rs:421` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/db/repositories/active_clipboard_register_repo.rs:145` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/active_clipboard_register_repo.rs:263` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/db/repositories/active_clipboard_register_repo.rs:291` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/db/repositories/active_clipboard_register_repo.rs:335` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/active_clipboard_register_repo.rs:370` | `debug_span` | - |
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
| `<module>` | `crates/uc-infra/src/db/repositories/entry_delivery_repo.rs:132` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_delivery_repo.rs:160` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_file_set_repo.rs:525` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_file_set_repo.rs:557` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_replace_repo.rs:175` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/file_transfer_repo.rs:88` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/file_transfer_repo.rs:138` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/db/repositories/migration_repo.rs:49` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/migration_repo.rs:78` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/migration_repo.rs:109` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/migration_repo.rs:135` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/migration_repo.rs:152` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/migration_repo.rs:183` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/migration_repo.rs:213` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/migration_repo.rs:240` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/legacy_bootstrap.rs:198` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/legacy_bootstrap.rs:275` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:209` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:293` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:353` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:413` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:487` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:612` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:623` | `record` | - |
| `<module>` | `crates/uc-infra/src/file_transfer/privacy_maintenance.rs:68` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/file_transfer/privacy_maintenance.rs:98` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/file_transfer/projection/sqlite.rs:129` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:82` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:120` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:132` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:138` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:144` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:148` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/fs/hidden_path.rs:65` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/fs/hidden_path.rs:72` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:40` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:60` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:88` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:114` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:121` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:152` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:137` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:143` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:227` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:237` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:239` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:269` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:282` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:322` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:380` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:407` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:422` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:430` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/dispatch_adapter.rs:64` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/dispatch_adapter.rs:76` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/dispatch_adapter.rs:105` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:72` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:84` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:96` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:128` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:179` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:139` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:155` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:174` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:186` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:193` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:200` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:211` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:220` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:149` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:164` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:179` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:197` | `debug` | sensitive-field, message-body |
| `iroh.addr_filter` | `crates/uc-infra/src/network/iroh/addr_filter.rs:135` | `warn` | message-body |
| `iroh.addr_filter` | `crates/uc-infra/src/network/iroh/addr_filter.rs:157` | `info` | message-body |
| `iroh.addr_filter` | `crates/uc-infra/src/network/iroh/addr_filter.rs:170` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:188` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:224` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:234` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:279` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:289` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:303` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:336` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:398` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:407` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:423` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:434` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:446` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:458` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:471` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:480` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:510` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:539` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:551` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:590` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:616` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:630` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:644` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:669` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:687` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:704` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:731` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:748` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs:174` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs:210` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs:270` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs:347` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:162` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:195` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:235` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:263` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:336` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:339` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:347` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/connection_channel_adapter.rs:79` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/connection_channel_adapter.rs:83` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/group_update_adapter.rs:94` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/group_update_adapter.rs:168` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/group_update_adapter.rs:175` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:117` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:125` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:129` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:137` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:141` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_branch_recovery_adapter.rs:314` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_history_exchange_adapter.rs:128` | `warn` | message-body |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:205` | `warn` | message-body |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:245` | `warn` | message-body |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:404` | `info` | message-body |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:437` | `warn` | message-body |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:482` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:394` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:429` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:601` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:791` | `instrument` | - |
| `iroh.addr_filter` | `crates/uc-infra/src/network/iroh/node.rs:807` | `info` | message-body |
| `iroh.address_lookup` | `crates/uc-infra/src/network/iroh/node.rs:866` | `info` | message-body |
| `iroh.bind` | `crates/uc-infra/src/network/iroh/node.rs:887` | `info` | message-body |
| `iroh.bind` | `crates/uc-infra/src/network/iroh/node.rs:902` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:987` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:1439` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:1443` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:1460` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node/shutdown.rs:53` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/node/shutdown.rs:89` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node/shutdown.rs:110` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:196` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:230` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:259` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:356` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:358` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:367` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:384` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:573` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:652` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:729` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:809` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:847` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:856` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:865` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:870` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/relay_probe.rs:151` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/network/iroh/relay_probe.rs:212` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/relay_probe.rs:289` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/relay_probe.rs:329` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/runtime_consts.rs:22` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/runtime_consts.rs:40` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/runtime_consts.rs:53` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission/server.rs:257` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission/server.rs:261` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission/server.rs:265` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission/server.rs:271` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission/server.rs:275` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:176` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:190` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:209` | `trace` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:216` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:220` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:255` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:265` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:278` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:292` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/pairing/invitation_resolver.rs:44` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_publisher.rs:117` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_publisher.rs:156` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_publisher.rs:200` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_resolver.rs:85` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_resolver.rs:179` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_resolver.rs:185` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_resolver.rs:189` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:160` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:192` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:221` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:246` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:255` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:188` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:201` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:229` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:241` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:254` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:339` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:388` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:514` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:523` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:535` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:540` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:550` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:564` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/search/rows.rs:167` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/search/rows.rs:177` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:361` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:395` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:930` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1003` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1174` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1191` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1194` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1197` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1441` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1483` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1497` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1517` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1531` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1638` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1667` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1673` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1694` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1946` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1962` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1994` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2008` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2043` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2074` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2084` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2133` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2167` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2171` | `warn` | message-body |
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
| `<module>` | `crates/uc-infra/src/security/encrypted_blob_store.rs:210` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/security/encrypted_blob_store.rs:270` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/security/encrypted_blob_store.rs:273` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/security/encrypted_blob_store.rs:330` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/security/encrypting_clipboard_event_writer.rs:60` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/security/encrypting_clipboard_event_writer.rs:88` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/security/profile_key_recovery.rs:887` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/security/profile_key_recovery.rs:898` | `warn` | message-body |
| `uc_infra::security::profile_storage_upgrade` | `crates/uc-infra/src/security/profile_storage_upgrade/diagnostics.rs:54` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/settings/migration.rs:54` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/admission/display.rs:24` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/display.rs:53` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/activation_state.rs:19` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/activation_state.rs:47` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/activation.rs:650` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/cancellation.rs:35` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/cancellation.rs:70` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/start_state.rs:17` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/start_state.rs:60` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/recovery/pending_state.rs:22` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/recovery/pending_state.rs:74` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:120` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:134` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/state.rs:31` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/state.rs:122` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:445` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:451` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:464` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:480` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1571` | `record` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1594` | `record` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1748` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1790` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1797` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1806` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1813` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1835` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1838` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1856` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1861` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1865` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1869` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1880` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1888` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1906` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1914` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1937` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1939` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1947` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1960` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1965` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1978` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1980` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1988` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2001` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2015` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2036` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2038` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2046` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2056` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2066` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2074` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2101` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2111` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2138` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2140` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2147` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2158` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2176` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2197` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2214` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2216` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2221` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2225` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2228` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2238` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2250` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2263` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2266` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2279` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2292` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2295` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2304` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2317` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2321` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2325` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2333` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2337` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2580` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:3138` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:3153` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:3162` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:579` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:585` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:605` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:609` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:613` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/session.rs:311` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/space/security/session.rs:323` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/session.rs:833` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/space/security/session.rs:837` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/time/timer.rs:45` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/time/timer.rs:53` | `debug` | message-body |
| `uc.connectivity` | `crates/uc-observability-contract/src/diagnostics/connectivity.rs:183` | `event` | sensitive-field |
| `uc.connectivity` | `crates/uc-observability-contract/src/diagnostics/connectivity.rs:186` | `event` | sensitive-field |
| `uc.connectivity` | `crates/uc-observability-contract/src/diagnostics/connectivity.rs:189` | `event` | sensitive-field |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/membership_recovery.rs:92` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:80` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:200` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:204` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:864` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:871` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:878` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:881` | `record` | - |
| `uc.local_diagnostic` | `crates/uc-observability-contract/src/diagnostics/profile_upgrade_backup.rs:10` | `event` | - |

## product analytics

共 2 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `<module>` | `crates/uc-observability-contract/src/analytics/facade.rs:182` | `warn` | message-body |
| `<module>` | `crates/uc-observability-contract/src/analytics/facade.rs:209` | `warn` | message-body |

## delete

共 0 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |

## 输出边界

- 系统日志、本地文件和远程发送默认拒绝 `local debug` 与 `delete`。
- `stable remote` 只能由诊断契约和 Engine 完整能力装饰器产生，并接受固定字段检查。
- `product analytics` 使用独立合同，不共享诊断身份、流程号或发送路径。
- 风险标记用于安排后续清理；被默认拒绝的历史调用点不等于允许输出。
