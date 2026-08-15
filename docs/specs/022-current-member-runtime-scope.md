# 规格 022：当前成员运行范围统一派生

## 状态

- **状态**：已实施，真实双 profile 验收待执行
- **日期**：2026-08-13
- **修订**：2026-08-15，补充旧空间分批安全提升在当前历史起点建立后的有界继续条件和完成边界
- **后续约束**：规格 023 待实施；V2 AddDevice 对观察者需有 Applied 回执，本机作为加入方还需 Complete
  和 J3，才进入各自普通运行范围
- **问题基线**：`6ec8c35c21220c4b41b8be31f235c111b2226e73`
- **调查基线**：`5e6d073`
- **相关文档**：`docs/adr/020-membership-reconciliation-and-user-decisions.md`、
  `docs/specs/016-workspace-wide-convergence.md`、
  `docs/specs/021-device-trust-reconciliation-product-contract.md`、
  `docs/specs/023-durable-membership-proof-and-admission-activation.md`、
  `docs/architecture/architecture-bible.md`

# 1. Overview

ADR-020 已规定：签名成员历史中的本机已应用分支，是当前成员资格的唯一事实来源。其他设备提出
移除时，本机先保存事件；只有用户接受后，目标设备才退出本机当前分支。拒绝则保留当前分支并只
隔离相关设备关系。

本规格负责从已应用历史统一派生普通运行范围。规格 023 进一步规定：新 V2 AddDevice 即使已经正式写入，
发起方和第三方在取得加入方 Applied 回执及匹配安全状态前仍排除它；加入方本机还必须取得 Complete 并
完成 J3。该门禁只能减少历史已经授予的资格，不能成为第二份成员事实；本规格现有“已实施”结论不代表
规格 023 已完成。

当前实现虽然正确推进了成员历史和安全状态，但普通设备名单、可信关系和网络地址仍分别保存在
其他仓储中。设备列表、主动拨号、旧空间升级、成员发现、内容发送、活动状态恢复和部分网络接收端
又分别从这些仓储枚举或反查设备。接受远端移除时，两处“应用新历史事件”循环只处理新增设备资料
和安全更新，没有统一改变这些使用方看到的设备范围。

因此同一时刻可能出现以下结果：成员历史已排除目标，二次移除正确返回目标不存在，内容门禁也阻止
普通发送；但旧成员记录或地址仍使目标出现在列表、主动拨号和旧空间升级任务中。当前分支新增的
在线接纳确认能防止未获当前空间接纳的连接被报告为在线，但不能阻止错误候选被主动选择。

本规格不把修复定义为“在 RemoveDevice 分支删除若干记录”。它要求 `WorkspaceConvergence` 从本机
已接受历史一次生成完整的“当前成员运行范围”，所有普通公开列表、主动后台任务和普通网络授权都
消费该范围。成员、可信关系和地址记录只提供名称、历史身份或连接资料，存在本身不再授予当前资格。
已移除设备所需的历史验证和有界决定传递继续保留在受限成员历史流程内。

# 2. Goals

- 本机接受移除并保存后，目标立即退出所有普通公开列表和普通后台候选，无需等待记录清理或重启。
- 当前成员运行范围只从本机已接受成员历史派生；成员表、可信关系和地址表不能单独授予资格。
- 本地新增、本地移除、远端普通新增、远端移除接受、重复消息和重启恢复共用同一派生规则。
- 普通设备列表、主动连接、旧空间升级、成员发现、内容发送、活动状态恢复和文件相关任务使用同一份
  完整范围快照，不各自复制成员判断。
- 身份解析与当前授权明确分离：系统仍能识别已移除设备和验证历史，但不会因此允许普通工作。
- 已移除设备仍能通过受限成员历史通道收到或提交完成核对所需的有界决定；该例外不开放普通内容、
  当前成员、地址、安全资料或在线状态。
- 应用成员事件的安全效果和运行范围切换可重复执行；中途失败或进程退出后由同一负责人恢复。
- 旧版本遗留的成员、可信关系或地址记录不会在升级或重启后重新获得当前资格。
- 所有新增持久化状态继续使用 MasterKey AEAD 加密，日志不记录文件名、路径、内容或敏感身份资料。

# 3. Non-Goals

