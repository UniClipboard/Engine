# 运行期观测清单

> 由 `scripts/architecture/check-observability-privacy.mjs --write-inventory` 从生产 Rust 源生成。
> 该文件只描述最后一次运行生成命令时的代码事实，不是新增埋点的授权清单。

## stable remote

共 9 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:369` | `span` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:749` | `span` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:764` | `span` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:989` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:999` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:1055` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:1066` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:1109` | `event` | - |
| `uc.telemetry` | `crates/uc-observability-contract/src/diagnostics/mod.rs:1128` | `event` | - |

## local operational

共 9 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `observability.health` | `crates/uc-observability-contract/src/diagnostics/mod.rs:650` | `event` | - |
| `observability.health` | `crates/uc-observability-contract/src/diagnostics/mod.rs:665` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:80` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:92` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:109` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:126` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:139` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:150` | `event` | - |
| `observability.health` | `crates/uc-observability-runtime/src/remote_health.rs:161` | `event` | - |

## local debug

共 1330 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `<module>` | `bindings/uc-engine-uniffi/src/observability.rs:36` | `record` | - |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:59` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:68` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:75` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:82` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:88` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:94` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:101` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:108` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:115` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:122` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:129` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:136` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:147` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:158` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:171` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:842` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime/event_recorder.rs:47` | `record` | - |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime/startup_lifecycle.rs:57` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime/worker_join.rs:60` | `warn` | message-body |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime/worker_shutdown.rs:27` | `warn` | message-body |
| `<module>` | `bindings/uc-ohos-napi/src/host.rs:66` | `warn` | message-body |
| `<module>` | `bindings/uc-ohos-napi/src/lib.rs:314` | `warn` | message-body |
| `<module>` | `bindings/uc-ohos-napi/src/lib.rs:333` | `warn` | message-body |
| `<module>` | `bindings/uc-ohos-napi/src/local_diagnostics.rs:304` | `warn` | message-body |
| `<module>` | `bindings/uc-ohos-napi/src/observability.rs:38` | `record` | - |
| `<module>` | `bindings/uc-ohos-napi/src/runtime.rs:1110` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/facade/file_upload.rs:423` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/facade/file_upload.rs:477` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/facade/outbound_adapter.rs:111` | `info` | sensitive-field, raw-error, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/facade/outbound_adapter.rs:122` | `info` | sensitive-field, raw-error, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/facade/outbound_adapter.rs:128` | `warn` | sensitive-field, raw-error, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/mobile_sync/file_staging.rs:139` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/mobile_sync/file_staging.rs:145` | `debug` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/mobile_sync/file_staging.rs:229` | `debug` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/mobile_sync/file_staging.rs:239` | `debug` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/mobile_sync/file_staging.rs:241` | `debug` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/mobile_sync/file_staging.rs:271` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/mobile_sync/file_staging.rs:284` | `debug` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/mobile_sync/file_staging.rs:324` | `debug` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/mobile_sync/file_staging.rs:382` | `debug` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/mobile_sync/file_staging.rs:409` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/mobile_sync/file_staging.rs:424` | `debug` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/mobile_sync/file_staging.rs:432` | `debug` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:339` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:462` | `instrument` | - |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:493` | `info` | sensitive-field, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:518` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:532` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:673` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:723` | `warn` | sensitive-field, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:733` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:738` | `info` | sensitive-field, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:761` | `warn` | sensitive-field, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:790` | `warn` | sensitive-field, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:804` | `warn` | sensitive-field, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:916` | `info` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:944` | `debug` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:979` | `info` | sensitive-field, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:993` | `debug` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:1008` | `debug` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs:1020` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/authenticate_basic.rs:105` | `instrument` | implicit-arguments |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/authenticate_request.rs:43` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/get_file.rs:105` | `instrument` | implicit-arguments |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/get_file.rs:123` | `debug` | sensitive-field, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/get_file.rs:132` | `debug` | sensitive-field, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/get_file.rs:142` | `warn` | sensitive-field, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/get_file.rs:153` | `debug` | sensitive-field, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/get_file.rs:166` | `debug` | sensitive-field, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/get_file.rs:176` | `warn` | sensitive-field, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/get_file.rs:184` | `debug` | sensitive-field, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/get_file.rs:201` | `debug` | sensitive-field, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/get_latest_doc.rs:85` | `instrument` | - |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/get_latest_doc.rs:116` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/get_latest_doc.rs:121` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/get_latest_doc.rs:131` | `debug` | sensitive-field, message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/get_settings.rs:108` | `instrument` | implicit-arguments |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/list_devices.rs:107` | `instrument` | implicit-arguments |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/list_lan_interfaces.rs:72` | `instrument` | implicit-arguments |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/list_lan_interfaces.rs:78` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/register_device.rs:293` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/register_device.rs:313` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/register_device.rs:340` | `instrument` | implicit-arguments |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/register_device.rs:624` | `warn` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/register_device.rs:633` | `info` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/revoke_device.rs:52` | `instrument` | implicit-arguments |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/update_device.rs:124` | `instrument` | implicit-arguments |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/update_device.rs:207` | `debug` | message-body |
| `<module>` | `compatibility/uc-mobile-lan/src/usecases/update_settings.rs:119` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/application.rs:256` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/application.rs:263` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/application.rs:270` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/current.rs:18` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/current.rs:24` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/lifecycle.rs:111` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/lifecycle.rs:273` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:261` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:344` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:354` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:363` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:366` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:449` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/transport/error.rs:83` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/transport/error.rs:115` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/transport/pull_receiver.rs:55` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/transport/pull_receiver.rs:91` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/transport/pull.rs:27` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/active/transport/receiver.rs:55` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/active/transport/sender.rs:26` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:261` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:281` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:312` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:347` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:354` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:361` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:411` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:471` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:514` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:557` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:785` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:795` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:830` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:907` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:919` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:926` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1065` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1115` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1250` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1454` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1536` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history_tags/flow.rs:66` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history_tags/flow.rs:115` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history_tags/flow.rs:156` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history_tags/flow.rs:177` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history_tags/flow.rs:200` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history_tags/flow.rs:254` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history_tags/flow.rs:271` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:105` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:110` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:131` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:158` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:185` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:192` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:197` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:225` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:246` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:252` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:306` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:314` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:328` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:332` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:341` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:353` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:380` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:396` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:407` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:458` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:512` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:533` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:693` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:723` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:759` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:57` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:70` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:80` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:90` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:69` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:79` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:112` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:120` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:130` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:136` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:145` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:151` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:159` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:163` | `info` | sensitive-field, message-body |
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
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:72` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:75` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:120` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:126` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:137` | `info` | message-body |
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
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:141` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:144` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:174` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:193` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:204` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:215` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:223` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:231` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:280` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:291` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:339` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:347` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/local.rs:206` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:234` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:282` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:316` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:329` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:352` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:361` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:394` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:475` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:703` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:741` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:750` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:853` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:1035` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:1086` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:99` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:123` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:131` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:153` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:166` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:193` | `warn` | sensitive-field, message-body |
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
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:71` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:88` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:102` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:134` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:146` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:176` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:103` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:118` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:168` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:194` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:198` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:214` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:227` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:113` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:121` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:125` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:146` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:163` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:177` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:183` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:193` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:80` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:92` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:134` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:143` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:98` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:114` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:159` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:206` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:214` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:238` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:471` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:474` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:481` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:516` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:876` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:897` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:913` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:933` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:971` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:975` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:983` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1039` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1075` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1084` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1129` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1342` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1363` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1432` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1451` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1489` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1606` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1807` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1823` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1834` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1841` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1850` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1941` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1948` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:73` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:91` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:109` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:127` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:174` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:298` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:346` | `info` | message-body |
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
| `<module>` | `crates/uc-application/src/clipboard/sync/existing_local_entry_delivery.rs:155` | `info` | sensitive-field, message-body |
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
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:149` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:170` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:203` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:241` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:284` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:305` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:321` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:343` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:370` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:383` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:395` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:404` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:500` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs:520` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/active_register.rs:77` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/active_register.rs:88` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:159` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:186` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:208` | `info` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:285` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:346` | `error` | message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:394` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/write/mobile_consumability.rs:31` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/clipboard/write/mobile_consumability.rs:61` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/device/query_local_device/use_case.rs:31` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:181` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:202` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:221` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:235` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:251` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:319` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:379` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:396` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:414` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:110` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:155` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:186` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:203` | `instrument` | - |
| `<module>` | `crates/uc-application/src/profile/startup/use_case.rs:34` | `instrument` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:251` | `instrument` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:259` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:278` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:281` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:286` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:292` | `instrument` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:309` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:324` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:338` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:348` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:361` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:377` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:392` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:413` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:416` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:466` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:483` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:509` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:530` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:552` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:570` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:612` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:647` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:656` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:677` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:696` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:702` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:707` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:781` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:794` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:801` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:816` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:832` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:914` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:932` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:934` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:943` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:945` | `info` | message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:956` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:963` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:984` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:1000` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:1013` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:1026` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:1038` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:1044` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/live_index/mod.rs:115` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/live_index/mod.rs:131` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/search/live_index/mod.rs:168` | `debug` | sensitive-field, message-body |
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
| `<module>` | `crates/uc-application/src/settings/facade.rs:238` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:262` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:266` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/settings/facade.rs:287` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:295` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/settings/facade.rs:299` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:316` | `info` | message-body |
| `<module>` | `crates/uc-application/src/settings/relay_configuration.rs:464` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/settings/relay_configuration.rs:487` | `info` | message-body |
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
| `<module>` | `crates/uc-application/src/space/admission/protocol/recovery/recover_pending/execute/legacy_sponsor.rs:39` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/admission/protocol/recovery/recover_pending/execute/legacy_sponsor.rs:51` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/admission/protocol/recovery/recover_pending/execute/legacy_sponsor.rs:73` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/admission/protocol/recovery/recover_pending/execute/legacy_sponsor.rs:83` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/admission/protocol/recovery/recover_pending/execute/legacy_sponsor.rs:96` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/record.rs:29` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/record.rs:35` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/runtime.rs:233` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/runtime.rs:251` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/runtime.rs:261` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/runtime.rs:542` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/runtime.rs:657` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/transport/admission.rs:192` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/transport/admission.rs:195` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/transport/owner.rs:230` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/transport/owner.rs:298` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/transport/owner.rs:421` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/connectivity/peer_connections/transport/owner.rs:439` | `warn` | message-body |
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
| `<module>` | `crates/uc-application/src/space/membership/group_update_delivery.rs:325` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/group_update_delivery.rs:372` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/group_update_delivery.rs:379` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/group_update/coordinator.rs:68` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/group_update/coordinator.rs:88` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/group_update/recovery.rs:102` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/group_update/recovery.rs:207` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:46` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:99` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:183` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:216` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:321` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:589` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/history_transport/exchange.rs:34` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/history_transport/receiver.rs:167` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/history_transport/receiver.rs:172` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/history_transport/receiver.rs:186` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/history_transport/receiver.rs:191` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/history_transport/retention.rs:16` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/history_transport/retention.rs:25` | `warn` | message-body |
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
| `<module>` | `crates/uc-application/src/space/membership/remove_space_member/use_case.rs:107` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/remove_space_member/use_case.rs:380` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/remove_space_member/use_case.rs:391` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/resolve_conflict/use_case.rs:36` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/membership/worker.rs:193` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker.rs:211` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker.rs:218` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker.rs:427` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/effects.rs:70` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:212` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:253` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:284` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:314` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:323` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:394` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:397` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:486` | `info` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:490` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/space/membership/worker/history_sync.rs:495` | `info` | message-body |
| `<module>` | `crates/uc-application/src/support/host_event_bus.rs:69` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/support/host_event_bus.rs:96` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:18` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:70` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:91` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:127` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:181` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:395` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:405` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:556` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:766` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:958` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:1090` | `record` | - |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:1145` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/fetch_blob.rs:91` | `instrument` | - |
| `<module>` | `crates/uc-application/src/transfer/blob/fetch_blob.rs:174` | `instrument` | - |
| `<module>` | `crates/uc-application/src/transfer/blob/notifications.rs:89` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/publish_blob.rs:60` | `instrument` | - |
| `<module>` | `crates/uc-application/src/transfer/blob/publish_blob.rs:154` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/publish_blob.rs:219` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/transport/runtime/owner.rs:163` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/transport/runtime/provide.rs:115` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/transfer/blob/transport/runtime/provide.rs:151` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:199` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:217` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:232` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:265` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:278` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:299` | `info_span` | - |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:315` | `info_span` | - |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:325` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:329` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:445` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:457` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:478` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:494` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/file/timeout_runtime.rs:34` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/cancellation/owner.rs:85` | `instrument` | - |
| `<module>` | `crates/uc-application/src/transfer/receive/cancellation/owner.rs:114` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:182` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:192` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:203` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:319` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:330` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:336` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:353` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:366` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:697` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:779` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:818` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:861` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:952` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:1151` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:1206` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:1231` | `info` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:1293` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:1298` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:1339` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:1540` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:1546` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:1940` | `record` | raw-error |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:1962` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:1970` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:2368` | `debug` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:2467` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/materialization/workflow.rs:2470` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:131` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:195` | `instrument` | - |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:252` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:286` | `error` | sensitive-field, message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:348` | `error` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:355` | `info` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/retry/owner.rs:207` | `warn` | message-body |
| `<module>` | `crates/uc-application/src/transfer/receive/retry/owner.rs:227` | `warn` | message-body |
| `bootstrap.network` | `crates/uc-engine/src/assembly/facade.rs:84` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/host.rs:227` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/assembly/host.rs:287` | `warn` | raw-error, message-body |
| `settings.network` | `crates/uc-engine/src/assembly/lifecycle.rs:73` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/lifecycle.rs:132` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/lifecycle.rs:144` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/mobile_lan.rs:25` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/mobile_lan.rs:29` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/mobile_lan.rs:33` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:85` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:96` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:134` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:141` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:156` | `warn` | message-body |
| `settings.network` | `crates/uc-engine/src/assembly/network.rs:182` | `info` | message-body |
| `settings.network` | `crates/uc-engine/src/assembly/network.rs:201` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:209` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/observability/storage_upgrade.rs:37` | `record` | - |
| `<module>` | `crates/uc-engine/src/assembly/observability/storage_upgrade.rs:41` | `record` | - |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:158` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:176` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:188` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:196` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:203` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:210` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/settings_notification.rs:58` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine.rs:113` | `instrument` | - |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine.rs:142` | `instrument` | - |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine.rs:687` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine/outbound_progress.rs:196` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/wire/mod.rs:379` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/assembly/wire/mod.rs:410` | `info` | message-body |
| `uc_engine::observability` | `crates/uc-engine/src/contract/observability.rs:69` | `info` | message-body |
| `uc_engine::observability` | `crates/uc-engine/src/contract/observability.rs:75` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:123` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:490` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:493` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:553` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:599` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:622` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:626` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:657` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:662` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:667` | `record` | - |
| `<module>` | `crates/uc-engine/src/dev/space_work.rs:678` | `record` | - |
| `<module>` | `crates/uc-engine/src/engine/lifecycle.rs:233` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/engine/lifecycle.rs:239` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/operations/clipboard/capture.rs:22` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/clipboard/restore.rs:56` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:46` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:67` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:634` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/peer_connections.rs:69` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/operations/device/peer_connections.rs:79` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/delivery.rs:93` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/history.rs:190` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/receive.rs:279` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/receive.rs:286` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/resend.rs:68` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/resend.rs:76` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/resource.rs:68` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/search.rs:225` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/search.rs:319` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/history/tags.rs:201` | `error` | message-body |
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
| `<module>` | `crates/uc-engine/src/operations/space/device_group_choice.rs:115` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/device_group_choice.rs:125` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/device_group_choice.rs:133` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/device_group_choice.rs:140` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/encryption_passphrase.rs:47` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/encryption_passphrase.rs:59` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/factory_reset.rs:34` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/invitation.rs:74` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/invitation.rs:160` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/invitation.rs:172` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/join_space.rs:98` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/reset_space.rs:18` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/session_recovery.rs:61` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/setup_state.rs:22` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/operations/space/setup_state.rs:51` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/operations/space/unlock.rs:55` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/census.rs:39` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/dispatch.rs:638` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/dispatch.rs:649` | `warn` | message-body |
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
| `<module>` | `crates/uc-engine/src/runtime/mod.rs:393` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/mod.rs:428` | `error` | raw-error, message-body |
| `<module>` | `crates/uc-engine/src/runtime/mod.rs:483` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/mod.rs:499` | `error` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/profile_recovery.rs:181` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/profile_recovery.rs:291` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/profile_recovery.rs:329` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/profile_recovery.rs:436` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/profile_recovery.rs:454` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/profile_recovery.rs:459` | `warn` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/profile_recovery.rs:613` | `info` | message-body |
| `<module>` | `crates/uc-engine/src/runtime/profile_recovery.rs:751` | `warn` | message-body |
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
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:154` | `info_span` | - |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:172` | `error` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:194` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:224` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:231` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:246` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:251` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:256` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:274` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:280` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:286` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:314` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:326` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:359` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:382` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:394` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:401` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:427` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:433` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:439` | `error` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:460` | `error` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_blob_worker.rs:478` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_runtime.rs:110` | `info` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_runtime.rs:125` | `info` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_runtime.rs:150` | `info` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_runtime.rs:174` | `info` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/background_runtime.rs:176` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/broadcasting_advance.rs:40` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/change_origin.rs:217` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/change_origin.rs:224` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/change_origin.rs:284` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/change_origin.rs:301` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/change_origin.rs:305` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/chunked_transfer.rs:461` | `info_span` | - |
| `<module>` | `crates/uc-infra-content/src/clipboard/chunked_transfer.rs:484` | `info_span` | - |
| `<module>` | `crates/uc-infra-content/src/clipboard/durable_spool_queue.rs:73` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/normalizer.rs:98` | `trace` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/normalizer.rs:119` | `trace` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/normalizer.rs:140` | `trace` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/payload_resolver.rs:50` | `info_span` | - |
| `<module>` | `crates/uc-infra-content/src/clipboard/payload_resolver.rs:71` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/payload_resolver.rs:89` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/payload_resolver.rs:99` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/payload_resolver.rs:106` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/payload_resolver.rs:111` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/payload_resolver.rs:122` | `error` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/payload_resolver.rs:164` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_janitor.rs:70` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_janitor.rs:77` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_janitor.rs:86` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_janitor.rs:98` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_manager.rs:74` | `info` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_manager.rs:83` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_manager.rs:207` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_manager.rs:219` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_manager.rs:232` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_manager.rs:271` | `record` | - |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_manager.rs:282` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_manager.rs:456` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_manager.rs:462` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_scanner.rs:62` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_scanner.rs:67` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_scanner.rs:81` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_scanner.rs:93` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_scanner.rs:104` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_scanner.rs:109` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/spool_scanner.rs:120` | `info` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/staged_reconciler.rs:70` | `info` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/staged_reconciler.rs:88` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/staged_reconciler.rs:111` | `info` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/staged_reconciler.rs:117` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/staged_reconciler.rs:123` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/staged_reconciler.rs:129` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/clipboard/staged_reconciler.rs:139` | `info` | message-body |
| `<module>` | `crates/uc-infra-content/src/content_protection/blob_store.rs:57` | `info_span` | - |
| `<module>` | `crates/uc-infra-content/src/content_protection/blob_store.rs:60` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/content_protection/blob_store.rs:103` | `info_span` | - |
| `<module>` | `crates/uc-infra-content/src/content_protection/blob_store.rs:114` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/decrypting_clipboard_event_repo.rs:63` | `trace` | message-body |
| `<module>` | `crates/uc-infra-content/src/decrypting_representation_repo.rs:93` | `trace` | message-body |
| `<module>` | `crates/uc-infra-content/src/decrypting_representation_repo.rs:130` | `trace` | message-body |
| `<module>` | `crates/uc-infra-content/src/decrypting_representation_repo.rs:150` | `trace` | message-body |
| `<module>` | `crates/uc-infra-content/src/decrypting_representation_repo.rs:231` | `warn` | message-body |
| `<module>` | `crates/uc-infra-content/src/decrypting_representation_repo.rs:240` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra-content/src/encrypted_blob_store.rs:211` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/encrypted_blob_store.rs:271` | `info_span` | - |
| `<module>` | `crates/uc-infra-content/src/encrypted_blob_store.rs:274` | `debug` | message-body |
| `<module>` | `crates/uc-infra-content/src/encrypted_blob_store.rs:331` | `info_span` | - |
| `<module>` | `crates/uc-infra-content/src/encrypting_clipboard_event_writer.rs:60` | `trace` | message-body |
| `<module>` | `crates/uc-infra-content/src/encrypting_clipboard_event_writer.rs:88` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra-crypto/src/mls_group.rs:575` | `warn` | message-body |
| `<module>` | `crates/uc-infra-crypto/src/mls_group.rs:581` | `warn` | message-body |
| `<module>` | `crates/uc-infra-crypto/src/mls_group.rs:601` | `warn` | message-body |
| `<module>` | `crates/uc-infra-crypto/src/mls_group.rs:605` | `warn` | message-body |
| `<module>` | `crates/uc-infra-crypto/src/mls_group.rs:609` | `warn` | message-body |
| `<module>` | `crates/uc-infra-local/src/blob/blob_writer.rs:60` | `debug_span` | - |
| `<module>` | `crates/uc-infra-local/src/blob/blob_writer.rs:82` | `debug` | message-body |
| `<module>` | `crates/uc-infra-local/src/blob/blob_writer.rs:92` | `debug` | message-body |
| `<module>` | `crates/uc-infra-local/src/blob/blob_writer.rs:125` | `debug` | message-body |
| `<module>` | `crates/uc-infra-local/src/blob/blob_writer.rs:158` | `warn` | message-body |
| `<module>` | `crates/uc-infra-local/src/blob/blob_writer.rs:181` | `debug_span` | - |
| `<module>` | `crates/uc-infra-local/src/blob/blob_writer.rs:210` | `debug` | message-body |
| `<module>` | `crates/uc-infra-local/src/blob/blob_writer.rs:242` | `debug_span` | - |
| `<module>` | `crates/uc-infra-local/src/blob/blob_writer.rs:251` | `debug` | message-body |
| `<module>` | `crates/uc-infra-local/src/blob/filesystem_store.rs:135` | `debug` | message-body |
| `<module>` | `crates/uc-infra-local/src/blob/filesystem_store.rs:149` | `debug` | message-body |
| `<module>` | `crates/uc-infra-local/src/blob/filesystem_store.rs:169` | `debug` | message-body |
| `<module>` | `crates/uc-infra-local/src/blob/filesystem_store.rs:188` | `debug` | message-body |
| `<module>` | `crates/uc-infra-local/src/device/mod.rs:46` | `info` | message-body |
| `<module>` | `crates/uc-infra-local/src/device/storage.rs:63` | `warn` | message-body |
| `<module>` | `crates/uc-infra-local/src/fs/atomic_publish.rs:82` | `warn` | message-body |
| `<module>` | `crates/uc-infra-local/src/fs/atomic_publish.rs:120` | `warn` | message-body |
| `<module>` | `crates/uc-infra-local/src/fs/atomic_publish.rs:132` | `debug` | message-body |
| `<module>` | `crates/uc-infra-local/src/fs/atomic_publish.rs:138` | `warn` | message-body |
| `<module>` | `crates/uc-infra-local/src/fs/atomic_publish.rs:144` | `debug` | message-body |
| `<module>` | `crates/uc-infra-local/src/fs/atomic_publish.rs:148` | `debug` | message-body |
| `<module>` | `crates/uc-infra-local/src/fs/hidden_path.rs:65` | `debug` | message-body |
| `<module>` | `crates/uc-infra-local/src/fs/hidden_path.rs:72` | `debug` | message-body |
| `<module>` | `crates/uc-infra-local/src/fs/inbound_target.rs:40` | `warn` | message-body |
| `<module>` | `crates/uc-infra-local/src/fs/inbound_target.rs:60` | `warn` | message-body |
| `<module>` | `crates/uc-infra-local/src/fs/inbound_target.rs:88` | `warn` | message-body |
| `<module>` | `crates/uc-infra-local/src/fs/inbound_target.rs:114` | `debug` | message-body |
| `<module>` | `crates/uc-infra-local/src/fs/inbound_target.rs:121` | `warn` | message-body |
| `<module>` | `crates/uc-infra-local/src/fs/inbound_target.rs:152` | `warn` | message-body |
| `<module>` | `crates/uc-infra-local/src/fs/work_directory.rs:16` | `warn` | message-body |
| `<module>` | `crates/uc-infra-local/src/settings/migration.rs:54` | `info` | message-body |
| `<module>` | `crates/uc-infra-local/src/time/timer.rs:46` | `debug` | message-body |
| `<module>` | `crates/uc-infra-local/src/time/timer.rs:54` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/clipboard/receiver.rs:121` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/clipboard/receiver.rs:178` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/clipboard/receiver.rs:232` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/clipboard/receiver.rs:303` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/clipboard/receiver.rs:311` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/clipboard/receiver.rs:319` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/active_clipboard/local_work.rs:32` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/active_clipboard/local_work.rs:59` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/active_clipboard/pull_serve_adapter.rs:42` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/active_clipboard/pull_serve_adapter.rs:55` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/active_clipboard/receiver_adapter.rs:44` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/active_clipboard/receiver_adapter.rs:57` | `warn` | message-body |
| `iroh.addr_filter` | `crates/uc-infra-p2p/src/network/iroh/addr_filter.rs:56` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/blob_exchange/handler.rs:32` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/blob_exchange/io.rs:84` | `record` | raw-error |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/blob_exchange/io.rs:99` | `record` | raw-error |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/blobs.rs:95` | `instrument` | - |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/blobs.rs:133` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/blobs.rs:141` | `instrument` | - |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/blobs.rs:188` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/blobs.rs:196` | `instrument` | - |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/blobs.rs:210` | `instrument` | - |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/blobs.rs:219` | `instrument` | - |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/blobs.rs:232` | `instrument` | - |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/blobs.rs:238` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/connection_census.rs:94` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/connection_channel_adapter.rs:78` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/connection_channel_adapter.rs:82` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/connectivity/clipboard_handler.rs:33` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/connectivity/presence_handler.rs:35` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/discovery/runtime.rs:24` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/discovery/runtime.rs:69` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/group_update_adapter.rs:95` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/group_update_adapter.rs:111` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/identity_store.rs:101` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/identity_store.rs:120` | `instrument` | - |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/identity_store.rs:129` | `instrument` | - |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/identity_store.rs:138` | `instrument` | - |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/membership_branch_recovery_adapter.rs:268` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/membership_branch_recovery_adapter.rs:315` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/membership_branch_recovery_adapter.rs:320` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/membership_branch_recovery_adapter.rs:326` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/membership_branch_recovery_adapter.rs:354` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/membership_history_exchange_adapter.rs:51` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/membership_history_exchange_adapter.rs:59` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/membership_history_exchange_adapter.rs:71` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/membership_history_exchange_adapter/address.rs:38` | `warn` | message-body |
| `iroh.net_recovery` | `crates/uc-infra-p2p/src/network/iroh/net_recovery.rs:204` | `warn` | message-body |
| `iroh.net_recovery` | `crates/uc-infra-p2p/src/network/iroh/net_recovery.rs:244` | `warn` | message-body |
| `iroh.net_recovery` | `crates/uc-infra-p2p/src/network/iroh/net_recovery.rs:403` | `info` | message-body |
| `iroh.net_recovery` | `crates/uc-infra-p2p/src/network/iroh/net_recovery.rs:436` | `warn` | message-body |
| `iroh.net_recovery` | `crates/uc-infra-p2p/src/network/iroh/net_recovery.rs:481` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/network_log_e2e.rs:72` | `record` | - |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/node.rs:456` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/node.rs:488` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/node.rs:658` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/node.rs:863` | `instrument` | - |
| `iroh.addr_filter` | `crates/uc-infra-p2p/src/network/iroh/node.rs:879` | `info` | message-body |
| `iroh.address_lookup` | `crates/uc-infra-p2p/src/network/iroh/node.rs:934` | `info` | message-body |
| `iroh.bind` | `crates/uc-infra-p2p/src/network/iroh/node.rs:965` | `info` | message-body |
| `iroh.bind` | `crates/uc-infra-p2p/src/network/iroh/node.rs:979` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/node.rs:1045` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/node.rs:1534` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/node.rs:1538` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/node.rs:1555` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/node/shutdown.rs:54` | `instrument` | - |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/node/shutdown.rs:104` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/node/shutdown.rs:125` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/persistable_addr.rs:135` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/protocol_router.rs:80` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/relay_probe.rs:152` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/relay_probe.rs:213` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/relay_probe.rs:267` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/relay_probe.rs:307` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/runtime_consts.rs:22` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/runtime_consts.rs:40` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/runtime_consts.rs:53` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/session_generation.rs:305` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/session_generation.rs:324` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/space_admission/server.rs:279` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/space_admission/server.rs:288` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/space_admission/server.rs:307` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/space_admission/server.rs:315` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/space_admission/server.rs:336` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/space_admission/server.rs:344` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/space_admission/server.rs:352` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/space_admission/server.rs:359` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/space_admission/server.rs:367` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/transfer_progress_adapter.rs:206` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/transfer_progress_adapter.rs:217` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/transfer_progress_adapter.rs:235` | `trace` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/transfer_progress_adapter.rs:241` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/transfer_progress_adapter.rs:245` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/transfer_progress_adapter.rs:279` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/transfer_progress_adapter.rs:289` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/transfer_progress_adapter.rs:302` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/network/iroh/transfer_progress_adapter.rs:316` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/pairing/invitation_resolver.rs:61` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/pairing/mdns_interfaces.rs:23` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/pairing/mdns_publisher.rs:117` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/pairing/mdns_publisher.rs:156` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/pairing/mdns_resolver.rs:87` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/pairing/mdns_resolver.rs:112` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/pairing/mdns_resolver.rs:188` | `info` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/pairing/mdns_resolver.rs:194` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/pairing/mdns_resolver.rs:198` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/client.rs:160` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/client.rs:192` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/client.rs:221` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/client.rs:246` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/client.rs:255` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/invitation_adapter.rs:189` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/invitation_adapter.rs:199` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/invitation_adapter.rs:227` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/invitation_adapter.rs:239` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/invitation_adapter.rs:252` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/invitation_adapter.rs:337` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/invitation_adapter.rs:386` | `warn` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/invitation_adapter.rs:512` | `instrument` | - |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/invitation_adapter.rs:521` | `instrument` | - |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/invitation_adapter.rs:533` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/invitation_adapter.rs:538` | `debug` | message-body |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/invitation_adapter.rs:548` | `instrument` | - |
| `<module>` | `crates/uc-infra-p2p/src/rendezvous/invitation_adapter.rs:562` | `instrument` | - |
| `<module>` | `crates/uc-infra-profile/src/config_migration/adapter.rs:412` | `info` | message-body |
| `<module>` | `crates/uc-infra-profile/src/config_migration/adapter.rs:434` | `error` | message-body |
| `<module>` | `crates/uc-infra-profile/src/config_migration/adapter.rs:443` | `error` | message-body |
| `<module>` | `crates/uc-infra-profile/src/config_migration/adapter.rs:576` | `info` | message-body |
| `<module>` | `crates/uc-infra-profile/src/config_migration/adapter.rs:583` | `instrument` | - |
| `<module>` | `crates/uc-infra-profile/src/config_migration/adapter.rs:595` | `instrument` | - |
| `<module>` | `crates/uc-infra-profile/src/config_migration/adapter.rs:621` | `info` | message-body |
| `<module>` | `crates/uc-infra-profile/src/config_migration/adapter.rs:658` | `warn` | message-body |
| `<module>` | `crates/uc-infra-profile/src/config_migration/adapter.rs:660` | `info` | message-body |
| `<module>` | `crates/uc-infra-profile/src/config_migration/adapter.rs:670` | `instrument` | - |
| `<module>` | `crates/uc-infra-profile/src/config_migration/adapter.rs:696` | `error` | message-body |
| `<module>` | `crates/uc-infra-profile/src/config_migration/adapter.rs:702` | `warn` | message-body |
| `<module>` | `crates/uc-infra-profile/src/config_migration/staging.rs:274` | `info` | message-body |
| `<module>` | `crates/uc-infra-profile/src/config_migration/staging.rs:279` | `error` | message-body |
| `<module>` | `crates/uc-infra-profile/src/config_migration/staging.rs:292` | `info` | message-body |
| `<module>` | `crates/uc-infra-profile/src/config_migration/staging.rs:302` | `error` | message-body |
| `<module>` | `crates/uc-infra-profile/src/config_migration/staging.rs:310` | `error` | message-body |
| `<module>` | `crates/uc-infra-profile/src/config_migration/staging.rs:320` | `info` | message-body |
| `<module>` | `crates/uc-infra-profile/src/config_migration/staging.rs:353` | `info` | message-body |
| `<module>` | `crates/uc-infra-profile/src/security/profile_key_recovery.rs:204` | `warn` | message-body |
| `<module>` | `crates/uc-infra-profile/src/security/profile_key_recovery.rs:221` | `warn` | message-body |
| `<module>` | `crates/uc-infra-profile/src/security/profile_key_recovery.rs:287` | `info` | message-body |
| `<module>` | `crates/uc-infra-profile/src/security/profile_key_recovery.rs:294` | `warn` | message-body |
| `<module>` | `crates/uc-infra-profile/src/security/profile_key_recovery.rs:840` | `warn` | message-body |
| `<module>` | `crates/uc-infra-profile/src/security/profile_key_recovery.rs:851` | `warn` | message-body |
| `uc_infra_profile::security::profile_storage_upgrade` | `crates/uc-infra-profile/src/security/profile_storage_upgrade/diagnostics.rs:55` | `error` | message-body |
| `<module>` | `crates/uc-infra-profile/src/security/profile_upgrade_backup/security_materials.rs:83` | `warn` | message-body |
| `<module>` | `crates/uc-infra-profile/src/security/v3_device_management_reset/mod.rs:240` | `info` | message-body |
| `<module>` | `crates/uc-infra-profile/src/security/v3_device_management_reset/mod.rs:246` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra-profile/src/security/v3_device_management_reset/mod.rs:353` | `info` | message-body |
| `<module>` | `crates/uc-infra-profile/src/security/v3_device_management_reset/mod.rs:372` | `info` | message-body |
| `<module>` | `crates/uc-infra-profile/src/space/admission/display.rs:28` | `instrument` | - |
| `<module>` | `crates/uc-infra-profile/src/space/admission/display.rs:41` | `instrument` | - |
| `<module>` | `crates/uc-infra-profile/src/space/admission/failure_log.rs:18` | `warn` | - |
| `<module>` | `crates/uc-infra-profile/src/space/admission/joiner/activation_state.rs:20` | `instrument` | - |
| `<module>` | `crates/uc-infra-profile/src/space/admission/joiner/activation_state.rs:55` | `instrument` | - |
| `<module>` | `crates/uc-infra-profile/src/space/admission/joiner/activation.rs:651` | `warn` | message-body |
| `<module>` | `crates/uc-infra-profile/src/space/admission/joiner/cancellation.rs:36` | `instrument` | - |
| `<module>` | `crates/uc-infra-profile/src/space/admission/joiner/cancellation.rs:78` | `instrument` | - |
| `<module>` | `crates/uc-infra-profile/src/space/admission/joiner/start_state.rs:18` | `instrument` | - |
| `<module>` | `crates/uc-infra-profile/src/space/admission/joiner/start_state.rs:65` | `instrument` | - |
| `<module>` | `crates/uc-infra-profile/src/space/admission/recovery/pending_state.rs:23` | `instrument` | - |
| `<module>` | `crates/uc-infra-profile/src/space/admission/recovery/pending_state.rs:43` | `instrument` | - |
| `<module>` | `crates/uc-infra-profile/src/space/admission/recovery/pending_state.rs:100` | `instrument` | - |
| `<module>` | `crates/uc-infra-profile/src/space/admission/sponsor/complete.rs:121` | `info` | message-body |
| `<module>` | `crates/uc-infra-profile/src/space/admission/sponsor/complete.rs:135` | `info` | message-body |
| `<module>` | `crates/uc-infra-profile/src/space/admission/sponsor/state.rs:35` | `instrument` | - |
| `<module>` | `crates/uc-infra-profile/src/space/admission/sponsor/state.rs:127` | `instrument` | - |
| `<module>` | `crates/uc-infra-profile/src/space/encryption_passphrase_change.rs:90` | `info` | message-body |
| `<module>` | `crates/uc-infra-profile/src/space/encryption_passphrase_change.rs:97` | `info` | message-body |
| `<module>` | `crates/uc-infra-profile/src/space/encryption_passphrase_change.rs:101` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:356` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:363` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:459` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:464` | `error` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:476` | `error` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:490` | `error` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:1154` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:1178` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:1685` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:1728` | `record` | - |
| `<module>` | `crates/uc-infra-security/src/access.rs:1751` | `record` | - |
| `<module>` | `crates/uc-infra-security/src/access.rs:1905` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:1947` | `error` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:1954` | `error` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:1963` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:1970` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:1992` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:1995` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2011` | `debug` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2016` | `debug` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2020` | `debug` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2024` | `debug` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2035` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2042` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2059` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2066` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2087` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra-security/src/access.rs:2089` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2097` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2107` | `debug` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2115` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2127` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra-security/src/access.rs:2129` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2137` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2147` | `debug` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2164` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2184` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra-security/src/access.rs:2186` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2194` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2204` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2214` | `debug` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2225` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2249` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2262` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2269` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2295` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra-security/src/access.rs:2297` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2304` | `debug` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2315` | `debug` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2336` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2357` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2373` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra-security/src/access.rs:2375` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2380` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2384` | `debug` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2390` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2397` | `debug` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2409` | `debug` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2422` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2429` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2447` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2459` | `info_span` | - |
| `<module>` | `crates/uc-infra-security/src/access.rs:2462` | `debug` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2471` | `error` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2483` | `debug` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2487` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2491` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2498` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2502` | `error` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:2747` | `warn` | raw-error, message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:3339` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/access.rs:3348` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/admission_proof.rs:104` | `debug` | message-body |
| `<module>` | `crates/uc-infra-security/src/admission_proof.rs:128` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/admission_proof.rs:143` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/admission_proof.rs:155` | `warn` | message-body |
| `<module>` | `crates/uc-infra-security/src/admission_proof.rs:160` | `info` | message-body |
| `<module>` | `crates/uc-infra-security/src/session.rs:336` | `debug_span` | - |
| `<module>` | `crates/uc-infra-security/src/session.rs:348` | `debug` | message-body |
| `<module>` | `crates/uc-infra-security/src/session.rs:843` | `debug_span` | - |
| `<module>` | `crates/uc-infra-security/src/session.rs:847` | `debug` | message-body |
| `<module>` | `crates/uc-infra-storage/src/db/pool.rs:259` | `info` | message-body |
| `<module>` | `crates/uc-infra-storage/src/db/pool.rs:533` | `info` | message-body |
| `<module>` | `crates/uc-infra-storage/src/db/pool.rs:536` | `info` | message-body |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/active_clipboard_register_repo.rs:145` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/active_clipboard_register_repo.rs:264` | `warn` | message-body |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/active_clipboard_register_repo.rs:292` | `warn` | message-body |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/active_clipboard_register_repo.rs:336` | `debug_span` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/active_clipboard_register_repo.rs:371` | `debug_span` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/blob_repo.rs:46` | `debug_span` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/blob_repo.rs:64` | `debug_span` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/clipboard_entry_repo.rs:99` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/clipboard_entry_repo.rs:123` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/clipboard_entry_repo.rs:171` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/clipboard_entry_repo.rs:196` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/clipboard_entry_repo.rs:219` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/clipboard_entry_repo.rs:257` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/clipboard_entry_repo.rs:286` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/clipboard_entry_repo.rs:326` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/clipboard_event_repo.rs:87` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/clipboard_event_repo.rs:128` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/clipboard_event_repo.rs:166` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/clipboard_selection_repo.rs:155` | `error` | sensitive-field, raw-error, message-body |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/entry_availability_repo.rs:37` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/entry_delivery_repo.rs:132` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/entry_delivery_repo.rs:159` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/entry_file_set_repo.rs:525` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/entry_file_set_repo.rs:557` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/entry_replace_repo.rs:177` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/file_transfer_repo.rs:113` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/file_transfer_repo.rs:163` | `debug` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/history_tag_repo.rs:133` | `warn` | message-body |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/history_tag_repo.rs:171` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/migration_repo.rs:49` | `debug_span` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/migration_repo.rs:78` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/migration_repo.rs:109` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/migration_repo.rs:135` | `debug_span` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/migration_repo.rs:152` | `debug_span` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/migration_repo.rs:183` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/migration_repo.rs:213` | `debug_span` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/migration_repo.rs:240` | `debug_span` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/space_security_store/legacy_bootstrap.rs:198` | `record` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/space_security_store/legacy_bootstrap.rs:275` | `record` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/space_security_store/revocation.rs:244` | `warn` | message-body |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/space_security_store/revocation.rs:330` | `record` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/space_security_store/revocation.rs:397` | `record` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/space_security_store/revocation.rs:459` | `record` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/space_security_store/revocation.rs:535` | `record` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/space_security_store/revocation.rs:662` | `record` | - |
| `<module>` | `crates/uc-infra-storage/src/db/repositories/space_security_store/revocation.rs:673` | `record` | - |
| `<module>` | `crates/uc-infra-storage/src/file_transfer/privacy_maintenance.rs:68` | `info` | message-body |
| `<module>` | `crates/uc-infra-storage/src/file_transfer/privacy_maintenance.rs:98` | `warn` | message-body |
| `<module>` | `crates/uc-infra-storage/src/file_transfer/projection/sqlite.rs:129` | `debug` | message-body |
| `<module>` | `crates/uc-infra-storage/src/search/rows.rs:167` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra-storage/src/search/rows.rs:177` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:385` | `debug` | message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:419` | `warn` | message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:449` | `warn` | message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:985` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:1058` | `warn` | sensitive-field, message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:1388` | `debug` | message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:1404` | `warn` | message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:1412` | `warn` | message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:1420` | `warn` | message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:1669` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:1711` | `warn` | message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:1725` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:1745` | `warn` | message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:1759` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:1826` | `debug` | message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:1855` | `warn` | message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:1860` | `debug` | message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:1925` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:2177` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:2193` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:2225` | `warn` | message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:2327` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:2362` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:2393` | `info` | message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:2403` | `instrument` | - |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:2452` | `warn` | message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:2489` | `warn` | message-body |
| `<module>` | `crates/uc-infra-storage/src/search/sqlite_index.rs:2498` | `warn` | message-body |
| `uc.connectivity` | `crates/uc-observability-contract/src/diagnostics/connectivity.rs:195` | `event` | sensitive-field |
| `uc.connectivity` | `crates/uc-observability-contract/src/diagnostics/connectivity.rs:198` | `event` | sensitive-field |
| `uc.connectivity` | `crates/uc-observability-contract/src/diagnostics/connectivity.rs:201` | `event` | sensitive-field |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/membership_recovery.rs:92` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:113` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:259` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:263` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:953` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:960` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:967` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:970` | `record` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/network_scope.rs:30` | `info_span` | - |
| `uc.local_diagnostic` | `crates/uc-observability-contract/src/diagnostics/profile_upgrade_backup.rs:11` | `event` | - |
| `<module>` | `crates/uc-observability-contract/src/log_event.rs:25` | `event` | - |
| `<module>` | `crates/uc-observability-runtime/src/module_log.rs:267` | `record` | - |
| `<module>` | `crates/uc-observability-runtime/src/network_log/layer.rs:265` | `record` | - |
| `<module>` | `crates/uc-observability-runtime/src/process_census.rs:305` | `info` | message-body |
| `<module>` | `crates/uc-observability-runtime/src/process_census.rs:324` | `info` | message-body |
| `<module>` | `crates/uc-observability-runtime/src/process_census.rs:341` | `warn` | message-body |
| `<module>` | `crates/uc-observability-runtime/src/subscriber.rs:51` | `record` | - |

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
