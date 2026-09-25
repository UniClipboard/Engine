# 错误来源丢失修改点清单

本清单是 [错误来源保留执行计划](2026-09-24-error-source-preservation.md) 的附录，逐行列出待处理位置。

- **快照**：提交 `48a2e95c`（2026-09-24）。扫描对象是该提交的已跟踪文件，不含当时工作区中未提交的改动；修复后行号会漂移，
  每个切片开工前按计划中的扫描规则重新生成本清单。
- **范围**：`crates/`、`bindings/`、`compatibility/`、`tests/` 下的 Rust 文件。测试文件、`testing/` 目录与文件末尾
  `#[cfg(test)] mod` 之后的内容归为测试代码，只在文末计数，不列入修改点。
- **精度**：扫描基于文本规则。S4 的来源类别由调用链关键词推断，只作排期参考；最终是否属于允许例外，由修复切片逐项确认。
- **备注**：“049 处理中”表示该文件已由 049 成员重写会话在修复同类问题，本计划不重复排期，待其合入后复核。

## 总览

| 类别 | 写法 | 生产代码 | 测试代码 |
| --- | --- | ---: | ---: |
| S1 | `anyhow!(error.to_string())` / `Error::msg(error)` | 91 | 3 |
| S2 | `anyhow!("动作: {error}")` 把下层错误拼进文本 | 82 | 1 |
| S3 | 错误变体或字段只保存 `error.to_string()` / `format!(.. error ..)` | 524 | 12 |
| S4 | `map_err(|_| ..)` 丢弃来源 | 914 | 91 |
| L1 | 日志字段 `error = %error` / `?error` 输出错误正文 | 329 | 1 |

## S1 字符串化后重新包装成 anyhow

替换为 `anyhow::Error::new(error)`、`From` 转换或直接 `?`。若同时需要动作说明，用 `.context("固定动作")`。

生产代码共 91 处，按 crate 分布：

| crate | 数量 |
| --- | ---: |
| crates/uc-infra | 87 |
| crates/uc-engine | 4 |

| 文件 | 行号 | 数量 | 备注 |
| --- | --- | ---: | --- |
| [`crates/uc-engine/src/assembly/host.rs`](../../../crates/uc-engine/src/assembly/host.rs) | 93, 158, 238, 251 | 4 |  |
| [`crates/uc-infra/src/db/repositories/blob_reference_repo.rs`](../../../crates/uc-infra/src/db/repositories/blob_reference_repo.rs) | 50, 76, 88 | 3 |  |
| [`crates/uc-infra/src/db/repositories/entry_receive_attempt_repo.rs`](../../../crates/uc-infra/src/db/repositories/entry_receive_attempt_repo.rs) | 113, 184 | 2 |  |
| [`crates/uc-infra/src/db/repositories/migration_repo.rs`](../../../crates/uc-infra/src/db/repositories/migration_repo.rs) | 59, 90, 122, 137, 151, 189, 217, 230 | 8 |  |
| [`crates/uc-infra/src/db/repositories/mobile_device_repo.rs`](../../../crates/uc-infra/src/db/repositories/mobile_device_repo.rs) | 106, 113, 138, 144, 162, 168, 180, 186, 201, 242 | 10 |  |
| [`crates/uc-infra/src/db/repositories/relationship_store.rs`](../../../crates/uc-infra/src/db/repositories/relationship_store.rs) | 601, 641, 775 | 3 |  |
| [`crates/uc-infra/src/db/repositories/space_security_store/legacy_bootstrap.rs`](../../../crates/uc-infra/src/db/repositories/space_security_store/legacy_bootstrap.rs) | 148, 252, 272, 285, 287, 296, 302, 347, 367, 396, 427, 452, 458, 467 | 14 | 049 处理中 |
| [`crates/uc-infra/src/db/repositories/space_security_store/revocation.rs`](../../../crates/uc-infra/src/db/repositories/space_security_store/revocation.rs) | 106, 120, 194, 244, 248, 382, 408, 426, 445, 452, 457, 459, 461, 540, 548, 564, 605, 607, 610, 614, 617, 630, 636, 638, 672, 688, 691, 694, 698, 701, 708, 714, 752, 765, 768, 772, 775, 782, 788, 827, 838, 846, 849, 856, 862 | 45 | 049 处理中 |
| [`crates/uc-infra/src/network/iroh/membership_branch_recovery_adapter.rs`](../../../crates/uc-infra/src/network/iroh/membership_branch_recovery_adapter.rs) | 67 | 1 |  |
| [`crates/uc-infra/src/rendezvous/invitation_adapter.rs`](../../../crates/uc-infra/src/rendezvous/invitation_adapter.rs) | 597 | 1 |  |

## S2 把下层错误拼进 anyhow 文本

替换为 `.context("固定动作")` 或 `anyhow::Error::new(error).context(..)`；文本中的路径、标签值、内容片段一并移除。

生产代码共 82 处，按 crate 分布：

| crate | 数量 |
| --- | ---: |
| crates/uc-application | 41 |
| crates/uc-infra | 36 |
| crates/uc-engine | 5 |

| 文件 | 行号 | 数量 | 备注 |
| --- | --- | ---: | --- |
| [`crates/uc-application/src/clipboard/capture/usecase.rs`](../../../crates/uc-application/src/clipboard/capture/usecase.rs) | 657 | 1 |  |
| [`crates/uc-application/src/clipboard/history/cleanup.rs`](../../../crates/uc-application/src/clipboard/history/cleanup.rs) | 477, 551, 682, 685 | 4 |  |
| [`crates/uc-application/src/clipboard/history/clear_history.rs`](../../../crates/uc-application/src/clipboard/history/clear_history.rs) | 159 | 1 |  |
| [`crates/uc-application/src/clipboard/history/delete_entry.rs`](../../../crates/uc-application/src/clipboard/history/delete_entry.rs) | 103, 112, 121, 130 | 4 |  |
| [`crates/uc-application/src/clipboard/history/retention_policy.rs`](../../../crates/uc-application/src/clipboard/history/retention_policy.rs) | 148 | 1 |  |
| [`crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs`](../../../crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs) | 935, 1248, 1353, 1368, 1966, 1998, 2302 | 7 |  |
| [`crates/uc-application/src/clipboard/sync/payload_codec.rs`](../../../crates/uc-application/src/clipboard/sync/payload_codec.rs) | 101, 122, 134, 136, 172, 216, 265, 336, 351, 356, 361, 365, 452, 456, 470, 484, 485, 502, 505, 512, 520, 528 | 22 |  |
| [`crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs`](../../../crates/uc-application/src/space/lifecycle/initialize_space/use_case.rs) | 320 | 1 |  |
| [`crates/uc-engine/src/assembly/host.rs`](../../../crates/uc-engine/src/assembly/host.rs) | 102 | 1 |  |
| [`crates/uc-engine/src/subsystems/reconcile.rs`](../../../crates/uc-engine/src/subsystems/reconcile.rs) | 34, 42, 104, 110 | 4 |  |
| [`crates/uc-infra/src/clipboard/durable_spool_queue.rs`](../../../crates/uc-infra/src/clipboard/durable_spool_queue.rs) | 66 | 1 |  |
| [`crates/uc-infra/src/clipboard/spool_manager.rs`](../../../crates/uc-infra/src/clipboard/spool_manager.rs) | 438 | 1 |  |
| [`crates/uc-infra/src/db/mappers/blob_reference_mapper.rs`](../../../crates/uc-infra/src/db/mappers/blob_reference_mapper.rs) | 30 | 1 |  |
| [`crates/uc-infra/src/db/mappers/snapshot_representation_mapper.rs`](../../../crates/uc-infra/src/db/mappers/snapshot_representation_mapper.rs) | 103 | 1 |  |
| [`crates/uc-infra/src/db/pool.rs`](../../../crates/uc-infra/src/db/pool.rs) | 49, 136, 140, 185, 239 | 5 |  |
| [`crates/uc-infra/src/db/repositories/blob_repo.rs`](../../../crates/uc-infra/src/db/repositories/blob_repo.rs) | 75 | 1 |  |
| [`crates/uc-infra/src/db/repositories/clipboard_event_repo.rs`](../../../crates/uc-infra/src/db/repositories/clipboard_event_repo.rs) | 184, 187 | 2 |  |
| [`crates/uc-infra/src/db/repositories/representation_repo.rs`](../../../crates/uc-infra/src/db/repositories/representation_repo.rs) | 79, 104, 129, 179, 207, 242, 256, 279, 316 | 9 |  |
| [`crates/uc-infra/src/db/repositories/thumbnail_repo.rs`](../../../crates/uc-infra/src/db/repositories/thumbnail_repo.rs) | 43 | 1 |  |
| [`crates/uc-infra/src/file_transfer/persistence_cipher.rs`](../../../crates/uc-infra/src/file_transfer/persistence_cipher.rs) | 327, 342, 344 | 3 |  |
| [`crates/uc-infra/src/fs/cache_fs.rs`](../../../crates/uc-infra/src/fs/cache_fs.rs) | 36, 57, 63, 74, 81, 96, 103 | 7 |  |
| [`crates/uc-infra/src/search/rows.rs`](../../../crates/uc-infra/src/search/rows.rs) | 112 | 1 |  |
| [`crates/uc-infra/src/search/search_key_derivation.rs`](../../../crates/uc-infra/src/search/search_key_derivation.rs) | 139 | 1 |  |
| [`crates/uc-infra/src/security/identity_fingerprint.rs`](../../../crates/uc-infra/src/security/identity_fingerprint.rs) | 60 | 1 |  |
| [`crates/uc-infra/src/settings/repository.rs`](../../../crates/uc-infra/src/settings/repository.rs) | 119 | 1 |  |

## S3 错误类型只保存字符串

所在错误类型改为携带 `#[source]`（具体错误或 `anyhow::Error`）。为保存字符串而派生的 `Clone`/`PartialEq`/`Eq` 按错误处理规范调整调用方与测试，不得保留字符串副本。文本中含路径或标识的（如 `path.display()`、`{parent:?}`）同时违反隐私规则，优先处理。

生产代码共 524 处，按 crate 分布：

| crate | 数量 |
| --- | ---: |
| crates/uc-infra | 323 |
| crates/uc-application | 168 |
| compatibility/uc-mobile-lan | 17 |
| compatibility/uc-mobile | 7 |
| crates/uc-engine | 5 |
| compatibility/uc-mobile-proto | 3 |
| crates/uc-observability-contract | 1 |