- 不改变 ADR-020 的接受、拒绝、分叉或重新加入规则。
- 不让第二次手动移除生成新事件，也不把目标不存在改成幂等成功。
- 不把在线状态、安全组接纳、可信关系或网络地址重新定义为成员资格。
- 不删除不可变成员历史、已完成决定、历史身份或验证旧事件所需的资料。
- 不要求产品端维护过滤列表、合并增量事件或编排后台停止顺序。
- 不修改桌面端 `retainedTrustPeers` 的显示修复；产品仍须按规格 021 排除 `membership=removed` 的普通
  操作入口。本规格保证 Engine 的公开事实和后台安全不依赖该产品修复。
- 不新增服务器、管理员、固定主设备、多数投票或自动分支合并。
- 不以本次改动顺带删除整个低于 `1.1` 的兼容支持；只收紧它的候选来源。
- 不为未来可能出现的设备类型或网络协议预先增加配置层。

# 4. Current Architecture Context

```text
Component: WorkspaceConvergence
Path: crates/uc-application/src/space/convergence/
Responsibility: 成员历史核对、用户决定、安全效果、重启恢复、查询和通知的唯一完整负责人。
Relationship: 已能从 applied_head 计算有效成员，但尚未向所有普通运行流程提供统一范围。
```

```text
Component: MembershipHistory / MembershipReconciliation
Path: crates/uc-core/src/membership/membership_history.rs
Responsibility: 保存不可变事件、known_head、applied_head、决定和有效成员计算规则。
Relationship: 是当前成员资格的唯一事实来源，不读取仓储、网络或在线状态。
```

```text
Component: Relationship repositories
Path: crates/uc-core/src/membership/ports.rs
Path: crates/uc-core/src/trusted_peer/ports.rs
Path: crates/uc-core/src/ports/peer_address.rs
Path: crates/uc-infra/src/db/repositories/relationship_store.rs
Responsibility: 加密保存设备资料、可信身份、地址和候选资料。
Relationship: 当前被多个流程直接枚举，形成并列的事实来源；底层已有一次事务保存多类关系的能力。
```

```text
Component: Ordinary peer consumers
Path: crates/uc-application/src/facade/roster/facade.rs
Path: crates/uc-application/src/space/convergence/reachability.rs
Path: crates/uc-application/src/space/convergence/membership_connectivity.rs
Path: crates/uc-application/src/space/convergence/legacy_upgrade.rs
Path: crates/uc-application/src/clipboard/sync/
Responsibility: 展示当前设备，主动拨号，运行升级、成员核对、内容发送和恢复任务。
Relationship: 分别从成员表、可信关系或地址表取得候选，再由部分路径追加不同门禁。
```

```text
Component: Network identity and admission adapters
Path: crates/uc-infra/src/network/iroh/
Path: crates/uc-infra/src/security/peer_admission_adapter.rs
Responsibility: 把网络身份解析为设备，并检查具体协议是否允许该设备进入。
Relationship: 身份解析可使用历史资料；普通协议授权不得仅因解析成功或旧安全资料仍存在而通过。
```

当前主要数据流为：

```text
签名事件到达或用户接受
  -> MembershipReconciliation 推进 applied_head
  -> 两处调用方各自遍历 newly_applied_events_after(...)
  -> AddDevice 保存成员、可信关系和地址
  -> 应用安全更新
  -> 保存 WorkspaceConvergenceState

普通使用方
  -> 各自遍历 member / trusted_peer / peer_address
  -> 部分路径再检查列表可见性、内容门禁或安全组接纳
```

问题不在单个判断结果，而在调用方必须知道“先从哪张表枚举，再加哪一道门”。接口复杂度已经接近
实现复杂度，且测试需要逐路径证明没有漏掉候选来源。

# 5. Proposed Design

## Components

### `CurrentWorkspacePeerScope`

- **职责**：由 `WorkspaceConvergence` 当前已保存状态生成一份不可变、稳定排序的当前成员运行范围。
- **输入**：本机已接受成员历史、显式旧空间模式和本机设备标识。
- **输出**：范围版本、来源模式、本机资格和当前对端设备标识集合。
- **关系**：作为 `WorkspaceConvergence` 内部深模块；调用方看不到事件、历史头、决定或仓储组合方式。

范围规则固定如下：

