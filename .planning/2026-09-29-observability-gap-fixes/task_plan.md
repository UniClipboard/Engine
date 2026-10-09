# 任务计划：关键日志缺口修复

来源：`findings.md`（55 条，编号沿用）。只补“定位真实现场故障所需”的记录，不追求覆盖率。

## 硬约束（每个切片都要满足）
- 不新增 Engine 对 Application/Core 内部阶段、状态、标识的暴露；Engine 只装饰既有完整能力。
- 不含剪贴板内容、密钥、令牌、设备名、地址、文件名、路径；字段必须登记在字段目录（`uc_observability_contract::log_fields`），
  错误只走 `error_kind` / `io_error_kind` / 已登记 `log_safe_errors!` 类型，不用 `%e` `?e`。
- 状态迁移只在提交迁移的负责人处记一次；已返回给上层的错误不重复记。
- 对端可触发的失败用 debug 或限速 warn；“终止整个流程”的丢弃不得只写 debug。
- 诊断日志是永久日志，不做临时调试输出。

## 顺序（用户 2026-09-29 决定）
ADR-030 的第 1 步（类型化 `uc_*!` 宏、字段目录、零容忍检查）已于 2026-09-29 一次性完成；其第 3 步（错误分类）与 `DiagnosticTaskKind` 声明生成未做。本任务的补日志现在可以开始，一律用 `uc_*!` 宏编写。
因此本计划的所有阶段暂缓，P-A 中“新增 DiagnosticTaskKind / 登记类型”改按 ADR 第 2、3 步的声明与分类方式书写，
不再沿用 `log_safe_errors!` 逐层登记。ADR 采纳前，findings 仍是有效的缺口清单。

## 前置决定（需用户确认）
1. 本任务依赖 `2026-09-29-module-log-channel`（登记宏、白名单、模块日志层）。
   建议：先提交该任务的 P0，再开始 P-C 起的切片；P-A 可并行。
2. 语言：延续本目录既有中文规划文件。

## 阶段
- [x] **P-A 契约与登记（一次性，先于一切）**
  - 新增 `DiagnosticTaskKind`：MembershipMaintenanceRound、ActiveClipboardWorkerGroup、
    OutboundProgressTranslator、EngineStartup、MobileWorker；同步 `as_str`、schema 快照、隐私测试。
  - `uc-engine` 再导出 `record_task_join_failure`，绑定只依赖 `uc-engine`（#14）。
  - 登记 `log_safe_errors!`：SpaceActivityError、PeerConnectionError、KeyEpochError、MlsGroupError、
    ConfigMigrationError、SearchShutdownError、LifecycleError、Relay*Error、ClipboardHistoryError。
  - 白名单补 `recovery_state`、`stage`、`phase`、`rollback_target` 等确需的字段；
    `skip_reason`/`from_phase`/`to_phase` 改用 `reason`/`previous_phase`/`next_phase`。
- [x] **P-B 错误转换修正（无新日志）**：#52 `unavailable_from`、#16/#17 `Unexpected`/`StableFailure`
  携带固定分类变体。先做，后续日志才有可用分类。
- [x] **P-C P0 吞错点，按 crate 分批**（每批一个提交，Cargo 验证串行）
  1. Application space：#1 #2 #3 #4 #5
  2. Application clipboard/transfer/search：#6 #7 #8 #9（#9 “截断后是否置 READY”另列行为决策，不混入）
  3. Infra：#10 #12 #13
  4. Engine + 绑定：#11 #14 #15
- [x] **P-D P1**：Application #16-#27；Engine #28-#35；Infra #36-#43；Observability/绑定 #44 #45。
  #44 需要合同新增 health 事件，单独评估，可能顺延。
- [x] **P-E P2**：#46-#55，多为一行 `error_kind`/`io_error_kind` 补充。
- [x] **P-F 收尾**：更新 `docs/design-docs/observability.md`（任务退出记录清单、新字段），
  跑交付前检查；需要负责人提供不透明观测上下文的 5 处（member.rs:610、cancel_join_space、
  config_migration adapter、reconciliation 恢复关联、recovery 激活关联）逐个走设计确认后再改。

## 每批验证
- 先写失败测试：日志捕获断言 `error_kind` 出现且不含敏感字段（复用模块日志隐私测试）。
- `cargo check --workspace --all-targets --locked`；`-p uc-infra --features lan-compat`。
- `node scripts/architecture/check-rust-style.mjs`、`check-engine-repository.mjs`、`cargo fmt --check`、`git diff --check`。
- Cargo 由单一负责人使用共享 `target` 串行执行。

## 明确不做
取消/正常退出 info、返回给调用方的错误重复记录、`RemoteOrigin` 等正常过滤、不可达的 `spawn` 布尔结果、
`fingerprint` 字段、`degraded_reason` 泄露问题（另开跟进）。

## 状态（2026-09-30）
全部阶段完成；55 条缺口均已实施。P-A、P-B 的登记项按 ADR-030 第 2、3 步改以字段目录与 `ErrorClass` 实现。
需要不透明观测上下文的 5 处经设计确认均无需新增类型（完成记录归流程负责人，Engine 只映射；恢复类作独立诊断）。
仍无专门日志测试的项见 progress.md 末尾。