| 文件 | 行号 | 数量 | 备注 |
| --- | --- | ---: | --- |
| [`compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs`](../../../compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs) | 914 | 1 |  |
| [`compatibility/uc-mobile-lan/src/usecases/authenticate_basic.rs`](../../../compatibility/uc-mobile-lan/src/usecases/authenticate_basic.rs) | 231 | 1 |  |
| [`compatibility/uc-mobile-lan/src/usecases/get_settings.rs`](../../../compatibility/uc-mobile-lan/src/usecases/get_settings.rs) | 116 | 1 |  |
| [`compatibility/uc-mobile-lan/src/usecases/latest_snapshot_adapter.rs`](../../../compatibility/uc-mobile-lan/src/usecases/latest_snapshot_adapter.rs) | 123, 135, 145, 165, 186, 195 | 6 |  |
| [`compatibility/uc-mobile-lan/src/usecases/list_devices.rs`](../../../compatibility/uc-mobile-lan/src/usecases/list_devices.rs) | 135 | 1 |  |
| [`compatibility/uc-mobile-lan/src/usecases/register_device.rs`](../../../compatibility/uc-mobile-lan/src/usecases/register_device.rs) | 368, 599, 605 | 3 |  |
| [`compatibility/uc-mobile-lan/src/usecases/revoke_device.rs`](../../../compatibility/uc-mobile-lan/src/usecases/revoke_device.rs) | 78 | 1 |  |
| [`compatibility/uc-mobile-lan/src/usecases/update_device.rs`](../../../compatibility/uc-mobile-lan/src/usecases/update_device.rs) | 276 | 1 |  |
| [`compatibility/uc-mobile-lan/src/usecases/update_settings.rs`](../../../compatibility/uc-mobile-lan/src/usecases/update_settings.rs) | 153, 183 | 2 |  |
| [`compatibility/uc-mobile-proto/src/connect_uri.rs`](../../../compatibility/uc-mobile-proto/src/connect_uri.rs) | 282, 353, 355 | 3 |  |
| [`compatibility/uc-mobile/src/client.rs`](../../../compatibility/uc-mobile/src/client.rs) | 444, 452, 577, 932, 978, 1271, 1278 | 7 |  |
| [`crates/uc-application/src/clipboard/active/mod.rs`](../../../crates/uc-application/src/clipboard/active/mod.rs) | 387, 389, 415 | 3 |  |
| [`crates/uc-application/src/clipboard/history/list_entry_projections.rs`](../../../crates/uc-application/src/clipboard/history/list_entry_projections.rs) | 293 | 1 |  |
| [`crates/uc-application/src/clipboard/history/toggle_favorite.rs`](../../../crates/uc-application/src/clipboard/history/toggle_favorite.rs) | 51 | 1 |  |
| [`crates/uc-application/src/clipboard/outbound/mod.rs`](../../../crates/uc-application/src/clipboard/outbound/mod.rs) | 414, 454, 978, 986, 1032 | 5 |  |
| [`crates/uc-application/src/clipboard/outbound/payload_prep.rs`](../../../crates/uc-application/src/clipboard/outbound/payload_prep.rs) | 196, 209 | 2 |  |
| [`crates/uc-application/src/clipboard/resource/mod.rs`](../../../crates/uc-application/src/clipboard/resource/mod.rs) | 69, 84, 100, 115, 151, 159, 171, 198 | 8 |  |
| [`crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs`](../../../crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs) | 231 | 1 |  |
| [`crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs`](../../../crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs) | 566, 572, 582, 612, 620, 644, 668, 699, 715, 747, 1198 | 11 |  |
| [`crates/uc-application/src/clipboard/sync/dispatch_entry/mod.rs`](../../../crates/uc-application/src/clipboard/sync/dispatch_entry/mod.rs) | 468 | 1 |  |
| [`crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs`](../../../crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs) | 72, 75 | 2 |  |
| [`crates/uc-application/src/clipboard/sync/existing_local_entry_delivery.rs`](../../../crates/uc-application/src/clipboard/sync/existing_local_entry_delivery.rs) | 69, 75, 119, 130, 186, 196 | 6 |  |
| [`crates/uc-application/src/clipboard/sync/get_entry_delivery_view.rs`](../../../crates/uc-application/src/clipboard/sync/get_entry_delivery_view.rs) | 137, 144, 167, 227, 233 | 5 |  |
| [`crates/uc-application/src/clipboard/sync/resend_entry.rs`](../../../crates/uc-application/src/clipboard/sync/resend_entry.rs) | 227, 235, 263 | 3 |  |
| [`crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs`](../../../crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs) | 201 | 1 |  |
| [`crates/uc-application/src/facade/clipboard_capture/mod.rs`](../../../crates/uc-application/src/facade/clipboard_capture/mod.rs) | 91, 146, 165, 208 | 4 |  |
| [`crates/uc-application/src/facade/clipboard_history/mod.rs`](../../../crates/uc-application/src/facade/clipboard_history/mod.rs) | 342, 347, 368, 416, 475, 496, 513, 522, 619 | 9 |  |
| [`crates/uc-application/src/facade/clipboard/cancel_entry_receive.rs`](../../../crates/uc-application/src/facade/clipboard/cancel_entry_receive.rs) | 27, 105, 122, 171, 181, 198 | 6 |  |
| [`crates/uc-application/src/facade/clipboard/facade.rs`](../../../crates/uc-application/src/facade/clipboard/facade.rs) | 280, 292, 302, 480, 508, 541 | 6 |  |
| [`crates/uc-application/src/facade/roster/facade.rs`](../../../crates/uc-application/src/facade/roster/facade.rs) | 98, 104, 116, 126, 196, 214, 227, 241, 250 | 9 |  |
| [`crates/uc-application/src/search/coordinator.rs`](../../../crates/uc-application/src/search/coordinator.rs) | 774 | 1 |  |
| [`crates/uc-application/src/search/live_index/mod.rs`](../../../crates/uc-application/src/search/live_index/mod.rs) | 84, 98, 167, 177 | 4 |  |
| [`crates/uc-application/src/settings/config_migration/facade.rs`](../../../crates/uc-application/src/settings/config_migration/facade.rs) | 88, 139 | 2 |  |
| [`crates/uc-application/src/settings/diagnostics.rs`](../../../crates/uc-application/src/settings/diagnostics.rs) | 68, 86, 92, 121, 147, 197, 203, 210, 212, 216, 218, 232, 234, 236, 238, 259, 261, 265, 281 | 19 |  |
| [`crates/uc-application/src/settings/facade.rs`](../../../crates/uc-application/src/settings/facade.rs) | 141, 219 | 2 |  |
| [`crates/uc-application/src/settings/relay_configuration.rs`](../../../crates/uc-application/src/settings/relay_configuration.rs) | 120, 163, 189, 210, 344 | 5 |  |
| [`crates/uc-application/src/settings/storage/mod.rs`](../../../crates/uc-application/src/settings/storage/mod.rs) | 60, 90, 98, 122 | 4 |  |
| [`crates/uc-application/src/settings/upgrade/acknowledge.rs`](../../../crates/uc-application/src/settings/upgrade/acknowledge.rs) | 35 | 1 |  |
| [`crates/uc-application/src/settings/upgrade/detect.rs`](../../../crates/uc-application/src/settings/upgrade/detect.rs) | 67, 78 | 2 |  |
| [`crates/uc-application/src/settings/upgrade/facade.rs`](../../../crates/uc-application/src/settings/upgrade/facade.rs) | 53, 73 | 2 |  |
| [`crates/uc-application/src/space/admission/invitation/issuer.rs`](../../../crates/uc-application/src/space/admission/invitation/issuer.rs) | 52 | 1 |  |
| [`crates/uc-application/src/space/admission/protocol/joiner/start_join/execute.rs`](../../../crates/uc-application/src/space/admission/protocol/joiner/start_join/execute.rs) | 202, 210 | 2 |  |
| [`crates/uc-application/src/space/lifecycle/lock_space_session/use_case.rs`](../../../crates/uc-application/src/space/lifecycle/lock_space_session/use_case.rs) | 32 | 1 |  |
| [`crates/uc-application/src/space/lifecycle/query_space_setup_state/use_case.rs`](../../../crates/uc-application/src/space/lifecycle/query_space_setup_state/use_case.rs) | 38, 50, 55 | 3 |  |
| [`crates/uc-application/src/space/lifecycle/recover_space_session/use_case.rs`](../../../crates/uc-application/src/space/lifecycle/recover_space_session/use_case.rs) | 40, 54 | 2 |  |
| [`crates/uc-application/src/space/lifecycle/reset_space/error.rs`](../../../crates/uc-application/src/space/lifecycle/reset_space/error.rs) | 32, 34, 35, 36, 38 | 5 |  |
| [`crates/uc-application/src/space/lifecycle/reset_space/use_case.rs`](../../../crates/uc-application/src/space/lifecycle/reset_space/use_case.rs) | 59, 64 | 2 |  |
| [`crates/uc-application/src/space/lifecycle/session/activity.rs`](../../../crates/uc-application/src/space/lifecycle/session/activity.rs) | 133 | 1 |  |
| [`crates/uc-application/src/space/lifecycle/unlock_space/readiness.rs`](../../../crates/uc-application/src/space/lifecycle/unlock_space/readiness.rs) | 39, 46 | 2 |  |
| [`crates/uc-application/src/space/lifecycle/upgrade_space/use_case.rs`](../../../crates/uc-application/src/space/lifecycle/upgrade_space/use_case.rs) | 82 | 1 |  |
| [`crates/uc-application/src/transfer/blob/facade.rs`](../../../crates/uc-application/src/transfer/blob/facade.rs) | 331, 512, 534, 597, 752 | 5 |  |
| [`crates/uc-application/src/transfer/blob/fetch_blob.rs`](../../../crates/uc-application/src/transfer/blob/fetch_blob.rs) | 81, 90, 95, 100, 107, 111, 147, 157, 161 | 9 |  |
| [`crates/uc-application/src/transfer/blob/publish_blob.rs`](../../../crates/uc-application/src/transfer/blob/publish_blob.rs) | 98, 111, 127, 138, 149, 195, 206, 214 | 8 |  |
| [`crates/uc-engine/src/assembly/platform.rs`](../../../crates/uc-engine/src/assembly/platform.rs) | 137 | 1 |  |
| [`crates/uc-engine/src/assembly/wire/infra.rs`](../../../crates/uc-engine/src/assembly/wire/infra.rs) | 24, 34, 210, 377 | 4 |  |
| [`crates/uc-infra/src/app_version_state.rs`](../../../crates/uc-infra/src/app_version_state.rs) | 86, 114, 136, 146, 149, 152, 156 | 7 |  |
| [`crates/uc-infra/src/clipboard/chunked_transfer.rs`](../../../crates/uc-infra/src/clipboard/chunked_transfer.rs) | 153, 197, 308, 365, 415, 444, 514, 547, 618, 667, 705, 706, 725, 732, 743 | 15 |  |
| [`crates/uc-infra/src/clipboard/payload_resolver.rs`](../../../crates/uc-infra/src/clipboard/payload_resolver.rs) | 132 | 1 |  |
| [`crates/uc-infra/src/db/repositories/blob_reference_repo.rs`](../../../crates/uc-infra/src/db/repositories/blob_reference_repo.rs) | 52, 57, 79, 91 | 4 |  |
| [`crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs`](../../../crates/uc-infra/src/db/repositories/clipboard_entry_repo.rs) | 342, 521 | 2 |  |
| [`crates/uc-infra/src/db/repositories/directory_publish_log_repo.rs`](../../../crates/uc-infra/src/db/repositories/directory_publish_log_repo.rs) | 69, 100, 123 | 3 |  |
| [`crates/uc-infra/src/db/repositories/entry_availability_repo.rs`](../../../crates/uc-infra/src/db/repositories/entry_availability_repo.rs) | 72 | 1 |  |
| [`crates/uc-infra/src/db/repositories/entry_file_set_repo.rs`](../../../crates/uc-infra/src/db/repositories/entry_file_set_repo.rs) | 147, 156, 194, 201, 204, 226, 234, 238, 321, 332, 338, 367, 379, 386, 554 | 15 |  |
| [`crates/uc-infra/src/db/repositories/entry_receive_attempt_repo.rs`](../../../crates/uc-infra/src/db/repositories/entry_receive_attempt_repo.rs) | 31 | 1 |  |
| [`crates/uc-infra/src/db/repositories/entry_replace_repo.rs`](../../../crates/uc-infra/src/db/repositories/entry_replace_repo.rs) | 42 | 1 |  |
| [`crates/uc-infra/src/db/repositories/file_transfer_repo.rs`](../../../crates/uc-infra/src/db/repositories/file_transfer_repo.rs) | 76, 178, 192, 203, 256, 276, 281, 315, 328, 334, 556 | 11 |  |
| [`crates/uc-infra/src/db/repositories/inbound_receive_commit_repo.rs`](../../../crates/uc-infra/src/db/repositories/inbound_receive_commit_repo.rs) | 95, 218, 224 | 3 |  |
| [`crates/uc-infra/src/db/repositories/migration_repo.rs`](../../../crates/uc-infra/src/db/repositories/migration_repo.rs) | 62, 93, 126, 140, 154, 193, 221, 234 | 8 |  |
| [`crates/uc-infra/src/db/repositories/mobile_device_repo.rs`](../../../crates/uc-infra/src/db/repositories/mobile_device_repo.rs) | 89, 116, 148, 172, 191, 203, 245 | 7 |  |
| [`crates/uc-infra/src/db/repositories/peer_address_repo.rs`](../../../crates/uc-infra/src/db/repositories/peer_address_repo.rs) | 87, 107, 115 | 3 |  |
| [`crates/uc-infra/src/db/repositories/receive_artifact_log_repo.rs`](../../../crates/uc-infra/src/db/repositories/receive_artifact_log_repo.rs) | 96, 114, 119 | 3 |  |
| [`crates/uc-infra/src/db/repositories/relationship_store.rs`](../../../crates/uc-infra/src/db/repositories/relationship_store.rs) | 175, 603, 643, 777, 786, 803, 822 | 7 |  |
| [`crates/uc-infra/src/db/repositories/space_member_repo.rs`](../../../crates/uc-infra/src/db/repositories/space_member_repo.rs) | 32, 39, 46, 53 | 4 |  |
| [`crates/uc-infra/src/db/repositories/space_security_store/legacy_bootstrap.rs`](../../../crates/uc-infra/src/db/repositories/space_security_store/legacy_bootstrap.rs) | 87, 90, 93, 96, 121, 124, 133, 191, 205, 212, 214, 233, 244, 322, 332, 350, 360, 369, 379, 381, 400, 410, 412, 431, 443, 490 | 26 | 049 处理中 |
| [`crates/uc-infra/src/db/repositories/space_security_store/revocation.rs`](../../../crates/uc-infra/src/db/repositories/space_security_store/revocation.rs) | 895, 897, 919 | 3 | 049 处理中 |
| [`crates/uc-infra/src/db/repositories/trusted_peer_repo.rs`](../../../crates/uc-infra/src/db/repositories/trusted_peer_repo.rs) | 32, 39, 46, 53 | 4 |  |
| [`crates/uc-infra/src/engine_version_state.rs`](../../../crates/uc-infra/src/engine_version_state.rs) | 44 | 1 |  |
| [`crates/uc-infra/src/file_secure_storage.rs`](../../../crates/uc-infra/src/file_secure_storage.rs) | 23 | 1 |  |
| [`crates/uc-infra/src/file_transfer/privacy_maintenance.rs`](../../../crates/uc-infra/src/file_transfer/privacy_maintenance.rs) | 76, 77 | 2 |  |
| [`crates/uc-infra/src/first_sync_state.rs`](../../../crates/uc-infra/src/first_sync_state.rs) | 81, 111, 129, 136, 139, 142, 146 | 7 |  |
| [`crates/uc-infra/src/fs/atomic_publish.rs`](../../../crates/uc-infra/src/fs/atomic_publish.rs) | 49, 61 | 2 |  |
| [`crates/uc-infra/src/fs/directory_staging_cleanup.rs`](../../../crates/uc-infra/src/fs/directory_staging_cleanup.rs) | 48 | 1 |  |
| [`crates/uc-infra/src/fs/receive_artifact_cleanup.rs`](../../../crates/uc-infra/src/fs/receive_artifact_cleanup.rs) | 35, 44 | 2 |  |
| [`crates/uc-infra/src/migration_state.rs`](../../../crates/uc-infra/src/migration_state.rs) | 68, 126, 326 | 3 |  |
| [`crates/uc-infra/src/mobile_sync/file_staging.rs`](../../../crates/uc-infra/src/mobile_sync/file_staging.rs) | 203, 210, 218, 235 | 4 |  |
| [`crates/uc-infra/src/mobile_sync/lan_probe.rs`](../../../crates/uc-infra/src/mobile_sync/lan_probe.rs) | 48 | 1 |  |
| [`crates/uc-infra/src/mobile_sync/password_hasher.rs`](../../../crates/uc-infra/src/mobile_sync/password_hasher.rs) | 45, 47, 52, 55, 63, 70, 74 | 7 |  |
| [`crates/uc-infra/src/network/iroh/active_clipboard/dispatch_adapter.rs`](../../../crates/uc-infra/src/network/iroh/active_clipboard/dispatch_adapter.rs) | 117, 127, 129 | 3 |  |
| [`crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs`](../../../crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs) | 139, 143, 145, 150 | 4 |  |
| [`crates/uc-infra/src/network/iroh/blobs.rs`](../../../crates/uc-infra/src/network/iroh/blobs.rs) | 222, 277, 324, 330, 374, 388, 431, 444, 458, 469, 536, 587, 703 | 13 |  |
| [`crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs`](../../../crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs) | 216, 224, 231, 414 | 4 |  |
| [`crates/uc-infra/src/network/iroh/identity_store.rs`](../../../crates/uc-infra/src/network/iroh/identity_store.rs) | 85, 146 | 2 |  |
| [`crates/uc-infra/src/network/iroh/node.rs`](../../../crates/uc-infra/src/network/iroh/node.rs) | 549, 944, 1503 | 3 |  |
| [`crates/uc-infra/src/network/iroh/relay_probe.rs`](../../../crates/uc-infra/src/network/iroh/relay_probe.rs) | 69, 104, 191, 198, 209, 212, 224, 231, 233, 247, 259 | 11 |  |
| [`crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs`](../../../crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs) | 343, 348, 350 | 3 |  |
| [`crates/uc-infra/src/pairing/mdns_publisher.rs`](../../../crates/uc-infra/src/pairing/mdns_publisher.rs) | 65, 71 | 2 |  |
| [`crates/uc-infra/src/pairing/mdns_resolver.rs`](../../../crates/uc-infra/src/pairing/mdns_resolver.rs) | 53 | 1 |  |
| [`crates/uc-infra/src/rendezvous/invitation_adapter.rs`](../../../crates/uc-infra/src/rendezvous/invitation_adapter.rs) | 104, 309, 496 | 3 |  |
| [`crates/uc-infra/src/search/render_payload.rs`](../../../crates/uc-infra/src/search/render_payload.rs) | 153 | 1 |  |
| [`crates/uc-infra/src/search/search_key_derivation.rs`](../../../crates/uc-infra/src/search/search_key_derivation.rs) | 62, 71, 80, 89 | 4 |  |
| [`crates/uc-infra/src/search/sqlite_index.rs`](../../../crates/uc-infra/src/search/sqlite_index.rs) | 203, 241, 246, 272, 275, 289, 358, 379, 421, 481, 522, 545, 556, 585, 598, 613, 695, 744, 783, 836, 847, 1025, 1071, 1094, 1165, 1169, 1178, 1183, 1311, 1335, 1346, 1356, 1444, 1482, 1501, 1516, 1533, 1562, 1581, 1586, 1623, 1642, 1675, 1732, 1741, 1745, 1765, 1769, 1817, 1821, 1908, 1912, 1976, 1982, 2005, 2026, 2038, 2045, 2056, 2077, 2106, 2123, 2129, 2133 | 64 |  |
| [`crates/uc-infra/src/security/decrypting_representation_repo.rs`](../../../crates/uc-infra/src/security/decrypting_representation_repo.rs) | 245 | 1 |  |
| [`crates/uc-infra/src/security/key_migration_adapter.rs`](../../../crates/uc-infra/src/security/key_migration_adapter.rs) | 60, 66, 110, 122, 124, 144, 159 | 7 |  |
| [`crates/uc-infra/src/security/profile_key_recovery.rs`](../../../crates/uc-infra/src/security/profile_key_recovery.rs) | 639, 643 | 2 | 049 处理中 |
| [`crates/uc-infra/src/security/space_control_generation/persistence.rs`](../../../crates/uc-infra/src/security/space_control_generation/persistence.rs) | 27 | 1 | 049 处理中 |
| [`crates/uc-infra/src/space/security/access.rs`](../../../crates/uc-infra/src/space/security/access.rs) | 229, 332, 335, 415, 422, 668, 705, 774, 782, 791, 803, 826, 834, 854, 921, 967, 989, 1018, 1886, 1897, 1924, 1935, 1989, 1997, 2085, 2095, 2119, 2143, 2699, 2748, 2753, 3593 | 32 |  |
| [`crates/uc-infra/src/space/security/membership_update.rs`](../../../crates/uc-infra/src/space/security/membership_update.rs) | 52, 67 | 2 |  |
| [`crates/uc-observability-contract/src/analytics/facade.rs`](../../../crates/uc-observability-contract/src/analytics/facade.rs) | 186 | 1 |  |