1. 已存在至少一个已应用成员历史事件时，只使用 `applied_head` 对应的有效成员；不得回退旧成员表。
2. 只有明确处于受支持的旧空间模式、且尚未建立当前成员历史时，才允许使用旧成员资料生成
   `Legacy` 普通范围，供原地升级初始化使用。当前历史起点建立后，普通范围立即切到
   `CurrentHistory`；持久升级记录中的保留成员只可在旧升级内部完成已经开始的同一次共同保护和历史接纳，
   不能借此进入普通范围。
3. 状态锁定、损坏、身份不匹配、读取失败或模式不明确时返回 `Unavailable`；所有普通调用方失败关闭。
4. 本机已被移除时，普通对端集合为空，且普通入站和出站全部拒绝。
5. 同一设备重新加入后只认新成员实例；旧实例和旧地址不能把设备提前加入范围。

### `AppliedMembershipEffectExecutor`

- **职责**：唯一处理刚进入本机已应用分支的事件效果，替代本地决定和远端接收中的两套循环。
- **输入**：前一已应用位置、当前已应用位置和按历史顺序排列的事件。
- **输出**：所有必要效果完成，或保存可恢复的未完成状态并返回明确失败。
- **关系**：只由 `WorkspaceConvergence` 调用；调用方不能逐事件保存资料、应用安全状态或发布成功。

事件规则：

| 事件 | 必须效果 | 不得效果 |
| --- | --- | --- |
| `AddDevice` | 验证并保存成员显示资料、历史身份和可用地址；应用绑定的安全更新 | 在效果未完成前将新增设备放入普通运行范围 |
| `RemoveDevice` | 立即从普通运行范围排除目标；应用绑定的安全更新；停止普通任务选择 | 删除成员历史、已完成决定或受限核对所需身份资料 |

### Ordinary peer consumers

- **职责**：取得一次完整范围快照，再与本流程需要的资料相交。
- **输入**：`CurrentWorkspacePeerSnapshot`，以及地址、偏好、在线观察等非资格资料。
- **输出**：本次调用的最终目标或公开列表。
- **关系**：地址缺失可以使当前成员本次不可连接；地址存在不能使非当前设备成为目标。

以下规则必须替换现有直接枚举语义：

| 使用方 | 范围的使用方式 |
| --- | --- |
| 普通设备和连接列表 | 只展示范围内设备；名称从历史准入资料或成员资料补充 |
| 主动在线探测和 keepalive | 只拨号“当前范围 ∩ 有地址” |
| 成员连通与当前历史核对 | 只自动拨号当前范围；受限决定传递使用独立计划 |
| 旧空间升级 | 初始化使用 `Legacy` 范围；建立当前历史起点后，普通范围立即改用当前历史，未完成的同一次安全提升只处理持久升级记录中明确等待重新接纳的成员；该有界升级范围不授予普通资格 |
| 成员候选发现 | 当前成员可作为资料来源；候选本身仍不获得成员资格 |
| 普通内容、活动状态和恢复广播 | 先限制在当前范围，再应用关系、版本、设置和内容偏好规则 |
| 手动重发 | 用户目标必须位于当前范围；可信关系存在不能绕过该要求 |
| 文件和进度协议 | 普通传输只接受当前范围设备；既有会话终结按传输规格处理，不恢复新权限 |

### Restricted membership-history delivery

- **职责**：传递历史核对完成所需的有界事件和决定，包括向刚被移除的目标发送接受或拒绝结果。
- **输入**：由 `WorkspaceConvergence` 在应用决定前确定并保存的精确接收计划。
- **输出**：有界接收结果；失败后按同一计划重试或等待明确恢复条件。
- **关系**：不使用普通运行范围授予权限，也不进入普通在线、升级、内容或发现流程。

受限计划只能携带完成投递所需的设备标识、历史位置和经过验证的路由引用。若需要持久化新的计划或
路由资料，必须作为 MasterKey AEAD 密文保存。受限接收方不得获得当前成员列表、普通地址目录、内容、
密钥或可区分的在线状态。

## Data Model

### `CurrentWorkspacePeerSnapshot`

这是应用层内部只读值，不新增公开 Engine 字段，也不单独落库。

