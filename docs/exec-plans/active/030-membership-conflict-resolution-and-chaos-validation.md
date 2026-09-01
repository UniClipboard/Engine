# 规格 030：成员分叉选择与复杂拓扑验证

## 状态

- **状态**：设计中
- **日期**：2026-08-30
- **前置规格**：`docs/exec-plans/active/029-durable-membership-history-anti-entropy.md`
- **相关决策**：`docs/design-docs/decisions/020-membership-reconciliation-and-user-decisions.md`、`docs/design-docs/decisions/021-workspace-convergence-internal-boundaries.md`

# 1. Overview

规格 029 已证明合法单父历史能够跨四节点离线链和五节点树型传播，但尚未验证真实分叉。当前系统能够识别
`Diverged`、隔离双方内容与历史，并允许用户接受或拒绝远端移除；它没有一个完整入口让用户在已经形成的
任意分叉中选择目标分支，并让本机可恢复地切换过去。

本规格补齐两个结果：

1. Application 唯一负责“展示可选分支、接受一次用户选择、恢复或重新配对、原子切换、重启续跑”；产品端
   不编排内部步骤。
2. Desktop CLI E2E 建立可复现的复杂拓扑与故障脚本，验证冲突隔离、相反选择、最终归队、安全状态和正文
   传输，而不只验证成员数量。

分叉历史禁止合并。用户选择的含义是选择一个完整目标分支；其他分支通过 generation transition 放弃旧运行
资格并切换到目标状态。系统不得按时间、成员数、在线数、设备标识或消息到达顺序自动选择赢家。

# 2. Goals

- 对共享祖先后的 sibling 历史返回稳定冲突及可选分支摘要。
- 提供一个公开的 `resolve_membership_conflict` 动作，调用方只提交冲突编号和目标分支编号。
- 本机仍是目标分支有效成员时，使用成员凭据恢复目标历史与安全状态，不新增成员事件。
- 本机已被目标分支移除时，稳定返回 `RePairingRequired`，只能通过目标分支的新邀请产生新成员实例。
- 分支切换在进程崩溃、网络失败和重复调用后只向前恢复，不出现混合历史、混合密钥或双重活动 generation。
- 相反选择保持两个隔离分支；只有后续明确选择同一目标才收敛。
- 用确定性复杂拓扑覆盖分叉、乱序、离线、重启、重复、阶段故障和最终数据面互通。

# 3. Non-Goals

- 不自动合并 sibling 历史、MLS commits 或内容密钥目录。
- 不实现多数投票、选主、最后写入获胜或云端仲裁。
- 不让一个设备替其他设备作出分支选择。
- 不通过 LAN compatibility 绕过 P2P、成员历史或安全门禁。
- 不把随机压力测试的单次成功当作验收证据。
- 不在冲突解决中删除剪贴板历史；不可重封装的记录按现有跨 Space 迁移规则保留为不可读密文。

# 4. Current Architecture Context

```text
Component: VersionedMembershipHistory
Path: crates/uc-core/src/membership/versioned_membership_history.rs
Responsibility: 验证单父历史、祖先关系、移除决定和 Diverged。
Relationship: 继续作为分支真实性和成员资格的唯一事实来源；不执行切换。
```

```text
Component: MembershipLedger
Path: crates/uc-application/src/space/membership/ledger/
Responsibility: 原子保存当前历史、peer 关系、effects、入站页和 revision。
Relationship: 冲突记录与选择 intent 必须进入同一加密提交边界。
```

```text
Component: DecideDeviceTrustChangeUseCase
Path: crates/uc-application/src/space/membership/decide_device_trust_change/
Responsibility: 接受或拒绝一个待决定移除。
Relationship: 不是通用分叉选择入口；拒绝移除后只建立 Diverged。
```

```text
Component: Cross-Space transition
Path: crates/uc-application/src/space/admission/space_transition/
Responsibility: 备份来源、准备目标 generation、重封装资料并原子提升目标。
Relationship: 新分支切换复用其事务语义，但同 lineage 分叉不能伪装成普通 SameSpace join。
```

```text
Component: MembershipHistoryAntiEntropy
Path: crates/uc-application/src/space/membership/synchronize_history/ 和 handle_history_message/
Responsibility: 合法单父分支内最终传播。
Relationship: Diverged 时继续失败关闭；只有显式 conflict-resolution capability 可读取候选分支。
```

当前缺口是：`QueryDeviceTrust` 能展示 `Diverged`，`JoinSpace` 能执行普通准入，`FactoryResetSpace` 能彻底清理，
但没有模块拥有“验证目标分支并可恢复地切换”的完整结果。

