# 规格 025：用户明确加入安全取代旧加入

## 状态

- **状态**：仓库内实现与自动化验证完成；Android 双端实体设备验收待补
- **日期**：2026-08-18
- **决策依据**：`docs/adr/022-user-initiated-join-supersession.md`
- **相关文档**：`docs/adr/017-pairing-as-workspace-admission.md`、
  `docs/adr/021-workspace-convergence-internal-boundaries.md`、
  `docs/specs/023-durable-membership-proof-and-admission-activation.md`、
  `docs/specs/024-workspace-convergence-internal-boundaries.md`、
  `docs/architecture/architecture-bible.md`

# 1. Overview

当前每次公开 `JoinSpace` 在发送网络请求前都会查询本机未结束加入。如果找到记录，
`DurableAdmissionTransaction::prepare_join_before_network` 会调用 `reopen_join_start`，把本次输入当成旧加入的恢复。
输入不同会被折叠成 `AdmissionInProgress` 或内部失败，最终可能映射成通用 Engine 错误 1238；输入相同则继续复用旧
`attempt_id`、`join_id`、成员实例和安全材料。

这无法区分两种本质不同的动作：断线、重启和后台重试应继续同一加入；用户退出旧流程后再次明确执行
`JoinSpace`，应开始一次新的加入。现场问题正是旧 `Initiated` 记录拦住了用户第二次提交，第二次请求没有到达邀请方。

本规格实施 ADR-022：每次公开 `JoinSpace` 都是新用户操作；旧本机 Joiner 只有在尚未持久保存 `Prepared` 时可被
原子取代。`Prepared` 及以后可能已经使邀请方正式提交，必须继续旧加入并返回专用冲突。整个裁决、保存、清理、
恢复和公开结果仍由 `WorkspaceConvergence` 完整负责，调用方不新增取消、重置或分步推进。

# 2. Goals

- 每次公开 `JoinSpace` 通过预检后都创建全新的 attempt、join、本机序号、成员实例、KeyPackage、续接密钥和初始请求。
- 断线、重启、消息重放、投递退避和后台恢复继续原 attempt，不通过再次调用公开 `JoinSpace` 恢复。
- 旧本机 Joiner 处于 `Initiated` 或 `Candidate` 且无不可取代恢复工作时，在一个持久事务中保存旧终态并创建新加入。
- 旧本机 Joiner 已持久保存 `Prepared` 或更晚阶段时，返回稳定错误
  `PreviousJoinCannotBeSuperseded`，不产生新加入副作用，旧加入继续恢复。
- 被取代加入的迟到消息、取消通知和终态压缩保持隔离，不能覆盖新 `current_join` 或产生第二成员事实。
- 相同邀请码再次提交也创建新的本机尝试，同时保持邀请方的一次性消费和 attempt 绑定不变。
- Fresh、Same-Space 和 Cross-Space 都保留当前 Space、设备身份、历史、设置、关系、搜索和受管文件。
- iOS、Android 和 HarmonyOS 通过现有绑定得到同一错误编号、类别和是否可重试，不形成平台规则分支。

# 3. Non-Goals

- 不允许取代已持久保存 `Prepared` 或更晚阶段的加入。
- 不改变邀请创建、过期、一次性消费、身份绑定或邀请方拒绝规则。
- 不新增公开的 Supersede、Abort、Reset 或分步准入操作。
- 不把 `SupersededByNewJoin` 加入公开 `JoinSpaceStatusSummary`；公开状态仍只有 Active、Pending 和 Rejected。
- 不并行运行两个当前本机加入；旧记录只保留隔离清理和防重放职责。
- 不改变 P2P 默认能力，不因失败自动切换 LAN 兼容线。
- 不清除 profile、设备身份、主密钥、当前 Space 或本机业务资料。
- 不在本规格阶段重构整个准入协议、仓储格式或绑定框架。
- 不用超时、连接状态、邀请码内容或产品端缓存判断旧加入是否可取代。

# 4. Current Architecture Context

```
Component: JoinSpace public operation
Path: crates/uc-engine/src/operations/space/join_space.rs
Responsibility: 接收稳定 JoinSpace 输入，调用应用 facade，并把应用失败映射为稳定 Engine 错误。
Relationship: 当前未知本机准入冲突落入通用 1238；实施后只增加一个专用冲突映射。
```

```
Component: Joiner handshake
Path: crates/uc-application/src/space/admission/joiner/joiner_handshake.rs
Responsibility: 收集本机资料，通过 WorkspaceAdmissionOwnerPort 在联网前保存加入，再发送 JoinerRequest。
Relationship: 当前把本机保存失败统一折叠为 Internal；实施后必须保留不可取代冲突的稳定语义。
```

```
Component: WorkspaceConvergence admission flow
Path: crates/uc-application/src/space/convergence/admission/flow.rs
Responsibility: 在 profile state_lock 下执行本机加入准备、准入消息推进、恢复和通知。
Relationship: 仍是完整流程的唯一负责人；锁负责进程内串行，不能替代数据库事务和版本检查。
```

