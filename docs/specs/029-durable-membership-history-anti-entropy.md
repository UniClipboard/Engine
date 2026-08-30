# 规格 029：持久化成员历史反熵

## 状态

- **状态**：实施中（Core/Application/Infra 主路径已切换，Desktop C1 已通过；四/五节点与实体设备矩阵待验收）
- **日期**：2026-08-30
- **实施方式**：Core、Application、Infra、Engine 与 Desktop E2E 一次性切换；不得长期保留旧调度语义
- **修正范围**：修正规格 020、021、023、027、028 中“成员上线后核对即可最终传播”的不完整实现；成员历史、移除决定、分叉隔离和准入完成边界保持不变
- **相关决策**：`docs/adr/020-membership-reconciliation-and-user-decisions.md`、`docs/adr/025-application-space-membership-one-shot-rewrite.md`

# 1. Overview

当前成员历史同步由易失触发器驱动：启动、恢复、状态变化或 peer 上线时，Application 在全局十秒预算内按顺序向当前 peer 推送完整历史。失败或预算耗尽只增加本轮 `deferred_peer_count`，没有保存“哪个 peer 尚未确认哪个历史位置”。周期维护又只检查暂停关系，不比较本机历史位置与 peer 的确认位置。因此一次触发未覆盖的设备可能永久停留在旧历史。

三设备 Desktop 场景已经稳定复现：A 接纳 B，B 再接纳 C 后，B 和 C 已激活，但 A 长期只保留 A/B。根因不是三设备特例：Sponsor 激活会把所有 peer 重建为 `Consistent`，并把它们的 `confirmed_position` 直接写成本机最新位置，即使这些 peer 从未确认该位置；接收新历史后也不会立即唤醒效果恢复和继续传播。链式、树型、分批在线、离线重连和超过单轮预算的设备集合都会出现同类丢失。

本规格把成员历史传播改为持久化反熵。Application 的 `MembershipHistoryAntiEntropy` 对“本分支中每个合法 peer 最终确认本机历史，且本机应用收到的合法历史”负唯一责任。每个 peer 的确认水位只能由该 peer 的认证 ACK 推进；本机历史变化会持久建立传播欠账；网络失败、预算耗尽、进程重启和拓扑中间节点离线都只能延后欠账，不能删除它。Runtime 只提供触发和有界调度，不拥有正确性。

# 2. Goals

- 任一本机正式历史变化后，为同一分支的每个合法 peer 建立可跨重启恢复的同步责任。
- `confirmed_position` 只能由对应认证 peer 对该位置的有效 ACK 推进，禁止本机推断或批量覆盖。
- 任意一次发送失败、超时、预算耗尽或进程退出后，未确认 peer 保持待同步并按持久退避重试。
- 接收并提交合法历史后立即唤醒成员效果恢复，并为除来源外的其他合法 peer 建立继续传播责任。
- 支持星型、链式、树型、临时分区和设备交错在线；新历史不要求全部设备同时在线，也不依赖固定主设备。
- 单轮工作使用有界并发、总预算和持久公平游标；设备数量增加时不允许排序靠后的 peer 永久饥饿。
- 历史交换先比较有界摘要，只传输接收方缺少的连续历史页；不在每轮默认发送完整历史。
- 普通新增自动应用；未由本机接受的移除继续进入待决定；确认分叉的 peer 之间停止完整历史和普通内容传播。
- 所有反熵状态随 membership ledger 使用 MasterKey AEAD 持久化，不新增明文 peer、历史位置或重试记录。
- 提供确定性的多节点模型测试和真实 Desktop 多进程 E2E，证明多跳传播、重启恢复和公平性。

# 3. Non-Goals

- 不修改 Space admission 的 OPAQUE、OpenMLS、Candidate/Commit/Complete 或 JoinSpace 产品契约。
- 不引入中心服务器、固定 leader、主设备、多数投票、Raft、CRDT 成员集合或服务器推送。
- 不自动合并已确认分叉，也不改变用户接受或拒绝移除的权利。
- 不保证永久离线设备在离线期间收敛；它重新可达后必须从持久欠账继续。
- 不让 reachability、成员 roster、可信关系或地址记录成为成员资格来源。
- 不以增加 timeout、缩短周期、重复广播全部历史或无限并发作为正确性方案。
- 不要求全连接物理拓扑；只要求任何时刻存在的认证路径能够逐跳传播同一分支历史。
- 不公开内部水位、页游标、重试次数或调度步骤到 `uc-engine` 稳定 API。
- 不保留新旧两套同步调度器、双写状态或长期 feature flag。