# 5. Proposed Design

## Components

### `MembershipConflictResolution`

- **位置**：`crates/uc-application/src/space/membership/resolve_conflict/`
- **职责**：唯一负责查询冲突选项、验证用户选择、取得目标分支恢复资料、保存 transition intent、切换 generation、
  恢复 effects/runtime 并返回稳定结果。
- **输入**：`conflict_id`、`target_branch_id`。
- **输出**：`Completed`、`Pending`、`RePairingRequired`、`StateChanged` 或稳定错误。
- **调用方唯一动作**：调用一次 `resolve_membership_conflict`；Pending 后只查询状态，不推进内部步骤。

删除检查：若删除该模块，分支验证、网络交换、来源备份、安全恢复、generation 提升和重启恢复会重新散落到
Facade、Engine 和产品端，因此该模块必须存在；Engine 只组装和映射结果。

### `MembershipConflictPolicy`

- **位置**：`crates/uc-core/src/membership/`
- **职责**：从两条已验证历史产生稳定冲突编号、分支编号、可选择性和本机在目标分支的资格。
- **输入**：共享祖先、两个 head、两条历史中的本机成员实例状态。
- **输出**：`ActiveMemberRecovery`、`RePairingRequired`、`AlreadySelected` 或 `InvalidConflict`。
- **限制**：只做纯规则，不读取网络、数据库或 generation。

### `MembershipBranchRecoveryPort`

- **所有者**：Application `resolve_conflict`。
- **实现位置**：`uc-infra` 的 Iroh adapter 与安全 adapter。
- **职责**：在显式用户授权后，从目标分支的当前有效成员取得有界、签名且与 target head 绑定的恢复包。
- **限制**：普通反熵通道仍禁止 Diverged 双方交换完整历史；恢复包不能携带明文业务负载。

### `MembershipBranchTransitionPort`

- **所有者**：Application `resolve_conflict`。
- **职责**：复用跨 Space transition 的备份、重封装、manifest 提升和恢复能力，执行同 lineage、不同 branch 的
  generation 切换。
- **边界**：只能接收 Application 已验证的完整计划，Infra 不重新选择分支。

## Data Model

### `MembershipConflictRecord`

随 membership ledger 整体加密：

| 字段 | 含义 |
| --- | --- |
| `conflict_id` | lineage、共同祖先和排序后两个 branch id 的领域摘要 |
| `local_branch_id` | 本机当前已应用 head 的稳定摘要 |
| `remote_branch_id` | 已验证对端 head 的稳定摘要 |
| `remote_peer_device_id` | 与该冲突关联的认证 peer；敏感，不得记录日志 |
| `detected_at_revision` | 首次原子保存冲突时的 ledger revision |
| `status` | `Unresolved`、`Selected`、`Transitioning`、`Completed`、`RePairingRequired` |
| `selected_branch_id` | 用户已选目标；选择后不可被后台任务改写 |
| `transition_id` | branch transition 的幂等编号 |

同一组 branch heads 无论从哪个 peer、何种顺序到达，都产生同一个 `conflict_id`。同一 remote branch 经多个 peer
观察只增加证据来源，不重复提示。

### `MembershipBranchRecoveryPackageV1`

包含目标完整签名历史、目标位置、目标成员签署的恢复授权、接收方成员凭据引用、MLS 恢复材料及内容密钥目录
密文。包绑定 `conflict_id`、`target_branch_id`、接收设备身份、过期时间和一次性 nonce。任何字段不一致都稳定
拒绝，不产生 transition 副作用。

### `MembershipBranchTransitionV1`

独立加密保存，阶段为：

`Prepared -> SourceBackedUp -> TargetVerified -> TargetStaged -> Promoted -> RuntimeRestored -> Completed`

每个阶段幂等且只前进。活动 manifest 在 `Promoted` 前始终指向来源 generation，在 `Promoted` 后始终指向完整目标
generation；不允许原地覆盖来源数据库或密钥。

## API / Interface

```rust
pub struct ResolveMembershipConflictInput {
    pub conflict_id: MembershipConflictId,
    pub target_branch_id: MembershipBranchId,
}

pub enum ResolveMembershipConflictResult {
    Completed { status: DeviceTrustStatus },
    Pending { conflict_id: MembershipConflictId },
    RePairingRequired { conflict_id: MembershipConflictId },
    AlreadyCompleted { status: DeviceTrustStatus },
    StateChanged { current_conflict_id: Option<MembershipConflictId> },
}
```