```
Component: DurableAdmissionTransaction
Path: crates/uc-application/src/space/convergence/admission/transaction.rs
Responsibility: 创建、推进、恢复、投影和压缩加密准入尝试。
Relationship: prepare_join_before_network 当前遇到 Pending 时调用 reopen_join_start；这里是行为替换的主要入口。
```

```
Component: Admission domain model and repository port
Path: crates/uc-core/src/membership/admission_attempt.rs
Path: crates/uc-core/src/membership/ports.rs
Responsibility: 定义 Joiner 阶段、终态、outbox、尝试、profile 元数据、当前加入投影和持久化语义接口。
Relationship: 当前终态只有 Active、Completed、Rejected，仓储也没有原子结束旧加入并创建新加入的语义操作。
```

```
Component: Encrypted admission store
Path: crates/uc-infra/src/db/repositories/admission_attempt_store.rs
Responsibility: 用 profile 主密钥和每尝试数据密钥加密保存尝试，在 SQLite immediate transaction 中分配序号、推进 revision、版本检查和压缩终态。
Relationship: 新原子操作必须在这里一次提交；不得先保存旧终态再单独 create 新尝试。
```

```
Component: Current join projection and recovery
Path: crates/uc-application/src/space/convergence/admission/transaction.rs
Responsibility: 按最大 local_join_ordinal 生成 current_join，扫描未结束 outbox、写前记录、Space transition 和 cleanup。
Relationship: 被取代终态不公开投影，但需保留到旧消息清理和防重放事实可安全压缩。
```

```
Component: Mobile bindings
Path: bindings/uc-engine-uniffi/
Path: bindings/uc-ohos-napi/
Responsibility: iOS/Android 通过 UniFFI、HarmonyOS 通过 N-API 传递 Engine 错误和加入状态。
Relationship: 平台层透传同一 code/category/retryable，不判断内部阶段，也不自行重试 JoinSpace。
```

当前用户动作的数据流是：

1. `execute_join_space` 调用应用 facade。
2. `JoinerHandshakeCoordinator` 生成稳定请求绑定并调用 `prepare_local_join_before_network`。
3. `WorkspaceConvergence` 取得 `state_lock`，调用 `prepare_join_before_network`。
4. 当前实现若发现 Pending，就恢复旧尝试；否则生成加入材料并调用仓储 `create`。
5. 保存成功后才发送 JoinRequest；后续 Candidate、Prepared、Commit、Applied、Complete 均按 attempt 路由并先保存后发送。

# 5. Proposed Design

## Components

### AdmissionAttempt 的取代规则

在 `crates/uc-core/src/membership/admission_attempt.rs` 追加：

- `JoinerAdmissionStageV1::Superseded`，阶段排名为终态排名 6；只允许 Joiner 使用。
- `AdmissionTerminalResultV1::SupersededByNewJoin`，只允许与 Joiner `Superseded` 配对。
- 一个 crate 内领域方法，用于从已验证的 `Initiated` 或 `Candidate` 构造被取代终态。该方法一次完成：停止旧的
  JoinRequest/Candidate 等提交前 outbox、保留 inbox 防重放记录、生成隔离的 `CancelRequested` 清理通知、设置
  `terminal_result`，并保证 `rejection_reason` 为空。

该领域方法必须拒绝以下记录：非 Joiner、已有终态、`Prepared` 或更晚阶段、存在 `prepared_proof`、存在
`write_ahead_recovery`、已开始 `space_transition`、结构或恢复材料不完整。不能只检查 `stage_rank < 3` 后忽略相互矛盾的字段。

### 原子本机加入提交

在 `crates/uc-core/src/membership/ports.rs` 增加一个语义输入和一个仓储方法。建议命名：

```text
LocalJoinStartMutationV1
  Create { replacement }
  Supersede {
    expected_previous_attempt_id,
    expected_previous_record_version,
    previous_terminal,
    replacement
  }

AdmissionAttemptRepositoryPort::commit_local_join_start(mutation)
```

该接口只表达一次完整结果，不暴露“先结束、再建新记录、再推进投影”的步骤。`create` 继续服务 Sponsor、
CompletionHelper 和明确 attempt 身份的内部幂等恢复；公开用户加入只能通过 `commit_local_join_start` 创建本机 Joiner。

`DieselAdmissionAttemptStore` 在同一个 SQLite immediate transaction 中：

1. 打开并验证全部相关密文；
2. 核对期望旧 attempt、record version、当前最高序号的非终态 Joiner 和安全阶段；
3. 验证旧终态只能由原记录进行允许的取代转换；
4. 验证 replacement 是全新 `Initiated` Joiner，ordinal 等于 `next_local_join_ordinal`，且只有自己的初始 JoinRequest；
5. 用旧 wrapped data key 重新密封旧记录，为新记录创建独立 wrapped data key；
6. 保存旧终态和新尝试，`next_local_join_ordinal` 加一，`device_trust_revision` 只加一；
7. 提交后才允许发送新 JoinRequest。

事务允许更早的 `SupersededByNewJoin` 记录继续处理隔离的取消 outbox，但这些记录不能参与当前投影、占用当前
本机加入名额或阻止后续明确加入。任何其他非终态 Sponsor、CompletionHelper、写前恢复、Space transition、
彻底重置或不可隔离的 profile 工作仍返回既有忙碌结果。