## S4 `map_err(|_| ..)` 丢弃来源

处置代码：**F** 来源来自被调用能力或仓库内其他模块，必须保留；**E** 来源不含诊断信息或不能保存（锁中毒持有 guard、
`TryFromIntError`、`Elapsed`、携带负载的 `SendError`），属于允许例外，需在修复切片中按规范加注释；**R** 需逐项判断：
纯输入校验可作为例外，读取持久数据或外部输入时必须保留来源。`uc-core` 中的 F 项多为 Core 内部校验错误改分类，
按“纯业务判断”规则逐项判断能否保留具体原因。

生产代码共 914 处，按来源类别分布：

| 处置 / 来源类别 | 数量 |
| --- | ---: |
| F 仓库内函数或 port | 279 |
| F 编解码 | 261 |
| F 密码 / MLS / 签名 | 72 |
| F 文件系统 / IO | 70 |
| E 整数 / 定长切片转换 | 58 |
| F 网络 / 流 | 33 |
| E 超时 | 30 |
| E 通道关闭 | 30 |
| F 运行时 / 单例 | 15 |
| F 安全存储 | 14 |
| F 数据库 | 13 |
| R 文本解析 | 13 |
| E 锁中毒 | 11 |
| R UTF-8 | 10 |
| F 任务 join | 3 |
| F 宿主回调 | 2 |

按 crate 分布：

| crate | 数量 |
| --- | ---: |
| crates/uc-infra | 569 |
| crates/uc-core | 136 |
| crates/uc-application | 83 |
| bindings/uc-engine-uniffi | 53 |
| crates/uc-engine | 38 |
| bindings/uc-ohos-napi | 17 |
| crates/uc-observability-runtime | 10 |
| compatibility/uc-mobile-lan | 3 |
| compatibility/uc-mobile-proto | 3 |
| compatibility/uc-mobile | 2 |

行号后的字母为处置代码。

