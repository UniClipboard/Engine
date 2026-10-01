# 运行期观测清单

> 由 `scripts/architecture/check-observability-privacy.mjs --write-inventory` 从生产 Rust 源生成。
> 该文件只描述最后一次运行生成命令时的代码事实，不是新增埋点的授权清单。

## stable remote

共 9 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:310` | `span` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:682` | `span` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:697` | `span` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:916` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:926` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:982` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:993` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:1036` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:1055` | `event` | - |

## local operational

共 7 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `observability.health` | `crates/uc-observability-contract/src/diagnostics/mod.rs:583` | `event` | - |
| `observability.health` | `crates/uc-observability-contract/src/diagnostics/mod.rs:598` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:80` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:92` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:109` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:126` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:139` | `event` | - |

## local debug

共 1251 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:56` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:65` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:72` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:79` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:85` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:91` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:98` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:105` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:112` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:119` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:126` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:133` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:144` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:155` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:168` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:728` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime/event_recorder.rs:47` | `record` | - |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime/startup_lifecycle.rs:57` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime/worker_join.rs:60` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime/worker_shutdown.rs:27` | `warn` | message-body |
| `<module>` | `bindings/uc-ohos-napi/src/host.rs:68` | `warn` | message-body |
| `<module>` | `bindings/uc-ohos-napi/src/lib.rs:266` | `warn` | message-body |
| `<module>` | `bindings/uc-ohos-napi/src/lib.rs:285` | `warn` | message-body |
| `<module>` | `bindings/uc-ohos-napi/src/local_diagnostics.rs:279` | `warn` | message-body |
| `<module>` | `bindings/uc-ohos-napi/src/runtime.rs:969` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/application.rs:252` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/application.rs:259` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/application.rs:266` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/current.rs:18` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/current.rs:24` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/lifecycle.rs:117` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/lifecycle.rs:284` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:256` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:338` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:348` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:357` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:360` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:442` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:259` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:279` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:309` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:344` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:351` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:358` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:408` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:468` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:511` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:554` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:778` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:788` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:823` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:900` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:912` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:919` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1058` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1108` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1243` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1447` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1529` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:136` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:141` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:180` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:207` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:234` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:241` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:246` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:274` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:295` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:301` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:355` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:363` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:377` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:381` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:390` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:402` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:429` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:445` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:456` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:507` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:561` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:582` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:742` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:772` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:808` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:76` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:81` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:118` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:129` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:157` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:77` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:89` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:110` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:119` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:128` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:142` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:150` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:182` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:189` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:195` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:208` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:214` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:222` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:226` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:309` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:316` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:334` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:342` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:376` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:405` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:479` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:497` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:535` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:104` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:113` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:131` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:168` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:191` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:207` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:104` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:107` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:167` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:173` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:184` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:65` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:71` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:95` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:115` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:124` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:135` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:152` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/toggle_favorite.rs:44` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/toggle_favorite.rs:67` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/toggle_favorite.rs:76` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/toggle_favorite.rs:82` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:140` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:143` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:173` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:191` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:202` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:213` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:221` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:229` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:278` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:289` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:337` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:345` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/local.rs:206` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:229` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:277` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:311` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:324` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:347` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:356` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:384` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:464` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:692` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:730` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:739` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:842` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:1022` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:1071` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:91` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:115` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:123` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:144` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:157` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:184` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/resource/mod.rs:61` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/resource/mod.rs:90` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/resource/mod.rs:147` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:138` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:140` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:154` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:168` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:238` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:332` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:337` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:359` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:401` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:420` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_selection.rs:119` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_selection.rs:121` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_selection.rs:170` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_selection.rs:176` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:237` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:249` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:255` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:269` | `instrument` | - |
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
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:451` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:467` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:474` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:478` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:506` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:569` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:584` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:590` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:43` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:74` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:85` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:113` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:103` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:118` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:168` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:194` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:198` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:214` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:227` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:117` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:125` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:142` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:166` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:172` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:81` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:93` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:135` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:144` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:100` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:114` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:156` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:200` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:208` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:239` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:398` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:408` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:419` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:535` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:546` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:552` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:569` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:582` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:869` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:923` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:934` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:958` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1031` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1177` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1212` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1234` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1285` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1290` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1331` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1523` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1529` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1897` | `record` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1919` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1927` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:2348` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:2446` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:2449` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:2630` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:455` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:458` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:465` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:496` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:829` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:850` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:866` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:886` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:923` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:927` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:935` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:977` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1003` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1011` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1056` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1195` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1264` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1282` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1317` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1421` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1612` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1628` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1639` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1646` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1655` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1746` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1753` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:72` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:90` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:108` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:126` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:173` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:254` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:302` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/header.rs:64` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/mod.rs:483` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/mod.rs:515` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/per_peer.rs:87` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/per_peer.rs:175` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/per_peer.rs:189` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs:126` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs:133` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs:142` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs:146` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/existing_local_entry_delivery.rs:153` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/get_entry_delivery_view.rs:196` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/get_entry_delivery_view.rs:306` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/outbound_plan.rs:72` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:67` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:76` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:93` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:101` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:109` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:128` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/resend_entry.rs:231` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/resend_entry.rs:236` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:59` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:68` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:75` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:84` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:88` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:131` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:216` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:239` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:274` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:335` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:342` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:349` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:355` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:134` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:95` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:116` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:149` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:212` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:233` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:249` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:271` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:293` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:304` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:313` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:321` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:417` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:437` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/active_register.rs:77` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/active_register.rs:88` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:158` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:185` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:207` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:282` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:341` | `error` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:389` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/write/mobile_consumability.rs:31` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/mobile_consumability.rs:61` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/restore_broadcast.rs:50` | `trace` | message-body |
| `<module>` | `crates/uc-application/src/device/query_local_device/use_case.rs:31` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:180` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:201` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:220` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:234` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:250` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/facade/clipboard/cancel_entry_receive.rs:105` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:354` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:414` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:431` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:449` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:110` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:155` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:186` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:203` | `instrument` | - |
| `<module>` | `crates/uc-application/src/profile/startup/use_case.rs:34` | `instrument` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:239` | `instrument` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:247` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:266` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:269` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:274` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:280` | `instrument` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:297` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:312` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:326` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:336` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:349` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:365` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:380` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:401` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:404` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:454` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:471` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:497` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:518` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:540` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:582` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:617` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:626` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:647` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:666` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:672` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:677` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:751` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:764` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:771` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:786` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:802` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:884` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:902` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:904` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:913` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:915` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:926` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:933` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:954` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:965` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:978` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:990` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:996` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/live_index/mod.rs:112` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/live_index/mod.rs:128` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/live_index/mod.rs:155` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/query.rs:19` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/search/query.rs:34` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/search/task_scope.rs:214` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/settings/config_migration/facade.rs:77` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/config_migration/facade.rs:101` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/config_migration/facade.rs:120` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/diagnostics.rs:73` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/settings/diagnostics.rs:93` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/diagnostics.rs:114` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/diagnostics.rs:147` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/diagnostics.rs:157` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:216` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:240` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:244` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/settings/facade.rs:265` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:273` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/settings/facade.rs:277` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:294` | `info` | message-body |
| `<module>` | `crates/uc-application/src/settings/relay_configuration.rs:356` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/settings/relay_configuration.rs:379` | `info` | message-body |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:50` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:62` | `info` | message-body |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:81` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:104` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:111` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:127` | `info` | message-body |
| `<module>` | `crates/uc-application/src/settings/upgrade/acknowledge.rs:47` | `instrument` | - |
| `upgrade` | `crates/uc-application/src/settings/upgrade/acknowledge.rs:51` | `warn` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/acknowledge.rs:69` | `info` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:83` | `warn` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:112` | `debug` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:122` | `debug` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:132` | `debug` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:140` | `debug` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:151` | `debug` | message-body |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:161` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/admission/invitation/cancel/use_case.rs:27` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/admission/invitation/cancel/use_case.rs:54` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/admission/invitation/issue_for_address/use_case.rs:25` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/admission/invitation/issue/use_case.rs:48` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/admission/invitation/issuer.rs:110` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/admission/invitation/query_addresses/use_case.rs:19` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/admission/invitation/query_addresses/use_case.rs:28` | `record` | - |
| `<module>` | `crates/uc-application/src/space/admission/protocol/joiner/activate_complete/execute.rs:170` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/admission/protocol/joiner/cancel_join/execute.rs:31` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/admission/protocol/joiner/cancel_join/execute.rs:36` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/admission/protocol/joiner/cancel_join/execute.rs:42` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/record.rs:29` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/record.rs:35` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/runtime.rs:197` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/runtime.rs:215` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/runtime.rs:225` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/runtime.rs:465` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:432` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:447` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:462` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:475` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:491` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:499` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:687` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:707` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:729` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/change_encryption_passphrase/use_case.rs:30` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:110` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:157` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:173` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:189` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:213` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:302` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/lock_space_session/use_case.rs:27` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/query_space_access_state/use_case.rs:53` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/rebuild_space/use_case.rs:65` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/recover_space_session/use_case.rs:35` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/reset_space/use_case.rs:32` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/session/activity.rs:226` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/session/recovery.rs:125` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/session/recovery.rs:139` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/unlock_space/use_case.rs:46` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/unlock_space/use_case.rs:73` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/unlock_space/use_case.rs:86` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/lifecycle/upgrade_space/use_case.rs:71` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/decide_device_trust_change/use_case.rs:43` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/group_update_delivery.rs:231` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/group_update_delivery.rs:262` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/group_update_delivery.rs:269` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:45` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:98` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:182` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:215` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:320` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:586` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/initializer.rs:45` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/maintenance/use_case.rs:50` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/owner.rs:150` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/dependency.rs:33` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/query_member_roster.rs:70` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/reconcile_history_evidence/use_case.rs:24` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/issuer.rs:95` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:86` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:357` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:408` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:416` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:564` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:573` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/remove_space_member/use_case.rs:33` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/remove_space_member/use_case.rs:38` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/remove_space_member/use_case.rs:44` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/remove_space_member/use_case.rs:104` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/remove_space_member/use_case.rs:350` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/remove_space_member/use_case.rs:361` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/resolve_conflict/use_case.rs:36` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/worker.rs:194` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker.rs:212` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker.rs:219` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker.rs:421` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/effects.rs:70` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:205` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:246` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:277` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:307` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:316` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:387` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:390` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:479` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:483` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:488` | `info` | message-body |
| `<module>` | `crates/uc-application/src/support/host_event_bus.rs:69` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/support/host_event_bus.rs:96` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:18` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:70` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:91` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:127` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:181` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:361` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:371` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:421` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:466` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:478` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:514` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:660` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:673` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:706` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:836` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:849` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:890` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:1029` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/fetch_blob.rs:77` | `instrument` | - |
| `<module>` | `crates/uc-application/src/transfer/blob/fetch_blob.rs:136` | `instrument` | - |
| `<module>` | `crates/uc-application/src/transfer/blob/publish_blob.rs:60` | `instrument` | - |
| `<module>` | `crates/uc-application/src/transfer/blob/publish_blob.rs:154` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/publish_blob.rs:219` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:196` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:214` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:223` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:248` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:261` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:282` | `info_span` | - |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:298` | `info_span` | - |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:308` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:312` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:428` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:440` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:461` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:477` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/timeout_runtime.rs:34` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:120` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:183` | `instrument` | - |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:240` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:274` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:336` | `error` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:343` | `info` | message-body |
| `bootstrap.network` | `crates/uc-engine/src/assembly/facade.rs:82` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/host.rs:266` | `warn` | message-body |
| `settings.network` | `crates/uc-engine/src/assembly/lifecycle.rs:58` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/lifecycle.rs:115` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/lifecycle.rs:127` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/mobile_lan.rs:25` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/mobile_lan.rs:29` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/mobile_lan.rs:33` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:84` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:95` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:133` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:140` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:155` | `warn` | message-body |
| `settings.network` | `crates/uc-engine/src/assembly/network.rs:181` | `info` | message-body |
| `settings.network` | `crates/uc-engine/src/assembly/network.rs:200` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:208` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/observability/storage_upgrade.rs:37` | `record` | - |
| `<module>` | `crates/uc-engine/src/assembly/observability/storage_upgrade.rs:41` | `record` | - |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:156` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:174` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:186` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:194` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:201` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:208` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/settings_notification.rs:58` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine.rs:120` | `instrument` | - |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine.rs:154` | `instrument` | - |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine.rs:545` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine/outbound_progress.rs:196` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/wire/mod.rs:378` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/wire/mod.rs:409` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:393` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:420` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:425` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:430` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:441` | `record` | - |
| `<module>` | `crates/uc-engine/src/operations/clipboard/capture.rs:22` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/clipboard/restore.rs:56` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:43` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:64` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:592` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/peer_connections.rs:69` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/peer_connections.rs:79` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/delivery.rs:93` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/history.rs:190` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/receive.rs:153` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/receive.rs:160` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/resend.rs:68` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/resend.rs:76` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/resource.rs:68` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/search.rs:225` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/search.rs:319` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/config_migration.rs:32` | `record` | - |
| `<module>` | `crates/uc-engine/src/operations/settings/config_migration.rs:59` | `record` | - |
| `<module>` | `crates/uc-engine/src/operations/settings/config_migration.rs:108` | `record` | - |
| `<module>` | `crates/uc-engine/src/operations/settings/config_migration.rs:145` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/config_migration.rs:157` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/diagnostics.rs:70` | `record` | - |
| `<module>` | `crates/uc-engine/src/operations/settings/diagnostics.rs:83` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/diagnostics.rs:95` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/encryption.rs:14` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/encryption.rs:30` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/encryption.rs:48` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/settings/storage.rs:47` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/cancel_invitation.rs:26` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/create_space.rs:59` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/device_group_choice.rs:20` | `debug` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/device_group_choice.rs:97` | `debug` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/encryption_passphrase.rs:47` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/encryption_passphrase.rs:59` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/factory_reset.rs:34` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/invitation.rs:126` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/invitation.rs:138` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/join_space.rs:98` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/reset_space.rs:18` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/session_recovery.rs:61` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/setup_state.rs:22` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/space/setup_state.rs:51` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/unlock.rs:55` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/dispatch.rs:586` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/dispatch.rs:597` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:59` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:70` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:78` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:85` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:150` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:173` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:198` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_file.rs:25` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_file.rs:30` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_file.rs:35` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:118` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:174` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:183` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:339` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:360` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:372` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:383` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:414` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:498` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/mod.rs:383` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/mod.rs:418` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/mod.rs:470` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/mod.rs:486` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/profile_recovery.rs:228` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/profile_recovery.rs:342` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/profile_recovery.rs:380` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/profile_recovery.rs:499` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:133` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:147` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:165` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:177` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:1059` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:1258` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor/shutdown.rs:27` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor/shutdown.rs:30` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor/shutdown.rs:45` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor/shutdown.rs:56` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor/shutdown.rs:67` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/shutdown.rs:70` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/task_shutdown.rs:54` | `record` | - |
| `<module>` | `crates/uc-engine/src/runtime/task_shutdown.rs:55` | `record` | - |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:52` | `debug` | message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:56` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:64` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:68` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:114` | `debug` | message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:118` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:126` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:129` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:60` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:82` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:92` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:125` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:158` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:181` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:210` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:242` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:251` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/filesystem_store.rs:135` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/filesystem_store.rs:149` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/filesystem_store.rs:169` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/blob/filesystem_store.rs:188` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:154` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:172` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:194` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:224` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:231` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:246` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:251` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:256` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:274` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:280` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:286` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:314` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:326` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:359` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:382` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:394` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:401` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:427` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:433` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:439` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:460` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:478` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:110` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:125` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:150` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:174` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:176` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/broadcasting_advance.rs:40` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:217` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:224` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:284` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:301` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:305` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/chunked_transfer.rs:484` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/clipboard/chunked_transfer.rs:506` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/clipboard/durable_spool_queue.rs:73` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/normalizer.rs:98` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/normalizer.rs:119` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/normalizer.rs:140` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:50` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:71` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:89` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:99` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:106` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:111` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:122` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:164` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_janitor.rs:70` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_janitor.rs:77` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_janitor.rs:86` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_janitor.rs:98` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:74` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:83` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:207` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:219` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:232` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:271` | `record` | - |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:282` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:456` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:462` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:62` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:67` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:81` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:93` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:104` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:109` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:120` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:70` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:88` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:111` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:117` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:123` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:129` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:139` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:411` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:433` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:442` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:575` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:582` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:594` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:620` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:657` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:659` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:669` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:695` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:701` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:274` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:279` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:292` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:302` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:310` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:320` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:353` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/db/pool.rs:148` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/db/pool.rs:420` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/db/pool.rs:423` | `info` | message-body |
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
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_selection_repo.rs:155` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_availability_repo.rs:37` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_delivery_repo.rs:132` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_delivery_repo.rs:159` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_file_set_repo.rs:525` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_file_set_repo.rs:557` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_replace_repo.rs:175` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/file_transfer_repo.rs:92` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/file_transfer_repo.rs:142` | `debug` | sensitive-field, message-body |
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
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:210` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:294` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:354` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:414` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:488` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:613` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:624` | `record` | - |
| `<module>` | `crates/uc-infra/src/device/mod.rs:46` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/device/storage.rs:63` | `warn` | message-body |
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
| `<module>` | `crates/uc-infra/src/fs/work_directory.rs:16` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:139` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:145` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:229` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:239` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:241` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:271` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:284` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:324` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:382` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:409` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:424` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:432` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/dispatch_adapter.rs:64` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/dispatch_adapter.rs:76` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/dispatch_adapter.rs:105` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:72` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:84` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:96` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:128` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:179` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:139` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:154` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:172` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:183` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:189` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:193` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:205` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:213` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:149` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:163` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:177` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:194` | `debug` | message-body |
| `iroh.addr_filter` | `crates/uc-infra/src/network/iroh/addr_filter.rs:134` | `warn` | message-body |
| `iroh.addr_filter` | `crates/uc-infra/src/network/iroh/addr_filter.rs:156` | `info` | message-body |
| `iroh.addr_filter` | `crates/uc-infra/src/network/iroh/addr_filter.rs:168` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:147` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:183` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:191` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:236` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:244` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:258` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:291` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:352` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:360` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:375` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:382` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:392` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:404` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:417` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:426` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:453` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:478` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:487` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:523` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:546` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:556` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:566` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:588` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:603` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:617` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:642` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:657` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs:174` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs:210` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs:270` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs:347` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:162` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:195` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:234` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:262` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:332` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:335` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:343` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/connection_channel_adapter.rs:79` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/connection_channel_adapter.rs:83` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/group_update_adapter.rs:95` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/group_update_adapter.rs:169` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/group_update_adapter.rs:176` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:101` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:120` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:129` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:138` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_branch_recovery_adapter.rs:268` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_branch_recovery_adapter.rs:315` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_branch_recovery_adapter.rs:320` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_branch_recovery_adapter.rs:326` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_branch_recovery_adapter.rs:354` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_history_exchange_adapter.rs:128` | `warn` | message-body |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:204` | `warn` | message-body |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:244` | `warn` | message-body |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:403` | `info` | message-body |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:436` | `warn` | message-body |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:481` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:396` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:428` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:603` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:793` | `instrument` | - |
| `iroh.addr_filter` | `crates/uc-infra/src/network/iroh/node.rs:809` | `info` | message-body |
| `iroh.address_lookup` | `crates/uc-infra/src/network/iroh/node.rs:867` | `info` | message-body |
| `iroh.bind` | `crates/uc-infra/src/network/iroh/node.rs:888` | `info` | message-body |
| `iroh.bind` | `crates/uc-infra/src/network/iroh/node.rs:902` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:986` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:1437` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:1441` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:1458` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node/shutdown.rs:54` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/node/shutdown.rs:90` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/node/shutdown.rs:111` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:197` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:231` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:238` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:252` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:269` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:298` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:326` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:377` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:379` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:388` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:405` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:594` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:673` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:750` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:830` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:868` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:877` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:886` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs:891` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/persistable_addr.rs:135` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/relay_probe.rs:152` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/network/iroh/relay_probe.rs:213` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/relay_probe.rs:267` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/relay_probe.rs:307` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/runtime_consts.rs:22` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/runtime_consts.rs:40` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/runtime_consts.rs:53` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/session_generation.rs:262` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission/server.rs:254` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission/server.rs:268` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission/server.rs:275` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission/server.rs:282` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission/server.rs:288` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission/server.rs:295` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:178` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:189` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:207` | `trace` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:213` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:217` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:251` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:261` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:274` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:288` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/pairing/invitation_resolver.rs:45` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_publisher.rs:117` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_publisher.rs:155` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_publisher.rs:196` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_resolver.rs:86` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_resolver.rs:180` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_resolver.rs:186` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/pairing/mdns_resolver.rs:190` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:160` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:192` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:221` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:246` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:255` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:189` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:199` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:227` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:239` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:252` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:337` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:386` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:512` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:521` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:533` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:538` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:548` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:562` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/search/rows.rs:167` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/search/rows.rs:177` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:367` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:401` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:431` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:951` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1024` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1278` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1294` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1302` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1310` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1559` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1601` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1615` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1635` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1649` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1712` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1741` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1746` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1811` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2063` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2079` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2111` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2125` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2160` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2191` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2201` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2250` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2287` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2296` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/security/admission_proof.rs:104` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/security/admission_proof.rs:128` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/security/admission_proof.rs:143` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/security/admission_proof.rs:155` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/security/admission_proof.rs:160` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/security/content_protection/blob_store.rs:57` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/security/content_protection/blob_store.rs:60` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/security/content_protection/blob_store.rs:103` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/security/content_protection/blob_store.rs:114` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/security/decrypting_clipboard_event_repo.rs:63` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/security/decrypting_representation_repo.rs:70` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/security/decrypting_representation_repo.rs:107` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/security/decrypting_representation_repo.rs:127` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/security/decrypting_representation_repo.rs:213` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/security/decrypting_representation_repo.rs:221` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/security/encrypted_blob_store.rs:211` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/security/encrypted_blob_store.rs:271` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/security/encrypted_blob_store.rs:274` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/security/encrypted_blob_store.rs:331` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/security/encrypting_clipboard_event_writer.rs:60` | `trace` | message-body |
| `<module>` | `crates/uc-infra/src/security/encrypting_clipboard_event_writer.rs:88` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra/src/security/profile_key_recovery.rs:266` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/security/profile_key_recovery.rs:283` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/security/profile_key_recovery.rs:349` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/security/profile_key_recovery.rs:356` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/security/profile_key_recovery.rs:902` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/security/profile_key_recovery.rs:913` | `warn` | message-body |
| `uc_infra::security::profile_storage_upgrade` | `crates/uc-infra/src/security/profile_storage_upgrade/diagnostics.rs:54` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/security/profile_upgrade_backup/security_materials.rs:82` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/security/v3_device_management_reset/mod.rs:240` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/security/v3_device_management_reset/mod.rs:246` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/security/v3_device_management_reset/mod.rs:353` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/security/v3_device_management_reset/mod.rs:372` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/settings/migration.rs:54` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/admission/display.rs:25` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/space/admission/display.rs:59` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/space/admission/failure_log.rs:18` | `warn` | - |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/activation_state.rs:20` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/activation_state.rs:55` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/activation.rs:651` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/cancellation.rs:36` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/cancellation.rs:76` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/start_state.rs:18` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/start_state.rs:65` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/space/admission/recovery/pending_state.rs:23` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/space/admission/recovery/pending_state.rs:37` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/space/admission/recovery/pending_state.rs:92` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:121` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:135` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/state.rs:35` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/state.rs:127` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:355` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:362` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:457` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:462` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:474` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:488` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1151` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1177` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1558` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1603` | `record` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1626` | `record` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1780` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1822` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1829` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1838` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1845` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1867` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1870` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1886` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1891` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1895` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1899` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1910` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1917` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1934` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1941` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1962` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1964` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1972` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1982` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1990` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2002` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2004` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2012` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2022` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2039` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2059` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2061` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2069` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2079` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2089` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2100` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2124` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2137` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2144` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2170` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2172` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2179` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2190` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2211` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2232` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2248` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2250` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2255` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2259` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2265` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2272` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2284` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2297` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2304` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2322` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2334` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2337` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2346` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2358` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2362` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2366` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2373` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2377` | `error` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2619` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:3177` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:3192` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:3201` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/encryption_passphrase_change.rs:92` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/encryption_passphrase_change.rs:99` | `info` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/encryption_passphrase_change.rs:103` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:580` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:586` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:606` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:610` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:614` | `warn` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/session.rs:336` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/space/security/session.rs:348` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/space/security/session.rs:860` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/space/security/session.rs:864` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/time/timer.rs:46` | `debug` | message-body |
| `<module>` | `crates/uc-infra/src/time/timer.rs:54` | `debug` | message-body |
| `uc.connectivity` | `crates/uc-observability-contract/src/diagnostics/connectivity.rs:183` | `event` | sensitive-field |
| `uc.connectivity` | `crates/uc-observability-contract/src/diagnostics/connectivity.rs:186` | `event` | sensitive-field |
| `uc.connectivity` | `crates/uc-observability-contract/src/diagnostics/connectivity.rs:189` | `event` | sensitive-field |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/membership_recovery.rs:92` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:80` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:200` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:204` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:880` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:887` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:894` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:897` | `record` | - |
| `uc.local_diagnostic` | `crates/uc-observability-contract/src/diagnostics/profile_upgrade_backup.rs:11` | `event` | - |
| `<module>` | `crates/uc-observability-contract/src/log_event.rs:25` | `event` | - |
| `<module>` | `crates/uc-observability-runtime/src/module_log.rs:261` | `record` | - |

## product analytics

共 2 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `<module>` | `crates/uc-observability-contract/src/analytics/facade.rs:183` | `warn` | message-body |
| `<module>` | `crates/uc-observability-contract/src/analytics/facade.rs:210` | `warn` | message-body |

## delete

共 0 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |

## 输出边界

- 系统日志、本地文件和远程发送默认拒绝 `local debug` 与 `delete`。
- `stable remote` 只能由诊断契约和 Engine 完整能力装饰器产生，并接受固定字段检查。
- `product analytics` 使用独立合同，不共享诊断身份、流程号或发送路径。
- 风险标记用于安排后续清理；被默认拒绝的历史调用点不等于允许输出。