### DurableAdmissionTransaction 的唯一用户动作入口

`prepare_join_before_network` 改为明确处理“用户新动作”，不再调用 `reopen_join_start`：

1. 执行现有来源历史预检。
2. 加载当前本机 Pending 并验证记录。
3. 若旧记录已到 `Prepared` 或更晚，立即返回 `PreviousJoinCannotBeSuperseded`，不生成新编号或安全材料。
4. 若旧记录为 `Initiated` 或 `Candidate` 且可取代，先生成全新组加入材料、成员实例、续接密钥、attempt 和 join。
5. 构造旧终态与新 `Initiated`，调用一次 `commit_local_join_start::Supersede`。
6. 没有旧 Pending 时调用 `commit_local_join_start::Create`。
7. 仓储版本冲突后重新读取权威状态并重新分类；不得把已推进到 Prepared 的旧记录降级为普通重试。

同一 attempt 的自动恢复继续由 `recoverable`、`recover_with`、outbox 重放、Space transition 恢复和当前状态查询负责。
`reopen_join_start` 及其“相同输入即恢复”的公开调用路径必须删除；测试若需要恢复，应直接加载同一 attempt 或运行恢复入口，
不能再次调用用户动作入口。

### 旧消息隔离与恢复

下列按 attempt 载入记录的入口必须先处理 `SupersededByNewJoin`：

- Candidate、重复 JoinRequest、取消回复和其他提交前消息：只返回保存的终止/取消结果或更新旧 inbox/outbox 清理，
  不改变当前加入、成员历史、Space transition 或新尝试。
- 旧 `CancelRequested` 投递：可继续到邀请方；邀请方按既有提交前取消规则幂等结束旧候选。
- 邀请方拒绝或取消确认：只确认旧清理并保留本机 `SupersededByNewJoin`，不得改写为公开 Rejected。
- Commit、Applied、Complete 或任何证明远端已越过本机安全边界的消息：返回并持久保存
  `RecoveryRequired`/一致性失败，不接受成员事实，不回退新尝试，也不静默丢弃证据。
- 恢复扫描：可同时推进一个当前加入与多个旧取代清理；所有写入严格按各自 attempt_id 隔离。
- 终态压缩：旧 outbox 静止后保留 attempt、join、ordinal、邀请摘要、终态、必要 inbox Ack 和防重放结果，再删除大块安全材料。

`SupersededByNewJoin` 不进入 `CurrentJoinStatus`。原子事务后，新 attempt 的 ordinal 更大，
`project_current_local_join` 只返回新 Pending；如果投影选到被取代终态，视为仓储不一致并失败关闭。

### 公开错误与绑定

增加以下稳定错误链：

| 层 | 新增结果 | 规则 |
| --- | --- | --- |
| `WorkspaceConvergenceError` | `PreviousJoinCannotBeSuperseded` | 只表示旧本机 Joiner 已到不可取代边界 |
| `RedeemPairingInvitationError` | `PreviousJoinCannotBeSuperseded` | 从本机保存入口原样映射，不归入 Internal |
| Engine | `JOIN_SPACE_PREVIOUS_JOIN_CANNOT_BE_SUPERSEDED_CODE = 1295` | category=`Conflict`，retryable=`false` |
| UniFFI / N-API | 透传 1295、Conflict、false | 不新增平台判断或自动操作 |

该冲突不表示邀请已使用，也不表示邀请方拒绝。调用方收到后应继续查询或订阅现有 `current_join`；Engine 不要求产品
理解 Prepared 或主动恢复旧 attempt。

## Data Model

| 数据 | 修改 | 生命周期与约束 |
| --- | --- | --- |
| `JoinerAdmissionStageV1` | 末尾追加 `Superseded` | 只与本机 Joiner 的取代终态配对；旧序列化判别值不改变 |
| `AdmissionTerminalResultV1` | 末尾追加 `SupersededByNewJoin` | 内部终态；不是 Rejected，不写成员历史 |
| `AdmissionAttemptV1` | 不新增公开业务字段 | 复用阶段、终态、outbox、inbox、版本和加密材料；被取代时 rejection_reason 必须为空 |
| `TerminalAdmissionAttemptV1` | 允许新终态值 | 压缩后继续保留最小防重放和旧消息重建事实 |
| `AdmissionProfileMetadataV1` | 字段不变 | 一次原子取代只推进一个 ordinal 和一次 revision；邀请消费映射不改绑 |
| `CurrentLocalJoinProjectionV1` | 字段不变 | 只投影新 attempt；不公开旧取代终态 |

枚举新值必须追加，不能改变现有值的序列化顺序。新版本可读取旧密文；包含新终态的资料由旧二进制打开时必须在写入前
失败，不允许把未知值当成 Rejected、Completed 或空状态。全部尝试正文、终态和清理资料继续按现有 MasterKey AEAD
规则加密；日志只允许固定阶段和错误分类，不输出邀请码、设备名、文件名、路径、密钥或底层错误正文。

## API / Interface

公开 `uc-engine` 输入和成功结果不变：

```text
JoinSpace(input) -> Active | Pending | Rejected
```

新增一个可判断失败：