| 字段 | 含义 |
| --- | --- |
| `revision` | 生成快照的工作空间状态版本，用于同一操作内保持一致读取 |
| `source` | `CurrentHistory`、`Legacy` 或 `Unavailable` |
| `local_membership` | `Active`、`Removed` 或 `Unavailable` |
| `peer_device_ids` | 当前普通运行允许考虑的非本机设备，去重并按设备标识排序 |

快照只回答成员范围，不同时编码在线、分叉、版本兼容、内容偏好或地址可用性。后续流程可以收窄范围，
但不得扩大范围。

### `PendingAppliedMembershipEffects`

为跨安全状态和加密关系资料的中断恢复，在工作空间加密状态中保存有界的未完成效果：

| 字段 | 含义 |
| --- | --- |
| `from_applied_head` | 本轮开始前已完成效果的位置 |
| `target_applied_head` | 本轮需要完成到的位置 |
| `next_event_id` | 下一项需要执行或核对的事件 |
| `completed_effects` | 已确认完成的效果类别，不保存业务明文 |

每项效果必须以事件编号和安全更新摘要实现幂等。新增设备在其必要效果完成前不得进入普通范围；移除
设备从移除被本机接受并持久化起立即退出普通范围。存在无法确认的未完成效果时，相关普通交换失败
关闭，负责人继续恢复；不得发布“决定已完整完成”或“历史已应用”的成功结果。

完成后清除未完成效果。最近已完成决定继续按规格 021 保留，不由本结构替代。

## API / Interface

在 `uc-core` 的成员端口中定义内部接口，供应用层和基础设施适配器依赖：

```text
CurrentWorkspacePeerScopePort
  snapshot() -> Result<CurrentWorkspacePeerSnapshot, CurrentWorkspacePeerScopeError>
```

接口只有一个完整查询。调用方不得先查成员、再查模式、最后查移除门禁来拼装范围。

稳定错误至少区分：

- `Locked`：当前无法读取加密状态；
- `Unavailable`：负责人尚未建立可判断状态；
- `Corrupt`：历史、身份或持久化资料无法验证。

对产品公开的操作和结果不新增步骤：`QueryPeerConnections`、设备信任查询、移除和决定仍使用现有
Engine 入口。内部错误按现有稳定类别映射；列表和后台任务不能在错误时返回未过滤旧记录。

现有 `ContentExchangeGatePort` 继续表达待决定、分叉、无效和版本等普通内容关系限制，但应改为先验证
设备位于当前范围。`DeviceVisibilityGatePort` 在所有调用方迁移后删除，避免同一成员规则同时存在为
“范围查询”和“隐藏判断”。低层 `PeerAdmissionPort` 继续验证安全组接纳，但不能替代当前范围授权。

## Workflow

### 接受远端移除

1. `WorkspaceConvergence` 在状态锁内验证当前待决定项和用户选择。
2. 在决定前的历史中确定发起者、移除目标和受限决定接收计划。
3. 保存签名决定、推进本机已应用历史，并保存未完成效果状态。
4. 从这一保存点起，当前运行范围立即排除目标；所有普通列表和任务读取新范围。
5. 效果执行器按历史顺序应用安全更新并核对必要资料效果。
6. 效果全部完成后清除未完成状态，保存并发布完整结果。
7. 锁外按已保存受限计划发送决定；失败不恢复目标的普通资格，由负责人按受限规则重试。

### 接收并自动应用普通新增

1. 验证并保存连续签名事件，建立未完成效果状态。
2. 在新增成员资料和安全效果完成前，不把新增设备加入普通运行范围。
3. 效果执行器保存资料、应用安全更新并核对结果。
4. 全部完成后推进可公开的当前范围，保存并发布一次完整变化。

### 启动和重启

1. 加载加密工作空间状态，不读取旧表猜测模式。
2. 若存在未完成效果，先按事件编号恢复；普通流程对相关设备失败关闭。
3. 生成当前范围快照并启动各空间运行期。
4. 可选整理旧成员、可信关系和地址资料，但整理失败不得扩大范围或阻止后续再次整理。
5. 明确旧空间模式且当前历史为空时，旧升级运行期取得 `Legacy` 范围；若当前历史起点已建立但同一次提升
   尚未覆盖全部保留成员，运行期从持久升级记录继续有界接纳和历史交接。重启不得重新按设备编号选择负责人，
   也不得把该升级范围提供给普通流程。

