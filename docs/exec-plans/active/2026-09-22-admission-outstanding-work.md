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

- `Terminal.RecoveryRequired` 记录的恢复动作为无，且终态不计入 `missing_deadline`，因此不会经恢复索引把空间工作模式切到 `NeedsAttention`。用户可见的加入状态另由 `display.rs` 的当前加入投影用 `needs_attention()` 上报，不会静默；待确认的是维护侧是否也应停下。
- 邀请方未到期的配对不计入 `pairing_in_progress`：已修复，见下节。

## 统一收尾期限类型（已实现）

Core 新增 [`membership/settlement_window.rs`](../../../crates/uc-core/src/membership/settlement_window.rs)：

- `SettlementWindow::until(deadline_ms)`：收尾沿用既有截止时间，不另开窗口；Joiner 放弃通知使用。
- `SettlementWindow::from_stored_start(duration_ms, started_at_ms)`：非正起点表示尚未起算，首次处理时 `started(now)` 保存起点；已非当前成员对端的受限通知使用。
- `state(now)` 给出 `Unstarted`/`Open { deadline_ms }`/`Expired`，到期一律走“本机结束该项责任”；起点溢出按已到期处理。

配对尝试契约 `AdmissionAttemptTimeline` 保持独立：它是双方协商、固定五分钟的协议边界，不是本机可自定的收尾期限，合并会让协议契约看起来可调。

## 角色包装层去重（已实现）

`capability.rs` 的体量主要来自按角色收窄的转发方法，这是接口设计本身，不是重复判断。实际重复只有两处，已消除：

- `JoinerAdmission::needs_attention` 与 `supersede` 各自列举“无期限且无摘要的后期加入”，现由 `is_unbounded_late_join()` 统一表达；两者不会再各自漂移。
- `JoinerAdmission::try_from_record` 与 `SponsorAdmission::try_from_record` 各自列举本角色状态，现改为 `record_role()` 判断，并以测试锁定两种角色互不接受。

未引入额外的“阶段枚举”：阶段类问题已由 `AdmissionOutstandingWork` 回答，再加一层会产生第二套分类。

## 邀请方进行中配对的运行资格（已修复）

规格把 `Pairing` 定义为“至少一个本机未终结的有效准入义务”，但恢复索引只在记录到期时才取出记录体，
`pairing_in_progress` 又按取出的记录判断，于是未到期的邀请方配对不占用运行资格：

- `Sponsor::Applied`（已回 `Complete`、等待最终确认）因带确认摘要而被计入，正式提交前的关键窗口原本就有保护。
- `Accepted`、`Candidate`、`Committed` 三档不被计入。这段时间里普通维护可以取得运行资格，并在整轮维护期间持有协议锁，
  推迟邀请方的下一步回复。影响是配对变慢与额外重试，不破坏正确性；加入方一侧没有该问题，其记录始终被加载。

修复：恢复索引把“未到期的邀请方义务”作为独立事实上报（`sponsor_pairing_open`），不加载记录体，因此不额外解密。
`SponsorConfirmation` 与 `SponsorDeadline` 两类未到期动作都计入，行为与加入方一侧对齐。

## 持久资源生命周期：收件人已失去资格的设备组更新

### 问题

一条设备组密钥更新滞留在持久队列：收件人是早先加入、现已被移除的设备。它跨重启存活、无限退避重试，
使空间设备更新状态永远到不了“已完成”。

`due_space_group_updates` 取待投递项前只结清收件人是本机的冗余项，没有任何地方结清收件人已非成员的项。
该结清写在 Infra，判据（谁是当前生效成员）却属于成员历史，位置本身就是错的。

### 资源分类

每个持久资源归入三类之一，判据必须来自已提交的成员历史，不得用投影、对端核对记录或“连不上”推断：

1. **理由可证明消失 → 立即结清**，不设期限。收件人不在当前生效成员中的设备组更新属于此类：
   它已无权持有新密钥，投递出去反而扩大暴露面，连有界窗口都不该给。
2. **已失去资格但告知仍有价值 → 有界窗口**，默认五分钟。目前只有发给已非成员设备的受限历史通知，
   已由 `SettlementWindow` 实现：被移除设备需要知道自己被移除，值得等它上线。