```text
PreviousJoinCannotBeSuperseded
code: 1295
category: Conflict
retryable: false
```

内部唯一用户动作仍是：

```text
WorkspaceAdmissionOwnerPort::prepare_local_join_before_network(...)
```

该接口的参数和成功输出不需要新增 supersede 标志。每次调用本身就表示新的用户动作；恢复不经过这个接口。

仓储只新增 `commit_local_join_start` 这一项完整语义。不得公开以下接口组合：

- `can_supersede(attempt_id)` 加 `mark_superseded(attempt_id)` 加 `create_join(...)`；
- 由调用方读取阶段后决定调用哪个仓储方法；
- 由产品、绑定或 `uc-engine` 先 Cancel/Reset 再 Join。

## Workflow

### 没有旧 Pending

1. 用户调用 `JoinSpace`。
2. 负责人完成来源预检并确认没有当前本机 Pending。
3. 负责人生成全新材料和编号，仓储以 `Create` 原子保存并推进 ordinal/revision。
4. 提交成功后发送新 JoinRequest；网络失败时恢复扫描继续同一 attempt。

### 旧加入可安全取代

1. 用户再次调用 `JoinSpace`，无论输入是否相同。
2. 负责人读取旧 Joiner，确认其为 `Initiated`/`Candidate`，没有 Prepared 证明、写前恢复或 Space transition。
3. 负责人生成全新材料，构造旧 `SupersededByNewJoin` 和新 `Initiated`。
4. 仓储在一个事务中重新核对旧版本，保存旧终态和新尝试，并只推进一次 revision。
5. 当前投影立即只显示新 Pending；旧清理 outbox 可在后台继续。
6. 事务提交后发送新 JoinRequest。发送失败不回退原子结果，后台继续新 attempt。

### 旧加入不可取代

1. 用户再次调用 `JoinSpace`。
2. 负责人读取到旧 Joiner 已持久保存 `Prepared` 或更晚阶段。
3. 返回 1295 Conflict/non-retryable；不生成 attempt、join、ordinal、成员实例、密钥或新 outbox。
4. 旧 `current_join` 保持不变，原恢复流程继续向 Active 或协议允许的 Rejected 推进。

### 同一邀请码再次提交

1. 本机仍按上述规则创建新 attempt，不复用旧本机身份。
2. 邀请方若尚未消费邀请，可正常处理新请求。
3. 邀请方若已把邀请绑定旧 attempt，新请求稳定得到邀请不可用或身份冲突。
4. 本机不得复制、移动或删除邀请方的旧消费事实。

# 6. Implementation Plan

实施必须按下列纵向阶段推进。每一阶段先增加对应失败测试，再完成最小实现，并满足退出条件后才能进入下一阶段。
阶段之间不允许长期保留旧的“公开 JoinSpace 重开 Pending”路径。

## Phase 1: 固定基线与公开结果

**File:** `crates/uc-application/src/space/convergence/admission/tests.rs`、
`crates/uc-application/src/space/admission/joiner/joiner_handshake.rs`、
`crates/uc-application/src/facade/space_setup/errors.rs`、
`crates/uc-engine/src/contract/error_codes.rs`、`crates/uc-engine/src/operations/space/join_space.rs`

**Change:**

- 将现有“相同输入恢复、不同输入 AdmissionInProgress”测试改为两组明确契约：自动恢复继续同一 attempt；再次公开加入必须产生新动作。
- 新增 `PreviousJoinCannotBeSuperseded` 应用错误和 Engine 1295 映射测试，先打通错误类型但不把阶段判断放到 Engine。
- 为后续阶段确定测试名和数量，运行 `--list` 确认筛选不是零测试。

**Risk:** 误删自动恢复幂等测试会掩盖重复成员风险。公开调用与内部恢复必须使用不同测试入口。

**Exit Gate:** 现有重启/断线恢复测试继续通过；1295 映射测试通过；Phase 2 的行为用例、测试名称和夹具边界已记录，
但不在本阶段提交尚未实现的失败测试。

## Phase 2: 跑通 Initiated 取代的最小端到端闭环

**File:** `crates/uc-core/src/membership/admission_attempt.rs`、
`crates/uc-core/src/membership/ports.rs`、
`crates/uc-infra/src/db/repositories/admission_attempt_store.rs`、
`crates/uc-application/src/space/convergence/admission/transaction.rs`、
`crates/uc-application/src/space/convergence/admission/flow.rs`

**Change:**

- 追加 Joiner `Superseded` 和 `SupersededByNewJoin`，完成核心不变条件验证。
- 实现 `commit_local_join_start` 的 Create 与 Supersede 两种原子分支。
- 将公开本机加入准备改为每次生成新身份；先覆盖旧 `Initiated`、不同邀请码和相同邀请码三种路径。
- 原子提交后仅返回新 Pending；删除公开路径上的 `reopen_join_start` 回退。

**Risk:** 先终结旧记录再创建新记录会留下空窗；先创建新记录会留下两个当前加入。只允许单一事务实现。

**Exit Gate:** 一个真实加密临时数据库测试从 Engine/应用入口完成“第一次停在 Initiated，第二次得到新 Pending”；
旧记录为 Superseded，新旧 attempt/join/ordinal/member/key material 全部不同，current_join 只显示新记录。

