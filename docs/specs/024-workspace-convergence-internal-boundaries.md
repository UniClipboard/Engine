# 规格 024：成员收敛内部职责边界实施

> 旧配对自动恢复部分已由 ADR-023 和规格 026 取代；`legacy_upgrade.rs` 及其运行期、端点和网络通道已经删除。

## 状态

- **状态**：已实施
- **日期**：2026-08-17
- **决策依据**：`docs/adr/021-workspace-convergence-internal-boundaries.md`
- **相关文档**：`docs/adr/016-workspace-wide-convergence.md`、
  `docs/adr/017-pairing-as-workspace-admission.md`、
  `docs/adr/020-membership-reconciliation-and-user-decisions.md`、
  `docs/specs/022-current-member-runtime-scope.md`、
  `docs/specs/023-durable-membership-proof-and-admission-activation.md`、
  `docs/architecture/architecture-bible.md`

# 1. Overview

`crates/uc-application/src/space/convergence/mod.rs` 是成员收敛的唯一应用层负责人。实施前它约 5,382 行，直接包含 profile 级投影、准入协议与完成恢复、加密状态读写及成员效果、成员历史核对、移除与用户决定、成员事实初始化、旧升级兼容、当前成员范围、设备信任快照和多个受限入口实现。实施后主文件只保留负责人定义、公开类型、共享状态操作和必要协调，完整工作分别归入下述四个领域目录。

这曾使一次修改必须在同一文件中穿过无关的准入、移除和恢复逻辑才能确认影响范围。更重要的是，继续在主文件追加流程会诱发两类错误：把同一业务判断复制到多个位置，或把重试、恢复和调用顺序暴露给上层。

本规格将 ADR-021 落实为一次可分阶段执行的内部整理。它不改变成员收敛的规则或公开结果，而是在 `WorkspaceConvergence` 内部按完整工作收口实现和测试。整理后的调用方仍只有一个完整负责人；维护者可以从负责位置读懂一项工作从输入到稳定结果、重试或重启恢复的全过程。

# 2. Goals

- 将成员收敛主文件收敛为负责人定义、稳定入口、共享状态操作和必要协调，不再直接承载各项工作的长篇流程。
- 以 `admission`、`membership`、`projection` 和 `connectivity` 四个领域子模块收口内部工作，而不是新增一排同级流程文件。
- 保持 `WorkspaceConvergence`、`ProfileWorkspaceConvergence`、现有 facade、Engine 操作、绑定结果和受限端点的公开行为不变。
- 将现有成员收敛测试随其完整工作移动或分组，使每项工作都能独立定位成功、重复、中断恢复和稳定失败的验证。
- 不新增第二份成员状态、第二个恢复负责人、明文持久化或由调用方安排协议步骤的路径。

# 3. Non-Goals

- 不修改成员历史、用户决定、当前成员范围、设备信任或空间准入的业务规则。
- 不修改 `uc-engine`、iOS、Android 或 HarmonyOS 的公开操作、结果、错误或事件。
- 不改变 SQLite 布局、加密格式、网络消息格式、签名算法或旧数据迁移分类。
- 不将 `space/admission/adapter.rs` 的既有受限协议接线改造成新的公开接口。
- 不用一次无边界重写替代可逐步验证的文件移动。
- 不以文件行数作为唯一完成条件，也不创建只转发一次调用的长期空层。

# 4. Current Architecture Context

```
Component: WorkspaceConvergence
Path: crates/uc-application/src/space/convergence/mod.rs
Responsibility: 成员收敛的唯一完整负责人，协调加密状态、成员历史、准入、移除、恢复、查询和变化通知。
Relationship: 由 uc-engine 组装；产品端和绑定只使用稳定动作、查询和结果，不能推进内部步骤。
```

```
Component: DurableAdmissionTransaction
Path: crates/uc-application/src/space/convergence/admission/transaction.rs
Responsibility: 保存和推进可恢复的本机加入尝试、发起方候选、终态投影和发送记录。
Relationship: 只持有准入持久状态与事务规则；准入协议、成员历史验证和完成恢复仍由 WorkspaceConvergence 协调。
```

