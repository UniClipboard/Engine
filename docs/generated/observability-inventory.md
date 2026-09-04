# 运行期观测清单

> 由 `scripts/architecture/check-observability-privacy.mjs --write-inventory` 从生产 Rust 源生成。
> 生成日期：2026-09-04。该文件只描述生成时的代码事实，不是新增埋点的授权清单。

## stable remote

共 0 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |

## local operational

共 28 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:104` | `info` | - |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:112` | `info` | - |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:172` | `info` | - |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:188` | `info` | - |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:201` | `info` | - |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:209` | `info` | - |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:305` | `info` | - |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:314` | `info` | - |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:366` | `info` | - |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:374` | `info` | - |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:493` | `info` | - |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:501` | `info` | - |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:591` | `info` | - |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:598` | `info` | - |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:669` | `info` | - |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:676` | `info` | - |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:878` | `info` | - |
| `admission.performance` | `crates/uc-engine/src/assembly/observability/admission.rs:886` | `info` | - |
| `membership.performance` | `crates/uc-engine/src/assembly/observability/membership.rs:143` | `info` | - |
| `membership.performance` | `crates/uc-engine/src/assembly/observability/membership.rs:151` | `info` | - |
| `membership.performance` | `crates/uc-engine/src/assembly/observability/membership.rs:190` | `info` | - |
| `membership.performance` | `crates/uc-engine/src/assembly/observability/membership.rs:199` | `info` | - |
| `membership.performance` | `crates/uc-engine/src/assembly/observability/membership.rs:252` | `info` | - |
| `membership.performance` | `crates/uc-engine/src/assembly/observability/membership.rs:259` | `info` | - |
| `membership.performance` | `crates/uc-engine/src/assembly/observability/membership.rs:295` | `info` | - |
| `membership.performance` | `crates/uc-engine/src/assembly/observability/membership.rs:302` | `info` | - |
| `membership.performance` | `crates/uc-engine/src/assembly/observability/membership.rs:370` | `info` | - |
| `membership.performance` | `crates/uc-engine/src/assembly/observability/membership.rs:378` | `info` | - |

## local debug

共 1273 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:43` | `warn` | - |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:52` | `warn` | - |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:59` | `warn` | - |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:66` | `warn` | - |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:72` | `warn` | - |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:74` | `warn` | - |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:81` | `warn` | - |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:88` | `warn` | - |
| `<module>` | `bindings/uc-engine-uniffi/src/runtime.rs:537` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:264` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:423` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:503` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:595` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:596` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:597` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:690` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:700` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:709` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:712` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/active/mod.rs:789` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:330` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:334` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:341` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:391` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:451` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:498` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:544` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:769` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:778` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:809` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:882` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:890` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:897` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1022` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1072` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1207` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/capture/usecase.rs:1481` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:133` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:138` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:177` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:200` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:227` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:234` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:239` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:267` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:287` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:293` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:346` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:354` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:369` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:373` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:381` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:392` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:415` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:431` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:441` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:492` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:545` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:566` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:728` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:757` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/cleanup.rs:792` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:73` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:78` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:115` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:125` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/clear_history.rs:153` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:74` | `instrument` | sensitive-field, implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:86` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:107` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:116` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:125` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:139` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:146` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:178` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:185` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:190` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:203` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:209` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:216` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/delete_entry.rs:220` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:306` | `warn` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:313` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:330` | `warn` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:338` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:371` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:399` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:472` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:489` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/list_entry_projections.rs:526` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:97` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:106` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:124` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:150` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:160` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/maintenance_runtime.rs:168` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:102` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:105` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:136` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:157` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:180` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:186` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/reconcile_missing_files.rs:196` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:60` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:66` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:90` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:110` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:119` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:125` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/retention_policy.rs:142` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/history/toggle_favorite.rs:41` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/clipboard/history/toggle_favorite.rs:60` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/history/toggle_favorite.rs:68` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/history/toggle_favorite.rs:70` | `warn` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:113` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:116` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:146` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:164` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:175` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:186` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:193` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:200` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:236` | `warn` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:248` | `warn` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:277` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/inbound/runtime.rs:284` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/local.rs:196` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:221` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:266` | `warn` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:297` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:306` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:322` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:331` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:359` | `warn` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:437` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:670` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:679` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:781` | `warn` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:948` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/outbound/mod.rs:994` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:92` | `warn` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:116` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:123` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:143` | `warn` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:156` | `warn` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/outbound/payload_prep.rs:183` | `warn` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/resource/mod.rs:61` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/resource/mod.rs:89` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/resource/mod.rs:140` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:137` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:148` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:231` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:324` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:329` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:350` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:389` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs:408` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_selection.rs:118` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_selection.rs:167` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/restore/restore_selection.rs:173` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:234` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:241` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:247` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:261` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:284` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:295` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:306` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:312` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:316` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:324` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:367` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:387` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:415` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:424` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:430` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:434` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:446` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:450` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:454` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:478` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:541` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:556` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs:563` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:44` | `debug` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:74` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:81` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/fanout.rs:105` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:99` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:109` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:158` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:181` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:185` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs:197` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:114` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:122` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:139` | `info` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:162` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/reconcile.rs:171` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:79` | `instrument` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:86` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:127` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs:132` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:97` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:111` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:115` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:152` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:156` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:162` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:182` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:193` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:201` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:205` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:230` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs:234` | `debug` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:393` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:399` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:406` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:522` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:529` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:535` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:552` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:565` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:848` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:902` | `warn` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:913` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:936` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1009` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1155` | `warn` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1190` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1211` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1262` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1267` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1308` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1502` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1508` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1875` | `record` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1897` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:1901` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:2316` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:2413` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:2416` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs:2597` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/tests.rs:472` | `record` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/tests.rs:4779` | `record` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:437` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:440` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:443` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:469` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:766` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:787` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:803` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:822` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:859` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:863` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:870` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:908` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:931` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:936` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:982` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1123` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1180` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1199` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1231` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1335` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1526` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1541` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1555` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1559` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1568` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1659` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs:1665` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:62` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:80` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:98` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:116` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:157` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:192` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs:243` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/header.rs:64` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/mod.rs:487` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/per_peer.rs:87` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/per_peer.rs:177` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/per_peer.rs:190` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs:125` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs:132` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs:141` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs:145` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/existing_local_entry_delivery.rs:153` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/get_entry_delivery_view.rs:189` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/get_entry_delivery_view.rs:291` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/outbound_plan.rs:72` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:61` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:77` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:84` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:91` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/receive_gate.rs:109` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/resend_entry.rs:217` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:60` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:69` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:76` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:85` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/send_gate.rs:89` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:123` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:205` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:227` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:261` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:319` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:326` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:333` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs:339` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:110` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:123` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:196` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:213` | `warn` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:225` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:238` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:264` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:282` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:295` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:309` | `warn` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:326` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:344` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/clipboard/sync/sync_runtime.rs:436` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/write/active_register.rs:78` | `debug` | - |
| `<module>` | `crates/uc-application/src/clipboard/write/active_register.rs:89` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:142` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:169` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:191` | `info` | - |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:257` | `warn` | - |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:310` | `error` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/write/coordinator.rs:359` | `info_span` | - |
| `<module>` | `crates/uc-application/src/clipboard/write/mobile_consumability.rs:30` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/clipboard/write/mobile_consumability.rs:59` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/clipboard/write/restore_broadcast.rs:50` | `trace` | - |
| `<module>` | `crates/uc-application/src/device/query_local_device/use_case.rs:29` | `warn` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:177` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:198` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:217` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:231` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-application/src/facade/clipboard_restore/mod.rs:247` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/facade/clipboard/cancel_entry_receive.rs:91` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:332` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:392` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:409` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/clipboard/facade.rs:427` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:121` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:164` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:209` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:240` | `instrument` | - |
| `<module>` | `crates/uc-application/src/facade/roster/facade.rs:257` | `instrument` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:218` | `warn` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:239` | `warn` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:257` | `warn` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:296` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:354` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:362` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:373` | `info` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:376` | `warn` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:381` | `info` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:387` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:392` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:400` | `info` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:413` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:419` | `info` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:431` | `warn` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:447` | `info` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:461` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:481` | `info` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:484` | `debug` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:536` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:546` | `info_span` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:564` | `info` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:579` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:593` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:622` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:650` | `info` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:663` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:677` | `info` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:683` | `info` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:688` | `info` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:762` | `debug` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:774` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:781` | `debug` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:795` | `debug` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:810` | `debug` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:882` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:893` | `info` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:895` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:900` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:902` | `info` | - |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:913` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:917` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:933` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:941` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:949` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:956` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/search/coordinator.rs:959` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/search/live_index/mod.rs:111` | `debug` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/search/live_index/mod.rs:126` | `debug` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/search/live_index/mod.rs:152` | `debug` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/search/query.rs:18` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/search/query.rs:33` | `debug` | - |
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
| `<module>` | `crates/uc-application/src/settings/facade.rs:210` | `debug` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:214` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/facade.rs:231` | `info` | - |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:49` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:59` | `error` | raw-error |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:64` | `info` | - |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:83` | `instrument` | - |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:104` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:110` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/settings/storage/mod.rs:125` | `info` | - |
| `upgrade` | `crates/uc-application/src/settings/upgrade/acknowledge.rs:39` | `info` | - |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:82` | `debug` | - |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:92` | `debug` | - |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:102` | `debug` | - |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:110` | `debug` | - |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:123` | `debug` | - |
| `upgrade` | `crates/uc-application/src/settings/upgrade/detect.rs:135` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/space/admission/invitation/cancel/use_case.rs:45` | `info` | - |
| `<module>` | `crates/uc-application/src/space/admission/invitation/issue_for_address/use_case.rs:25` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/admission/invitation/issue/use_case.rs:48` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/admission/invitation/issuer.rs:110` | `warn` | - |
| `<module>` | `crates/uc-application/src/space/admission/invitation/query_addresses/use_case.rs:19` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/admission/invitation/query_addresses/use_case.rs:28` | `record` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:370` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:391` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:411` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:419` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:430` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:438` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:612` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:619` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:641` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/facade/facade.rs:709` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:105` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:152` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:170` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:186` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:210` | `info` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs:294` | `warn` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/unlock_space/use_case.rs:40` | `instrument` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/unlock_space/use_case.rs:63` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/unlock_space/use_case.rs:76` | `info` | - |
| `<module>` | `crates/uc-application/src/space/lifecycle/unlock_space/use_case.rs:106` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/space/membership/decide_device_trust_change/use_case.rs:91` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/decide_device_trust_change/use_case.rs:148` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/decide_device_trust_change/use_case.rs:194` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:94` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:180` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:184` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:193` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:265` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:279` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:282` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:362` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:436` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:493` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/handle_history_message/use_case.rs:525` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:110` | `warn` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:113` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:114` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:117` | `warn` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:123` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:131` | `warn` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:138` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:146` | `warn` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:149` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:170` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:183` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:186` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:190` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:195` | `warn` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:200` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/effect_executor.rs:206` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/repository.rs:182` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/repository.rs:202` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/repository.rs:215` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/repository.rs:235` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/repository.rs:240` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/ledger/restricted_delivery.rs:46` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/maintenance/tests.rs:28` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/maintenance/tests.rs:35` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/maintenance/tests.rs:42` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/maintenance/tests.rs:49` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/maintenance/tests.rs:56` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/maintenance/tests.rs:72` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/maintenance/tests.rs:79` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_admission/use_case.rs:38` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_admission/use_case.rs:43` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_admission/use_case.rs:56` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_admission/use_case.rs:60` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_admission/use_case.rs:64` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/use_case.rs:42` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/use_case.rs:53` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/use_case.rs:58` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/use_case.rs:75` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/use_case.rs:132` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/use_case.rs:148` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_device_trust/use_case.rs:172` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/query_diagnostics.rs:90` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/issuer.rs:66` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/issuer.rs:85` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/issuer.rs:90` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/issuer.rs:155` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:88` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:101` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:107` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:113` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:144` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:397` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/recover_conflict/use_case.rs:401` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/remove_space_member/use_case.rs:65` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/resolve_conflict/use_case.rs:43` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/resolve_conflict/use_case.rs:48` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/resolve_conflict/use_case.rs:57` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/resolve_conflict/use_case.rs:97` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/resolve_conflict/use_case.rs:136` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/resolve_conflict/use_case.rs:193` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/resolve_conflict/use_case.rs:204` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/target_use_case.rs:99` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/target_use_case.rs:122` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/target_use_case.rs:127` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/target_use_case.rs:212` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/target_use_case.rs:215` | `record` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/target_use_case.rs:274` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/target_use_case.rs:343` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/target_use_case.rs:366` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/target_use_case.rs:401` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/target_use_case.rs:429` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/target_use_case.rs:432` | `debug` | - |
| `<module>` | `crates/uc-application/src/space/membership/synchronize_history/target_use_case.rs:670` | `record` | - |
| `<module>` | `crates/uc-application/src/support/host_event_bus.rs:70` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/support/host_event_bus.rs:93` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:16` | `warn` | - |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:68` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:84` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:115` | `debug` | sensitive-field |
| `<module>` | `crates/uc-application/src/support/host_event_publisher.rs:178` | `warn` | - |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:357` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:362` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:404` | `info` | - |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:449` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:460` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:488` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:623` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:631` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:659` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:778` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:786` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:822` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/transfer/blob/facade.rs:956` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/transfer/blob/publish_blob.rs:138` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/transfer/blob/publish_blob.rs:203` | `info` | sensitive-field |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:187` | `info` | - |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:203` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:212` | `info` | - |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:234` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:246` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:266` | `info_span` | - |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:282` | `info_span` | - |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:287` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:293` | `info` | - |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:297` | `info` | - |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:386` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:397` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:417` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/transfer/file/lifecycle.rs:429` | `warn` | raw-error |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:175` | `instrument` | - |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:227` | `error` | sensitive-field |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:261` | `error` | sensitive-field |
| `<module>` | `crates/uc-application/src/transfer/receive/reconciliation.rs:316` | `error` | - |
| `<module>` | `crates/uc-core/src/task_registry.rs:60` | `debug` | - |
| `<module>` | `crates/uc-core/src/task_registry.rs:75` | `info` | - |
| `<module>` | `crates/uc-core/src/task_registry.rs:87` | `warn` | raw-error |
| `<module>` | `crates/uc-core/src/task_registry.rs:89` | `info` | - |
| `<module>` | `crates/uc-core/src/task_registry.rs:95` | `warn` | - |
| `bootstrap.network` | `crates/uc-engine/src/assembly/facade.rs:81` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/assembly/host.rs:253` | `warn` | - |
| `<module>` | `crates/uc-engine/src/assembly/lifecycle.rs:70` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/assembly/lifecycle.rs:81` | `warn` | raw-error |
| `settings.network` | `crates/uc-engine/src/assembly/lifecycle.rs:126` | `info` | - |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:81` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:91` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:128` | `warn` | - |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:136` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:151` | `warn` | raw-error |
| `settings.network` | `crates/uc-engine/src/assembly/network.rs:177` | `info` | sensitive-field |
| `settings.network` | `crates/uc-engine/src/assembly/network.rs:198` | `info` | - |
| `<module>` | `crates/uc-engine/src/assembly/network.rs:206` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/assembly/observability/admission.rs:521` | `record` | - |
| `<module>` | `crates/uc-engine/src/assembly/observability/admission.rs:536` | `record` | - |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:153` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:167` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:179` | `info` | - |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:187` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:190` | `warn` | - |
| `<module>` | `crates/uc-engine/src/assembly/platform.rs:198` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine.rs:179` | `instrument` | - |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine.rs:325` | `debug` | - |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine.rs:470` | `instrument` | - |
| `<module>` | `crates/uc-engine/src/assembly/sync_engine.rs:851` | `info` | - |
| `<module>` | `crates/uc-engine/src/assembly/wire/mod.rs:374` | `info` | - |
| `<module>` | `crates/uc-engine/src/assembly/wire/mod.rs:404` | `info` | - |
| `<module>` | `crates/uc-engine/src/operations/clipboard/capture.rs:22` | `error` | - |
| `<module>` | `crates/uc-engine/src/operations/clipboard/query_active.rs:11` | `error` | - |
| `<module>` | `crates/uc-engine/src/operations/clipboard/restore.rs:56` | `error` | - |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:37` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:52` | `info` | - |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:73` | `info` | - |
| `<module>` | `crates/uc-engine/src/operations/device/member.rs:581` | `error` | - |
| `<module>` | `crates/uc-engine/src/operations/device/peer_connections.rs:67` | `warn` | - |
| `<module>` | `crates/uc-engine/src/operations/device/peer_connections.rs:77` | `info` | - |
| `<module>` | `crates/uc-engine/src/operations/history/delivery.rs:93` | `error` | - |
| `<module>` | `crates/uc-engine/src/operations/history/history.rs:189` | `error` | - |
| `<module>` | `crates/uc-engine/src/operations/history/receive.rs:153` | `error` | - |
| `<module>` | `crates/uc-engine/src/operations/history/receive.rs:160` | `error` | - |
| `<module>` | `crates/uc-engine/src/operations/history/resend.rs:68` | `error` | - |
| `<module>` | `crates/uc-engine/src/operations/history/resend.rs:76` | `error` | - |
| `<module>` | `crates/uc-engine/src/operations/history/resource.rs:68` | `error` | - |
| `<module>` | `crates/uc-engine/src/operations/history/search.rs:177` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/operations/history/search.rs:265` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/operations/settings/config_migration.rs:141` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/operations/settings/diagnostics.rs:72` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/operations/settings/encryption.rs:14` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/operations/settings/encryption.rs:26` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/operations/settings/encryption.rs:40` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/operations/settings/storage.rs:47` | `error` | - |
| `<module>` | `crates/uc-engine/src/operations/space/cancel_invitation.rs:26` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/operations/space/create_space.rs:58` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/operations/space/device_group_choice.rs:17` | `debug` | - |
| `<module>` | `crates/uc-engine/src/operations/space/device_group_choice.rs:130` | `debug` | - |
| `<module>` | `crates/uc-engine/src/operations/space/factory_reset.rs:31` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/operations/space/invitation.rs:69` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/operations/space/join_space.rs:88` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/operations/space/reset_space.rs:18` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/operations/space/session_recovery.rs:58` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/operations/space/setup_state.rs:22` | `error` | - |
| `<module>` | `crates/uc-engine/src/operations/space/setup_state.rs:51` | `info` | - |
| `<module>` | `crates/uc-engine/src/operations/space/unlock.rs:54` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/dispatch.rs:483` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/dispatch.rs:493` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/dispatch.rs:641` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:35` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:45` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:50` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:123` | `error` | - |
| `<module>` | `crates/uc-engine/src/runtime/host_clipboard.rs:147` | `info` | - |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:114` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:165` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:170` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:321` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:338` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:346` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:353` | `warn` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:380` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/host_operations.rs:460` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/mod.rs:300` | `warn` | - |
| `<module>` | `crates/uc-engine/src/runtime/mod.rs:317` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/mod.rs:334` | `error` | raw-error |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:201` | `debug` | - |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:211` | `info` | - |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:213` | `info` | - |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:217` | `info` | - |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:240` | `info` | - |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:244` | `info` | - |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:246` | `info` | - |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:263` | `warn` | - |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:402` | `warn` | - |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:528` | `info` | - |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:536` | `warn` | - |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:539` | `info` | - |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:541` | `info` | - |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:543` | `warn` | - |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:549` | `error` | - |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:552` | `info` | - |
| `<module>` | `crates/uc-engine/src/runtime/session_supervisor.rs:727` | `warn` | - |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:56` | `debug` | - |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:60` | `info` | - |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:68` | `info` | sensitive-field |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:75` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:124` | `debug` | - |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:128` | `info` | - |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:136` | `info` | sensitive-field |
| `<module>` | `crates/uc-engine/src/subsystems/reconcile.rs:143` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:55` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:77` | `debug` | - |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:88` | `debug` | - |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:122` | `debug` | raw-error |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:155` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:177` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:206` | `debug` | raw-error |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:238` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/blob/blob_writer.rs:250` | `debug` | - |
| `<module>` | `crates/uc-infra/src/blob/filesystem_store.rs:134` | `debug` | - |
| `<module>` | `crates/uc-infra/src/blob/filesystem_store.rs:148` | `debug` | raw-error |
| `<module>` | `crates/uc-infra/src/blob/filesystem_store.rs:167` | `debug` | - |
| `<module>` | `crates/uc-infra/src/blob/filesystem_store.rs:186` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:122` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:140` | `error` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:158` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:187` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:194` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:206` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:211` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:216` | `warn` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:234` | `warn` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:240` | `warn` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:243` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:270` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:282` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:314` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:336` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:347` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:354` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:358` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:384` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:390` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:393` | `error` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:413` | `error` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/background_blob_worker.rs:430` | `warn` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:107` | `info` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:122` | `info` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:141` | `info` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:142` | `info` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:160` | `info` | - |
| `<module>` | `crates/uc-infra/src/clipboard/background_runtime.rs:162` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/broadcasting_advance.rs:39` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:129` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:138` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:192` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:204` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/clipboard/change_origin.rs:212` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/clipboard/chunked_transfer.rs:436` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/clipboard/chunked_transfer.rs:452` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/clipboard/durable_spool_queue.rs:73` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/normalizer.rs:95` | `trace` | - |
| `<module>` | `crates/uc-infra/src/clipboard/normalizer.rs:116` | `trace` | - |
| `<module>` | `crates/uc-infra/src/clipboard/normalizer.rs:137` | `trace` | - |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:45` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:62` | `error` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:70` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:84` | `error` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:92` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:102` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:109` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:114` | `warn` | - |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:125` | `error` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/payload_resolver.rs:165` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/spool_janitor.rs:67` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/spool_janitor.rs:74` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/spool_janitor.rs:83` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/spool_janitor.rs:94` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:182` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:190` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:200` | `warn` | - |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:249` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:426` | `warn` | - |
| `<module>` | `crates/uc-infra/src/clipboard/spool_manager.rs:432` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:59` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:64` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:78` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:89` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:99` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:104` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/spool_scanner.rs:114` | `info` | - |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:68` | `info` | - |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:86` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:108` | `info` | - |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:114` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:120` | `debug` | - |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:126` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/clipboard/staged_reconciler.rs:135` | `info` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:378` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:380` | `info` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:401` | `error` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:407` | `error` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:523` | `info` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:530` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:552` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:558` | `info` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:594` | `warn` | - |
| `<module>` | `crates/uc-infra/src/config_migration/adapter.rs:596` | `info` | - |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:237` | `info` | - |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:242` | `error` | - |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:255` | `info` | - |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:265` | `error` | - |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:273` | `error` | raw-error |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:282` | `info` | - |
| `<module>` | `crates/uc-infra/src/config_migration/staging.rs:310` | `info` | - |
| `<module>` | `crates/uc-infra/src/db/pool.rs:102` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/db/pool.rs:109` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/db/pool.rs:116` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/db/pool.rs:142` | `info` | - |
| `<module>` | `crates/uc-infra/src/db/pool.rs:405` | `info` | - |
| `<module>` | `crates/uc-infra/src/db/pool.rs:408` | `info` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/active_clipboard_register_repo.rs:139` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/active_clipboard_register_repo.rs:265` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/db/repositories/active_clipboard_register_repo.rs:286` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/db/repositories/active_clipboard_register_repo.rs:328` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/active_clipboard_register_repo.rs:362` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/blob_repo.rs:46` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/blob_repo.rs:64` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs:72` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs:96` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs:144` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs:169` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs:192` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs:230` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs:259` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs:299` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_event_repo.rs:87` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_event_repo.rs:128` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_event_repo.rs:166` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/clipboard_selection_repo.rs:154` | `error` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_availability_repo.rs:37` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_delivery_repo.rs:109` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_delivery_repo.rs:139` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_file_set_repo.rs:477` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_file_set_repo.rs:509` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/entry_replace_repo.rs:175` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/file_transfer_repo.rs:86` | `debug_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/db/repositories/file_transfer_repo.rs:136` | `debug` | sensitive-field |
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
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:172` | `warn` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:251` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:309` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:367` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:443` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:566` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/revocation.rs:577` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:330` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:334` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:363` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:368` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:390` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:410` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:542` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:550` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:706` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:710` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:718` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:731` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:732` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:733` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:734` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:735` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:744` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:755` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:841` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:921` | `record` | - |
| `<module>` | `crates/uc-infra/src/db/repositories/space_security_store/tests.rs:1007` | `record` | - |
| `<module>` | `crates/uc-infra/src/file_transfer/privacy_maintenance.rs:68` | `info` | - |
| `<module>` | `crates/uc-infra/src/file_transfer/privacy_maintenance.rs:92` | `warn` | - |
| `<module>` | `crates/uc-infra/src/file_transfer/projection/sqlite.rs:129` | `debug` | - |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:69` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:103` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:111` | `debug` | - |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:117` | `warn` | - |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:123` | `debug` | - |
| `<module>` | `crates/uc-infra/src/fs/atomic_publish.rs:127` | `debug` | raw-error |
| `<module>` | `crates/uc-infra/src/fs/hidden_path.rs:65` | `debug` | - |
| `<module>` | `crates/uc-infra/src/fs/hidden_path.rs:72` | `debug` | - |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:39` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:58` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:87` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:112` | `debug` | - |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:119` | `warn` | - |
| `<module>` | `crates/uc-infra/src/fs/inbound_target.rs:150` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:136` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:142` | `debug` | - |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:228` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:240` | `debug` | - |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:242` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:274` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:288` | `debug` | - |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:334` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:402` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:435` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:451` | `debug` | raw-error |
| `<module>` | `crates/uc-infra/src/mobile_sync/file_staging.rs:459` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/dispatch_adapter.rs:63` | `warn` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/dispatch_adapter.rs:75` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/dispatch_adapter.rs:103` | `debug` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:71` | `warn` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:83` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:95` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:126` | `debug` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs:168` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:122` | `warn` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:130` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:142` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:157` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:175` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:187` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:194` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:201` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:213` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:221` | `debug` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:244` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:257` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs:268` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:133` | `warn` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:143` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:155` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:169` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:183` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:201` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:223` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:236` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs:247` | `warn` | raw-error |
| `iroh.addr_filter` | `crates/uc-infra/src/network/iroh/addr_filter.rs:134` | `warn` | raw-error |
| `iroh.addr_filter` | `crates/uc-infra/src/network/iroh/addr_filter.rs:155` | `info` | - |
| `iroh.addr_filter` | `crates/uc-infra/src/network/iroh/addr_filter.rs:168` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:184` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:214` | `info` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:224` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:269` | `info` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:279` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:293` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:324` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:380` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:389` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:405` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:416` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:427` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:439` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:452` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:461` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:491` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:520` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:531` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:570` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:595` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:609` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:623` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:648` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:666` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:683` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:709` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/blobs.rs:725` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs:157` | `debug` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs:192` | `warn` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs:247` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs:305` | `debug` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:155` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:161` | `warn` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:178` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:190` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:226` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:250` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:309` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:312` | `debug` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:316` | `debug` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:332` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:345` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs:356` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/connect.rs:99` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/connect.rs:141` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/connect.rs:164` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/connect.rs:173` | `debug` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/connect.rs:182` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/connection_channel_adapter.rs:79` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/connection_channel_adapter.rs:83` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/group_update_adapter.rs:67` | `warn` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/group_update_adapter.rs:139` | `debug` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/group_update_adapter.rs:143` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:110` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:118` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:122` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:130` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/identity_store.rs:134` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs:243` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs:375` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs:512` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs:516` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs:594` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs:805` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs:819` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_branch_recovery_adapter.rs:317` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/membership_history_exchange_adapter.rs:68` | `warn` | - |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:228` | `warn` | raw-error |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:236` | `info` | - |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:247` | `warn` | - |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:261` | `warn` | raw-error |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:288` | `warn` | raw-error |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:437` | `info` | - |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:459` | `warn` | - |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:503` | `info` | - |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:524` | `warn` | raw-error |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:534` | `info` | - |
| `iroh.net_recovery` | `crates/uc-infra/src/network/iroh/net_recovery.rs:544` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:237` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:250` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:264` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:267` | `warn` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:273` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:405` | `info` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:440` | `info` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:624` | `warn` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:691` | `instrument` | - |
| `iroh.addr_filter` | `crates/uc-infra/src/network/iroh/node.rs:702` | `info` | - |
| `iroh.address_lookup` | `crates/uc-infra/src/network/iroh/node.rs:737` | `info` | - |
| `iroh.bind` | `crates/uc-infra/src/network/iroh/node.rs:758` | `info` | - |
| `iroh.bind` | `crates/uc-infra/src/network/iroh/node.rs:773` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:801` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:1345` | `info` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:1349` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/node.rs:1367` | `info` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:193` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:204` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:222` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:231` | `warn` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:264` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:292` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:334` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:339` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:350` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:363` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:552` | `warn` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:596` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:727` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:775` | `info` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:787` | `debug` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:812` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:841` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:851` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:864` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:889` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:896` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:916` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:945` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/presence_adapter.rs:1058` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/relay_probe.rs:87` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/network/iroh/relay_probe.rs:148` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/relay_probe.rs:223` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/relay_probe.rs:258` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/runtime_consts.rs:22` | `warn` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/runtime_consts.rs:40` | `warn` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/runtime_consts.rs:53` | `warn` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission.rs:349` | `debug` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/space_admission.rs:353` | `debug` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:166` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:172` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:179` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:193` | `debug` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:211` | `trace` | sensitive-field |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:218` | `debug` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:222` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:247` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:284` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:294` | `warn` | - |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:307` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs:317` | `warn` | - |
| `<module>` | `crates/uc-infra/src/pairing/invitation_resolver.rs:43` | `debug` | - |
| `<module>` | `crates/uc-infra/src/pairing/mdns_publisher.rs:117` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/pairing/mdns_publisher.rs:154` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/pairing/mdns_publisher.rs:198` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/pairing/mdns_resolver.rs:83` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/pairing/mdns_resolver.rs:182` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/pairing/mdns_resolver.rs:188` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/pairing/mdns_resolver.rs:192` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:138` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:167` | `debug` | raw-error |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:193` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:215` | `warn` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/client.rs:224` | `warn` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:159` | `debug` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:172` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:205` | `debug` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:217` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:230` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:313` | `debug` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:456` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:465` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:477` | `debug` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:482` | `debug` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:492` | `instrument` | - |
| `<module>` | `crates/uc-infra/src/rendezvous/invitation_adapter.rs:506` | `instrument` | sensitive-field |
| `<module>` | `crates/uc-infra/src/search/rows.rs:166` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/search/rows.rs:175` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:361` | `debug` | - |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:395` | `warn` | - |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:939` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1008` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1172` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1189` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1192` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1195` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1442` | `instrument` | sensitive-field, implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1482` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1492` | `instrument` | sensitive-field, implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1514` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1524` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1638` | `debug` | - |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1667` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1673` | `debug` | - |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1694` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1942` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1960` | `instrument` | sensitive-field, implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:1994` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2004` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2041` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2074` | `info` | - |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2084` | `instrument` | implicit-arguments |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2135` | `warn` | - |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2167` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/search/sqlite_index.rs:2171` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/security/admission_proof.rs:103` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/security/admission_proof.rs:131` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/security/admission_proof.rs:150` | `warn` | - |
| `<module>` | `crates/uc-infra/src/security/admission_proof.rs:165` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/security/admission_proof.rs:172` | `info` | - |
| `<module>` | `crates/uc-infra/src/security/content_protection/blob_store.rs:56` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/security/content_protection/blob_store.rs:59` | `debug` | - |
| `<module>` | `crates/uc-infra/src/security/content_protection/blob_store.rs:100` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/security/content_protection/blob_store.rs:111` | `debug` | - |
| `<module>` | `crates/uc-infra/src/security/decrypting_clipboard_event_repo.rs:63` | `trace` | - |
| `<module>` | `crates/uc-infra/src/security/decrypting_representation_repo.rs:70` | `trace` | - |
| `<module>` | `crates/uc-infra/src/security/decrypting_representation_repo.rs:107` | `trace` | - |
| `<module>` | `crates/uc-infra/src/security/decrypting_representation_repo.rs:127` | `trace` | - |
| `<module>` | `crates/uc-infra/src/security/decrypting_representation_repo.rs:210` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/security/encrypted_blob_store.rs:213` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/security/encrypted_blob_store.rs:216` | `debug` | - |
| `<module>` | `crates/uc-infra/src/security/encrypted_blob_store.rs:273` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/security/encrypted_blob_store.rs:318` | `debug` | - |
| `<module>` | `crates/uc-infra/src/security/encrypting_clipboard_event_writer.rs:60` | `trace` | - |
| `<module>` | `crates/uc-infra/src/security/encrypting_clipboard_event_writer.rs:88` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/settings/migration.rs:54` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/activation_state.rs:18` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/activation_state.rs:49` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/cancellation.rs:39` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/cancellation.rs:74` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/cancellation.rs:132` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/start_state.rs:16` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/joiner/start_state.rs:56` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/recovery/pending_state.rs:18` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/recovery/pending_state.rs:51` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:129` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:143` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:170` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:174` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:233` | `debug` | - |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:238` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/complete.rs:253` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/state.rs:30` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/admission/sponsor/state.rs:116` | `instrument` | raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:285` | `warn` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:291` | `error` | raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:304` | `error` | raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:315` | `error` | raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1356` | `record` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1379` | `record` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1580` | `error` | raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1583` | `error` | raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1588` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1591` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1609` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1612` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1630` | `debug` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1635` | `debug` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1640` | `debug` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1644` | `debug` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1649` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1655` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1657` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1660` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1673` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1676` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1694` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1696` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1699` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1702` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1710` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1714` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1719` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1732` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1734` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1737` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1740` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1748` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1752` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1755` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1760` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1770` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1774` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1786` | `debug` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1788` | `warn` | raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1795` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1816` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1818` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1826` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1831` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1834` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1839` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1843` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1846` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1850` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1862` | `info` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1880` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1890` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1894` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1906` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1918` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1920` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1927` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1930` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1937` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1941` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1952` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1956` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1966` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1980` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:1991` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2008` | `info_span` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2010` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2013` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2022` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2025` | `warn` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2035` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2047` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2054` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2060` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2062` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2065` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2078` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2091` | `info_span` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2094` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2099` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2107` | `debug` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2111` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2115` | `warn` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2119` | `info` | sensitive-field |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2123` | `error` | sensitive-field, raw-error |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2844` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2859` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/security/access.rs:2868` | `info` | - |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:439` | `warn` | - |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:445` | `warn` | - |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:465` | `warn` | - |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:469` | `warn` | - |
| `<module>` | `crates/uc-infra/src/space/security/mls_group.rs:473` | `warn` | - |
| `<module>` | `crates/uc-infra/src/space/security/session.rs:158` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/space/security/session.rs:167` | `debug` | - |
| `<module>` | `crates/uc-infra/src/space/security/session.rs:582` | `debug_span` | - |
| `<module>` | `crates/uc-infra/src/space/security/session.rs:591` | `debug` | - |
| `<module>` | `crates/uc-infra/src/time/timer.rs:45` | `debug` | - |
| `<module>` | `crates/uc-infra/src/time/timer.rs:53` | `debug` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:215` | `span` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:225` | `span` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:339` | `event` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:349` | `event` | - |
| `<module>` | `crates/uc-observability-contract/src/diagnostics/mod.rs:379` | `event` | - |
| `<module>` | `crates/uc-observability-contract/src/task_supervision.rs:53` | `warn` | raw-error |

## product analytics

共 2 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |
| `<module>` | `crates/uc-observability-contract/src/analytics/facade.rs:174` | `warn` | raw-error |
| `<module>` | `crates/uc-observability-contract/src/analytics/facade.rs:200` | `warn` | raw-error |

## delete

共 0 个调用点。

| Target | 调用点 | 类型 | 风险标记 |
| --- | --- | --- | --- |

## 输出边界

- 系统日志、本地文件和远程发送默认拒绝 `local debug` 与 `delete`。
- `stable remote` 只能由诊断契约和 Engine 完整能力装饰器产生，并接受固定字段检查。
- `product analytics` 使用独立合同，不共享诊断身份、流程号或发送路径。
- 风险标记用于安排后续清理；被默认拒绝的历史调用点不等于允许输出。