## Phase 3: Candidate、Prepared 与并发边界

**File:** 上述核心、仓储和准入文件，以及 `crates/uc-application/src/space/convergence/admission/tests.rs`

**Change:**

- 允许无恢复矛盾的旧 `Candidate` 原子取代。
- 对 `Prepared`、Committed、Applied 及更晚阶段返回 1295，零新加入副作用。
- 在仓储事务内重新核对旧 attempt/version/stage、最高 ordinal、write-ahead 和 Space transition。
- 对两个同时到达的公开 `JoinSpace` 使用 profile `state_lock` 串行；后到动作可按规则取代前一个新 `Initiated`。
- 版本冲突后重新读取权威状态，只重试安全的完整事务，不沿用已生成但未保存的旧判断。

**Risk:** 只在应用层检查 Prepared 会被并发推进穿透；数据库必须再次检查持久状态。

**Exit Gate:** Initiated/Candidate 成功、Prepared 及以后稳定冲突、两次并发调用确定性收敛、ordinal/revision 单调且无重复。

## Phase 4: 旧消息、清理、恢复与压缩

**File:** `crates/uc-application/src/space/convergence/admission/transaction.rs`、
`crates/uc-application/src/space/convergence/admission/flow.rs`、
`crates/uc-infra/src/db/repositories/admission_attempt_store.rs`、准入恢复测试

**Change:**

- 为 Candidate、Rejected/取消确认、Commit、Applied、Complete、投递确认和恢复扫描增加 superseded 分支。
- 复用 `CancelRequested` 作为旧 attempt 的隔离清理通知；远端确认只清理旧 outbox，不改写本机终态。
- 允许恢复扫描同时处理新当前加入和旧清理，但禁止跨 attempt 修改。
- 扩展终态压缩与读取，保留新终态、防重放和必要 Ack，删除不再需要的大块安全材料。
- 有效 Commit 或更晚消息命中被取代 attempt 时保存恢复错误证据并失败关闭。

**Risk:** 普通终态处理可能把 Superseded 误转为 Rejected，或迟到 Commit 产生第二成员事实。

**Exit Gate:** 每类迟到/重复/乱序消息和重启恢复都有测试；旧清理不会覆盖新投影；矛盾 Commit 不修改成员历史或活动 Space。

## Phase 5: Engine 与三端绑定契约

**File:** `crates/uc-engine/src/contract/error_codes.rs`、
`crates/uc-engine/src/operations/space/join_space.rs`、
`crates/uc-engine/tests/public_contract.rs`、
`bindings/uc-engine-uniffi/tests/public_contract.rs`、
`bindings/uc-ohos-napi/tests/public_contract.rs`、`bindings/uc-ohos-napi/tests/app_project_contract.rs`

**Change:**

- 固定 1295、Conflict、non-retryable，并纳入错误码唯一性测试。
- 验证 iOS/Android UniFFI 与 HarmonyOS N-API 原样传递三个字段。
- 保持 JoinSpace 输入、成功结果、current_join、事件和绑定方法签名不变。
- 不在绑定或宿主增加自动 Cancel、Reset、再次 Join 或阶段判断。

**Risk:** 只测 Rust Engine 而不测生成绑定会让产品端仍把新冲突当通用失败。

**Exit Gate:** Engine、UniFFI 和 N-API 契约测试均非零通过，生成声明无意外变化。

## Phase 6: 故障矩阵、回归与实体设备验收

**File:** 准入仓储/应用/Engine 集成测试、`tests/hosts/uc-mobile-probe-core/`、相关验收记录和架构文档

**Change:**

- 在材料生成、旧记录加密、新记录加密、数据库保存、提交后发送和重启恢复处注入失败。
- 覆盖 Fresh、Same-Space、Cross-Space、相同邀请、不同邀请、旧清理未完成和邀请已消费。
- 完成 Android 真机“第一次失败并留下 Pending，再次明确加入”的双端验证；确认第二个请求实际到达邀请方并可继续完成。
- iOS、HarmonyOS 或其他未执行实体设备项明确记录为“跳过”，不得写为通过。
- 删除旧重开入口、临时兼容分支和不再使用的测试夹具，更新规格状态和架构维护记录。

**Risk:** 模拟器、发送成功日志或本机 Pending 只能证明局部路径，不能替代邀请方实际收到第二请求的真机证据。

**Exit Gate:** 全部自动化通过，故障注入只恢复完整旧状态或完整新状态，Android 实体设备完成双端流程；未执行矩阵明确标为跳过。

# 7. Edge Cases

## Scenario: 没有旧加入

**Expected behavior:** 与当前正常加入一致，但使用新的原子用户动作入口保存全新 attempt。

**Implementation:** `LocalJoinStartMutationV1::Create` 核对没有当前非终态 Joiner，并允许已取代旧记录的隔离清理继续。

## Scenario: 相同邀请码再次明确提交

**Expected behavior:** 创建新的本机身份；若邀请已消费，由邀请方稳定拒绝，不复用旧 attempt。

**Implementation:** 本机取代规则不比较邀请码内容；邀请方消费映射保持 attempt 绑定且不可改绑。