# 4. Current Architecture Context

```text
Component: VersionedMembershipHistory
Path: crates/uc-core/src/membership/
Responsibility: 验证签名单父历史、成员资格、祖先关系、移除决定和分叉。
Relationship: 规则可以判断历史关系，但当前 Application 没有把每个 peer 的已确认位置变成可靠传播责任。
```

```text
Component: MembershipLedger
Path: crates/uc-application/src/space/membership/ledger/
Responsibility: 原子保存历史、peer reconciliation、分页入站传输、pending effects 和 revision。
Relationship: 已是正确事实边界；`PeerReconciliationRecord` 当前混合历史关系与确认位置，却没有持久同步欠账、退避和公平调度信息。
```

```text
Component: SynchronizeMembershipHistoryUseCase
Path: crates/uc-application/src/space/membership/synchronize_history/target_use_case.rs
Responsibility: 枚举当前 peer，在十秒预算内顺序推送历史并处理 ACK。
Relationship: Deferred 只存在于内存报告；周期判断只查看 paused peer，不比较确认位置；每轮为所有 peer 导出并发送完整历史。
```

```text
Component: HandleMembershipHistoryMessageUseCase
Path: crates/uc-application/src/space/membership/handle_history_message/use_case.rs
Responsibility: 接收入站分页，验证并合并历史，保存 pending effects 和 ACK。
Relationship: 提交新历史后没有 maintenance wake；来源确认与向其他 peer 继续传播没有形成同一持久 mutation。
```

```text
Component: Sponsor admission activation
Path: crates/uc-infra/src/space/admission/sponsor/complete.rs
Responsibility: 激活 Sponsor 安全状态、成员投影和正式历史。
Relationship: 当前重建全部 `peer_reconciliation`，把所有 peer 标为已确认最新位置，违反“确认只能来自 peer ACK”。
```

```text
Component: SpaceMembershipMaintenanceRuntime
Path: crates/uc-application/src/space/membership/maintenance/
Responsibility: 串行接收 Startup、Resume、Periodic、StateChanged 和 PeerOnline，并运行 admission、effect、restricted delivery、history sync 和 cleanup。
Relationship: Runtime 应只调度；当前同步正确性依赖某个易失 trigger 恰好运行并覆盖所有 peer。
```

```text
Component: Iroh membership history adapter
Path: crates/uc-infra/src/network/iroh/membership_history_exchange_adapter.rs
Responsibility: 在认证 Iroh 通道上传输有界成员历史消息并调用 Application endpoint。
Relationship: 保持 transport-only；不能决定 peer 已确认、重试完成、分叉或传播对象。
```

当前失败数据流：

```text
B admits C
  -> B commits history A/B/C
  -> B locally writes A.confirmed_position = A/B/C without A ACK
  -> one StateChanged round is missed/deferred
  -> periodic check sees no paused peer
  -> no later synchronization
  -> A remains A/B forever
```

# 5. Proposed Design

## Invariants and ownership

1. Application `MembershipHistoryAntiEntropy` 对历史传播、确认水位、重试、入站合并后的 fan-out 和重启恢复负唯一责任。
2. 调用方只提交“本机历史已提交”“收到认证消息”“peer 可达”或“运行维护轮次”；不得逐步编排摘要、页、ACK 和重试。
3. `peer_confirmed_position` 是远端陈述，只能由该 peer 在认证通道返回且绑定本次 transfer 的 ACK 推进。
4. `desired_position` 是本机传播目标，由当前已应用分支历史派生；不得用它覆盖确认水位。
5. `confirmed_position != desired_position` 即存在同步欠账；关系显示为 `Consistent` 也不能取消欠账。
6. 网络发送成功不是完成；只有 ACK 与 peer 身份、lineage、transfer、position 一致并持久提交后才完成。
7. 本机历史提交与为 eligible peers 标记欠账属于同一个 ledger mutation。
8. 入站历史合并、来源 peer 水位更新、pending effects 创建和其他 peer 欠账标记属于同一个 ledger mutation；提交后再返回 ACK。
9. Runtime trigger 可以丢失或合并；只要周期维护最终运行，持久欠账就必须继续。Runtime 队列不是 outbox。
10. 分叉关系禁止双方完整历史传播；待移除决定只允许现有受限投递规则，不能因反熵自动接受移除。
11. 每轮调度失败不能阻止其他 peer；稳定无效只隔离对应关系，临时失败进入持久退避。
12. 每个页面和摘要都有固定大小上限；不得随设备数或历史长度无界分配。

