# Progress

- 2026-09-29：workflow `wf_5470612e-e14` 完成扫描与核验，findings.md 落盘；task_plan.md 待用户确认后开工。
- 2026-09-29：用户决定先做 ADR-030 再补日志；clippy 试验完成，结论已写入 ADR。
- 2026-09-29：ADR-030 第 1 步一次性完成：936 处直接 tracing 调用点迁移到 `uc_*!`（198 个文件），零容忍检查 `check-direct-log-macros.mjs`；
  运行期白名单直接由字段目录得出，旧过渡清单与旧字段审定逻辑已删除。分类表：`field-classification.md`。
  验证：`cargo check --workspace --all-targets --locked`（默认与 `uc-engine/lan-compat`）、`-p uc-engine --features dev-tools,lan-compat`、
  `-p uc-infra --features lan-compat,test-util`、fmt、两个架构脚本与脚本测试、`cargo test --workspace`。
  测试中未通过的项均已在基线 `1f525b83` 上复现，非本次引入：`uc-upgrade-matrix` 全部 68 项（环境缺锚点宿主）、
  `interrupted_file_transfer_recovers_after_receiver_process_restart`（基线 3 次失败 1 次）、
  `engine_shutdown_removes_unfinished_mobile_upload_files`（lan-compat，基线 8 次失败 2 次）。
- 下一步：补日志（findings.md 55 条）直接使用 `uc_*!` 宏；ADR 第 3 步（错误分类）与 `DiagnosticTaskKind` 声明生成另行处理。
- 2026-09-29：补日志 P0 的 15 条全部完成（#1-#15，每条先写失败的日志捕获测试再实现）。新增任务类别
  `membership_maintenance_round`、`active_clipboard_worker`、`mobile_worker`；共享日志捕获辅助放在 `uc-testkit::log_capture`。
  验证：`cargo test --workspace` 3981 通过，失败仅为基线已有的 upgrade-matrix 与崩溃恢复偶发；直接日志宏检查为 0。
  下一步：P1（#16-#45）与 P2（#46-#55）。
- 2026-09-30：P1 完成 #16-#43、#45（#44 需要改观测合同，暂缓）。新增任务类别 `engine_startup`、`outbound_progress_translator`；
  新字段 `rollback_target`、`recovery_state`、`restart_required`、`can_submit_passphrase`、`evicted_count`、`max_bytes`。
  无专门日志测试的项：#25、#29、#30、#32、#37（需要真实 Engine 多线程运行或真实 iroh 连接）。下一步：P2（#46-#55）。

## P2 (#46-#55)

All ten items implemented and verified (fmt, workspace check default and lan-compat, uc-application/uc-infra/uc-engine tests, direct-log 0, style, repo preflight, diff check).

- Tested: #46 (settings failure source), #55 (shared `remove_work_directory_best_effort`, no path in logs).
- Compile and adjacent tests only: #47, #48, #50, #51, #53, #54 (need real session, iroh connection or vault fixtures); #49, #52 are trivial refactors.
- Deferred: #44 (needs a contract change: setup_degraded health event and `ObservabilityHealth` reason).

## #44 (done)

Remote exporter build failure now yields `RemoteSetupFailure` (http_client/trace_exporter/log_exporter) in `ObservabilityHealth.remote_setup_failure`, exposed by both bindings, plus one `uc.observability.setup_degraded` health event after the subscriber is installed. Tested via an injectable builder and a real local sink; the install-time emit call is not separately tested (process-global).

## 2026-09-30 ADR-030 steps 2/3

- Replaced `log_safe_errors!` registration with the fixed `uc_core::error_class::ErrorClass` trait; new catalog fields `error_class` and `source_class`; removed the registry, `warn_on_error!`, both `register_log_safe_errors()` functions and `check-module-log-errors.mjs`.
- Module log error chains now render only `io::Error` and `serde_json::Error` layers; per-layer character truncation was removed as unreachable, and the long-chain test now asserts the depth cap.
- Verification: fmt, workspace check (default and lan-compat), core/contract/runtime/application/infra tests, style, direct-log and engine repository checks all pass.
- Known pre-existing flake: `interrupted_file_transfer_recovers_after_receiver_process_restart` in uc-engine times out about half the time on the unmodified HEAD (4dda7e6 baseline: 2 of 4 runs failed); unrelated to this change.

## 2026-09-30 #[instrument] fields

- `check-rust-style.mjs` now requires `#[instrument(fields(..))]` names to be registered in the log field catalog and rejects `err`/`ret`; unit tests added. The check runs on added lines only, so a full-repo sweep was done with `--file`; only two extra sites needed changes.
- Removed span fields carrying addresses, device ids, profile ids, relay URLs and content-derived hashes (`selected_ip`, `device`, `target`, `target_device_id`, `profile`, `relay`, `snapshot_hash`).
- Registered scalar counters and the closed vocabularies `operator` and `ack`; added them to `NEWLY_VISIBLE`.
- Fixed `module_log_channel` again: record-size truncation is now exercised with an oversized record of many bounded fields, because opaque error layers can no longer reach the limit.

## 2026-09-30 opaque-context sites resolved by design

- Decision (confirmed by the user): none of the five sites needs a new opaque observation context. Completion records belong to the flow owner; Engine only maps error codes; restart recovery stays an independent diagnostic per the correlation standard.
- Remove member: `RemoveSpaceMemberUseCase::execute` now writes one completion record (`operation=remove_member`, `outcome=completed|rejected|failed`, `error_class`); the Engine boundary log was removed and its test now asserts no second record.
- Cancel join: `SpaceAdmissionProtocol::cancel_join` writes the outcome (`requested|completed|failed`); Engine mapping no longer logs; a missing join stays silent.
- Config migration keeps its port-boundary failure record; reconciliation and session recovery keep their existing independent records.