## Scenario: 不同邀请码再次明确提交

**Expected behavior:** Initiated/Candidate 时原子创建新加入，第二个请求实际使用新邀请码到达新邀请方。

**Implementation:** 新请求 payload、recipient、message_id 和全部安全材料从本次输入重新生成。

## Scenario: 旧加入已持久保存 Prepared，但 Prepared 尚未确认送达

**Expected behavior:** 返回 1295，继续旧加入，不创建新记录。

**Implementation:** 只看本机持久阶段和证明，不用 delivery Ack、在线状态或超时推断安全。

## Scenario: 阶段显示 Candidate，但存在 prepared_proof、Space transition 或写前恢复

**Expected behavior:** 失败关闭，不允许取代。

**Implementation:** 核心与仓储同时验证阶段和关联字段；矛盾记录返回 RecoveryRequired/Corrupt，而不是按数字排名放行。

## Scenario: 两个 JoinSpace 同时到达

**Expected behavior:** 按 profile 负责人串行形成确定顺序；最终只有最高 ordinal 的一个当前 Pending。

**Implementation:** `state_lock` 串行用户动作，仓储事务再用旧 attempt/version 和 next ordinal 防止过期写入。

## Scenario: 新材料生成失败

**Expected behavior:** 旧加入完全不变，没有新 ordinal、revision、attempt 或 outbox。

**Implementation:** 在持久事务前准备材料；只有全部材料通过本机验证后才构造原子 mutation。

## Scenario: 原子保存期间失败或进程崩溃

**Expected behavior:** 重启后看到完整旧状态或完整新状态，不存在中间状态。

**Implementation:** 旧重密封、新 key 创建、新密封、metadata 和状态保存全部位于同一个 SQLite transaction；失败触发回滚。

## Scenario: 原子提交成功但新请求发送失败

**Expected behavior:** 当前状态保持新 Pending，后台重发同一新 JoinRequest；旧加入不恢复为当前。

**Implementation:** 网络发送严格晚于提交，恢复扫描按新 attempt outbox 重放。

## Scenario: 被取代 attempt 收到迟到 Candidate 或拒绝

**Expected behavior:** 重放旧终止结果或完成旧清理，不改变新加入。

**Implementation:** 入口按旧 attempt_id 加载终态，所有保存限定旧记录，公开投影仍选择新 ordinal。

## Scenario: 被取代 attempt 收到有效 Commit 或更晚消息

**Expected behavior:** 进入稳定恢复错误，不接受消息、不应用历史、不切换 Space。

**Implementation:** 将其视为持久状态与远端证明矛盾，保存脱敏失败分类和必要防重放证据后停止自动推进。

## Scenario: 多个历史被取代记录仍有清理 outbox

**Expected behavior:** 它们可以独立重试，但不阻止当前安全阶段允许的下一次用户加入。

**Implementation:** 准入名额只由当前非终态 Joiner 和不可隔离 profile 工作占用；恢复调度按 attempt 隔离并有界推进。

## Scenario: 入站 Sponsor 准入、彻底重置或其他 profile 工作正在进行

**Expected behavior:** 返回既有稳定忙碌/不可用结果，不清除该工作，也不误报 1295。

**Implementation:** 1295 只用于当前本机 Joiner 已越过 Prepared；其他互斥由现有 profile 工作分类处理。

## Scenario: 旧版本数据和未知新终态

**Expected behavior:** 新版本读取旧数据不需迁移；旧版本遇到新终态时在任何写入前失败。

**Implementation:** 枚举只在末尾追加，解密后严格验证版本和变体，不使用默认值吞掉未知终态。

## Scenario: ordinal、revision 或 record_version 溢出

**Expected behavior:** 原子操作整体失败并保持旧状态。

**Implementation:** 全部计数使用 checked_add，任何溢出映射为损坏/恢复错误并回滚事务。

# 8. Testing Strategy

所有筛选命令先加 `-- --list`，记录实际匹配数量；匹配为零不得计为通过。测试日志不得输出邀请码、设备名、密钥、
完整 token、文件名或路径。

## Unit Test

### uc-core 状态不变条件

- 输入 Joiner Initiated/Candidate，执行 supersede 领域方法；预期阶段为 Superseded、终态为
  SupersededByNewJoin、rejection_reason 为空、旧提交前 outbox 停止、取消清理与防重放资料保留。
- 输入 Prepared/Committed/Applied、Sponsor、已有终态、带 prepared_proof 的矛盾 Candidate、带 write-ahead 或
  Space transition 的记录；预期全部拒绝且输入不变。
- 对旧枚举样本解码；预期现有判别值和字段不变。对新终态压缩/展开；预期身份和重放事实一致。

### uc-infra 原子仓储

- Create：预期新 ordinal、revision 和密文一次提交。
- Supersede：预期旧终态与新尝试同时存在，revision 只增加一，current projection 只返回新尝试。
- 旧版本、错误当前 attempt、错误 ordinal、Prepared、恢复工作、重复 attempt/join、计数溢出；预期事务回滚。
- 使用代表性秘密标记扫描数据库、WAL 和 SHM；预期不出现加入 payload、邀请码、密钥或设备资料明文。