开工前固定答案：

| 问题 | 答案 |
| --- | --- |
| 谁负责完整结果 | Application `MembershipHistoryAntiEntropy` |
| 调用方唯一执行什么 | 提交历史变化或触发一次维护；endpoint 交付一条认证消息 |
| 成功是什么 | 对目标 peer 的确认水位持久推进到该次 desired position；全局收敛是所有 eligible peer 无欠账 |
| 失败是什么 | Deferred 保留欠账；Invalid/Diverged 更新该 peer 关系并停止不允许的传播；Corrupt 失败关闭 |
| 谁负责重启和重试 | `MembershipHistoryAntiEntropy` 从加密 membership ledger 扫描欠账并恢复 |

## Components

### Core reconciliation planner

- **Path:** `crates/uc-core/src/membership/`
- **职责:** 比较 lineage/position，判断 Same、LocalAhead、RemoteAhead、Diverged、Invalid，验证摘要请求和 ACK 能否推进水位。
- **输入:** 本机验证历史位置、远端认证摘要或 ACK、已知 peer 水位。
- **输出:** `Noop`、`OfferSuffix`、`RequestSuffix`、`Diverged` 或稳定错误。
- **关系:** 不执行网络、时间、退避和持久化；不认识 Iroh。

### Application `MembershipHistoryAntiEntropy`

- **Path:** 将 `membership/synchronize_history/` 与 `membership/handle_history_message/` 收口为一个负责模块；允许内部保留 send/receive 文件，但只暴露一个 endpoint 和一个 maintenance port。
- **职责:** 原子建立欠账、选择公平批次、交换摘要/缺失页、提交 ACK、恢复入站分页、创建 effects、fan-out 和计算下一重试。
- **输入:** ledger、认证 peer message、clock、history transport、maintenance trigger。
- **输出:** 单轮脱敏报告、认证回复和已提交 ledger 状态。
- **关系:** `MaintainSpaceMembershipUseCase` 只调用一次 `run_round(trigger)`；不得分别理解同步和接收后的恢复步骤。

### Membership maintenance runtime

- **Path:** `crates/uc-application/src/space/membership/maintenance/`
- **职责:** 合并触发、暂停/恢复、周期唤醒和关闭；同一 profile 最多运行一个反熵轮次。
- **输入:** Startup、Resume、Periodic、StateChanged、PeerOnline。
- **输出:** 无业务状态；只驱动完整 use case。
- **关系:** StateChanged 是降低延迟的提示，周期扫描持久欠账是最终可靠性来源。

### Infra history transport

- **Path:** `crates/uc-infra/src/network/iroh/membership_history_exchange_adapter.rs`
- **职责:** 编解码固定版本有界消息、认证来源、执行单请求 deadline。
- **输入:** typed summary/request/page/ack；summary 和 page 携带发送者准入声明。
- **输出:** typed reply或 Offline/Transport/Rejected。
- **关系:** 连接层只核对远端公钥指纹与准入声明一致；未知发送者仅可进入有界后缀验证，成员资格必须由 Application/Core 对完整签名历史确认。Infra 不保存 retry、不选择 peer、不更新水位。

## Data Model

`PeerReconciliationRecord` 保留关系事实，并增加独立同步状态：

```text
PeerHistorySyncState
  confirmed_position: Option<BaseMembershipHistoryPosition>
  pending_since_revision: Option<u64>
  retry_attempt: u32
  next_attempt_at_ms: i64
  last_attempt_outcome: Never | Deferred | Acked | StableRejected
```

字段语义：

