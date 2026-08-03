# Phase 1 行为保护矩阵

## 状态说明

- `稳定入口已证明`：已通过稳定 Engine operation、结果、事件或关闭行为验证。
- `规则已保护，待迁移`：现有测试能保护业务规则，但位于后续会被替换的内部边界；对应负责人阶段必须迁到最终 Interface 后才能删除旧测试。
- `最终边界已证明`：稳定 Engine 行为与最终负责人边界共同覆盖完整结果。
- `待最终负责人补齐`：当前稳定入口无法安全注入该故障，不新增测试后门；对应负责人阶段未补齐前不得完成。

## 剪贴板入站

| 行为 | 当前证据 | 状态 | 后续门槛 |
|---|---|---|---|
| 首次接收成功并落入历史 | `engine_clipboard_inbound_preserves_success_duplicate_and_shutdown_behavior` | 稳定入口已证明 | Phase 4 保持通过 |
| 重复内容不生成第二条历史 | 同上；sender 重发结果为 duplicate，receiver 历史仍为一条 | 稳定入口已证明 | Phase 4 保持通过 |
| 解码失败 | `decode_failed_on_truncated_envelope` 验证真实内容规则；`runtime_settles_every_apply_result_and_emits_from_that_result` 验证最终 rejected receipt 和宿主结果 | 最终边界已证明 | 后续阶段保持通过 |
| 文件失败与部分成功 | `partial_materialize_persists_entry_but_skips_os_write`、`file_cache_blob_materializer_removes_reserved_placeholder_on_fetch_error` 验证清理和部分成功；最终运行期统一按应用结果产生事件和 receipt | 最终边界已证明 | 文件传输会话迁移时保持清理和唯一终态 |
| 普通接收与只保存拉取差异 | 普通接收由稳定双端场景覆盖；`store_only_pull_persists_without_a_clipboard_write_dependency` 从最终入口证明新内容会保存并进入搜索，且构造参数中不存在系统剪贴板写入能力 | 稳定入口已证明 | Phase 4 保持普通接收行为；活动剪贴板收敛继续负责拉取后的系统剪贴板写入 |
| 接收总开关、成员缺失和成员读取失败 | `receive_disabled_rejects_before_decrypt_or_apply`、`unavailable_member_preferences_reject_before_decrypt_or_apply` | 最终边界已证明 | 后续阶段保持先拒绝、后续步骤不执行 |
| 内容类型关闭 | `disabled_text_category_rejects_after_decrypt_and_before_apply` | 最终边界已证明 | 后续阶段保持解密后拒绝且不应用 |
| 解密失败后继续接收 | `decrypt_failure_rejects_one_inbound_and_continues_with_the_next` | 最终边界已证明 | 后续阶段保持单条失败不终止运行期 |
| 关闭等待入站任务退出 | 稳定双端场景中双方 `shutdown` 均在期限内完成；`shutdown_waits_for_the_active_inbound_to_reach_a_receipt` 证明等待在途应用；`shutdown_does_not_start_an_inbound_that_is_still_queued` 证明取消后不再启动排队内容 | 最终边界已证明 | 后续阶段保持通过 |

## 文件传输与移动上传

| 行为 | 当前证据 | 状态 | 后续门槛 |
|---|---|---|---|
| 文件传输开始、进度、完成 | `beginning_receiver_transfer_records_context_and_started_event`、`progress_is_monotonic_inside_one_session`、`repeating_same_terminal_call_is_idempotent` | 最终边界已证明 | 后续阶段保持通过 |
| 文件传输只有一个终态 | `concurrent_terminal_calls_append_only_one_terminal_event`、`closing_facade_cancels_every_active_session_and_rejects_new_sessions` | 最终边界已证明 | 后续阶段保持通过 |
| 移动上传开始、追加、完成、取消 | 稳定 Engine 两项场景保持通过；负责人成功、并发完成和重复取消测试覆盖内部完整结果 | 最终边界已证明 | 后续阶段保持四动作兼容 |
| 移动上传进度失败 | `engine_mobile_upload_progress_failure_cleans_up_and_invalidates_handle` 迁移后保持错误码、清理和句柄失效 | 最终边界已证明 | 后续阶段保持通过 |
| 移动上传关闭清理 | 真实暂存区清理场景保持通过；`close_waits_for_an_inflight_append_before_cancelling_the_upload` 证明等待在途操作 | 最终边界已证明 | 后续阶段保持通过 |
| 移动上传 append I/O 失败 | `append_failure_aborts_staging_marks_failed_and_invalidates_the_handle` 从最终负责人入口注入可控写盘失败 | 最终边界已证明 | 后续阶段保持统一清理 |

## 历史维护

| 行为 | 当前证据 | 状态 | 后续门槛 |
|---|---|---|---|
| 固定执行顺序 | `later_pass_failure_does_not_change_the_fixed_order` | 规则已保护，待迁移 | Phase 7 迁到 `HistoryMaintenanceRuntime` Interface |
| reconcile 失败跳过剩余步骤 | `reconciliation_failure_stops_delete_capable_passes` | 规则已保护，待迁移 | Phase 7 从最终 Interface 保持行为 |
| cleanup 失败仍执行 retention | `later_pass_failure_does_not_change_the_fixed_order` | 规则已保护，待迁移 | Phase 7 从最终 Interface 保持行为 |
| 关闭立即唤醒并等待退出 | 当前引擎任务注册表提供通用关闭，未从历史负责人观察 | 待最终负责人补齐 | Phase 7 用暂停时间验证 `HistoryMaintenanceRuntime::shutdown` 不等待五分钟定时器 |

## Phase 1 结论

Phase 1 仍为进行中。剪贴板入站、文件传输和移动上传已经从稳定 Engine 与最终负责人边界完整保护；仅历史维护的最终负责人和关闭立即唤醒仍绑定 Phase 7，未记为通过。