稳定错误至少区分 `Locked`、`InvalidChoice`、`TargetUnavailable`、`RecoveryRequired`、`CommittedButPending`。依赖失败
必须保留 source chain；错误和 Debug 不包含设备标识、branch/head、路径、凭据或恢复包正文。

`QueryDeviceTrust` 增加可选 `current_conflict`，只返回冲突编号、两个可选 branch id、本机是否位于各分支以及每个
选项是 `Recoverable` 还是 `RePairingRequired`。产品不得从设备数量或在线状态自行生成选项。

## Workflow

### 发现冲突

1. 反熵验证共享祖先但 heads 互不为祖先。
2. Core 生成稳定冲突和分支编号。
3. MembershipLedger 在同一 mutation 保存 `Diverged` 关系与 `MembershipConflictRecord::Unresolved`。
4. 普通历史、内容、群组更新继续隔离；查询发布新 revision。

### 保留本机分支

1. 用户选择 `local_branch_id`。
2. Application 原子保存 `Selected/Completed`；本机历史和安全状态不变。
3. 对端仍为 Diverged。该动作只确认本机选择，不宣称全 Space 已收敛。
4. 其他设备若要归队，必须在各自设备上明确选择该 branch。

### 采用对端分支且本机仍有效

1. 用户选择 `remote_branch_id`，Application 保存不可变 intent。
2. 通过专用恢复 capability 获取并验证目标包；普通反熵门禁不放宽。
3. transition 备份来源 generation，验证目标历史包含本机当前成员实例且为 Active。
4. 顺序恢复目标 MLS 状态和内容密钥目录，重封装需保留的本机数据并 stage 完整目标 generation。
5. 原子提升 manifest，重装 session/runtime，重新运行 membership effects 和反熵。
6. 提升后失败由同一 transition 向前恢复，不回切旧 generation。

### 采用的分支已移除本机

1. Core 返回 `RePairingRequired`，不得安装目标安全材料或把本机伪装为 Active。
2. 产品展示需要目标分支成员签发新邀请。
3. 用户执行现有 JoinSpace；来源按 cross-space 语义备份，即使 lineage 相同也使用新 generation。
4. 目标分支产生新的成员实例。旧实例保持 Removed，不复活、不复用旧凭据。

# 6. Implementation Plan

```text
Step 1
Files: crates/uc-core/src/membership/*
Change: 增加稳定 conflict/branch id、选择资格和转换矩阵；先写 sibling history 与顺序无关测试。
Risk: 摘要规范不稳定会让同一冲突重复出现。
```

```text
Step 2
Files: crates/uc-application/src/space/membership/ledger/*
Change: 加密 ledger 模型加入 conflict record，并让冲突关系与记录同 commit。
Risk: 旧 ledger 解码；当前版本要求用户 rebuild，禁止维护双写兼容路径。
```

```text
Step 3
Files: crates/uc-application/src/space/membership/resolve_conflict/*
Change: 实现唯一完整 use case、幂等结果、并发重读和重启恢复。
Risk: 接口若暴露内部阶段会把复杂度泄漏到产品端。
```

```text
Step 4
Files: crates/uc-application/src/space/admission/space_transition/*, crates/uc-infra/src/space/*
Change: 增加同 lineage branch transition 计划、恢复包验证及 generation staging adapter。
Risk: 原地覆盖会造成历史与密钥混合，必须使用新 generation。
```

```text
Step 5
Files: crates/uc-engine/src/contract/*, crates/uc-engine/src/operations/*, bindings/*
Change: 暴露一次 resolve 动作和完整查询结果；三端同版本。
Risk: 产品把本机已选择误报为全局已解决。
```

```text
Step 6
Files: desktop/tests/e2e/tests/membership_conflict.rs
Change: 建立确定性拓扑驱动器、故障点和通信矩阵断言。
Risk: 只等待最终数量会掩盖短暂越权和错误密钥。
```

# 7. Edge Cases

### Scenario: 同一父节点并发 Add/Add

**Expected behavior:** 两条签名均有效但互不自动合并；交叉 peer 为 Diverged，各分支内部继续工作。
**Implementation:** 共享祖先 + sibling heads 生成稳定 conflict id。

### Scenario: Add/Remove 与 Remove/Remove sibling

**Expected behavior:** 不按成员数选赢家；被某分支移除的本机只能重新配对进入该分支。
**Implementation:** 对每个目标分支独立计算本机资格。

### Scenario: 同一移除的 Accept/Reject

**Expected behavior:** 相反签名决定形成 Diverged；选择分支不能改写或删除任一历史决定。
**Implementation:** branch transition 选择完整历史，不合并 decision 集。