- `confirmed_position`：该 peer 最后通过认证 ACK 确认拥有的位置；仅 ACK mutation 可写。
- `pending_since_revision`：本机首次发现 peer 落后时的 ledger revision；用于公平排序和观测，不表示历史身份。
- `retry_attempt`：连续临时失败次数；成功 ACK 清零，溢出视为 Corrupt。
- `next_attempt_at_ms`：固定上限的指数退避时间；时间倒退时按立即可重试处理，不改写历史。
- `last_attempt_outcome`：脱敏稳定分类；不得保存错误字符串、设备名或地址。

`desired_position` 不重复持久化，始终取本机当前已应用历史位置。若 peer 与本机处于允许传播的同一分支，且 `confirmed_position != desired_position`，该 peer 就是 pending。

入站传输继续持久保存，但必须绑定：

```text
InboundMembershipTransfer
  source_device_id
  transfer_id
  lineage_id
  base_position
  target_position
  page_count
  next_page_index / pages
  total_bytes
```

旧 ledger decode 后不得把旧 `confirmed_position` 当作可信最新 ACK。迁移策略：保留关系；所有 eligible peer 的确认位置清为 Unknown，并建立待同步。该状态整体由现有 MasterKey AEAD 保存。

## API / Interface

Application 内部主要接口：

```text
MembershipHistoryAntiEntropy::run_round(trigger)
  -> MembershipAntiEntropyReport

MembershipHistoryAntiEntropy::handle_authenticated_message(source, message)
  -> MembershipHistoryMessage

MembershipHistoryAntiEntropy::mark_local_history_changed(mutation)
  -> committed ledger snapshot
```

`run_round` 返回计数：acked、deferred、stable_rejected、remaining_pending、effects_recovered。报告不包含 peer id、历史 id、地址或错误字符串。

Transport port 保持一次 typed request-response，不公开连接：

```text
exchange_membership_history(peer, message)
  -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError>
```

消息协议使用固定 version，并至少包含：

- `Summary { lineage, current_position, transfer_id }`
- `RequestSuffix { transfer_id, known_position, max_pages }`
- `HistoryPage { transfer_id, base_position, target_position, index, count, payload }`
- `Ack { transfer_id, confirmed_position, outcome }`

接收方只有在完整页已验签并原子提交后才返回确认 target position 的 ACK。未知祖先、错 lineage、超限、错页序、ACK 超前或身份不匹配返回稳定 Invalid；网络异常保留欠账。

所有依赖错误遵守 `crates/uc-application/src/error.rs`：依赖失败保留 `#[source]`，不得字符串化或吞掉来源。

## Workflow

### 本机历史变化

1. 准入、普通新增、移除或用户决定准备一个 history mutation。
2. `MembershipHistoryAntiEntropy` 在同一 ledger commit 中保存新历史/effects，并保留每个 peer 的真实确认水位。
3. 对同分支 eligible peer，水位低于新 head 自然成为 pending；不批量写“已确认”。
4. commit 成功后发送 StateChanged wake。
5. `run_round` 按 `next_attempt_at`、`pending_since_revision` 和持久公平游标选择有界 peer 批次。

### 出站反熵

1. 向 peer 发送本机 summary。
2. peer 已有相同位置时返回 ACK；发送方持久推进该 peer 水位。
3. peer 落后时请求缺失 suffix；发送方分页发送，最后一页提交后取得 target ACK。
4. peer 更新时，本机请求其 suffix，经验证合并后在同一 mutation 创建 effects 和其他 peer 欠账。
5. Offline/Transport/timeout 增加持久 retry 并计算下一时间；继续调度其他 peer。
6. Invalid/Diverged 只更新对应关系，不能阻塞其他 peer。

### 入站合并与逐跳传播

1. Infra 从认证连接得到 source device，并交付 typed message。
2. Application 核对 source 是本机当前分支允许的 peer。
3. 完整验证远端 suffix；未完成分页只保存 transfer，不修改正式历史。
4. 同一 commit 保存新历史、pending effects、来源确认关系，并让其他 eligible peer 相对新 head 成为 pending。
5. commit 后唤醒 effects 与下一反熵轮次，再返回 ACK。
6. 因此 `A <- B <- C <- D` 可逐跳传播，不要求 A 与 D 同时在线。

### 公平调度