### 应用与 Engine 映射

- `PreviousJoinCannotBeSuperseded` 从 convergence 到 redeem error 再到 Engine 1295 保持独立。
- Engine 结果为 Conflict/non-retryable，且与 1238、邀请不可用和邀请方拒绝编号不同。
- analytics 只记录固定失败类别；不得把内部错误正文或加入输入作为属性。

## Integration Test

至少新增并按精确名称运行下列测试：

| 测试 | 输入与操作 | 预期 |
| --- | --- | --- |
| `explicit_join_supersedes_initiated_attempt_atomically` | 第一次停在 Initiated，第二次使用不同邀请 | 新旧身份全不同，旧终态与新 Pending 同时提交，第二请求可发送 |
| `explicit_join_with_same_invitation_starts_new_attempt` | Initiated 后再次提交相同邀请 | 本机新 attempt；邀请方消费规则仍按原 attempt 判断 |
| `explicit_join_supersedes_candidate_before_prepared` | 旧加入推进到 Candidate 后再次提交 | 成功取代，不写成员历史或目标 Space 副作用 |
| `explicit_join_after_prepared_returns_stable_conflict` | 旧加入持久保存 Prepared 后再次提交 | 1295，旧 current_join 不变，无新编号、材料或网络请求 |
| `automatic_recovery_keeps_the_same_join_identity` | 每阶段断线、重启、丢 Ack | 同一 attempt/join/member/key material 恢复 |
| `superseded_late_candidate_cannot_replace_current_join` | 旧 Candidate 在新加入后迟到 | 只处理旧清理，新投影不变 |
| `superseded_late_commit_fails_closed` | 向被取代 attempt 注入有效 Commit | 无第二成员事实，进入稳定恢复错误 |
| `concurrent_explicit_joins_leave_one_current_attempt` | 两个公开加入并发 | 串行收敛，只有最高 ordinal 当前加入 |
| `supersession_failure_recovers_whole_old_or_new_state` | 在加密和保存边界逐点失败并重启 | 只出现完整旧状态或完整新状态 |

代表性精确命令在对应测试落地后执行：

```bash
cargo test -p uc-core admission_attempt
cargo test -p uc-infra commit_local_join_start
cargo test -p uc-application explicit_join_
cargo test -p uc-application superseded_late_
cargo test -p uc-engine previous_join_cannot_be_superseded
cargo test -p uc-engine-uniffi engine_errors_keep_their_stable_code_category_and_retryability
cargo test -p uc-ohos-napi previous_join_conflict_keeps_its_stable_summary
```

如果实际测试名与建议名不同，实施记录必须列出最终精确名称、`--list` 匹配数和通过数，不能只报告包编译成功。

### 2026-08-18 实施验证记录

所有筛选命令均先执行 `-- --list`。最终匹配数与通过数如下：

| 命令筛选 | 匹配数 | 通过数 |
| --- | ---: | ---: |
| `uc-core admission_attempt` | 3 | 3 |
| `uc-infra commit_local_join_start` | 1 | 1 |
| `uc-infra supersession_` | 2 | 2 |
| `uc-application explicit_join_` | 6 | 6 |
| `uc-application superseded_late_` | 6 | 6 |
| `uc-engine previous_join_cannot_be_superseded` | 1 | 1 |
| `uc-engine-uniffi engine_errors_keep_their_stable_code_category_and_retryability` | 1 | 1 |
| `uc-ohos-napi previous_join_conflict_keeps_its_stable_summary` | 1 | 1 |

完整工作区 `cargo test --workspace --all-targets --locked` 通过；其中 `uc-application` 通过 845 项、
`uc-core` 通过 264 项、`uc-infra` 通过 809 项、`uc-engine` 通过 196 项、UniFFI 绑定通过 44 项、
HarmonyOS 绑定通过 33 项。`uc-infra` 中 4 项原有手动性能测试保持忽略，未计为通过。

故障验证覆盖新资料生成失败、旧记录重新加密失败、新记录密钥生成失败、新记录加密失败、版本冲突、
数据库写入失败、提交后发送失败和重启恢复；每种情况都只恢复完整旧状态或完整新状态。

## Regression Test

- 保留并运行现有 `automatic_recovery_keeps_the_same_join_identity`、
  `durable_join_preparation_is_not_regenerated_after_restart`、取消与 Commit 竞争、乱序消息、终态压缩和邀请消费测试。
- Fresh、Same-Space、Cross-Space 各覆盖 Initiated/Candidate 取代和 Prepared 冲突。
- 运行完整工作区测试，并记录各包实际测试数；任何筛选为零都需修正命令或 feature 后重跑。
- 运行仓库强制检查：