### 普通候选选择

1. 使用方在一次操作开始时取得完整范围快照。
2. 读取本流程需要的地址、名称、在线观察或偏好。
3. 只对范围中的设备做交集、排序和进一步限制。
4. 快照不可用时，本次不选择任何普通目标，并返回或记录稳定的可重试失败。

# 6. Implementation Plan

## Step 1: 建立失败测试和使用方清单

**File:** `crates/uc-application/src/space/convergence/tests.rs`
**File:** `crates/uc-engine/tests/space_membership_auto_pairing_e2e.rs`
**Change:** 先增加接受移除后当前范围、公开列表、主动拨号和旧升级均排除目标的失败测试；用仓库搜索
固定所有 `member_repo.list()`、`trusted_peer_repo.list()` 和 `peer_addr_repo.list()` 的生产枚举点。
**Risk:** 过滤测试名称可能运行 0 个；必须先 `--list` 并确认预期测试数量。

## Step 2: 增加核心快照值和单接口

**File:** `crates/uc-core/src/membership/ports.rs`
**File:** `crates/uc-core/src/membership/mod.rs`
**Change:** 增加 `CurrentWorkspacePeerSnapshot`、来源和错误类型，以及只有 `snapshot()` 的端口；增加排序、
本机排除、移除本机、旧模式和不可用测试。
**Risk:** 不得把在线、关系或地址加入快照，避免形成新的综合状态对象。

## Step 3: 在唯一负责人中派生范围

**File:** `crates/uc-application/src/space/convergence/mod.rs`
**Change:** 由已应用成员历史实现当前范围；明确旧模式判断；删除“读取失败回退旧成员表”的可能路径。
**Risk:** 首次旧空间升级仍需要旧范围，必须通过明确模式测试保留，而不是用历史为空推断。

## Step 4: 合并成员事件效果执行

**File:** `crates/uc-core/src/membership/workspace_convergence.rs`
**File:** `crates/uc-application/src/space/convergence/mod.rs`
**File:** `crates/uc-infra/src/db/repositories/workspace_convergence_store.rs`
**Change:** 增加加密保存的未完成效果状态；把本地决定和远端事件两处循环替换为一个按序、幂等、可恢复
的执行入口；重复事件不得重复产生决定或非幂等效果。
**Risk:** 序列化格式变化需验证旧密文状态升级读取；安全更新重复应用必须以现有代次和摘要核对。

## Step 5: 迁移普通公开列表和应用层候选

**File:** `crates/uc-application/src/facade/roster/facade.rs`
**File:** `crates/uc-application/src/facade/space_setup/facade.rs`
**File:** `crates/uc-application/src/space/convergence/reachability.rs`
**File:** `crates/uc-application/src/space/convergence/membership_connectivity.rs`
**File:** `crates/uc-application/src/space/convergence/legacy_upgrade.rs`
**File:** `crates/uc-application/src/clipboard/sync/`
**Change:** 每个流程先取得范围快照，再与资料仓储相交；删除直接把某个仓储列表解释为当前成员的注释和
逻辑。
**Risk:** 手动重发和旧空间升级容易被遗漏；必须用生产调用点清单逐项关闭。

## Step 6: 收紧网络身份解析与授权

**File:** `crates/uc-infra/src/network/iroh/`
**File:** `crates/uc-infra/src/security/peer_admission_adapter.rs`
**Change:** 保留历史身份解析，但普通协议在解析后必须检查当前范围和自身关系门禁；受限成员历史通道
使用独立授权，不向普通协议复用例外。
**Risk:** 不能因删除旧身份资料导致历史决定无法送达，也不能因身份可解析而恢复普通权限。

## Step 7: 删除重复成员可见性规则

**File:** `crates/uc-core/src/membership/ports.rs`
**File:** `crates/uc-application/src/space/convergence/assembly.rs`
**Change:** 所有消费者迁移后删除 `DeviceVisibilityGatePort` 及生产组装入口；保留内容关系限制和安全接纳
各自独立职责。
**Risk:** 不得在旧接口旁长期保留新接口，否则继续存在两套判断。

## Step 8: 旧数据校正与架构检查