```
Component: ProfileWorkspaceConvergence
Path: crates/uc-application/src/space/convergence/projection/profile.rs
Responsibility: 在没有活动 Space 时继续提供加入投影、取消、重置门禁和设备信任查询，并在活动负责人变化时转发版本变化。
Relationship: 持有零或一个 WorkspaceConvergence；不得成为另一套成员流程负责人。
```

```
Component: Membership History and User Decisions
Path: crates/uc-core/src/membership/ and crates/uc-application/src/space/convergence/membership/
Responsibility: uc-core 判断历史、成员关系和决定效果；应用层验证、保存、收发并对外发布完整结果。
Relationship: 历史规则不读取网络或持久化；应用层不复制 uc-core 的判断规则。
```

```
Component: Assembly and Runtime
Path: crates/uc-application/src/space/convergence/assembly.rs and runtime.rs
Responsibility: 组装同一负责人并在设备上线、暂停、恢复和关闭时触发既有完整流程。
Relationship: 只获取受限端点和启动运行期；不持有第二份成员状态，也不重排内部步骤。
```

```
Component: Existing Domain Runtimes
Path: crates/uc-application/src/space/convergence/membership/legacy_upgrade.rs
Path: crates/uc-application/src/space/convergence/connectivity/membership.rs
Path: crates/uc-application/src/space/convergence/connectivity/reachability.rs
Path: crates/uc-application/src/space/convergence/network_recovery.rs
Responsibility: 分别处理旧升级、当前成员连接、可达性刷新和网络会话恢复。
Relationship: 这些文件已经按明确职责拆开；本次不重组其业务规则，只保持其对 WorkspaceConvergence 的受限调用。
```

# 5. Proposed Design

## Components

### 主模块与共享状态

`mod.rs` 保留公开类型、`WorkspaceConvergence` 和 `ProfileWorkspaceConvergence` 的定义、依赖集合、共享锁、事件发布、状态加载/保存、共同唤醒，以及需要连接两个内部工作的少量协调入口。它继续导出既有公开运行期和错误类型。

主模块不得保留准入帧处理、成员历史消息处理、移除决定推进、旧升级初始化或设备信任明细拼装等长篇业务流程。共享状态操作只能保存或发布状态，不得重新实现某一内部工作的业务判断。

### 目标目录

```text
crates/uc-application/src/space/convergence/
├── mod.rs
├── assembly.rs
├── runtime.rs
├── network_recovery.rs
├── discovery/
├── admission/
│   ├── mod.rs
│   ├── transaction.rs
│   ├── flow.rs
│   ├── completion_recovery.rs
│   └── tests.rs
├── membership/
│   ├── mod.rs
│   ├── history.rs
│   ├── removal.rs
│   ├── effects.rs
│   ├── bootstrap.rs
│   ├── legacy_upgrade.rs
│   ├── group_update_delivery.rs
│   └── tests.rs
├── projection/
│   ├── mod.rs
│   ├── profile.rs
│   ├── device_trust.rs
│   ├── current_scope.rs
│   └── tests.rs
├── connectivity/
│   ├── mod.rs
│   ├── reachability.rs
│   └── membership.rs
└── testing/
    └── mod.rs
```

主文件保留公开类型、`WorkspaceConvergence` 和 `ProfileWorkspaceConvergence` 的定义、依赖、锁、事件发布、状态加载/保存及真正跨领域的协调。目录中的 `mod.rs` 是对应领域的唯一 crate 内入口；它选择同领域内部实现，但不把调用方变成流程编排者。

### Admission

`admission/` 负责从加入请求到 Active、Pending 或 Rejected 的完整工作。`transaction.rs` 是现有 `admission_transaction.rs` 的迁移目标，继续唯一负责可恢复尝试的存取和状态转换。`flow.rs` 负责请求验证、基础历史验证、候选、准备、正式提交、应用、完成、确认、取消、来源检查和本机加入准备。`completion_recovery.rs` 负责完成消息丢失或发起方不可用时的帮助设备恢复。

`admission/mod.rs` 是此领域的唯一入口；它协调持久尝试、正常流程和完成恢复。领域外代码不得分别调用候选、提交或恢复步骤。

### Membership

