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