**File:** `crates/uc-infra/src/db/repositories/relationship_store.rs`
**File:** `scripts/architecture/check-engine-repository.mjs`
**Change:** 在不删除历史的前提下提供幂等整理；增加检查，禁止普通候选选择直接把成员、可信关系或地址
列表当成当前范围。
**Risk:** 整理只能回收无普通用途的资料，不能删除受限核对仍依赖的历史身份或路由。

## Step 9: 同步正式文档和产品交接

**File:** `docs/architecture/architecture-bible.md`
**File:** `docs/specs/021-device-trust-reconciliation-product-contract.md`
**File:** desktop 对应设备列表接入文档
**Change:** 记录当前范围、历史身份和受限通信边界；产品端明确不把 removed 历史关系补回普通列表。
**Risk:** Engine 修复不能以产品过滤完成为前提。

# 7. Edge Cases

### Scenario: 接受移除后进程在安全效果执行前退出
**Expected behavior:** 目标已经退出普通范围；重启后继续同一效果，不恢复目标，也不重复决定。
**Implementation:** 从加密保存的未完成效果按事件编号恢复，相关普通交换保持关闭。

### Scenario: 安全效果完成后、完成标记保存前退出
**Expected behavior:** 重启核对代次和摘要后将效果视为已完成，不重复推进或报损坏。
**Implementation:** 每项效果以事件编号、前后代次和摘要实现幂等核对。

### Scenario: 同一移除决定重复送达
**Expected behavior:** 当前范围不变化，不产生新事件；相同决定返回已完成，相反决定返回状态已变化。
**Implementation:** 复用规格 021 的完成决定记录，效果执行器按事件去重。

### Scenario: 一批事件中 AddDevice 后紧跟 RemoveDevice
**Expected behavior:** 最终范围不含目标；中间状态不向普通使用方发布。
**Implementation:** 在状态锁内计算整批目标，按序完成效果后发布一个完整快照。

### Scenario: 用户拒绝移除
**Expected behavior:** 目标继续位于本机当前范围；仅与提议方的关系进入分叉，相关普通内容按关系门禁暂停。
**Implementation:** 不运行移除效果，范围继续从未前进的 applied_head 派生。

### Scenario: 已移除设备重新加入
**Expected behavior:** 只有新成员实例的新增效果完成后才重新进入范围；旧实例资料不能提前恢复资格。
**Implementation:** 范围按有效成员实例映射设备，并以新准入事件作为唯一加入依据。

### Scenario: 地址存在但设备不在当前范围
**Expected behavior:** 不拨号、不展示、不发送内容、不运行升级。
**Implementation:** 所有普通枚举先取范围，再与地址相交。

### Scenario: 当前成员缺少地址
**Expected behavior:** 仍是当前成员并可出现在设备事实查询中，但本次连接目标为空或报告地址不可用。
**Implementation:** 地址只影响可连接性，不改变成员资格。

### Scenario: 历史状态损坏但旧成员表完整
**Expected behavior:** 返回不可验证并停止普通流程，绝不回退旧成员表。
**Implementation:** `snapshot()` 返回 `Corrupt`，调用方失败关闭。

### Scenario: 明确的旧空间正在分批安全提升
**Expected behavior:** 当前历史为空时只允许旧升级流程使用 `Legacy` 范围；建立当前历史起点后，普通流程永久
切换为 `CurrentHistory`，但持久升级记录中的保留成员仍可完成同一次共同保护和历史接纳。共同保护状态与
已应用历史都覆盖全部保留成员后，旧升级范围永久停止。
**Implementation:** 模式与保留成员来自持久升级状态，不由历史读取失败、空表、设备编号或旧关系表临时猜测；
当前历史存在时，旧成员资料不生成普通范围。

### Scenario: 已移除目标需要收到本机决定
**Expected behavior:** 普通范围已排除目标，但受限成员历史计划仍可投递有界决定。
**Implementation:** 决定前保存精确受限计划，独立授权和重试，不重新加入普通候选。

### Scenario: 关系资料整理失败
**Expected behavior:** 普通行为仍正确；后续启动或唤醒再次整理，不显示成功清理日志中的敏感资料。
**Implementation:** 正确性依赖范围，不依赖物理删除；整理幂等且只记录数量和稳定类别。