1. 每轮固定总时间、最大 peer 数和最大并发数；这些是调度上限，不是任务生命周期。
2. 选择顺序以最早 `next_attempt_at`、最早 `pending_since_revision`、持久 round-robin cursor 决定。
3. 本轮未选中或预算耗尽的 peer 不改变欠账，下一轮从 cursor 后继续。
4. 单 peer 慢请求不能占用全部并发槽之外的其他工作。

# 6. Implementation Plan

## Step 1：锁定 Core 水位与历史关系规则

- **Files:** `crates/uc-core/src/membership/`、对应 Core tests。
- **Change:** 增加 summary/position planner、ACK 推进验证和 suffix 导入导出规则；明确关系与确认水位是正交事实。
- **Risk:** 把 removal divergence 错判为普通落后；必须复用现有历史祖先与决定规则。

## Step 2：扩展加密 ledger 数据模型和迁移

- **Files:** `crates/uc-application/src/space/membership/ledger/model.rs`、persistence codec、`crates/uc-infra/migrations/`。
- **Change:** 增加 peer sync state、入站 base/target position 和公平 cursor；旧确认水位降为 Unknown，保持 AEAD 整体保存。
- **Risk:** 错误信任旧水位会保留本次 bug；迁移必须重开验证且可故障恢复。

## Step 3：收口 Application 反熵负责人

- **Files:** `membership/synchronize_history/`、`membership/handle_history_message/`、`membership/maintenance/`、`space/application.rs`。
- **Change:** 建立一个 `MembershipHistoryAntiEntropy`，统一本机变化、入站提交、出站同步、重试和 wake；maintenance 只调用完整入口。
- **Risk:** 新旧 use case 并存形成双负责人；切换完成后删除旧独立判断。

## Step 4：修正所有历史写入点

- **Files:** admission Sponsor/Joiner activation、remove、decision、initializer 和 ledger helpers。
- **Change:** 任何新 head commit 保留真实 peer 水位并自然建立欠账；删除 Sponsor 批量确认最新位置的逻辑。
- **Risk:** 准入双方已经通过协议获得历史的水位需要精确证据；只能对实际发送并确认的对端推进。

## Step 5：实现摘要与缺失 suffix wire

- **Files:** Core membership messages、Infra Iroh adapter、codec tests。
- **Change:** 用 summary/request/page/ack 替代默认完整历史推送；固定 frame、page、transfer 和总大小上限。
- **Risk:** wire 切换期间出现双协议；使用一次性 clean cutover，不自动回退旧 wire。

## Step 6：有界并发、公平重试和生命周期

- **Files:** Application anti-entropy scheduler、maintenance runtime tests。
- **Change:** 持久退避、batch、并发上限、cursor；周期始终扫描到期欠账，而非 paused UI 状态。
- **Risk:** retry storm 或饥饿；使用确定 clock 测试证明上限和公平性。

## Step 7：效果恢复与 fan-out 闭环

- **Files:** inbound endpoint、effect executor、maintenance wiring。
- **Change:** 新历史 commit 后立即 wake；效果失败保留 phase；其他 peer 欠账与历史同 commit 建立。
- **Risk:** ACK 已返回但 effect 未应用；这是允许的持久阶段，普通 scope 在 effect 完成前继续失败关闭。

## Step 8：删除旧语义并更新文档

- **Files:** 旧 periodic predicate、旧全历史单轮实现、架构脚本、Spec/ADR 状态和 architecture bible。
- **Change:** 删除只看 paused peer、易失 Deferred 和伪确认逻辑；禁止其符号回归。
- **Risk:** 留下兼容别名掩盖第二实现；架构检查必须按行为和所有权检测。

# 7. Edge Cases

```text
Scenario: 当前只有本机一个成员
Expected behavior: 无 pending peer，周期轮次零网络调用。
Implementation: 从验证历史派生 eligible peers，空集直接完成。
```

```text
Scenario: peer 已确认旧位置，本机连续提交多个新增事件
Expected behavior: 只保留到最新 desired position 的一个欠账，发送可验证连续 suffix，不逐事件建立无界 outbox。
Implementation: confirmed watermark + current head 派生欠账。
```

```text
Scenario: 发送成功但 ACK 丢失
Expected behavior: 欠账保留；重试相同或新的 transfer 均可由接收端幂等回答已确认位置。
Implementation: ACK 之前不推进水位；completed inbound transfer 有界去重。
```