`membership/` 负责成员关系本身。`history.rs` 处理历史消息、单对端串行核对、补齐、对端确认和升级要求；它是普通成员历史与受限决定投递共用的唯一核对入口。`removal.rs` 负责移除提交、用户决定、设备信任选择和关系更新。`effects.rs` 负责已保存成员效果和已保存决定的投递，保证保存成员事实、应用安全更新和重启继续仍由同一个领域完整处理。`bootstrap.rs` 负责本机成员事实、首次成员历史和旧迁移标记清理；`legacy_upgrade.rs` 与 `group_update_delivery.rs` 归入同一领域。

`membership/mod.rs` 负责跨历史、移除、效果和旧升级的必要协调。它不复制 `uc-core` 的历史判断，也不让连接运行期、基础设施或产品端单独应用成员效果。

### Projection

`projection/` 只将已保存事实转换为稳定查询和运行范围。`profile.rs` 承载 `ProfileWorkspaceConvergence` 的构造、活动负责人附着、加入状态投影、取消、普通重置门禁、无活动 Space 的设备信任结果和版本变化转发。`device_trust.rs` 负责完整设备信任查询。`current_scope.rs` 负责当前成员运行范围、内容交换门禁和相应受限端点。

`projection/mod.rs` 是投影领域入口。它只能读取既有加密状态或转交活动负责人的完整查询；成员资料、地址、在线状态和安全组关系只能作为独立事实，不能授予普通成员资格。

### Connectivity and Existing Runtimes

`connectivity/` 收口已有的可达性刷新和当前成员连接维持；它只使用 `projection/current_scope.rs` 给出的当前范围，不复制成员关系判断。`network_recovery.rs` 和 `discovery/` 保留现有独立职责，不在本次整理中与成员关系流程混合。

端点实现跟随所属领域：包含准入完整流程的端点放在 `admission/`，历史和移除端点放在 `membership/`，范围和内容门禁端点放在 `projection/`。不得新增一个只容纳端点转发的同级目录。

测试跟随上述领域，放在对应文件的 `#[cfg(test)]` 区域或对应领域的私有测试模块。仅共享构造夹具和无业务含义的断言可留在受限测试支持位置；不得继续用单一 `tests.rs` 作为所有成员收敛规则的默认归宿。

## Data Model

本次不新增、删除或改变持久化数据。以下状态继续是单一事实来源：

| 数据 | 生命周期 | 规则 |
| --- | --- | --- |
| `WorkspaceConvergenceState` | 活动 Space 的加密状态 | 只由 `WorkspaceConvergence` 的共享保存边界读写；内部文件不能复制或旁路保存 |
| 准入尝试与终态投影 | profile 级加密准入仓储 | 由 `DurableAdmissionTransaction` 管理；流程文件只通过其现有语义读写 |
| `CurrentJoinStatus` | 临时查询结果 | 由 profile 投影或活动负责人生成；字段与结果分类不变 |
| `DeviceTrustSnapshot` / `WorkspaceSnapshot` | 临时查询与变化结果 | 从当前加密状态和当前成员范围推导；不得保存为第二份业务状态 |

文件移动不得改变序列化名称、版本、字段、加密边界、日志内容或数据迁移路径。

## API / Interface

本次没有新增公开 API，也没有删除、改名或修改下列既有入口的参数、结果和错误分类：

| 入口 | 保持的责任 |
| --- | --- |
| `WorkspaceConvergence` 的成员动作、查询和订阅 | 对活动 Space 返回完整成员收敛结果 |
| `ProfileWorkspaceConvergence` 的加入、取消、重置门禁和设备信任查询 | 在无活动 Space 或活动切换期间维持稳定投影 |
| `SpaceTransitionRecoveryPort` | 继续由同一负责人判断并恢复空间切换 |
| `MembershipHistoryExchangeEndpointPort` 与完成恢复端点 | 继续只接收受限、已认证的内部消息 |
| `CurrentWorkspacePeerScopePort` 与内容交换门禁 | 继续从已接受成员历史给出唯一普通运行范围 |

crate 内方法可移动或改为私有，只要 `assembly.rs`、`runtime.rs`、`space/admission/adapter.rs` 和既有测试仍通过同一个完整工作入口完成原有职责。不得以保留旧位置为由增加永久转发层。