```bash
cargo metadata --locked --format-version 1
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

## Device Acceptance

Android 真机至少使用一台加入方和一台邀请方：

1. 第一次加入在请求已保存后制造连接失败，确认加入方保留 Pending。
2. 用户退出并再次明确加入，可使用相同或不同新邀请。
3. 确认加入方得到新 join，邀请方实际收到第二个请求，流程可继续到 Active 或稳定邀请拒绝。
4. 确认旧请求迟到或重放不会覆盖新状态。
5. 采集脱敏时间线，只记录阶段、固定错误、计数和新旧身份是否不同，不记录邀请码或设备资料。

未执行的 iOS、HarmonyOS 或额外网络矩阵标为“跳过”。模拟器、单端日志或发送函数返回成功不算双端实体设备通过。

本次设备记录：

- Android 双端实体设备：跳过；当前仅连接一台 Android 实体设备，无法完成加入方与邀请方的双端流程。
- iOS 实体设备：跳过；当前没有在线 iPhone。
- HarmonyOS 实体设备及额外网络矩阵：跳过；本次未执行。

# 9. Acceptance Criteria

* [x] 每次公开 `JoinSpace` 都表示新用户动作；内部自动恢复继续原 attempt，二者没有共享入口语义。
* [x] 旧 Initiated/Candidate 可在一个事务中保存为 SupersededByNewJoin 并创建新 Initiated；不存在中间空窗或两个当前加入。
* [x] 相同邀请码和不同邀请码都创建新 attempt、join、ordinal、成员实例、KeyPackage、续接密钥和独立目标暂存身份。
* [x] Prepared 及以后再次加入返回 Engine 1295、Conflict、non-retryable；旧加入保持并继续恢复，新加入零持久和网络副作用。
* [x] 阶段与 prepared_proof、write-ahead、Space transition 或恢复资料矛盾时失败关闭，不允许按阶段数字放行。
* [x] 旧终态不使用 Rejected，不写 rejection_reason，不伪造邀请方裁决，也不写成员历史。
* [x] 被取代 attempt 的提交前消息和取消确认只处理旧清理；Commit 或后继消息稳定失败关闭且不产生第二成员事实。
* [x] 多个旧清理可与一个当前加入隔离恢复，current_join 始终只显示最高 ordinal 的新加入。
* [x] 邀请消费映射不改绑；同一邀请是否可再次成功完全由邀请方既有一次性规则决定。
* [x] Fresh、Same-Space 和 Cross-Space 在新加入完成既有边界前保持原活动 Space、身份、历史和本机资料。
* [x] 原子操作在每个可失败位置都只恢复完整旧状态或完整新状态，ordinal、revision 和 record_version 单调且无溢出。
* [x] 新终态、outbox、终态压缩和恢复资料全部保持密文，数据库/WAL/SHM 与日志不出现敏感明文。
* [x] `WorkspaceConvergence` 仍是唯一完整负责人；产品、绑定和 `uc-engine` 没有新增取消、重置或分步编排。
* [x] iOS、Android 和 HarmonyOS 绑定透传同一 1295/Conflict/false，公开 JoinSpace 成功结果和状态结构不变。
* [x] 所有新增和回归测试记录非零匹配数及通过数；仓库五项强制检查全部通过。
* [ ] Android 实体设备完成“第一次失败、第二次明确加入、邀请方收到第二请求”的双端验收；其他未执行项目明确记录为跳过。
* [x] 实施完成后删除旧公开重开路径和临时兼容分支，并同步更新规格 023、规格 025 状态及架构总览维护记录。

# 10. Risks and Trade-offs

| 风险或取舍 | 处理方式 |
| --- | --- |
| Prepared 之前取代可能给邀请方留下旧候选 | 旧本机立即终态，新加入不等待；复用隔离 CancelRequested 后台释放邀请方，全部按旧 attempt 路由。 |
| Prepared 边界较保守，用户可能暂时不能开始新加入 | 接受该限制；远端可能已正式提交时，继续恢复旧操作比创建第二成员事实更安全。 |
| 一个事务同时密封两份记录，写入成本略增 | 只发生在用户再次明确加入时；换取崩溃后无半份状态，成本可接受。 |
| 多个旧清理增加恢复扫描工作 | 调度按 attempt 隔离并有界；终态静止后压缩，不让旧清理占用当前加入投影。 |
| 复用 CancelRequested 容易被误解为公开取消 | 核心终态和 API 明确区分：Supersede 本机立即完成，CancelJoinSpace 等待邀请方裁决；只复用线上的清理通知。 |
| 追加枚举后旧二进制无法理解新终态 | 接受向前不兼容读取；旧二进制必须在写入前失败，新二进制向后读取旧资料。发布说明需覆盖降级限制。 |
| 新错误增加产品分支 | 只增加一个稳定冲突，三端共享同一编号和含义；产品无需理解内部阶段或执行恢复步骤。 |
| 生成新材料后事务因并发失效 | 丢弃未持久材料，重新读取权威状态；不得把这批材料用于另一 attempt 或自动重试旧判断。 |

未采用“先 Cancel/Reset 再 Join”，因为它让产品端承担失败恢复和顺序；未采用无条件覆盖，因为 Prepared 后可能已远端提交；
未采用新旧并行，因为 profile 只有一个当前加入和一个活动 Space；未继续按输入相同判断恢复，因为邀请码不是用户操作身份。

# 11. Open Questions

- 无阻塞问题。错误编号 1295、Prepared 安全边界、内部 SupersededByNewJoin 终态、单事务保存、复用
  CancelRequested 进行隔离清理以及三端透传规则均由本规格固定；实施中不得再次下放给产品端选择。