```text
Scenario: ACK 声称超过发送 target 或属于另一 lineage
Expected behavior: 稳定 Invalid，不推进水位。
Implementation: Core planner 验证 transfer、lineage、target 和认证 source。
```

```text
Scenario: 单轮预算小于全部 peer 工作量
Expected behavior: 未处理 peer 保持 pending，公平 cursor 保证后续轮次获得机会。
Implementation: budget 只停止本轮，不提交完成；cursor 与 ledger 一起保存。
```

```text
Scenario: 中间节点收到更新后立即崩溃
Expected behavior: 重启后历史、effects 和 fan-out 欠账要么全部存在，要么全部不存在，不出现只合并不传播。
Implementation: 单一 ledger mutation；commit 后 wake 只是延迟优化。
```

```text
Scenario: 新成员地址暂不可用
Expected behavior: 历史资格可先提交，effect/sync 保持 Deferred；地址恢复后继续，不从 roster 推断完成。
Implementation: 分离正式历史、effect phase 和 peer sync state。
```

```text
Scenario: 本机收到未确认移除及其后继
Expected behavior: 保存待决定事实，不自动应用破坏性变化，不向被隔离关系传播完整分支。
Implementation: 复用 ADR-020 pending decision/restricted delivery。
```

```text
Scenario: 两个合法分支不可比较
Expected behavior: 标记对应 peer Diverged，停止双方完整历史与普通内容；各自分支内继续反熵。
Implementation: planner 返回 Diverged，调度过滤该关系。
```

```text
Scenario: 系统时间倒退或 retry counter 溢出
Expected behavior: 时间倒退使任务立即可重试；counter 溢出进入 RecoveryRequired，不 wrap。
Implementation: checked arithmetic 和固定 clock tests。
```

```text
Scenario: 旧 ledger 含伪造的最新 confirmed_position
Expected behavior: 升级后不信任该位置，重新与所有 eligible peer 核对。
Implementation: migration 清空旧确认水位并保存 pending revision。
```

# 8. Testing Strategy

## Unit Test

- Core：Same、LocalAhead、RemoteAhead、Diverged、错 lineage、ACK 超前和 suffix 边界。
- Ledger：历史提交与所有 peer 欠账同 commit；ACK 只推进对应 peer；冲突不产生部分状态。
- Scheduler：10、50、200 个 peer，在固定预算和不同失败延迟下最终每个 peer 都被选择，无排序饥饿。
- Retry：Offline、Transport、timeout、重启、时间倒退和 counter overflow。
- Receiver：分页中断不改正式历史；最终页提交同时创建 effects 和 fan-out 欠账。
- Migration：旧 Consistent/latest 水位全部降为 Unknown；密文重开后状态一致，无敏感明文。

## Integration Test

- 两个内存 Application 节点交换 summary/suffix/ACK，验证双方水位和幂等重放。
- A/B/C 链式：只有 B 与 C 交换后，再由 B 与 A 交换，A 最终得到 C，无需 A/C 直连。
- 树型：A-B、A-C、B-D、C-E，交错上线后同一分支最终包含五台设备。
- 中间节点重启：B 收到 D 后、fan-out 前重启，恢复后 A 仍得到 D。
- 真实 SQLite fault injection：history/fan-out/ACK mutation 任一点失败都恢复旧状态或完整新状态。
- 真实 Iroh：摘要一致、缺失 suffix、多页、ACK 丢失、断线恢复和认证来源错误。

## Regression Test

- 普通新增自动应用，移除仍等待本机决定，拒绝后仅相关关系 Diverged。
- admission 双方 Active、session 重启、当前成员 scope 和正文传输保持通过。
- removed/diverged/invalid peer 不获得完整历史或普通内容。
- P2P 失败不自动切换 LAN compatibility。
- 日志、SQLite、缓存和测试诊断无设备名、地址、历史内容、凭据或密钥明文。

## Desktop CLI E2E

- 三节点在线链：A 邀请 B，B 邀请 C；A/B/C 均达到三成员，daemon 重启后保持，A↔C exact text transfer。
- 四节点离线链：A-B-C-D 分步加入，中间节点轮流离线；全部恢复后四节点收敛。
- 五节点树型：不同 sponsor 分支加入但历史保持单父合法延伸；所有在线节点最终一致。
- 超预算场景：注入慢/离线 peer，确认排序靠后的在线 peer 在后续轮次完成。
- 设备矩阵未执行项明确记录“跳过”，不得用同机多进程冒充实体设备。