## Workflow

### 正常成员收敛

1. 已认证设备上线或收到受限消息。
2. 运行期或端点调用 `membership/` 的完整历史核对入口。
3. 入口在状态锁内读取当前状态，锁外完成有界网络往来，再在状态锁内验证、保存和发布结果。
4. 若普通新增可应用，`membership/` 执行已保存成员效果；若遇到未确认移除，保留等待用户的稳定结果。
5. `projection/` 从同一已保存状态生成当前范围和查询结果。

### 空间准入与完成恢复

1. 产品或配对通道发起一次既有加入动作。
2. `admission/` 通过 `transaction.rs` 保存或恢复同一尝试，并验证所需历史与身份资料。
3. 发起方和加入方按既有候选、准备、提交、应用和完成顺序推进；每个网络回复前先保存可恢复状态。
4. 若完成消息中断，`admission/completion_recovery.rs` 只允许符合既有资格的成员从同一保存位置继续。
5. `projection/profile.rs` 只投影 Active、Pending 或 Rejected，不泄露内部步骤。

### 重启与旧升级

1. 启动或恢复运行期请求负责人继续未完成成员效果、准入或旧升级工作。
2. `membership/bootstrap.rs` 仅在既有证据满足时建立或恢复当前历史；证据不足维持既有安全拒绝。
3. 对于已保存的决定、效果和准入消息，对应工作从同一持久位置继续，不重新创建成员事实或绕过确认。

# 6. Implementation Plan

## Step 1: 建立不变条件与文件边界

**File:** `docs/specs/024-workspace-convergence-internal-boundaries.md`、`docs/adr/021-workspace-convergence-internal-boundaries.md`、`crates/uc-application/src/space/convergence/mod.rs`

**Change:** 以本规格的五类内部工作为唯一归属表。记录当前公开导出、端点实现、状态锁、状态保存和事件发布位置；不移动代码。

**Risk:** 把已有受限接线误判为第二个负责人。实施前必须确认每个调用只转入一个完整工作入口。

## Step 2: 建立领域目录与提取状态投影

**File:** 新增 `projection/{mod,profile,device_trust,current_scope}.rs`；修改 `mod.rs`、必要的 `facade/mod.rs` 与投影测试

**Change:** 先建立 `projection/` 的唯一领域入口，再移动 `ProfileWorkspaceConvergence`、设备信任查询和当前成员范围。公开类型继续从 `convergence` 模块导出，外部路径不变。

**Risk:** 活动负责人的版本变化转发、无活动 Space 查询和任务替换存在并发边界；迁移后必须覆盖附着、替换、锁定和关闭场景。

## Step 3: 建立准入领域并提取准入流程与完成恢复

**File:** 新增 `admission/{mod,flow,completion_recovery}.rs`，移动为 `admission/transaction.rs`；修改 `mod.rs`、`space/admission/adapter.rs` 与准入测试

**Change:** 将请求验证、候选到完成、取消、来源检查、本机准备以及帮助设备恢复移入 `admission/`。保持 `transaction.rs` 为准入持久状态转换的唯一位置，`admission/mod.rs` 为领域外唯一入口。

**Risk:** 准入存在严格保存顺序、重复帧和中断恢复。不得在移动中合并、重新生成或跳过任何已保存阶段。

## Step 4: 建立成员关系领域并提取历史核对

**File:** 新增 `membership/{mod,history}.rs`，移动 `membership/group_update_delivery.rs`；修改 `mod.rs`、`runtime.rs` 与成员关系测试

**Change:** 移动历史消息接收、单对端串行化、历史比较、补齐、对端确认和升级要求记录。保留已存在的受限决定投递角色与同一核对入口。

**Risk:** 状态锁和网络往来的先后不可改变；成员历史验证不能退回到按当前在线、地址或转发来源推断授权。

## Step 5: 在成员关系领域收口移除、决定、效果和旧升级

**File:** 新增 `membership/{removal,effects,bootstrap}.rs`，移动为 `membership/legacy_upgrade.rs`；修改 `mod.rs`、成员关系运行期和测试