### Scenario: 多个 peer 报告同一 remote branch

**Expected behavior:** 单一冲突提示、单一用户选择、多个证据来源；一个恶意 peer 不能替换已验证目标。
**Implementation:** conflict id 不包含 transport 来源。

### Scenario: 两次并发选择不同分支

**Expected behavior:** 只有第一个 ledger CAS 成功；第二个返回 `StateChanged`，不启动第二 transition。
**Implementation:** choice 与 revision/history digest 条件提交。

### Scenario: 选择后目标离线或恢复包丢失

**Expected behavior:** 保持 `Pending` 和来源活动 generation，退避重试；不回到 `Unresolved`。
**Implementation:** 持久 intent 与有界维护调度。

### Scenario: 在每个 transition 阶段崩溃

**Expected behavior:** 重启后停留在完整来源或完整目标；Promoted 后只能向前恢复。
**Implementation:** generation manifest + phase fault injection。

### Scenario: 选择期间又出现第三条 sibling branch

**Expected behavior:** 当前 transition 绑定的目标不变；新分支形成新的待处理冲突，不能偷换目标包。
**Implementation:** recovery package 精确绑定 conflict/target head。

### Scenario: 用户在不同设备上作出相反选择

**Expected behavior:** 两个分支继续安全隔离，不宣称完成；稍后设备可再次通过新的冲突记录选择共同目标。
**Implementation:** 无全局共识和隐式赢家。

### Scenario: 恢复包损坏、过期、跨设备或跨 Space 重放

**Expected behavior:** 稳定拒绝、零 generation 副作用、关系仍 Diverged。
**Implementation:** 签名、lineage、branch、recipient、nonce、expiry 全部验证。

# 8. Testing Strategy

## Unit Test

- sibling heads 以不同到达顺序产生相同 conflict/branch id。
- Same、ancestor、错 lineage、损坏签名不产生可选择冲突。
- 本机在目标分支 Active/Removed/Absent 分别得到 Recoverable/RePairingRequired/InvalidChoice。
- 重复相同选择幂等；并发相反选择只有一个成功。
- transition 所有阶段只前进，终态不可回退。
- 错 recipient、错 head、错 nonce、过期恢复包零副作用。

## Integration Test

- ledger 同 commit 保存 Diverged 和 conflict record，提交故障不留下半状态。
- 来源备份、目标 stage、manifest promote 任一点故障均可重启恢复。
- 目标安全状态从共同祖先按因果顺序恢复，最终 group epoch 与内容密钥目录一致。
- `RePairingRequired` 不安装任何目标密钥；新邀请产生新成员实例。
- Engine 与三端绑定对 choice、result、error 做完整映射。

## Deterministic Topology Matrix

| 编号 | 拓扑与动作 | 关键断言 |
| --- | --- | --- |
| F0 | A-B-C；A/B 从同一 head 分别新增 D/E | 两分支隔离，分支内正文正常，跨分支正文拒绝 |
| F1 | A-B-C-D；A 移除 D，B 从父 head 新增 E | D 在 A 分支 Removed；B 分支不受影响；无自动赢家 |
| F2 | A-B-C-D-E；B/C 同时移除不同叶子 | 两个合法 sibling；选择任一分支后成员与安全 epoch 精确匹配 |
| F3 | A 提议移除 C；B Accept、C Reject | 决定持久、重启后仍 Diverged；相反选择不泄漏内容 |
| F4 | 两个三节点分区通过单 bridge 短暂相遇 | bridge 不能把 sibling histories 拼成六节点假历史 |
| F5 | 环 A-B-C-D-A，两个相反方向传播冲突 | 同一 conflict 只提示一次，无消息环和重复 effects |
| F6 | 深链 A-B-C-D-E-F，中间 B/D 离线 | 叶子选择共同 branch 后逐跳收敛，无在线中间节点依赖 |
| F7 | 十节点不平衡树，三个 sibling branches | 每个分支内部公平反熵；冲突 peer 不饿死合法 peer |
| F8 | 选择目标后 Sponsor 离线、恢复包 ACK 丢失、chooser 重启 | intent 持久、幂等重发、单一 promoted generation |
| F9 | 五设备分成 3/2 作相反选择，之后 2 侧改选 3 侧 | 首轮保持两分支；第二次明确选择后五端收敛 |
| F10 | 目标分支已移除 chooser | 返回 RePairingRequired；重新邀请后旧实例不复活 |
| F11 | transition 各阶段 fault injection | 每个故障点重启后满足来源完整或目标完整 |
| F12 | 冲突期间注入重复、乱序、分页中断和旧 ACK | revision 单调、无部分历史、无错误水位推进 |
| F13 | 恶意 peer 提供合法连接但伪造目标包 | Invalid/Rejected，零密钥、成员或 generation 副作用 |

