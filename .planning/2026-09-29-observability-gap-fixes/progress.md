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