| 文件 | 行号 | F | E/R | 备注 |
| --- | --- | ---: | ---: | --- |
| [`bindings/uc-engine-uniffi/src/observability.rs`](../../../bindings/uc-engine-uniffi/src/observability.rs) | 110F, 126F, 130F | 3 | 0 |  |
| [`bindings/uc-engine-uniffi/src/runtime.rs`](../../../bindings/uc-engine-uniffi/src/runtime.rs) | 641R, 644R, 661R, 664R, 855F, 928F, 931E, 939E, 942E, 957E, 960E, 1079E, 1097F, 1100E, 1108E, 1111E, 1129F, 1132E, 1152F, 1155E, 1163E, 1166E, 1182F, 1185E, 1203F, 1206E, 1222F, 1225E, 1233E, 1236E, 1247E, 1250E, 1266F, 1269E, 1289F, 1292E, 1312E, 1330E, 1333E, 1863E, 2114F, 2123F, 2133F, 2518E, 2543F | 14 | 31 |  |
| [`bindings/uc-engine-uniffi/src/runtime/lifecycle.rs`](../../../bindings/uc-engine-uniffi/src/runtime/lifecycle.rs) | 21E | 0 | 1 |  |
| [`bindings/uc-engine-uniffi/src/runtime/shutdown.rs`](../../../bindings/uc-engine-uniffi/src/runtime/shutdown.rs) | 44E | 0 | 1 |  |
| [`bindings/uc-engine-uniffi/src/runtime/startup_lifecycle.rs`](../../../bindings/uc-engine-uniffi/src/runtime/startup_lifecycle.rs) | 58F | 1 | 0 |  |
| [`bindings/uc-engine-uniffi/src/runtime/worker_join.rs`](../../../bindings/uc-engine-uniffi/src/runtime/worker_join.rs) | 42F | 1 | 0 |  |
| [`bindings/uc-engine-uniffi/src/runtime/worker_shutdown.rs`](../../../bindings/uc-engine-uniffi/src/runtime/worker_shutdown.rs) | 23F | 1 | 0 |  |
| [`bindings/uc-ohos-napi/src/host.rs`](../../../bindings/uc-ohos-napi/src/host.rs) | 43F, 108E, 132E, 300F, 362R | 2 | 3 |  |
| [`bindings/uc-ohos-napi/src/lib.rs`](../../../bindings/uc-ohos-napi/src/lib.rs) | 262F, 273F | 2 | 0 |  |
| [`bindings/uc-ohos-napi/src/local_diagnostics.rs`](../../../bindings/uc-ohos-napi/src/local_diagnostics.rs) | 276F | 1 | 0 |  |
| [`bindings/uc-ohos-napi/src/observability.rs`](../../../bindings/uc-ohos-napi/src/observability.rs) | 117F, 129F, 133F | 3 | 0 |  |
| [`bindings/uc-ohos-napi/src/runtime.rs`](../../../bindings/uc-ohos-napi/src/runtime.rs) | 227F, 242E, 255F, 617F, 847E, 851E | 3 | 3 |  |
| [`compatibility/uc-mobile-lan/src/facade/file_upload.rs`](../../../compatibility/uc-mobile-lan/src/facade/file_upload.rs) | 202F, 250E, 380F | 2 | 1 |  |
| [`compatibility/uc-mobile-proto/src/connect_uri.rs`](../../../compatibility/uc-mobile-proto/src/connect_uri.rs) | 316R, 340R | 0 | 2 |  |
| [`compatibility/uc-mobile-proto/src/history_record.rs`](../../../compatibility/uc-mobile-proto/src/history_record.rs) | 126R | 0 | 1 |  |
| [`compatibility/uc-mobile/src/client.rs`](../../../compatibility/uc-mobile/src/client.rs) | 448E, 1231F | 1 | 1 |  |
| [`crates/uc-application/src/clipboard/capture/usecase.rs`](../../../crates/uc-application/src/clipboard/capture/usecase.rs) | 1307F, 1314F, 1363F | 3 | 0 |  |
| [`crates/uc-application/src/clipboard/outbound/mod.rs`](../../../crates/uc-application/src/clipboard/outbound/mod.rs) | 857E, 876E, 996E | 0 | 3 |  |
| [`crates/uc-application/src/clipboard/sync/payload_codec.rs`](../../../crates/uc-application/src/clipboard/sync/payload_codec.rs) | 395E, 417E | 0 | 2 |  |
| [`crates/uc-application/src/facade/roster/facade.rs`](../../../crates/uc-application/src/facade/roster/facade.rs) | 121F | 1 | 0 |  |
| [`crates/uc-application/src/profile/factory_reset/use_case.rs`](../../../crates/uc-application/src/profile/factory_reset/use_case.rs) | 67F, 79F | 2 | 0 |  |
| [`crates/uc-application/src/runtime_lifecycle/invocation.rs`](../../../crates/uc-application/src/runtime_lifecycle/invocation.rs) | 27E | 0 | 1 |  |
| [`crates/uc-application/src/settings/relay_configuration.rs`](../../../crates/uc-application/src/settings/relay_configuration.rs) | 351R, 378F | 1 | 1 |  |
| [`crates/uc-application/src/settings/relay_credentials.rs`](../../../crates/uc-application/src/settings/relay_credentials.rs) | 171F, 366F, 407F, 461R | 3 | 1 |  |
| [`crates/uc-application/src/space/admission/protocol/joiner/start_join/execute.rs`](../../../crates/uc-application/src/space/admission/protocol/joiner/start_join/execute.rs) | 70F, 89F, 91F, 102F, 120F, 133F | 6 | 0 |  |
| [`crates/uc-application/src/space/membership/decide_device_trust_change/use_case.rs`](../../../crates/uc-application/src/space/membership/decide_device_trust_change/use_case.rs) | 56F, 90F, 102F, 126F, 153F, 169F, 216F | 7 | 0 |  |
| [`crates/uc-application/src/space/membership/handle_history_message/transfer.rs`](../../../crates/uc-application/src/space/membership/handle_history_message/transfer.rs) | 50E, 71F | 1 | 1 |  |
| [`crates/uc-application/src/space/membership/handle_history_message/use_case.rs`](../../../crates/uc-application/src/space/membership/handle_history_message/use_case.rs) | 87F, 261F, 263F, 272F | 4 | 0 |  |
| [`crates/uc-application/src/space/membership/initializer.rs`](../../../crates/uc-application/src/space/membership/initializer.rs) | 51F, 59F, 64F, 72F, 77F, 91F, 99F, 109F | 8 | 0 |  |
| [`crates/uc-application/src/space/membership/maintenance/runtime.rs`](../../../crates/uc-application/src/space/membership/maintenance/runtime.rs) | 86F, 97E, 98F | 2 | 1 |  |
| [`crates/uc-application/src/space/membership/owner.rs`](../../../crates/uc-application/src/space/membership/owner.rs) | 195E, 203E, 211E | 0 | 3 |  |
| [`crates/uc-application/src/space/membership/owner/draft.rs`](../../../crates/uc-application/src/space/membership/owner/draft.rs) | 130F, 176F | 2 | 0 |  |
| [`crates/uc-application/src/space/membership/owner/view.rs`](../../../crates/uc-application/src/space/membership/owner/view.rs) | 44F, 112F, 163F | 3 | 0 |  |
| [`crates/uc-application/src/space/membership/query_device_trust/use_case.rs`](../../../crates/uc-application/src/space/membership/query_device_trust/use_case.rs) | 148F, 486F | 2 | 0 |  |
| [`crates/uc-application/src/space/membership/reconcile_history_evidence/use_case.rs`](../../../crates/uc-application/src/space/membership/reconcile_history_evidence/use_case.rs) | 63F, 73F, 75F | 3 | 0 |  |
| [`crates/uc-application/src/space/membership/record/presentation.rs`](../../../crates/uc-application/src/space/membership/record/presentation.rs) | 33F, 35F, 39F | 3 | 0 |  |
| [`crates/uc-application/src/space/membership/remove_space_member/use_case.rs`](../../../crates/uc-application/src/space/membership/remove_space_member/use_case.rs) | 171F, 181F, 200F, 311F, 344F, 349F | 6 | 0 |  |
| [`crates/uc-application/src/space/membership/resolve_conflict/presentation.rs`](../../../crates/uc-application/src/space/membership/resolve_conflict/presentation.rs) | 37F | 1 | 0 |  |
| [`crates/uc-application/src/space/membership/worker.rs`](../../../crates/uc-application/src/space/membership/worker.rs) | 147F | 1 | 0 |  |
| [`crates/uc-application/src/space/membership/worker/effects.rs`](../../../crates/uc-application/src/space/membership/worker/effects.rs) | 39F | 1 | 0 |  |
| [`crates/uc-application/src/space/membership/worker/history_sync.rs`](../../../crates/uc-application/src/space/membership/worker/history_sync.rs) | 138F, 300F, 324F | 3 | 0 |  |
| [`crates/uc-application/src/transfer/blob/facade.rs`](../../../crates/uc-application/src/transfer/blob/facade.rs) | 401E, 475E, 575E, 603E, 723E, 761E | 0 | 6 |  |
| [`crates/uc-application/src/transfer/file/facade.rs`](../../../crates/uc-application/src/transfer/file/facade.rs) | 173F | 1 | 0 |  |
| [`crates/uc-core/src/ids/device_id.rs`](../../../crates/uc-core/src/ids/device_id.rs) | 69F | 1 | 0 |  |
| [`crates/uc-core/src/membership/admission_content_key_catalog.rs`](../../../crates/uc-core/src/membership/admission_content_key_catalog.rs) | 145F, 150F | 2 | 0 |  |
| [`crates/uc-core/src/membership/ledger/aggregate.rs`](../../../crates/uc-core/src/membership/ledger/aggregate.rs) | 727F, 735F | 2 | 0 |  |
| [`crates/uc-core/src/membership/ledger/snapshot.rs`](../../../crates/uc-core/src/membership/ledger/snapshot.rs) | 129F | 1 | 0 |  |
| [`crates/uc-core/src/membership/membership_branch_recovery.rs`](../../../crates/uc-core/src/membership/membership_branch_recovery.rs) | 158F, 160F | 2 | 0 |  |
| [`crates/uc-core/src/membership/membership_conflict_policy.rs`](../../../crates/uc-core/src/membership/membership_conflict_policy.rs) | 224F, 235F | 2 | 0 |  |
| [`crates/uc-core/src/membership/space_admission/attempt.rs`](../../../crates/uc-core/src/membership/space_admission/attempt.rs) | 293F, 296F, 299E, 308R, 310F, 312F, 329E | 4 | 3 |  |
| [`crates/uc-core/src/membership/space_admission/message.rs`](../../../crates/uc-core/src/membership/space_admission/message.rs) | 300F, 408F | 2 | 0 |  |
| [`crates/uc-core/src/membership/space_admission/state/persistence/aggregate.rs`](../../../crates/uc-core/src/membership/space_admission/state/persistence/aggregate.rs) | 201F, 213F, 220F, 362F, 370F, 381F, 389F, 532F, 670F, 680F, 690F, 764F, 803F | 13 | 0 |  |
| [`crates/uc-core/src/membership/space_admission/state/persistence/initial.rs`](../../../crates/uc-core/src/membership/space_admission/state/persistence/initial.rs) | 45F, 62F, 70F, 72F, 115F, 120F, 124F, 163F, 168F, 171F, 173F, 209F, 213F, 218F, 245F, 247F, 252F, 254F | 18 | 0 |  |
| [`crates/uc-core/src/membership/space_admission/state/persistence/invitation.rs`](../../../crates/uc-core/src/membership/space_admission/state/persistence/invitation.rs) | 33F, 45F, 47F, 74F, 76F, 78F | 6 | 0 |  |
| [`crates/uc-core/src/membership/space_admission/state/persistence/joiner.rs`](../../../crates/uc-core/src/membership/space_admission/state/persistence/joiner.rs) | 36F, 199F | 2 | 0 |  |
| [`crates/uc-core/src/membership/space_admission/state/persistence/message.rs`](../../../crates/uc-core/src/membership/space_admission/state/persistence/message.rs) | 76F, 209F, 211F, 247F, 266F, 296F, 300F, 302F, 328F, 367F, 390F, 394F, 396F, 398F, 400F, 442F, 479F, 483F, 485F, 546F, 550F, 552F, 621F, 667F, 671F, 674F | 26 | 0 |  |
| [`crates/uc-core/src/membership/space_admission/state/persistence/mod.rs`](../../../crates/uc-core/src/membership/space_admission/state/persistence/mod.rs) | 43F, 50F | 2 | 0 |  |
| [`crates/uc-core/src/membership/space_admission/state/persistence/sponsor.rs`](../../../crates/uc-core/src/membership/space_admission/state/persistence/sponsor.rs) | 30F, 32F, 67F, 72F | 4 | 0 |  |
| [`crates/uc-core/src/membership/space_admission/state/persistence/terminal.rs`](../../../crates/uc-core/src/membership/space_admission/state/persistence/terminal.rs) | 69F, 111F | 2 | 0 |  |
| [`crates/uc-core/src/membership/space_admission/state/persistence/value.rs`](../../../crates/uc-core/src/membership/space_admission/state/persistence/value.rs) | 17F, 24F | 2 | 0 |  |
| [`crates/uc-core/src/membership/space_admission/state/transition/construct.rs`](../../../crates/uc-core/src/membership/space_admission/state/transition/construct.rs) | 14F, 51F | 2 | 0 |  |
| [`crates/uc-core/src/membership/space_admission/state/transition/helper.rs`](../../../crates/uc-core/src/membership/space_admission/state/transition/helper.rs) | 96F | 1 | 0 |  |
| [`crates/uc-core/src/membership/space_admission/state/transition/joiner.rs`](../../../crates/uc-core/src/membership/space_admission/state/transition/joiner.rs) | 364F, 785F, 792F, 814F, 821F | 5 | 0 |  |
| [`crates/uc-core/src/membership/space_admission/state/transition/sponsor.rs`](../../../crates/uc-core/src/membership/space_admission/state/transition/sponsor.rs) | 26F, 93F, 172F, 266F, 410F, 562F, 621F | 7 | 0 |  |
| [`crates/uc-core/src/membership/space_admission/state/transition/terminal.rs`](../../../crates/uc-core/src/membership/space_admission/state/transition/terminal.rs) | 58F, 102F, 265F, 285F, 318F, 326F, 380F, 386F, 388F | 9 | 0 |  |
| [`crates/uc-core/src/membership/versioned_membership_history/archive.rs`](../../../crates/uc-core/src/membership/versioned_membership_history/archive.rs) | 78F, 96F, 104F | 3 | 0 |  |
| [`crates/uc-core/src/membership/versioned_membership_history/exchange/mod.rs`](../../../crates/uc-core/src/membership/versioned_membership_history/exchange/mod.rs) | 101F, 134F | 2 | 0 |  |
| [`crates/uc-core/src/membership/versioned_membership_history/exchange/pages.rs`](../../../crates/uc-core/src/membership/versioned_membership_history/exchange/pages.rs) | 58F, 88E, 139F, 219E | 2 | 2 |  |
| [`crates/uc-core/src/membership/versioned_membership_history/exchange/proof.rs`](../../../crates/uc-core/src/membership/versioned_membership_history/exchange/proof.rs) | 103F | 1 | 0 |  |
| [`crates/uc-core/src/membership/versioned_membership_history/exchange/suffix.rs`](../../../crates/uc-core/src/membership/versioned_membership_history/exchange/suffix.rs) | 79E, 102E, 241F | 1 | 2 |  |
| [`crates/uc-core/src/network/protocol/clipboard_payload_v3.rs`](../../../crates/uc-core/src/network/protocol/clipboard_payload_v3.rs) | 72E, 96E, 121E, 146E | 0 | 4 |  |
| [`crates/uc-core/src/search/key.rs`](../../../crates/uc-core/src/search/key.rs) | 18E | 0 | 1 |  |
| [`crates/uc-engine/src/assembly/host.rs`](../../../crates/uc-engine/src/assembly/host.rs) | 109F, 507F, 519F | 3 | 0 |  |
| [`crates/uc-engine/src/dev/mod.rs`](../../../crates/uc-engine/src/dev/mod.rs) | 129F | 1 | 0 |  |
| [`crates/uc-engine/src/engine/lifecycle.rs`](../../../crates/uc-engine/src/engine/lifecycle.rs) | 97E, 100F | 1 | 1 |  |
| [`crates/uc-engine/src/engine/mod.rs`](../../../crates/uc-engine/src/engine/mod.rs) | 133F | 1 | 0 |  |
| [`crates/uc-engine/src/engine/operation.rs`](../../../crates/uc-engine/src/engine/operation.rs) | 102F | 1 | 0 |  |
| [`crates/uc-engine/src/engine/shutdown.rs`](../../../crates/uc-engine/src/engine/shutdown.rs) | 17F, 26E, 27E | 1 | 2 |  |
| [`crates/uc-engine/src/engine/startup_owner.rs`](../../../crates/uc-engine/src/engine/startup_owner.rs) | 32F | 1 | 0 |  |
| [`crates/uc-engine/src/operations/clipboard/query_active.rs`](../../../crates/uc-engine/src/operations/clipboard/query_active.rs) | 10F | 1 | 0 |  |
| [`crates/uc-engine/src/operations/device/member.rs`](../../../crates/uc-engine/src/operations/device/member.rs) | 40F | 1 | 0 |  |
| [`crates/uc-engine/src/operations/device/peer_connections.rs`](../../../crates/uc-engine/src/operations/device/peer_connections.rs) | 38F | 1 | 0 |  |
| [`crates/uc-engine/src/operations/history/history.rs`](../../../crates/uc-engine/src/operations/history/history.rs) | 26E | 0 | 1 |  |
| [`crates/uc-engine/src/operations/settings/config_migration.rs`](../../../crates/uc-engine/src/operations/settings/config_migration.rs) | 135F | 1 | 0 |  |
| [`crates/uc-engine/src/operations/settings/diagnostics.rs`](../../../crates/uc-engine/src/operations/settings/diagnostics.rs) | 22F, 39F, 60F | 3 | 0 |  |
| [`crates/uc-engine/src/operations/settings/settings.rs`](../../../crates/uc-engine/src/operations/settings/settings.rs) | 64F, 76F, 102F, 580E | 3 | 1 |  |
| [`crates/uc-engine/src/operations/settings/upgrade_backups.rs`](../../../crates/uc-engine/src/operations/settings/upgrade_backups.rs) | 31F, 51F | 2 | 0 |  |
| [`crates/uc-engine/src/operations/settings/upgrade.rs`](../../../crates/uc-engine/src/operations/settings/upgrade.rs) | 14F, 28F | 2 | 0 |  |
| [`crates/uc-engine/src/operations/space/cancel_join_space.rs`](../../../crates/uc-engine/src/operations/space/cancel_join_space.rs) | 13F, 14E | 1 | 1 |  |
| [`crates/uc-engine/src/runtime/dispatch.rs`](../../../crates/uc-engine/src/runtime/dispatch.rs) | 258F | 1 | 0 |  |
| [`crates/uc-engine/src/runtime/host_file.rs`](../../../crates/uc-engine/src/runtime/host_file.rs) | 18F, 24F, 45F, 58F, 62F | 5 | 0 |  |
| [`crates/uc-engine/src/runtime/lan_compatibility.rs`](../../../crates/uc-engine/src/runtime/lan_compatibility.rs) | 47F | 1 | 0 |  |
| [`crates/uc-engine/src/runtime/session_supervisor/lifecycle.rs`](../../../crates/uc-engine/src/runtime/session_supervisor/lifecycle.rs) | 74F | 1 | 0 |  |
| [`crates/uc-infra/src/clipboard/chunked_transfer.rs`](../../../crates/uc-infra/src/clipboard/chunked_transfer.rs) | 155E, 224F, 234E, 238E, 242E, 248E, 317F, 332F, 345F, 432E, 505E, 562E, 570R, 576F, 583E, 587E, 591E, 596E, 625F, 636F, 655F | 8 | 13 |  |
| [`crates/uc-infra/src/config_migration/adapter.rs`](../../../crates/uc-infra/src/config_migration/adapter.rs) | 160F, 180F, 214F, 217F, 266F, 284F, 343F, 356E, 390F, 408E, 436F, 478F, 490F, 504F, 512F, 517F, 520F, 526F, 595F | 17 | 2 |  |
| [`crates/uc-infra/src/config_migration/archive.rs`](../../../crates/uc-infra/src/config_migration/archive.rs) | 77F, 83F, 85F, 87F, 98F, 100F, 101F, 104F, 113F | 9 | 0 |  |
| [`crates/uc-infra/src/config_migration/bundle.rs`](../../../crates/uc-infra/src/config_migration/bundle.rs) | 223F, 228F, 251F, 255F, 284F, 305F, 314F, 338F, 349F | 9 | 0 |  |
| [`crates/uc-infra/src/config_migration/staging.rs`](../../../crates/uc-infra/src/config_migration/staging.rs) | 96F, 151F, 154F, 156F, 161F, 163F, 168F, 170F, 171F, 184F, 244F, 246F, 258F, 260F, 319F, 320F, 327F, 341F, 374F, 375F, 376F, 385F, 392F | 23 | 0 |  |
| [`crates/uc-infra/src/db/mappers/clipboard_selection_mapper.rs`](../../../crates/uc-infra/src/db/mappers/clipboard_selection_mapper.rs) | 61R | 0 | 1 |  |
| [`crates/uc-infra/src/db/pool.rs`](../../../crates/uc-infra/src/db/pool.rs) | 63E | 0 | 1 |  |
| [`crates/uc-infra/src/db/repositories/active_clipboard_register_cipher.rs`](../../../crates/uc-infra/src/db/repositories/active_clipboard_register_cipher.rs) | 77F, 102F, 139F, 141F, 169F, 171F | 6 | 0 |  |
| [`crates/uc-infra/src/db/repositories/directory_publish_log_cipher.rs`](../../../crates/uc-infra/src/db/repositories/directory_publish_log_cipher.rs) | 83F, 113F, 152F, 155F, 189F, 191F | 6 | 0 |  |
| [`crates/uc-infra/src/db/repositories/directory_publish_log_repo.rs`](../../../crates/uc-infra/src/db/repositories/directory_publish_log_repo.rs) | 118F, 185E, 248E | 1 | 2 |  |
| [`crates/uc-infra/src/db/repositories/entry_file_set_cipher.rs`](../../../crates/uc-infra/src/db/repositories/entry_file_set_cipher.rs) | 189F, 259F, 322F, 323R | 3 | 1 |  |
| [`crates/uc-infra/src/db/repositories/file_transfer_repo.rs`](../../../crates/uc-infra/src/db/repositories/file_transfer_repo.rs) | 557E, 565F | 1 | 1 |  |
| [`crates/uc-infra/src/db/repositories/receive_artifact_cipher.rs`](../../../crates/uc-infra/src/db/repositories/receive_artifact_cipher.rs) | 89F, 121F, 178F, 181F, 214F, 216F | 6 | 0 |  |
| [`crates/uc-infra/src/db/repositories/relationship_store.rs`](../../../crates/uc-infra/src/db/repositories/relationship_store.rs) | 791F, 808F, 859F, 861F, 879F | 5 | 0 |  |
| [`crates/uc-infra/src/db/repositories/space_security_store/encrypted_payload.rs`](../../../crates/uc-infra/src/db/repositories/space_security_store/encrypted_payload.rs) | 39F, 42F, 43F | 3 | 0 | 049 处理中 |
| [`crates/uc-infra/src/file_transfer/event_store/in_memory.rs`](../../../crates/uc-infra/src/file_transfer/event_store/in_memory.rs) | 25F, 35F | 2 | 0 |  |
| [`crates/uc-infra/src/file_transfer/persistence_cipher.rs`](../../../crates/uc-infra/src/file_transfer/persistence_cipher.rs) | 154F, 167F, 178F, 199F, 263F, 283F, 294F, 316F, 378F, 403F | 10 | 0 |  |
| [`crates/uc-infra/src/file_transfer/publisher/in_memory.rs`](../../../crates/uc-infra/src/file_transfer/publisher/in_memory.rs) | 21F, 33F | 2 | 0 |  |
| [`crates/uc-infra/src/fs/atomic_publish.rs`](../../../crates/uc-infra/src/fs/atomic_publish.rs) | 145F | 1 | 0 |  |
| [`crates/uc-infra/src/fs/key_slot_store.rs`](../../../crates/uc-infra/src/fs/key_slot_store.rs) | 53F, 56F, 65F, 74F, 80F, 84F, 88F, 99F, 105F | 9 | 0 |  |
| [`crates/uc-infra/src/migration_state.rs`](../../../crates/uc-infra/src/migration_state.rs) | 74F, 151F, 217F, 222F | 4 | 0 |  |
| [`crates/uc-infra/src/mobile_sync/file_staging.rs`](../../../crates/uc-infra/src/mobile_sync/file_staging.rs) | 206F, 220F, 473F | 3 | 0 |  |
| [`crates/uc-infra/src/network/iroh/active_clipboard/pull_wire.rs`](../../../crates/uc-infra/src/network/iroh/active_clipboard/pull_wire.rs) | 193R | 0 | 1 |  |
| [`crates/uc-infra/src/network/iroh/blobs.rs`](../../../crates/uc-infra/src/network/iroh/blobs.rs) | 116F | 1 | 0 |  |
| [`crates/uc-infra/src/network/iroh/group_update_adapter.rs`](../../../crates/uc-infra/src/network/iroh/group_update_adapter.rs) | 46E, 47E, 123F, 127E, 135F, 293F, 306F | 4 | 3 |  |
| [`crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs`](../../../crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs) | 76F, 81F, 90F, 210F, 223F, 240F, 254F, 259F, 272F, 275E, 276E, 287F, 295F, 373F, 385F, 394F, 400F, 408F, 437F, 441F, 470F, 478F, 481F, 533E, 534E, 555F, 597F, 621F, 627F, 660F, 672F, 684F, 708F, 709F, 806F, 807F, 923E, 924E, 935F, 939E, 956F | 34 | 7 |  |
| [`crates/uc-infra/src/network/iroh/membership_history_exchange_adapter.rs`](../../../crates/uc-infra/src/network/iroh/membership_history_exchange_adapter.rs) | 141F, 147E, 148E, 309F, 312F, 315F, 382F, 400F, 423F, 458F, 506E, 509F, 512F, 514F, 523E, 524E, 534E, 535E, 540E, 541E | 11 | 9 |  |
| [`crates/uc-infra/src/network/iroh/node.rs`](../../../crates/uc-infra/src/network/iroh/node.rs) | 597E | 0 | 1 |  |
| [`crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs`](../../../crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs) | 671F, 674F, 675F, 680F | 4 | 0 |  |
| [`crates/uc-infra/src/network/iroh/session_generation.rs`](../../../crates/uc-infra/src/network/iroh/session_generation.rs) | 397F | 1 | 0 |  |
| [`crates/uc-infra/src/network/iroh/space_admission_wire.rs`](../../../crates/uc-infra/src/network/iroh/space_admission_wire.rs) | 119F, 142F, 162F, 213E, 226E, 232F | 4 | 2 |  |
| [`crates/uc-infra/src/network/iroh/space_admission/client.rs`](../../../crates/uc-infra/src/network/iroh/space_admission/client.rs) | 77F, 88F, 112F, 126F, 129F, 132F, 142F, 214F, 229F | 9 | 0 |  |
| [`crates/uc-infra/src/network/iroh/space_admission/connection.rs`](../../../crates/uc-infra/src/network/iroh/space_admission/connection.rs) | 86E, 87E | 0 | 2 |  |
| [`crates/uc-infra/src/network/iroh/space_admission/crypto.rs`](../../../crates/uc-infra/src/network/iroh/space_admission/crypto.rs) | 35F | 1 | 0 |  |
| [`crates/uc-infra/src/network/iroh/space_admission/exchange.rs`](../../../crates/uc-infra/src/network/iroh/space_admission/exchange.rs) | 99F, 114F, 128F, 150F, 155F, 160F, 169F, 171F | 8 | 0 |  |
| [`crates/uc-infra/src/network/iroh/space_admission/route.rs`](../../../crates/uc-infra/src/network/iroh/space_admission/route.rs) | 25F, 38F, 46F, 51F, 62F, 74F | 6 | 0 |  |
| [`crates/uc-infra/src/network/iroh/space_admission/server.rs`](../../../crates/uc-infra/src/network/iroh/space_admission/server.rs) | 122F, 127F, 166F | 3 | 0 |  |
| [`crates/uc-infra/src/network/iroh/space_admission/server/authentication.rs`](../../../crates/uc-infra/src/network/iroh/space_admission/server/authentication.rs) | 52E, 62F, 68F, 92F, 146F | 4 | 1 |  |
| [`crates/uc-infra/src/rendezvous/invitation_adapter.rs`](../../../crates/uc-infra/src/rendezvous/invitation_adapter.rs) | 144F, 149F, 157F | 3 | 0 |  |
| [`crates/uc-infra/src/search/render_payload.rs`](../../../crates/uc-infra/src/search/render_payload.rs) | 156F, 186F, 188F | 3 | 0 |  |
| [`crates/uc-infra/src/search/sqlite_index.rs`](../../../crates/uc-infra/src/search/sqlite_index.rs) | 426F | 1 | 0 |  |
| [`crates/uc-infra/src/security/active_space_generation_manifest_store.rs`](../../../crates/uc-infra/src/security/active_space_generation_manifest_store.rs) | 97F, 109F, 119F, 342F, 366F, 494F, 516F, 547F, 567F, 610F, 641F, 696F, 739F, 776F, 799F | 15 | 0 |  |
| [`crates/uc-infra/src/security/admission_key_manager.rs`](../../../crates/uc-infra/src/security/admission_key_manager.rs) | 83F, 99F, 101F, 108F, 112F, 114F, 125F, 131F, 157F, 158F, 171F, 172F, 181F, 188F, 208F, 238F, 260F, 263E, 283F, 286F, 287F, 298F, 306F, 311F, 315F | 24 | 1 | 049 处理中 |
| [`crates/uc-infra/src/security/content_protection/envelope.rs`](../../../crates/uc-infra/src/security/content_protection/envelope.rs) | 45E | 0 | 1 |  |
| [`crates/uc-infra/src/security/crypto_model.rs`](../../../crates/uc-infra/src/security/crypto_model.rs) | 136F | 1 | 0 |  |
| [`crates/uc-infra/src/security/encrypted_blob_store.rs`](../../../crates/uc-infra/src/security/encrypted_blob_store.rs) | 107E, 121E, 130R, 132F, 135E | 1 | 4 |  |
| [`crates/uc-infra/src/security/encrypting_inbound_receive_commit.rs`](../../../crates/uc-infra/src/security/encrypting_inbound_receive_commit.rs) | 88F, 108F | 2 | 0 |  |
| [`crates/uc-infra/src/security/key_migration_adapter.rs`](../../../crates/uc-infra/src/security/key_migration_adapter.rs) | 136F | 1 | 0 |  |
| [`crates/uc-infra/src/security/profile_lifecycle.rs`](../../../crates/uc-infra/src/security/profile_lifecycle.rs) | 47F, 52F, 62F, 65F, 89E | 4 | 1 |  |
| [`crates/uc-infra/src/security/profile_reset.rs`](../../../crates/uc-infra/src/security/profile_reset.rs) | 52F, 59F, 63F, 80F, 84F, 96F, 100F, 127F, 131F, 161F, 167F, 170F, 181F, 182F, 185F, 192F, 263F, 284F, 286F | 19 | 0 |  |
| [`crates/uc-infra/src/security/secrets.rs`](../../../crates/uc-infra/src/security/secrets.rs) | 47F | 1 | 0 |  |
| [`crates/uc-infra/src/security/space_admission_auth.rs`](../../../crates/uc-infra/src/security/space_admission_auth.rs) | 313E | 0 | 1 |  |
| [`crates/uc-infra/src/security/v1_aead.rs`](../../../crates/uc-infra/src/security/v1_aead.rs) | 82F, 85F, 102F, 108F, 109F, 127F, 136F, 153F, 162F | 9 | 0 |  |
| [`crates/uc-infra/src/security/v3_initial_space_activation.rs`](../../../crates/uc-infra/src/security/v3_initial_space_activation.rs) | 64F | 1 | 0 |  |
| [`crates/uc-infra/src/space/adapters/current_space.rs`](../../../crates/uc-infra/src/space/adapters/current_space.rs) | 54F, 73F, 84F, 87F, 90F, 93F, 103F, 114F, 117F, 120F, 123F | 11 | 0 |  |
| [`crates/uc-infra/src/space/adapters/re_pairing_state.rs`](../../../crates/uc-infra/src/space/adapters/re_pairing_state.rs) | 51F, 66F, 74F, 77F, 80F, 83F | 6 | 0 |  |
| [`crates/uc-infra/src/space/adapters/rebuild_progress.rs`](../../../crates/uc-infra/src/space/adapters/rebuild_progress.rs) | 25F, 43F, 46F, 49F | 4 | 0 |  |
| [`crates/uc-infra/src/space/admission/display.rs`](../../../crates/uc-infra/src/space/admission/display.rs) | 91F | 1 | 0 |  |
| [`crates/uc-infra/src/space/admission/full_invitation.rs`](../../../crates/uc-infra/src/space/admission/full_invitation.rs) | 64F, 69F, 82F, 84F, 110F | 5 | 0 |  |
| [`crates/uc-infra/src/space/admission/joiner/invitation_start.rs`](../../../crates/uc-infra/src/space/admission/joiner/invitation_start.rs) | 43F | 1 | 0 |  |
| [`crates/uc-infra/src/space/admission/joiner/source_snapshot.rs`](../../../crates/uc-infra/src/space/admission/joiner/source_snapshot.rs) | 58F, 60F | 2 | 0 |  |
| [`crates/uc-infra/src/space/admission/joiner/sponsor_identity.rs`](../../../crates/uc-infra/src/space/admission/joiner/sponsor_identity.rs) | 25F, 28F | 2 | 0 |  |
| [`crates/uc-infra/src/space/admission/joiner/start_material.rs`](../../../crates/uc-infra/src/space/admission/joiner/start_material.rs) | 78F, 243F | 2 | 0 |  |
| [`crates/uc-infra/src/space/admission/recovery_material.rs`](../../../crates/uc-infra/src/space/admission/recovery_material.rs) | 61F, 70F, 93F, 138F, 147F | 5 | 0 |  |
| [`crates/uc-infra/src/space/admission/repository/codec.rs`](../../../crates/uc-infra/src/space/admission/repository/codec.rs) | 153F, 234F, 275F, 299F, 328F, 339F, 363F, 370F, 375F, 386F, 430F, 457F, 475F | 13 | 0 |  |
| [`crates/uc-infra/src/space/admission/repository/recovery_index.rs`](../../../crates/uc-infra/src/space/admission/repository/recovery_index.rs) | 94E, 295F, 332F | 2 | 1 |  |
| [`crates/uc-infra/src/space/admission/security/transition.rs`](../../../crates/uc-infra/src/space/admission/security/transition.rs) | 25F, 52F, 75F, 82F, 99F | 5 | 0 |  |
| [`crates/uc-infra/src/space/admission/sponsor/base_snapshot.rs`](../../../crates/uc-infra/src/space/admission/sponsor/base_snapshot.rs) | 23F, 47F, 57F, 59F | 4 | 0 |  |
| [`crates/uc-infra/src/space/admission/sponsor/state.rs`](../../../crates/uc-infra/src/space/admission/sponsor/state.rs) | 231F, 233F | 2 | 0 |  |
| [`crates/uc-infra/src/space/membership_record/codec.rs`](../../../crates/uc-infra/src/space/membership_record/codec.rs) | 27F, 48F | 2 | 0 |  |
| [`crates/uc-infra/src/space/membership_record/codec/migrate.rs`](../../../crates/uc-infra/src/space/membership_record/codec/migrate.rs) | 59F, 93F | 2 | 0 |  |
| [`crates/uc-infra/src/space/membership_record/codec/v5.rs`](../../../crates/uc-infra/src/space/membership_record/codec/v5.rs) | 137F, 166F, 200F, 220F | 4 | 0 |  |
| [`crates/uc-infra/src/space/membership_record/store.rs`](../../../crates/uc-infra/src/space/membership_record/store.rs) | 94F, 111F, 150F, 168F | 4 | 0 |  |
| [`crates/uc-infra/src/space/security/access.rs`](../../../crates/uc-infra/src/space/security/access.rs) | 259F, 290F, 527F, 534F, 544F, 553F, 579F, 589F, 599F, 608F, 628F, 648F, 874F, 2590F, 2606F, 2612F, 2616F, 2691F, 2743F, 2805F, 2831F, 2840F, 2847F, 2860F, 2869F, 2885F, 2903F, 2917F, 2939F, 2949F, 2951F, 2953F, 2961F, 2972F, 2996F, 3019F, 3029F, 3048F, 3055F, 3066F, 3073F, 3091F, 3142F, 3152F, 3162F, 3173F, 3183F, 3194F, 3197F, 3211F, 3221F, 3225F, 3229F, 3485F, 3514F, 3588F | 56 | 0 |  |
| [`crates/uc-infra/src/space/security/content_key_catalog.rs`](../../../crates/uc-infra/src/space/security/content_key_catalog.rs) | 22F, 26F, 45F, 53F, 63F | 5 | 0 |  |
| [`crates/uc-infra/src/space/security/membership_update.rs`](../../../crates/uc-infra/src/space/security/membership_update.rs) | 47F | 1 | 0 |  |
| [`crates/uc-infra/src/space/security/mls_group.rs`](../../../crates/uc-infra/src/space/security/mls_group.rs) | 256F, 263E, 265R, 364F, 372E, 394F, 397F, 400F, 409F, 413F, 448F, 451F, 453F, 455E, 474F, 478F, 481F, 488F, 494F, 499F, 502F, 518F, 544F, 548F, 574F, 588F, 591F, 596F, 613F, 633F, 652F, 657F, 679F, 696F, 706E, 710R, 733F, 737F, 745R, 759F, 766F, 784F, 790R, 804F, 837F, 843R, 880F, 883F, 885F, 888F, 894F, 990F, 993F, 1012F, 1019F, 1025F, 1030F, 1056F, 1057F | 50 | 9 |  |
| [`crates/uc-infra/src/space/security/session.rs`](../../../crates/uc-infra/src/space/security/session.rs) | 436F, 442F, 471F, 473F, 477F, 497F, 500F, 595F, 603F, 645F, 787F, 808F | 12 | 0 |  |
| [`crates/uc-observability-runtime/src/config.rs`](../../../crates/uc-observability-runtime/src/config.rs) | 82F, 253R | 1 | 1 |  |
| [`crates/uc-observability-runtime/src/local_capture.rs`](../../../crates/uc-observability-runtime/src/local_capture.rs) | 259R | 0 | 1 |  |
| [`crates/uc-observability-runtime/src/runtime.rs`](../../../crates/uc-observability-runtime/src/runtime.rs) | 59F, 63F, 532E | 2 | 1 |  |
| [`crates/uc-observability-runtime/src/subscriber.rs`](../../../crates/uc-observability-runtime/src/subscriber.rs) | 20F | 1 | 0 |  |
| [`crates/uc-observability-runtime/src/telemetry.rs`](../../../crates/uc-observability-runtime/src/telemetry.rs) | 182E, 191E, 200E | 0 | 3 |  |