每个 Desktop 场景必须同时断言：

1. 所有节点的 branch/head 等价类，而不只检查成员数量。
2. 分支内和跨分支的完整通信矩阵。
3. 每个节点的 group epoch、effective member count、pending conflict/effect 数量。
4. 重启前后选择、transition phase 和 revision 单调。
5. exact text 双向传输；错误密钥必须表现为失败，不能只看连接在线。
6. 日志不含设备名、设备标识、branch/head、恢复包、密钥、文件名、路径或正文。

## Chaos Driver

Desktop E2E 增加声明式脚本：`Start`、`Stop`、`Restart`、`Partition`、`Heal`、`Join`、`Remove`、`Decide`、
`ResolveConflict`、`DropNextFrame`、`CrashAtPhase`、`AssertSnapshot`、`AssertTransfer`。脚本和固定 seed 只决定动作
顺序；验收必须保存失败 seed 并能单独重放。至少运行固定的 20 个审查过的 seed，随机扩展只作为附加证据。

## Regression Test

- 规格 029 的 C0-C5 全部保持通过。
- 无冲突的 AddDevice 仍自动应用；移除仍等待本机决定。
- Diverged 未获用户选择时继续禁止完整历史、group update 和普通内容。
- P2P 失败不自动回退 LAN。
- 实体设备未执行项记录为“跳过”。

# 9. Acceptance Criteria

* [ ] 任意 sibling 历史以不同到达顺序产生相同冲突和分支编号。
* [ ] 产品只调用一次 `resolve_membership_conflict`，不编排恢复步骤。
* [ ] 保留本机分支不改变历史、安全状态或其他设备选择。
* [ ] 采用目标分支使用新 generation，绝不原地覆盖来源。
* [ ] Active chooser 可恢复；Removed chooser 必须重新配对并获得新成员实例。
* [ ] 相反选择保持隔离，不出现自动赢家或跨分支内容。
* [ ] transition 每个持久阶段的崩溃恢复通过真实 SQLite fault injection。
* [ ] F0-F13 确定性场景全部通过。
* [ ] 固定 20 个 chaos seeds 可重放且全部满足模型不变量。
* [ ] 冲突前、中、后通信矩阵与 group epoch 断言通过。
* [ ] 规格 029 C0-C5 回归通过。
* [ ] workspace check、fmt、architecture、diff gates 通过。
* [ ] 实体设备未执行项目明确记录为“跳过”。
* [ ] `docs/architecture/architecture-bible.md` 与 Engine 接口规格同步。

# 10. Risks and Trade-offs

- **同 lineage generation 切换复杂**：复用跨 Space transition 的事务语义会增加一种模式，但可避免第二套备份、
  重封装和 crash recovery 实现。
- **恢复包扩大协议面**：它只在明确用户选择后开放，并精确绑定 recipient/conflict/head；普通反熵仍失败关闭。
- **没有全局完成概念**：每台设备独立选择是去中心化系统的真实边界。UI 必须区分“本机已选择”和“所有已知
  设备已收敛”。
- **数据保留成本**：切换期间暂存来源备份和目标 generation 会临时增加磁盘占用；完成后按现有 generation
  清理规则回收。
- **大矩阵耗时**：F0-F13 不应全部进入每次快速单测；Core/Application 矩阵每次运行，Desktop 固定场景进入
  串行验收任务，chaos seeds 可拆为夜间任务。

替代方案“不提供采用分支，只要求 FactoryReset + 重新配对”实现更小，但会让仍是目标有效成员的设备无必要地
失去成员实例，并把备份、重置、邀请和恢复步骤泄漏给用户，因此不采用。

# 11. Open Questions

1. 产品是否允许用户只确认“保留本机分支”并关闭提示，还是必须持续展示仍存在的 Diverged peers？
2. 同一用户拥有多台设备时，是否需要二维码/短码之外的本地批量选择体验？核心仍按逐设备明确授权实现。
3. branch recovery package 的有效期沿用邀请时长还是定义更短的专用时长？
4. Desktop 固定 chaos 场景放入每次 CI、夜间 CI 还是发布门禁，需要结合当前流水线预算确定。
5. 实体设备首轮选择哪些最小矩阵：建议 Android + Desktop + iOS 三设备覆盖分区、相反选择和重新配对。