# 8. Testing Strategy

## Unit Test

1. 输入包含 A、B、C 的历史，应用 A 移除 B：快照只返回 C，且 B 的历史准入资料仍可查询。
2. known_head 含移除但 applied_head 尚未接受：快照仍包含目标。
3. 拒绝移除：快照不变，关系为分叉。
4. 接受移除：保存点后立即排除目标，重复接受结果不变。
5. 当前历史存在但读取资料仓储失败：不得产生 `Legacy` 范围。
6. 明确旧模式且历史为空：只生成 `Legacy` 普通范围；切换当前历史后，普通范围不再读取旧资料，但未完成的
   同一次安全提升仍按持久升级记录接纳保留成员，直到共同保护状态和已应用历史都覆盖这些成员。
7. 新增效果未完成：新设备不进入范围；移除效果未完成：被移除设备仍立即退出。
8. 效果完成标记丢失后重启：核对既有效果并完成，不重复非幂等操作。

## Integration Test

1. 接受远端移除后，成员、可信关系和地址仍保留旧测试资料时，普通设备列表和连接查询都不返回目标。
2. 主动在线刷新、keepalive、成员连通和旧升级各运行一轮，记录的目标均不含已移除设备。
3. 自动发送、手动重发、活动状态上线补发和恢复广播均不能选择已移除设备。
4. 普通入站协议中，已移除设备即使身份可解析也被拒绝；历史决定通道仍可交换对应有界决定。
5. AddDevice 后紧跟 RemoveDevice 的一批事件只发布最终范围。
6. 注入成员资料保存、安全效果和完成标记各阶段失败，重启后最终范围和安全状态一致。
7. `QueryPeerConnections`、普通成员摘要和设备信任查询对同一设备的当前资格一致；设备信任查询可额外保留
   `Removed` 历史关系，但不提供普通移除动作。

## Regression Test

1. 待决定移除期间目标仍是本机分支成员，普通内容只按待决定关系暂停，不提前删除成员。
2. 拒绝后本机分支内部设备继续工作，只有分叉关系被隔离。
3. 低于 `1.1` 的明确旧空间仍能分批完成原地升级；当前历史设备只对持久记录中的保留成员继续同一次安全提升，
   不向普通流程开放旧成员范围。
4. 当前成员地址暂缺不会从成员事实查询消失，也不会被误报为已移除。
5. 被移除设备用新实例重新加入后恢复普通列表和双向内容，旧实例不恢复。
6. 当前 presence 接纳确认继续要求安全组接纳，不能因范围检查替代而放宽。

## Multi-device Acceptance

使用独立目录和真实 P2P 运行 d、f 两个 profile，并在需要时加入第三台保留设备：

1. d 的移除事件到达 f，f 选择应用变化。
2. 决定返回后不重启，d 立即从 f 的普通设备列表和连接查询消失。
3. d 再上线，f 不对 d 运行普通 presence、旧升级、成员发现、内容或恢复发送。
4. f 仍能在设备信任历史中看到 d 为 `Removed`，且没有普通移除动作。
5. 受限决定传递完成，d 不能从该通道取得普通资料。
6. f 重启后结果保持一致；旧记录不能恢复 d。
7. d 以新成员实例重新加入后，完成准入前不进入范围，完成后恢复普通双向内容。

每个后续会参与断言的设备都必须先等待其公开成员事实达到预期。拨号成功、在线、发送计数或发起方
返回不能代替接收方保存和应用证明。

# 9. Acceptance Criteria