# 9. Acceptance Criteria

* [ ] Sponsor 激活不再为未 ACK 的旧成员写入最新 `confirmed_position`。
* [ ] 任一本机新历史与 peer 传播欠账在同一 encrypted ledger mutation 保存。
* [ ] 任一入站新历史与 effects、来源关系、fan-out 欠账在同一 mutation 保存。
* [ ] `confirmed_position` 只有认证 ACK 路径可以推进，并有架构测试禁止其他写入。
* [ ] 周期维护根据到期欠账运行，不依赖 paused UI 状态或易失 trigger。
* [ ] 网络失败、预算耗尽和重启后欠账仍存在并最终重试。
* [ ] 有界并发与持久公平 cursor 在 200-peer 确定测试中无饥饿。
* [ ] 摘要相同时零历史页；落后时只发送缺失 suffix；所有 frame/page/transfer 有固定上限。
* [ ] 链式 A-B-C-D 不要求 A/D 同时在线即可逐跳收敛。
* [ ] 五节点树型和交错上线集成测试最终收敛。
* [ ] 分叉只隔离相关 peer，各分支内部继续传播。
* [ ] 未确认移除不被反熵自动应用。
* [ ] SQLite fault injection 证明无“历史已提交但传播责任丢失”窗口。
* [x] Desktop 三节点加入、重启、Sponsor 离线恢复和 A↔C exact transfer 通过。
* [ ] Desktop 四节点离线链和五节点树型通过。
* [ ] Engine workspace tests、真实 Iroh、真实 SQLite、fmt、architecture 和 diff gates 通过。
* [ ] 实体设备未执行项目明确为“跳过”。
* [ ] `docs/architecture/architecture-bible.md` 与相关规格状态同步。

# 10. Risks and Trade-offs

## 每 peer 持久水位

空间内每个设备保存其他 eligible peer 的同步状态，元数据为 O(N)。个人设备空间通常远小于内容历史，代价可控；它换来可证明的逐 peer 交付责任。不能用单个“最近广播成功”布尔值替代，因为它无法指出欠账对象。

## 传播复杂度

一次新历史最终可能产生 O(N) 个确认交换。摘要和 suffix 降低字节量，有界并发限制瞬时资源。固定 leader 可降低部分流量，但会破坏离线可用且形成单点，本规格不采用。

## ACK 与效果分阶段

接收方可以在正式历史和 pending effect 原子保存后 ACK，而 roster/安全 effect 稍后完成。这样避免网络连接持有本机副作用事务；普通 scope 在 effect 完成前保持关闭，安全性不降低。

## Wire clean cutover

摘要/suffix 消息改变当前内部 wire。并行支持旧完整推送会形成第二实现和不同确认语义，因此一次性切换。已确认旧版本按现有 `UpgradeRequired` 策略暂停普通同步，不自动降级。

## 被拒绝的替代方案

- 增加 StateChanged wake：降低延迟但 trigger 仍会丢失，不能替代持久欠账。
- 每 30 秒向所有 peer 广播完整历史：可最终重试，但流量为 O(N×H)，且无法表达确认和公平性。
- 延长十秒预算：设备数和离线延迟仍可超过任何固定值，排序饥饿不消失。
- 仅保留 `Consistent/Diverged`：关系不等于水位，无法判断 peer 是否缺少新事件。
- 由 Sponsor 通知全部旧成员：要求 Sponsor 同时可达所有设备，不能支持链式和离线拓扑。
- 引入中心服务或 leader：改变离线优先产品和信任模型，超出既有 ADR。

# 11. Open Questions

没有阻止实施的产品问题。以下技术参数在 Step 6 开始前通过 benchmark 固定到常量并写入测试，不得成为运行时无限配置：

- 单轮最大 peer 数；
- 最大并发交换数；
- retry 初始延迟、倍率、抖动和上限；
- completed inbound transfer 的有界保留数量；
- summary/suffix 单页及单次 transfer 上限。

参数选择不能改变本规格的不变量：预算只限制单轮，不能删除欠账；任何有限设备集合在持续获得运行机会且网络最终可用时都必须无饥饿地收敛。
