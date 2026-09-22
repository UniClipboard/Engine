# 配对收尾唯一判定点

## 状态与完整责任

- **状态**：阶段 A 与统一收尾期限类型已实现并通过自动验证；阶段 B 待设计。
- **日期**：2026-09-22。
- **依据**：t-0010 真机问题中，“配对是否仍在进行”“是否还欠收尾”“是否阻止新配对”分别由多处独立推算，漏分支导致 Android 长期停在 `Pairing`、桌面持续显示更新中；见[配对生命周期与终态收尾](../../design-docs/pairing-lifecycle.md)。
- **完整负责人**：Core 的配对记录规则给出一条记录尚欠的全部收尾工作及各自截止时间；Application 的 `SpaceAdmissionProtocol` 按该结论推进恢复、判定工作模式；Infra 只持久化与索引该结论。
- **调用方唯一动作**：读取 `aggregate.outstanding_work()`。调用方不再直接组合 `is_terminal`、`pending_exchange`、`cleanup_obligation`、`has_pending_*`、`has_expirable_*` 推算阶段。
- **成功结果**：同一记录的“是否保持配对打开”“是否阻止新准入”“下一步恢复动作”“下一次截止时间”全部由同一结论派生；新增终态或义务时，编译器在唯一的穷尽 `match` 处提示。
- **失败结果**：无法安全判定的记录归入 `RecoveryRequired` 义务，仍阻止新准入并进入 `NeedsAttention`；不猜测提交状态、不补造期限。
- **重启与重试责任**：结论完全由已持久化的加密配对记录推导，不新增持久事实；恢复索引格式保持 `RECOVERY_SUMMARY_FORMAT_V3` 不变，只改为从结论映射。

## 现状：同一记录的四套推算

| 推算 | 位置 | 使用者 |
| --- | --- | --- |
| `has_unsettled_admission_work` | Core `state/capability.rs` | Infra 邀请方状态存储，阻止新准入 |
| `holds_pairing_open` | Core `state/capability.rs` | Application `recover_pending` 工作模式 |
| 恢复动作分类 `RecoveryAction` | Infra `repository/recovery_index.rs` | 恢复索引，决定加载哪些记录、何时到期 |
| `pending_recovery`、`has_pending_local_termination`、`has_pending_sponsor_abandonment`、`has_expirable_*` | Core `state/view.rs` | 以上三者及 Application 恢复流程 |

恢复动作分类属于业务规则，却位于 Infra，违反“Core 保存规则”。四处各有自己的 `match`，同一终态在不同地方可能被归为不同结论。

## 阶段 A：单条配对记录的唯一结论（本次实施）

Core 为每条记录给出 `AdmissionOutstandingWork`：

- 义务列表 `AdmissionObligation`，按执行先后排列：`ProtocolInFlight`、`SettlementPending`、`LocalSpaceIsolation`、`AbandonmentNotice`、`SponsorConfirmation`、`SponsorRevocation`、`RecoveryRequired`。
- 每种义务只在一处声明两条派生规则：`holds_pairing_open()`（后台收尾 `AbandonmentNotice`、`SponsorRevocation` 不算配对仍在进行）与 `blocks_new_admission()`。
- `next_step()`（`AdmissionRecoveryStep`）、`deadline_ms()`、`missing_deadline()`：恢复索引唯一来源。

迁移步骤：

1. Core 实现 `outstanding_work()`，以穷尽 `match` 覆盖全部 `SpaceAdmissionRecordState`；先用等价性测试证明它与现有四套推算在全部状态夹具上结论一致，差异逐条列出并由负责人确认。
2. `has_unsettled_admission_work`、`holds_pairing_open` 改为基于结论实现，随后删除旧实现。
3. Infra 恢复索引改为映射 `next_step()`、`deadline_ms()` 与 `missing_deadline()`，删除本地分类逻辑。
4. Application 恢复流程改为按义务分派；删除只为推算阶段存在的 `has_pending_*`/`has_expirable_*` 公开查询。
5. 更新[配对生命周期](../../design-docs/pairing-lifecycle.md)的“时钟、恢复与展示的边界”。

## 阶段 A 实施结果

- Core 新增 [`state/outstanding.rs`](../../../crates/uc-core/src/membership/space_admission/state/outstanding.rs)：`AdmissionOutstandingWork` 以义务列表表示一条记录尚欠的全部工作（一条终止记录可同时欠“隔离目标空间”和“放弃通知”，单一状态无法表达），并给出 `next_step`、`deadline_ms`、`missing_deadline`。
- `has_unsettled_admission_work`、`JoinerAdmission::holds_pairing_open` 已删除，调用方改为 `outstanding_work().blocks_new_admission()` 与 `outstanding_work().holds_pairing_open()`。
- Infra 恢复索引删除本地分类逻辑，只把 `next_step` 映射为既有持久枚举；`RECOVERY_SUMMARY_FORMAT_V3` 不变。今后调整 `next_step` 规则必须升级该缓存格式，否则旧摘要不会重建。
- `has_expirable_*`、`has_pending_local_termination`、`has_pending_sponsor_abandonment` 收窄为 Core 内部可见。
- 等价性核对：迁移前在 `AdmissionTransition::new` 临时断言新旧结论一致，uc-core、uc-application、uc-infra、uc-engine 全量测试零不一致，随后删除临时代码。

迁移中发现、未在阶段 A 改变的现有缺口：

- `Terminal.RecoveryRequired` 记录的恢复动作为无，且终态不计入 `missing_deadline`，因此不会经恢复索引触发 `NeedsAttention`；需要单独确认是否由其他入口上报。
- 邀请方未到期的 `SponsorDeadline` 记录不计入 `pairing_in_progress`（索引只加载已到期记录），工作模式对进行中的邀请方配对依赖协议处理入口而非恢复索引。

## 统一收尾期限类型（已实现）

Core 新增 [`membership/settlement_window.rs`](../../../crates/uc-core/src/membership/settlement_window.rs)：

- `SettlementWindow::until(deadline_ms)`：收尾沿用既有截止时间，不另开窗口；Joiner 放弃通知使用。
- `SettlementWindow::from_stored_start(duration_ms, started_at_ms)`：非正起点表示尚未起算，首次处理时 `started(now)` 保存起点；已非当前成员对端的受限通知使用。
- `state(now)` 给出 `Unstarted`/`Open { deadline_ms }`/`Expired`，到期一律走“本机结束该项责任”；起点溢出按已到期处理。

配对尝试契约 `AdmissionAttemptTimeline` 保持独立：它是双方协商、固定五分钟的协议边界，不是本机可自定的收尾期限，合并会让协议契约看起来可调。

## 阶段 B：跨记录的配对尾部（后续）

移除通知（`peer_reconciliation.restricted_delivery`）、成员效果与设备组密钥投递各有持久状态，不属于配对记录。阶段 B 在统一收尾期限类型的基础上，再由 Application 汇总为只读的“空间收尾工作”查询，供展示与维护共用。阶段 A 不改变这些状态。

## 验收

- 等价性测试覆盖所有记录状态夹具，差异已逐条确认。
- `holds_pairing_open`、`has_unsettled_admission_work`、恢复索引不再各自 `match` 记录状态。
- 恢复索引持久格式不变，既有索引记录无需迁移即可读取。
- 交付前检查全部通过。