* [x] `WorkspaceConvergence` 是当前成员运行范围的唯一负责人。
* [x] 当前历史存在时，任何失败都不会回退成员表、可信关系或地址表生成范围。
* [x] 本机接受移除并保存后，目标立即退出所有普通公开列表和普通后台候选。
* [x] 待决定移除不会提前移除目标；拒绝不会改变本机当前范围。
* [x] 所有普通候选来源都先使用同一完整范围快照，地址、可信关系和在线状态只能收窄结果。
* [x] `DeviceVisibilityGatePort` 在迁移完成后删除，不保留新旧两套成员判断。
* [x] 内容关系限制和安全组接纳保持独立，但都不能把非当前设备放回普通范围。
* [x] 身份解析成功不等于授权成功，所有普通入站协议均有当前范围验证。
* [x] 已移除设备仍可完成受限成员历史决定交换，但不能取得普通内容或当前资料。
* [x] 新增和移除的事件效果由同一入口按序、幂等并可跨重启恢复。
* [x] AddDevice 后紧跟 RemoveDevice 不发布中间普通范围。
* [x] 旧版本残留关系记录在启动后不能恢复任何普通资格。
* [x] 当前历史起点建立后普通范围立即改用当前历史；旧空间安全提升独立使用持久待重新接纳记录继续，不再由旧迁移标记或成员资料表控制。
* [x] `QueryPeerConnections` 和其他当前成员列表不返回 removed 设备。
* [x] 设备信任查询可保留 removed 历史关系，并且不提供普通移除动作。
* [x] 精确单元和集成测试实际运行且确认预期数量，不以 0 tests 作为通过。
* [ ] d、f 真实 profile 的即时、重新上线、重启和重新加入验收均有接收端证据。
* [ ] 规格 023 实施后，其他观察者缺 Applied、加入方本机缺 Complete/J3 时，V2 AddDevice 仍不进入对应
      本机的普通运行范围。
* [x] 新增持久化业务资料使用 MasterKey AEAD，生产日志不包含文件名、路径或敏感身份资料。
* [x] 架构检查禁止新增把成员、可信关系或地址列表直接解释为当前范围的生产路径。

# 10. Risks and Trade-offs

## 读取成本

当前范围需要读取加密工作空间状态，但个人设备规模明确较小。一次操作读取一个完整快照，随后在内存中
完成交集，比多个调用方分别查询和判断更可控。若实测需要缓存，只能在负责人内部按 `revision` 缓存；
缓存不是持久化事实，状态变化、锁定和空间切换必须使其失效。

## 历史资料保留

保留旧身份或路由资料会占用少量加密存储，但能支持历史验证和受限决定传递。代价优于立即删除后再
建立旁路恢复资料。普通正确性不依赖这些资料被物理删除。

## 未完成效果状态

增加恢复标记会扩大工作空间保存结构，但它把跨安全状态和关系仓储的失败变成显式、可测试状态。
替代方案是要求所有存储共享一次数据库事务；安全状态并非同一关系事务，无法提供真实原子性。

## 接口替换范围

迁移会触及多个普通使用方，但这是消除知识分散的必要范围。只在每个 worker 增加局部过滤改动更小，
却会永久保留遗漏风险，且无法通过一个测试面证明完整行为，因此不采用。

## 旧空间兼容

旧空间在当前历史建立前需要读取旧成员资料；建立历史起点后，尚未完成的同一次安全提升还需要从持久升级
记录取得明确等待重新接纳的成员。两种情况都由显式状态进入：前者只提供升级初始化所需的 `Legacy` 普通范围，
后者只提供有界的升级接纳范围。普通流程一旦切到当前历史不得回退；待重新接纳记录清空后，升级接纳范围自然停止。

## Alternative: 删除三类关系记录

接受移除时直接删除成员、可信关系和地址能修复部分现象，但无法保证跨存储中断一致，也可能破坏历史
身份解析和受限决定投递。即使实施幂等整理，也只能作为空间回收措施，不能作为授权机制。

## Alternative: 保留现有多个门禁

继续扩展内容门禁、列表隐藏和安全接纳，会要求每个新流程了解多项组合规则。删除任一门禁后复杂度只会
散落到调用方，不满足完整负责人的要求，因此用一个范围快照替换成员可见性判断。

# 11. Open Questions

当前没有阻塞本规格实施的产品问题。实施前只需用代码和测试确认以下工程事实，不得改变设计方向：

1. 受限成员历史发送当前如何解析刚被移除目标的路由；若依赖普通地址枚举，需在同一实施中改为保存
   精确受限计划或使用历史路由解析。
2. 现有安全更新适配器对“效果已完成、完成标记尚未保存”的重复调用能否按代次和摘要确认；不足时需
   在适配器内补齐幂等核对。
3. 哪些旧关系资料在所有受限计划完成后已无验证用途；只有证明无用途的资料才进入整理删除范围。

这些问题决定具体适配方式和可回收范围，不授权恢复第二套成员资格来源，也不允许推迟普通范围统一。