3. **收件人仍有资格 → 不设期限**。界面表达（说明在等哪台设备）属于 desktop 端，本仓不改。

兜底：判据无法计算时归入“需要处理”，不得静默删除。
例外：放弃通知共用配对尝试的五分钟，保持现状。

### 完整负责人与动作

`DeliverPendingGroupUpdatesUseCase` 是待投递 Group Epoch 的唯一完整负责人，结清归它：

- 判据统一到 `VerifiedMembershipLedger::is_removed_device()`，受限投递与本次结清共用同一判定点，
  不再各自展开 `effective_member_for_device`。
- Application 从账本导出**保留名单**（当前生效成员中除本机以外的设备），经
  `GroupRevocationPort::settle_obsolete_space_group_updates()` 下传；Infra 只做队列读写与名单匹配。
  本机冗余项因不在名单中被同一条规则结清，原 `settle_redundant_local_group_updates` 一并移除，
  不保留并列特例。
- 结清不受退避影响：保留名单针对整个队列生效，不只针对本轮到期项，否则那条已退避到三十分钟的
  滞留项要等到下次到期才被清掉。
- 结清覆盖**全部**待投递来源。投递索引有两个来源：空间资料里的 `pending_group_updates`，以及
  撤销暂存区的 outbox。移除设备走撤销流程，其投递全部来自 outbox；只结清空间资料会让这类投递
  完全绕过规则。撤销侧由 Core `RevocationStage::settle_obsolete_recipients()` 丢弃收件人已失去资格的
  未确认消息，与“永久失联设备”共用同一收尾语义；仓储 `settle_obsolete_revocation_recipients()`
  在同一事务内推进记录与暂存区，剩余消息全部确认时撤销完成，与逐个确认收件人走同一条完成路径。
  分发阶段的记录不能用 `stage_revocation` 写回：它只接受 `prepared` 状态。

首版实现只覆盖了空间资料一侧，真机上移除全部设备后状态仍停在“正在更新空间设备状态”。
回归测试 `revocation_outbox_for_a_later_removed_recipient_is_settled` 在修正前结清数为 0，
复现了该现象；`settling_distributing_revocation_recipients_completes_it_durably` 在真实 SQLite
仓储上锁定落库、跨重启与幂等，内存 mock 不校验状态条件，不能单独作为持久化证据。

### 成功、失败与重启

- 成功：队列中收件人不在保留名单的项被移除，本轮维护继续投递其余项。
- 账本缺失或成员历史无法验证：**不下传任何名单**，保持队列原样并推迟本轮，绝不按空名单清空。
- 重启：结清只改持久队列，无额外状态；下一轮维护重新计算名单，重复执行安全。

`acknowledge_group_update` 只是把项移出队列，不写确认字段，持久结构不区分“投递成功”与“无需投递”，
因此本次不涉及持久格式版本变更。

### 队列变化的刷新事件

队列写入不经过成员账本，而 `DeviceTrustChanged` 只由账本提交触发，因此清理完界面仍要等轮询。
维护轮次确实改变了设备更新状态时，Application 发 `MembershipHostEvent::SpaceDeviceUpdateChanged`，
Engine 映射为 `RefreshRequired { StateInvalidated }`。事件只表示“重新读快照”，
不携带阶段、状态对象或业务标识，也不为此扩大 facade 或结果接口。

### 验收

- 收件人已非生效成员的队列项在一轮维护后消失，且从未被投递。
- 收件人仍是生效成员的队列项不受影响，离线也继续保留。
- 成员历史不可用时队列保持原样，本轮推迟。
- 结清发生时宿主收到一次刷新事件。

## 阶段 B：跨记录的配对尾部（后续）

移除通知（`peer_reconciliation.restricted_delivery`）、成员效果与设备组密钥投递各有持久状态，不属于配对记录。阶段 B 在统一收尾期限类型的基础上，再由 Application 汇总为只读的“空间收尾工作”查询，供展示与维护共用。阶段 A 不改变这些状态。

## 验收

- 等价性测试覆盖所有记录状态夹具，差异已逐条确认。
- `holds_pairing_open`、`has_unsettled_admission_work`、恢复索引不再各自 `match` 记录状态。
- 恢复索引持久格式不变，既有索引记录无需迁移即可读取。
- 交付前检查全部通过。