## L1 日志直接输出错误正文

`%error` 只输出最外层文本，丢失 source chain；`{:#}` 或 `?error` 会输出整条链，可能带出路径、标识或内容片段。按[运行期观测](../../design-docs/observability.md#错误来源与日志字段)改为从 source chain 提取的固定分类字段（例如 `error_kind`、`io_error_kind`、`io_error_code`），不记录正文。

生产代码共 329 处，按 crate 分布：

| crate | 数量 |
| --- | ---: |
| crates/uc-application | 145 |
| crates/uc-infra | 122 |
| crates/uc-engine | 53 |
| compatibility/uc-mobile-lan | 6 |
| crates/uc-observability-contract | 2 |
| bindings/uc-engine-uniffi | 1 |

| 文件 | 行号 | 数量 | 备注 |
| --- | --- | ---: | --- |
| [`bindings/uc-engine-uniffi/src/runtime.rs`](../../../bindings/uc-engine-uniffi/src/runtime.rs) | 674 | 1 |  |
| [`compatibility/uc-mobile-lan/src/facade/outbound_adapter.rs`](../../../compatibility/uc-mobile-lan/src/facade/outbound_adapter.rs) | 127 | 1 |  |
| [`compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs`](../../../compatibility/uc-mobile-lan/src/usecases/apply_incoming.rs) | 508, 768, 781 | 3 |  |
| [`compatibility/uc-mobile-lan/src/usecases/get_file.rs`](../../../compatibility/uc-mobile-lan/src/usecases/get_file.rs) | 176 | 1 |  |
| [`compatibility/uc-mobile-lan/src/usecases/register_device.rs`](../../../compatibility/uc-mobile-lan/src/usecases/register_device.rs) | 293 | 1 |  |
| [`crates/uc-application/src/clipboard/active/mod.rs`](../../../crates/uc-application/src/clipboard/active/mod.rs) | 356 | 1 |  |
| [`crates/uc-application/src/clipboard/capture/usecase.rs`](../../../crates/uc-application/src/clipboard/capture/usecase.rs) | 391, 771, 882, 899, 1481 | 5 |  |
| [`crates/uc-application/src/clipboard/history/cleanup.rs`](../../../crates/uc-application/src/clipboard/history/cleanup.rs) | 177, 269, 294, 374, 382, 392, 433, 494, 568, 729, 758, 793 | 12 |  |
| [`crates/uc-application/src/clipboard/history/clear_history.rs`](../../../crates/uc-application/src/clipboard/history/clear_history.rs) | 117 | 1 |  |
| [`crates/uc-application/src/clipboard/history/delete_entry.rs`](../../../crates/uc-application/src/clipboard/history/delete_entry.rs) | 141, 186, 210 | 3 |  |
| [`crates/uc-application/src/clipboard/history/list_entry_projections.rs`](../../../crates/uc-application/src/clipboard/history/list_entry_projections.rs) | 315, 341, 374, 400, 474, 491 | 6 |  |
| [`crates/uc-application/src/clipboard/history/reconcile_missing_files.rs`](../../../crates/uc-application/src/clipboard/history/reconcile_missing_files.rs) | 173 | 1 |  |
| [`crates/uc-application/src/clipboard/history/retention_policy.rs`](../../../crates/uc-application/src/clipboard/history/retention_policy.rs) | 119 | 1 |  |
| [`crates/uc-application/src/clipboard/history/toggle_favorite.rs`](../../../crates/uc-application/src/clipboard/history/toggle_favorite.rs) | 63 | 1 |  |
| [`crates/uc-application/src/clipboard/outbound/mod.rs`](../../../crates/uc-application/src/clipboard/outbound/mod.rs) | 307, 319, 710 | 3 |  |
| [`crates/uc-application/src/clipboard/outbound/payload_prep.rs`](../../../crates/uc-application/src/clipboard/outbound/payload_prep.rs) | 117, 124 | 2 |  |
| [`crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs`](../../../crates/uc-application/src/clipboard/restore/restore_as_plain_text.rs) | 234, 332, 411 | 3 |  |
| [`crates/uc-application/src/clipboard/restore/restore_selection.rs`](../../../crates/uc-application/src/clipboard/restore/restore_selection.rs) | 175 | 1 |  |
| [`crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs`](../../../crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs) | 311, 372, 393, 459, 483, 542, 564 | 7 |  |
| [`crates/uc-application/src/clipboard/sync/active_state/fanout.rs`](../../../crates/uc-application/src/clipboard/sync/active_state/fanout.rs) | 45, 74, 81 | 3 |  |
| [`crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs`](../../../crates/uc-application/src/clipboard/sync/active_state/peer_online_resync_worker.rs) | 192, 205 | 2 |  |
| [`crates/uc-application/src/clipboard/sync/active_state/reconcile.rs`](../../../crates/uc-application/src/clipboard/sync/active_state/reconcile.rs) | 140 | 1 |  |
| [`crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs`](../../../crates/uc-application/src/clipboard/sync/active_state/restore_broadcast_worker.rs) | 134 | 1 |  |
| [`crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs`](../../../crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs) | 115, 156, 162, 182, 205, 230, 234 | 7 |  |
| [`crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs`](../../../crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs) | 393, 399, 522, 565, 917, 1195, 1509, 1897, 1901 | 9 |  |
| [`crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs`](../../../crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs) | 455, 482, 816, 837, 876, 1144, 1565 | 7 |  |
| [`crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs`](../../../crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs) | 123, 164, 242 | 3 |  |
| [`crates/uc-application/src/clipboard/sync/dispatch_entry/header.rs`](../../../crates/uc-application/src/clipboard/sync/dispatch_entry/header.rs) | 64 | 1 |  |
| [`crates/uc-application/src/clipboard/sync/dispatch_entry/mod.rs`](../../../crates/uc-application/src/clipboard/sync/dispatch_entry/mod.rs) | 516 | 1 |  |
| [`crates/uc-application/src/clipboard/sync/dispatch_entry/per_peer.rs`](../../../crates/uc-application/src/clipboard/sync/dispatch_entry/per_peer.rs) | 88, 175, 188 | 3 |  |
| [`crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs`](../../../crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs) | 146 | 1 |  |
| [`crates/uc-application/src/clipboard/sync/get_entry_delivery_view.rs`](../../../crates/uc-application/src/clipboard/sync/get_entry_delivery_view.rs) | 190, 297 | 2 |  |
| [`crates/uc-application/src/clipboard/sync/outbound_plan.rs`](../../../crates/uc-application/src/clipboard/sync/outbound_plan.rs) | 73 | 1 |  |
| [`crates/uc-application/src/clipboard/sync/send_gate.rs`](../../../crates/uc-application/src/clipboard/sync/send_gate.rs) | 90 | 1 |  |
| [`crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs`](../../../crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs) | 209, 232, 341 | 3 |  |
| [`crates/uc-application/src/clipboard/write/coordinator.rs`](../../../crates/uc-application/src/clipboard/write/coordinator.rs) | 342 | 1 |  |
| [`crates/uc-application/src/clipboard/write/mobile_consumability.rs`](../../../crates/uc-application/src/clipboard/write/mobile_consumability.rs) | 31, 59 | 2 |  |
| [`crates/uc-application/src/facade/clipboard_restore/mod.rs`](../../../crates/uc-application/src/facade/clipboard_restore/mod.rs) | 248 | 1 |  |
| [`crates/uc-application/src/search/coordinator.rs`](../../../crates/uc-application/src/search/coordinator.rs) | 292, 317, 499, 516, 549, 592, 692, 711, 725, 740, 817, 833, 838, 855, 879, 887, 897 | 17 |  |
| [`crates/uc-application/src/search/live_index/mod.rs`](../../../crates/uc-application/src/search/live_index/mod.rs) | 112, 127, 153 | 3 |  |
| [`crates/uc-application/src/settings/storage/mod.rs`](../../../crates/uc-application/src/settings/storage/mod.rs) | 59, 105, 111 | 3 |  |
| [`crates/uc-application/src/settings/upgrade/detect.rs`](../../../crates/uc-application/src/settings/upgrade/detect.rs) | 138 | 1 |  |
| [`crates/uc-application/src/space/lifecycle/unlock_space/use_case.rs`](../../../crates/uc-application/src/space/lifecycle/unlock_space/use_case.rs) | 115 | 1 |  |
| [`crates/uc-application/src/support/host_event_bus.rs`](../../../crates/uc-application/src/support/host_event_bus.rs) | 70, 95 | 2 |  |
| [`crates/uc-application/src/support/host_event_publisher.rs`](../../../crates/uc-application/src/support/host_event_publisher.rs) | 68, 84 | 2 |  |
| [`crates/uc-application/src/transfer/blob/facade.rs`](../../../crates/uc-application/src/transfer/blob/facade.rs) | 357, 362, 451, 460, 623, 631, 659, 778, 786, 822, 958 | 11 |  |
| [`crates/uc-application/src/transfer/file/lifecycle.rs`](../../../crates/uc-application/src/transfer/file/lifecycle.rs) | 211, 246, 258, 298, 425, 436, 455, 468 | 8 |  |
| [`crates/uc-engine/src/assembly/facade.rs`](../../../crates/uc-engine/src/assembly/facade.rs) | 83 | 1 |  |
| [`crates/uc-engine/src/assembly/lifecycle.rs`](../../../crates/uc-engine/src/assembly/lifecycle.rs) | 100, 111 | 2 |  |
| [`crates/uc-engine/src/assembly/network.rs`](../../../crates/uc-engine/src/assembly/network.rs) | 82, 92, 138, 153, 208 | 5 |  |
| [`crates/uc-engine/src/assembly/platform.rs`](../../../crates/uc-engine/src/assembly/platform.rs) | 153, 169, 187, 198 | 4 |  |
| [`crates/uc-engine/src/operations/history/search.rs`](../../../crates/uc-engine/src/operations/history/search.rs) | 177, 265 | 2 |  |
| [`crates/uc-engine/src/operations/settings/config_migration.rs`](../../../crates/uc-engine/src/operations/settings/config_migration.rs) | 141 | 1 |  |
| [`crates/uc-engine/src/operations/settings/diagnostics.rs`](../../../crates/uc-engine/src/operations/settings/diagnostics.rs) | 79 | 1 |  |
| [`crates/uc-engine/src/operations/settings/encryption.rs`](../../../crates/uc-engine/src/operations/settings/encryption.rs) | 14, 26, 40 | 3 |  |
| [`crates/uc-engine/src/operations/space/cancel_invitation.rs`](../../../crates/uc-engine/src/operations/space/cancel_invitation.rs) | 26 | 1 |  |
| [`crates/uc-engine/src/operations/space/create_space.rs`](../../../crates/uc-engine/src/operations/space/create_space.rs) | 58 | 1 |  |
| [`crates/uc-engine/src/operations/space/encryption_passphrase.rs`](../../../crates/uc-engine/src/operations/space/encryption_passphrase.rs) | 47, 55 | 2 |  |
| [`crates/uc-engine/src/operations/space/factory_reset.rs`](../../../crates/uc-engine/src/operations/space/factory_reset.rs) | 31 | 1 |  |
| [`crates/uc-engine/src/operations/space/invitation.rs`](../../../crates/uc-engine/src/operations/space/invitation.rs) | 126, 134 | 2 |  |
| [`crates/uc-engine/src/operations/space/join_space.rs`](../../../crates/uc-engine/src/operations/space/join_space.rs) | 92 | 1 |  |
| [`crates/uc-engine/src/operations/space/reset_space.rs`](../../../crates/uc-engine/src/operations/space/reset_space.rs) | 18 | 1 |  |
| [`crates/uc-engine/src/operations/space/session_recovery.rs`](../../../crates/uc-engine/src/operations/space/session_recovery.rs) | 58 | 1 |  |
| [`crates/uc-engine/src/operations/space/unlock.rs`](../../../crates/uc-engine/src/operations/space/unlock.rs) | 54 | 1 |  |
| [`crates/uc-engine/src/runtime/dispatch.rs`](../../../crates/uc-engine/src/runtime/dispatch.rs) | 576, 586 | 2 |  |
| [`crates/uc-engine/src/runtime/host_clipboard.rs`](../../../crates/uc-engine/src/runtime/host_clipboard.rs) | 38, 48, 53 | 3 |  |
| [`crates/uc-engine/src/runtime/host_operations.rs`](../../../crates/uc-engine/src/runtime/host_operations.rs) | 117, 169, 174, 326, 343, 351, 358, 385, 465 | 9 |  |
| [`crates/uc-engine/src/runtime/mod.rs`](../../../crates/uc-engine/src/runtime/mod.rs) | 408, 446 | 2 |  |
| [`crates/uc-engine/src/runtime/profile_recovery.rs`](../../../crates/uc-engine/src/runtime/profile_recovery.rs) | 161 | 1 |  |
| [`crates/uc-engine/src/runtime/session_supervisor.rs`](../../../crates/uc-engine/src/runtime/session_supervisor.rs) | 119, 132, 140 | 3 |  |
| [`crates/uc-engine/src/runtime/shutdown.rs`](../../../crates/uc-engine/src/runtime/shutdown.rs) | 69 | 1 |  |
| [`crates/uc-engine/src/subsystems/reconcile.rs`](../../../crates/uc-engine/src/subsystems/reconcile.rs) | 77, 145 | 2 |  |
| [`crates/uc-infra/src/blob/blob_writer.rs`](../../../crates/uc-infra/src/blob/blob_writer.rs) | 123, 156, 207 | 3 |  |
| [`crates/uc-infra/src/blob/filesystem_store.rs`](../../../crates/uc-infra/src/blob/filesystem_store.rs) | 150 | 1 |  |
| [`crates/uc-infra/src/clipboard/background_blob_worker.rs`](../../../crates/uc-infra/src/clipboard/background_blob_worker.rs) | 167, 188, 272, 312, 343, 365, 387, 422, 442 | 9 |  |
| [`crates/uc-infra/src/clipboard/background_runtime.rs`](../../../crates/uc-infra/src/clipboard/background_runtime.rs) | 173 | 1 |  |
| [`crates/uc-infra/src/clipboard/durable_spool_queue.rs`](../../../crates/uc-infra/src/clipboard/durable_spool_queue.rs) | 75 | 1 |  |
| [`crates/uc-infra/src/clipboard/payload_resolver.rs`](../../../crates/uc-infra/src/clipboard/payload_resolver.rs) | 64, 86, 127, 167 | 4 |  |
| [`crates/uc-infra/src/clipboard/spool_janitor.rs`](../../../crates/uc-infra/src/clipboard/spool_janitor.rs) | 85, 96 | 2 |  |
| [`crates/uc-infra/src/clipboard/spool_manager.rs`](../../../crates/uc-infra/src/clipboard/spool_manager.rs) | 182, 190, 251, 432 | 4 |  |
| [`crates/uc-infra/src/clipboard/spool_scanner.rs`](../../../crates/uc-infra/src/clipboard/spool_scanner.rs) | 80, 91, 106 | 3 |  |
| [`crates/uc-infra/src/clipboard/staged_reconciler.rs`](../../../crates/uc-infra/src/clipboard/staged_reconciler.rs) | 88, 128 | 2 |  |
| [`crates/uc-infra/src/config_migration/staging.rs`](../../../crates/uc-infra/src/config_migration/staging.rs) | 281 | 1 |  |
| [`crates/uc-infra/src/db/pool.rs`](../../../crates/uc-infra/src/db/pool.rs) | 102, 109, 116 | 3 |  |
| [`crates/uc-infra/src/db/repositories/active_clipboard_register_repo.rs`](../../../crates/uc-infra/src/db/repositories/active_clipboard_register_repo.rs) | 263, 290 | 2 |  |
| [`crates/uc-infra/src/fs/atomic_publish.rs`](../../../crates/uc-infra/src/fs/atomic_publish.rs) | 69, 103, 127 | 3 |  |
| [`crates/uc-infra/src/fs/inbound_target.rs`](../../../crates/uc-infra/src/fs/inbound_target.rs) | 40, 88, 151 | 3 |  |
| [`crates/uc-infra/src/mobile_sync/file_staging.rs`](../../../crates/uc-infra/src/mobile_sync/file_staging.rs) | 138, 276, 438, 453 | 4 |  |
| [`crates/uc-infra/src/network/iroh/active_clipboard/dispatch_adapter.rs`](../../../crates/uc-infra/src/network/iroh/active_clipboard/dispatch_adapter.rs) | 105 | 1 |  |
| [`crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs`](../../../crates/uc-infra/src/network/iroh/active_clipboard/pull_client_adapter.rs) | 128 | 1 |  |
| [`crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs`](../../../crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs) | 138, 153, 209, 217 | 4 |  |
| [`crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs`](../../../crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs) | 148, 162 | 2 |  |
| [`crates/uc-infra/src/network/iroh/addr_filter.rs`](../../../crates/uc-infra/src/network/iroh/addr_filter.rs) | 136 | 1 |  |
| [`crates/uc-infra/src/network/iroh/blobs.rs`](../../../crates/uc-infra/src/network/iroh/blobs.rs) | 428, 533, 584, 700 | 4 |  |
| [`crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs`](../../../crates/uc-infra/src/network/iroh/clipboard_dispatch_adapter.rs) | 160, 329 | 2 |  |
| [`crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs`](../../../crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs) | 160, 190, 227, 315, 319 | 5 |  |
| [`crates/uc-infra/src/network/iroh/net_recovery.rs`](../../../crates/uc-infra/src/network/iroh/net_recovery.rs) | 203, 243 | 2 |  |
| [`crates/uc-infra/src/network/iroh/node.rs`](../../../crates/uc-infra/src/network/iroh/node.rs) | 1533 | 1 |  |
| [`crates/uc-infra/src/network/iroh/node/shutdown.rs`](../../../crates/uc-infra/src/network/iroh/node/shutdown.rs) | 73 | 1 |  |
| [`crates/uc-infra/src/network/iroh/relay_probe.rs`](../../../crates/uc-infra/src/network/iroh/relay_probe.rs) | 223, 258 | 2 |  |
| [`crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs`](../../../crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs) | 190, 219, 274 | 3 |  |
| [`crates/uc-infra/src/search/rows.rs`](../../../crates/uc-infra/src/search/rows.rs) | 168 | 1 |  |
| [`crates/uc-infra/src/search/sqlite_index.rs`](../../../crates/uc-infra/src/search/sqlite_index.rs) | 941, 1013, 1202, 1205, 1208, 1494, 1526, 2019, 2192, 2196 | 10 |  |
| [`crates/uc-infra/src/space/security/access.rs`](../../../crates/uc-infra/src/space/security/access.rs) | 453, 464, 475, 1766, 1769, 1774, 1777, 1835, 1841, 1843, 1846, 1859, 1862, 1885, 1896, 1923, 1934, 1988, 1996, 2003, 2021, 2051, 2063, 2084, 2094, 2109, 2116, 2140, 2172, 2215, 2221, 2223, 2226, 2260, 2276, 2284 | 36 |  |
| [`crates/uc-observability-contract/src/analytics/facade.rs`](../../../crates/uc-observability-contract/src/analytics/facade.rs) | 175, 202 | 2 |  |

## 测试代码计数

测试代码不列入修改点。测试中的断言若依赖错误显示文本，在对应生产切片修改时一并改为按类型或 source 断言。

| 类别 / 位置 | 数量 |
| --- | ---: |
| S4 crates/uc-application | 44 |
| S4 tests/hosts/uc-mobile-probe-core | 21 |
| S3 crates/uc-application | 10 |
| S4 tests/hosts/connectivity | 10 |
| S4 compatibility/uc-mobile-lan | 4 |
| S4 crates/uc-infra | 4 |
| S1 crates/uc-infra | 3 |
| S4 bindings/uc-engine-uniffi | 3 |
| S4 tests/uc-testkit/src | 3 |
| L1 crates/uc-application | 1 |
| S2 crates/uc-infra | 1 |
| S3 crates/uc-infra | 1 |
| S3 tests/hosts/uc-mobile-probe-core | 1 |
| S4 crates/uc-engine | 1 |
| S4 tests/openmls-validation/tests | 1 |