**Change:** 移动移除提交、用户决定、设备信任选择、已保存决定投递、关系更新、待处理成员效果恢复、本机成员事实、首次成员历史、旧升级初始化和旧加入完成。共同状态保存只保留在主模块的受限方法中。

**Risk:** 接受、拒绝、重复和重启后的结果必须保持幂等；成员事实和安全更新不可因整理出现半完成或重复执行。

## Step 6: 收口连接领域并保留独立运行期

**File:** 新增 `connectivity/{mod,reachability,membership}.rs`；修改 `membership/legacy_upgrade.rs`、`projection/current_scope.rs`、`assembly.rs`、运行期和测试

**Change:** 将可达性刷新和当前成员连接维持归入 `connectivity/`。保留 `network_recovery.rs` 与 `discovery/` 的现有独立职责；连接领域只从 `projection/current_scope.rs` 获取当前范围，不取得成员关系判断副本。

**Risk:** 已移除设备不得因遗留成员资料、地址或旧升级标记重新进入普通范围；旧数据证据不足时必须保持拒绝。

## Step 7: 在各领域收口端点与测试，并删除旧位置

**File:** 修改 `admission/`、`membership/`、`projection/`、`assembly.rs`、`space/admission/adapter.rs`、各领域测试和 `docs/architecture/architecture-bible.md`

**Change:** 将端点实现留在所属领域，由领域 `mod.rs` 暴露一次完整入口。把测试按领域迁移，删除主文件中的旧实现和临时转发；更新架构总览的文档维护记录。

**Risk:** 未使用的旧导出或测试夹具会掩盖双路径。每步结束必须搜索旧方法位置并删除不再需要的入口。

# 7. Edge Cases

## Scenario: 没有活动 Space 时查询或取消加入

**Expected behavior:** `ProfileWorkspaceConvergence` 继续返回现有加入投影和不可用设备信任结果；不得构造不完整活动负责人。

**Implementation:** 只移动 profile 投影实现，不改变它对准入仓储和可选活动负责人的依赖。

## Scenario: 候选、提交、应用或完成之间断线或进程重启

**Expected behavior:** 从同一已保存尝试继续，或保持既有 Pending/Rejected；不得创建第二个成员实例、第二次安全状态或错误报告成功。

**Implementation:** 准入流程和完成恢复共享原有 `DurableAdmissionTransaction` 状态；测试覆盖每个保存阶段的重启。

## Scenario: 同一对端同时触发多次历史核对

**Expected behavior:** 同一对端的状态写入保持串行，重复消息获得既有幂等结果，不覆盖更晚保存的状态。

**Implementation:** 保留对端锁和“锁内决策、锁外网络、锁内应用”的顺序，不在子文件中另建锁。

## Scenario: 用户决定与后台历史消息或成员效果并发

**Expected behavior:** 只有一个已保存决定和一个对应效果结果；待处理效果在重启后继续，不能双重应用。

**Implementation:** 继续使用状态锁、设备信任决定锁和已有待处理效果记录；移动时不改变保存边界。

## Scenario: 已移除设备仍有历史资料、地址或在线观察

**Expected behavior:** 它可保留过去验证或受限恢复所需资料，但不能进入普通设备列表、连接、内容或文件范围。

**Implementation:** `projection/current_scope.rs` 和 `membership/bootstrap.rs` 只能从已接受成员历史派生普通范围；回归测试覆盖旧升级标记和遗留资料。

## Scenario: 旧状态无法完整验证或保存资料损坏

**Expected behavior:** 保留既有恢复要求或稳定失败，不清空、补造、降级或以新的成功路径掩盖问题。

**Implementation:** 旧升级初始化只复用已有验证与迁移分类；错误映射和加密保存格式不变。

## Scenario: 测试夹具需要跨内部工作构造状态

**Expected behavior:** 测试可复用无业务含义的夹具，但不通过公开逐步接口手工拼接完整流程。

**Implementation:** 将共享夹具保留在受限支持模块；每个工作从其真实入口验证结果。

# 8. Testing Strategy

## Unit Test

- Profile 投影：无活动 Space、活动负责人替换、锁定状态、版本变化转发、取消和重置门禁。
- 准入与恢复：请求无效、重复帧、取消与提交竞争、每个持久阶段重启、帮助设备完成和重建确认。
- 历史核对：相同历史、普通新增补齐、待移除决定、分叉、无效资料、重复与乱序消息、同对端并发。
- 移除与成员效果：未知目标、自身目标、接受、拒绝、重复决定、安全更新失败后的继续和重启恢复。
- 当前范围与旧升级：已移除设备的遗留资料、旧升级标记清理、旧空间初始化、历史或身份资料不完整。

## Integration Test

- 通过 `assembly.rs` 和运行期验证上线事件只启动一次成员核对，并在暂停、恢复和关闭时保持既有行为。
- 通过 `space/admission/adapter.rs` 验证完整加入、取消、完成恢复和历史同步仍经过同一个负责人。
- 通过 Engine 既有操作验证加入、取消、设备信任查询、移除、重置和空间切换恢复的公开结果不变。

## Regression Test

- 运行现有 `space::convergence` 测试，并确认实际执行的测试数量非零。
- 运行受影响的 `uc-application`、`uc-engine` 和 `uc-core` 成员关系测试。
- 在每个整理步骤后运行完整工作区检查、格式检查、仓库架构检查和差异检查。
- 对涉及真实持久状态的现有重启测试，确认保存资料继续加密且不输出敏感日志。

# 9. Acceptance Criteria

* [x] `WorkspaceConvergence` 仍是成员收敛唯一完整负责人，调用方没有新增逐步入口或调用顺序。
* [x] `mod.rs` 只保留负责人、公开类型、共享状态操作和必要协调；五类内部工作不再直接以长篇流程留在该文件。
* [x] `admission`、`membership`、`projection` 和 `connectivity` 均有领域入口；准入与完成恢复、成员历史核对、移除与用户决定、当前范围与状态查询、成员事实初始化与旧升级兼容均在对应领域内有唯一负责位置。
* [x] `DurableAdmissionTransaction` 仍是准入尝试及其持久状态转换的唯一位置，且没有第二份准入状态。
* [x] 当前成员范围仍只从已接受成员历史派生；遗留成员资料、地址、在线状态和安全组关系不能单独授予普通资格。
* [x] 加入、取消、移除、重新加入、历史核对、重复、乱序、断线和重启后的公开结果与整理前一致。
* [x] 每项内部工作都有成功、重复、中断恢复和稳定失败的可定位测试；测试不会依赖手工编排无关步骤。
* [x] 不新增持久化布局、明文业务资料、敏感日志、公开 API、长期转发层或新旧双路径。
* [x] `cargo metadata --locked --format-version 1`、`cargo check --workspace --all-targets --locked`、`cargo fmt --all -- --check`、`node scripts/architecture/check-engine-repository.mjs` 和 `git diff --check` 全部通过。
* [x] 架构总览和 ADR-021 与最终文件归属一致。

# 10. Risks and Trade-offs

| 风险或取舍 | 处理方式 |
| --- | --- |
| 文件数量增加 | 接受文件增加，要求每个文件承载一项完整工作；不以少文件数量掩盖知识混杂。 |
| 移动代码时改变锁、保存或网络顺序 | 每次只迁移一项工作，保留原有锁内/锁外边界，并用重启与并发回归测试验证。 |
| 私有方法移动后产生临时转发 | 每一步结束删除旧位置；端点层只允许一次调用完整入口。 |
| 共享状态操作变成新的杂项区 | `mod.rs` 只保留保存、发布、唤醒和真正跨领域协调；业务判断必须回到所属领域。 |
| 当前工作区仍有未提交成员收敛改动 | 实施每一步前重新读取当前 diff、函数位置和测试归属；不得按本规格中的行数或旧文件位置盲目覆盖。 |
| 性能变化 | 文件移动本身不改变网络、存储、锁粒度或任务数量；任何意外性能变化视为回归。 |

# 11. Open Questions

- 无阻塞问题。实施开始时唯一需要重新确认的是当前工作区中尚未提交的成员收敛修改应归入哪一项内部工作；该确认必须在首次代码移动前完成，不能通过覆盖或回退解决。
