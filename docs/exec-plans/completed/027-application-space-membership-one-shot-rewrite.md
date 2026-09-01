# Application Space 成员关系一次性重写

> 本规格落实 ADR-025，并取代 ADR-021 与规格 024 中“围绕 `WorkspaceConvergence` 渐进搬迁”的实施方式。
> ADR-020、规格 021、022、023、025、026 已固定的产品、安全、协议和恢复语义继续有效。

## 状态

一次性重写及 Core、Infra、Engine、绑定、数据库和真实网络接入均已完成；准入 wire/runtime 的最终实现与 clean-cutover 以规格 028 为准。决策依据为 `docs/design-docs/decisions/025-application-space-membership-one-shot-rewrite.md`。

# 1. Overview

`crates/uc-application/src/space` 当前同时存在三种结构：旧的成员关系总对象、从总对象逐步搬出的标准用例，以及仍由 facade、assembly 和多个 runtime 拼接的散落流程。当前工作树中的移动和改名只证明了实际调用、状态写入和恢复依赖，不能作为目标结构。

本规格只设计和替换 `uc-application`。Core 规则、Infra 适配、Engine 组装、绑定、数据库迁移和网络实现即使暂时与目标 application 接口不兼容，也不在本规格处理。Application 先用明确 ports 和测试替身证明自身业务闭环；其他层后续按已经固定的接口接入。

现状的具体问题是：

- `MembershipStateCoordinator` 同时掌握成员状态、准入判断、历史交换、移除、保存和通知；删除它后，复杂度会散回 admission、runtime 和 facade，说明它没有形成稳定的深模块接口。
- `SpaceMembershipState` 把成员真相、对端关系、分页进度、综合阶段、失败类别和产品修订混在一个对象中；application 因此必须同时理解多种持久资料和提交顺序。
- 正式产品入口、开发诊断入口、网络端点和后台恢复分别经过 `SpaceFacade`、`SpaceMembershipFacade`、`SpaceJoinFacade`、`MemberRosterFacade`、`SpaceModules` 和三个 runtime。
- 当前成员范围、内容门禁、连接候选、成员列表和历史同步各自组合部分规则，新增调用方容易漏掉成员资格、激活状态或关系限制中的一项。
- application 内的旧候选、公告和 gossip 流程仍持有一套成员发现、安全更新、可信关系和地址提升编排，与正式准入和 V2 成员历史重复。
- 现有加入日志会记录邀请码或完整设备标识，不符合仓库日志规则。

本次改动先固定实际业务规格，再在一个不可拆分的切换中替换旧成员关系和所有散落实现。实施中可以按内部步骤开发和验证，但主分支最终只接受完整目标结构；不合入兼容转发、双写、旧路径别名或半迁移状态。

# 2. Goals

- 只保留 V2 签名成员历史作为当前成员资格的正向事实。
- 用一个 application 持久接口原子提交成员历史、加入记录、关系、传输、待处理效果和设备信任修订。
- 删除 `MembershipStateCoordinator`、`SpaceAdmission`、`MembershipConvergence` 三个总对象及其步骤级接口。
- 每个用户动作、查询、网络接收和后台维护都由一个标准用例负责完整结果。
- 让 `SpaceFacade` 成为 application 唯一公开 Space 业务入口；不再公开内部用例、协调器或 runtime。
- 将现有三个成员 runtime 合并为一个可暂停、恢复和关闭的 Space 成员 runtime。
- 所有普通成员消费者只读取同一个最终授权范围，不再自行组合成员表、地址、在线状态和关系门禁。
- 固定 application facade 的命令、结果、错误和事件语义，供后续 Engine 与绑定接入。
- 保留加入、移除、拒绝分叉、离线恢复、跨 Space 切换和旧资料重建的已确认行为。
- 删除 application 内旧流程、旧测试替身和无调用者实现，不保留两套成员路径。
- 所有新增或迁移的持久负载使用 MasterKey AEAD 密文；所有日志符合脱敏规则。

# 3. Non-Goals

- 不改变 ADR-020 的单父成员历史、离线分支和用户决定规则。
- 不修改 `uc-core` 的成员规则、类型或 ports。
- 不修改 `uc-infra` 的数据库、密码、安全状态、网络 adapter、协议注册或 migration。
- 不修改 `uc-engine` 的操作、结果、组装和运行期。
- 不修改 iOS、Android、HarmonyOS 绑定和验收宿主。
- 不恢复低于当前版本的旧成员关系；旧资料仍按规格 026 重建单设备 Space 并重新配对。
- 不在 application 新增第三方依赖、配置开关或运行时 feature 来保留旧路径。
- 不物理删除仍用于验证过去签名或完成受限决定投递的历史身份资料。
- 不把 reachability、地址、成员显示名或可信关系变成成员资格来源。
- 不重写剪贴板、文件传输或搜索流程；只迁移它们取得当前成员授权范围的方式。

# 4. Current Architecture Context

```text
Component: Space facades
Path: crates/uc-application/src/facade/space_setup/
Path: crates/uc-application/src/facade/space_join/
Path: crates/uc-application/src/facade/space_membership/
Path: crates/uc-application/src/facade/roster/
Responsibility: 当前分别提供生命周期、加入、设备信任和成员名单入口。
Relationship: 同一 Space 业务被多个 facade 分割，部分 facade 直接读取存储或发送协议消息。
```

```text
Component: Legacy membership aggregate
Path: crates/uc-application/src/space/membership_state_coordinator.rs
Path: crates/uc-application/src/space/membership_state_coordinator_tests.rs
Responsibility: 当前保存共享状态、准入判断、历史核对、移除和通知。
Relationship: 是本次必须删除的旧总对象。
```

```text
Component: Authoritative membership history
Path: crates/uc-application/src/space/membership_history/
Responsibility: 调用既有 Core 规则加载、验证和编码不可变签名历史。
Relationship: Core 规则不在本规格修改；application 的读取和提交收进新的 membership ledger。
```

```text
Component: Durable admission
Path: crates/uc-application/src/space/admission/
Responsibility: 保存加入双方阶段、可靠消息、终态、成员历史和设备信任修订。
Relationship: application 用窄 ports 表达所需持久能力；步骤级 owner 和宽仓储接口替换为标准用例接口。
```

```text
Component: Current member scope
Path: crates/uc-application/src/space/current_membership_scope.rs
Responsibility: 从成员历史和加入状态派生当前成员，并叠加部分关系门禁。
Relationship: 规则正确但依赖过多，且与 ContentExchangeGatePort 重复；替换为一个最终授权范围查询。
```

```text
Component: Membership synchronization
Path: crates/uc-application/src/space/membership/synchronize_history/
Responsibility: 全量或单设备发送有界历史页，接收入站页并保存关系。
Relationship: 用例入口保留，内部不再依赖旧总对象或旧综合状态。
```

```text
Component: Membership runtimes
Path: crates/uc-application/src/space/membership_runtime/
Path: crates/uc-application/src/space/membership_convergence/
Path: crates/uc-application/src/space/connectivity/membership.rs
Path: crates/uc-application/src/space/runtime.rs
Responsibility: 当前分别处理恢复、旧 gossip 和连接维护。
Relationship: 合并为一个 runtime，业务恢复顺序交给一个维护用例。
```

## Current Data Flow

1. Application 通过多个 facade、`SpaceModules` 和多个 runtime 暴露同一 Space 业务。
2. 用户移除或决定通过 `SpaceMembershipFacade`，旧开发决定通过 `AppFacade` 和 `MemberRosterFacade`。
3. 用例、旧总对象和 admission 直接读取成员历史、旧状态、成员表和加入记录。
4. 状态变化分别推进旧 revision 和 profile `device_trust_revision`，查询再取最大值。
5. runtime 分别恢复 admission、送达决定、同步历史、运行旧 gossip 和拨号。
6. application 内的普通内容、名单和连接候选再分别组合当前范围与关系门禁。

# 5. Proposed Design

## Components

### `SpaceFacade`

- 职责：唯一公开 Space 入口，暴露稳定用户动作、查询、订阅和完整生命周期动作。
- 输入：稳定 facade 命令或查询。
- 输出：稳定结果和错误。
- 关系：只选择一个完整用例并转换输入输出；不读取存储、不发网络消息、不启动内部步骤。

### `MembershipLedger`

- 路径：`crates/uc-application/src/space/membership/ledger/`
- 职责：加载并验证当前成员历史与运行资料；执行带版本条件的原子提交；派生当前授权范围。
- 输入：期望 revision、规范成员变化、关系变化、传输变化或效果变化。
- 输出：已提交的新 revision 和经验证快照，或 Locked、Conflict、Corrupt、Unavailable。
- 关系：是私有深模块，不决定产品动作、网络重试或用户结果。产品用例和网络用例通过它共享事实。

### `QueryDeviceTrustUseCase`

- 路径：`crates/uc-application/src/space/membership/query_device_trust/`
- 职责：一次读取当前历史、关系、加入投影、成员显示资料和 reachability，生成完整设备信任结果。
- 输入：无。
- 输出：`DeviceTrustStatus`。
- 关系：不修复状态、不触发网络、不把读取失败转换为旧成员表回退。

### `RemoveSpaceMemberUseCase`

- 路径：`crates/uc-application/src/space/membership/remove_space_member/`
- 职责：验证目标、签名移除、原子保存历史变化与待处理效果、请求后台维护并返回最新结果。
- 输入：目标 `DeviceId`。
- 输出：`RemoveSpaceMemberResult`，包含变化编号、规范提交回执和最新设备信任状态。
- 关系：成功只表示本机正式事实已保存；远端何时接受不属于成功条件。

### `DecideDeviceTrustChangeUseCase`

- 路径：`crates/uc-application/src/space/membership/decide_device_trust_change/`
- 职责：处理当前待决定移除的接受、拒绝、重复、过期和本机移除确认。
- 输入：变化编号、选择、是否确认移除本机。
- 输出：Applied、KeptCurrentGroup、AlreadyCompleted、StateChanged 或 LocalConfirmationRequired，均携带最新状态。
- 关系：决定、关系变化、待处理效果、受限送达计划和 revision 在一次提交中保存。

### `SynchronizeMembershipHistoryUseCase`

- 路径：`crates/uc-application/src/space/membership/synchronize_history/`
- 职责：在一个固定总预算内同步全部当前成员，或同步一个明确上线的成员。
- 输入：`AllCurrentPeers` 或 `AuthenticatedPeer(DeviceId)`。
- 输出：整体完成或当前范围不可用；单设备暂时失败记录为 deferred。
- 关系：内部拥有 peer 锁、分页、ACK 验证和关系提交。调用方看不到页和游标。

### `HandleMembershipHistoryMessageUseCase`

- 路径：`crates/uc-application/src/space/membership/handle_history_message/`
- 职责：处理一条来自已认证成员通道的有界历史消息，先保存接收结果再返回 ACK。
- 输入：已认证来源设备和一条 `MembershipHistoryMessage`。
- 输出：一条 ACK 或稳定拒绝。
- 关系：网络 adapter 只调用一次 `execute()`；不拼接持久化步骤。

### `MaintainSpaceMembershipUseCase`

- 路径：`crates/uc-application/src/space/membership/maintenance/`
- 职责：完整恢复当前 Space 的未完成成员工作。
- 输入：Startup、Resume、Periodic、StateChanged 或 PeerOnline。
- 输出：按类别统计完成、延后和稳定失败数量。
- 关系：内部固定恢复顺序；runtime 不知道 admission、效果、决定或分页步骤。

### `SpaceMembershipMaintenanceRuntime`

- 路径：`crates/uc-application/src/space/membership/maintenance/runtime.rs`
- 职责：拥有 presence 订阅、定时器、退避、唤醒、暂停、恢复和关闭。
- 输入：运行期命令和事件。
- 输出：无业务状态；只调用 `MaintainSpaceMembershipUseCase`。
- 关系：替换现有三个 runtime。会话层只持有 pause/resume/shutdown 接口。

### Admission use cases

- 路径：`crates/uc-application/src/space/admission/`
- 职责：`JoinSpaceUseCase`、`CancelSpaceJoinUseCase`、`RecoverSpaceAdmissionsUseCase` 和入站处理用例分别负责完整产品或网络结果。
- 输入：稳定加入命令、取消命令、恢复触发或一条已认证准入消息。
- 输出：Active、Pending、Rejected 或协议回复。
- 关系：删除 `SpaceAdmission` 总对象及 Sponsor/Joiner 步骤级 owner port；双方协议阶段只在 admission 内部可见。

### `SpaceApplication`

- 路径：`crates/uc-application/src/space/application.rs`
- 职责：一次构造 SpaceFacade、网络端点和唯一 runtime 生命周期句柄。
- 输入：`SpaceApplicationDeps` 中的被动 ports/adapters。
- 输出：facade、规范 endpoint ports 和不透明 runtime handle。
- 关系：替换 `SpaceModules`。Application 外部只能取得 facade、规范 endpoint ports 和不透明生命周期句柄。

## Target Directory

```text
crates/uc-application/src/space/
  application.rs
  initialize_space/
  unlock_space/
  recover_space_session/
  lock_space_session/
  reset_space/
  rebuild_space/
  upgrade_space/
  query_space_setup_state/
  query_space_access_state/
  admission/
    join_space/
    cancel_space_join/
    query_space_join_status/
    complete_pending_space_transition/
    recover_space_admissions/
    invitation/
    protocol/
    tests/
  membership_ledger/
    mod.rs
    model.rs
    repository.rs
    current_scope.rs
    reconciliation.rs
    effect_executor.rs
    tests.rs
  query_device_trust/
    mod.rs
    model.rs
    error.rs
    ports.rs
    use_case.rs
    tests.rs
  remove_space_member/
    mod.rs
    model.rs
    error.rs
    ports.rs
    use_case.rs
    tests.rs
  decide_device_trust_change/
    mod.rs
    model.rs
    error.rs
    ports.rs
    use_case.rs
    tests.rs
  synchronize_membership_history/
    mod.rs
    error.rs
    ports.rs
    use_case.rs
    tests.rs
  handle_membership_history_message/
    mod.rs
    error.rs
    ports.rs
    use_case.rs
    tests.rs
  maintain_space_membership/
    mod.rs
    model.rs
    error.rs
    ports.rs
    use_case.rs
    runtime.rs
    tests.rs
  connectivity/
    network_recovery.rs
```

`use_case.rs` 只保留完整动作的入口与阶段编排。复杂的纯规则继续调用既有 Core 能力；数据库、密码、安全组和网络都只通过 application ports 使用。本规格不修改这些外部能力的实现。不建立 `coordinator.rs`、`owner.rs`、`helpers.rs` 或通用 `usecases/` 目录。

## Data Model

### `MembershipHistoryV2`

- 含义：当前 Space 不可变签名成员事件和决定。
- 生命周期：创建或重建 Space 时建立单成员根；加入、移除和决定只追加；永不由旧成员表补造。
- 约束：唯一正向成员资格来源；过去作者的验证资料永久保留。

### `PeerReconciliationRecord`

- `peer_device_id`：加密保存的对端标识。
- `relationship`：Unknown、Consistent、PendingLocalDecision、Diverged、Invalid、UpgradeRequired。
- `confirmed_position`：最近双方确认的历史位置；不能授予成员资格。
- `restricted_delivery`：仅完成指定决定所需的加密计划。
- `updated_at_ms`：排序和诊断时间。
- 生命周期：首次核对创建；关系变化原子替换；重建 Space 时清空。

### `InboundMembershipTransfer`

- `source_device_id`、`transfer_id`、`page_count`、已保存页和总字节数。
- 每个来源最多一个活动 transfer。
- 每页先加密保存再 ACK；完整组装和验证成功后才删除并提交正式历史。
- 重复相同页幂等；冲突页、超限页或 transfer 替换标为 Invalid 并清除该 transfer。

### `PendingMembershipEffect`

- `event_id`：稳定幂等键。
- `kind`：AddDevice 或 RemoveDevice。
- `phase`：Prepared、MemberFactsApplied、SecurityApplied、Activated。
- `payload`：完成本次效果所需的最小加密资料。
- 生命周期：成员变化与 Prepared 同次提交；各阶段只向前推进；全部完成后保留最小完成标记或按固定规则压缩。

### `DeviceTrustRevision`

- profile 内唯一单调 `u64`。
- 加入、成员历史、关系、待决定项、效果完成和投影可见变化与业务事实同事务推进。
- 溢出返回 Corrupt/RecoveryRequired，不回绕。
- 查询不再取两份 revision 的最大值。

### `CurrentSpaceMemberScope`

- 只读派生值，不落库。
- 字段：revision、本机资格、普通可用对端、暂停对端及稳定暂停原因。
- 派生顺序：已应用历史 -> 本机加入激活门禁 -> 关系限制 -> 效果完成门禁。
- 地址、在线状态和偏好不进入该值；调用方再与自己的非资格资料做交集。

## API / Interface

以下是目标应用层内部接口。具体命名可按 Rust 格式调整，但方法数量、输入语义和职责不得扩大。

```rust
impl QueryDeviceTrustUseCase {
    async fn execute(&self) -> Result<DeviceTrustStatus, QueryDeviceTrustError>;
}

impl RemoveSpaceMemberUseCase {
    async fn execute(
        &self,
        target: &DeviceId,
    ) -> Result<RemoveSpaceMemberResult, RemoveSpaceMemberError>;
}

impl DecideDeviceTrustChangeUseCase {
    async fn execute(
        &self,
        input: DecideDeviceTrustChange,
    ) -> Result<DecideDeviceTrustChangeResult, DecideDeviceTrustChangeError>;
}

impl SynchronizeMembershipHistoryUseCase {
    async fn execute(
        &self,
        target: MembershipSyncTarget,
    ) -> Result<MembershipSyncReport, SynchronizeMembershipHistoryError>;
}

impl HandleMembershipHistoryMessageUseCase {
    async fn execute(
        &self,
        source: &AuthenticatedMember,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, HandleMembershipHistoryMessageError>;
}

impl MaintainSpaceMembershipUseCase {
    async fn execute(
        &self,
        trigger: MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceReport;
}
```

Application 持久 seam 不暴露物理表。目标用例只依赖以下能力，并用内存 adapter 完成本规格测试：

```rust
trait LoadMembershipLedgerPort {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError>;
}

trait CommitMembershipLedgerPort {
    async fn compare_and_commit(
        &self,
        mutation: MembershipLedgerMutation,
    ) -> Result<CommittedMembershipLedger, MembershipLedgerError>;
}
```

`MembershipLedgerMutation` 必须包含期望 revision 和期望历史摘要。需要与加入记录共同提交时，application 只调用一次联合提交 port；调用方不能先提交一边再提交另一边。本规格不规定该 port 的 Infra 实现。

普通消费者只使用：

```rust
trait CurrentSpaceMemberScopePort {
    async fn snapshot(
        &self,
    ) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError>;
}
```

Application 不再依赖或实现 `ContentExchangeGatePort`，改为统一使用本层定义的最终 scope。Core 中该 port 是否删除不在本规格处理。所有 application 普通发送、接收、名单、拨号、补送和文件流程使用同一快照；快照不可用时失败关闭。

准入邀请只做一次一致读取：

```rust
struct MembershipAdmissionSnapshot {
    current_generation: u64,
    decision: MembershipAdmissionDecision,
}

impl QueryMembershipAdmissionUseCase {
    async fn execute(
        &self,
        invitation_generation: Option<u64>,
    ) -> Result<MembershipAdmissionSnapshot, QueryMembershipAdmissionError>;
}
```

Application facade 固定查询设备信任、处理当前变化、移除成员、加入、取消加入、重置和彻底重置的命令、结果、错误与事件。后续 Engine 和绑定如何适配这些接口不在本规格处理。

## Error Handling

- `Locked`：Space 尚未解锁；普通行为失败关闭，可重试。
- `Conflict`：期望 revision 或历史摘要已变化；用例重新读取一次，仍冲突则返回 StateChanged。
- `Corrupt`：签名、摘要、lineage、版本、持久阶段或身份映射矛盾；不删除、不重建、不回退旧表。
- `Unavailable`：网络或依赖暂时不可用；持久事实保持，后台继续恢复。
- `CommittedButPending`：正式事实已提交但效果尚未完成；不得回滚历史。产品查询显示受限状态，后台继续。
- 网络 Invalid/Rejected 不包含敏感原因正文，只返回协议稳定分类。

## Workflow

### Remove member

1. `SpaceFacade` 调用 `RemoveSpaceMemberUseCase::execute`。
2. 用例取得共享成员写锁，加载经验证 ledger。
3. 验证本机活动、目标活动且不是本机。
4. 从当前凭据创建并签名 `RemoveDevice`。
5. 构造 Prepared 效果、对端关系变化和受限送达计划。
6. 以期望 revision 和历史摘要一次提交全部事实并推进 revision。
7. 发布一次 revision 变化并唤醒维护 runtime。
8. 尝试执行当前可完成效果；失败保留 Pending，不重写历史。
9. 查询并返回同一 revision 或更新后的完整设备信任状态。

### Decide current change

1. 用例串行加载指定变化。
2. 已完成则补做 Pending 效果并返回原决定。
3. 不再是当前变化则返回 StateChanged 和最新状态。
4. 接受且会移除本机，但未明确确认时不写入，返回 LocalConfirmationRequired。
5. 签名 Accept 或 Reject。
6. Accept 原子提交决定、应用位置、关系、Prepared 效果和受限计划；Reject 原子提交决定并将提议方关系标为 Diverged。
7. 发布一次变化并唤醒维护。
8. 返回规范结果和最新状态。

### Receive history page

1. 网络 port 提供已认证来源和已完成大小限制的一条消息。
2. endpoint 调用一次入站用例。
3. 用例验证 envelope、来源、transfer、页号、页数、单页条数和总字节数。
4. 新页先加密保存；重复相同页幂等；跳页请求期望页；冲突页返回 Invalid。
5. 未收齐时返回 Continue。
6. 收齐后验证完整历史、作者凭据、父链、摘要、lineage 和候选激活事实。
7. 一致则记录 Consistent；普通后继先提交历史与 Prepared 效果；未确认移除记录 PendingLocalDecision；不可比较历史记录 Diverged；无效资料记录 Invalid。
8. 所有结果先提交并推进 revision，再返回 ACK。

### Synchronize history

1. AllCurrentPeers 取得一次最终授权范围，排序去重。
2. 整轮使用固定 10 秒截止时间，不按设备数叠加等待。
3. 每个对端使用独立串行锁，导出同一历史快照的有界页。
4. 每次只发送一页，严格校验 Continue/Consistent/UpdatesApplied/Diverged/Invalid。
5. 单设备离线、超时、拒绝或协议失败记为 deferred，继续其他设备。
6. AuthenticatedPeer 只同步指定已认证设备，不取得全量执行锁。
7. RestrictedDelivery 只发送已保存计划中的指定决定，不授予普通范围。

### Admission

1. 新用户 Join 先创建或安全取代一条持久加入记录；后台恢复永不创建新的用户尝试。
2. 邀请方提交前调用完整历史同步一次；暂时离线设备不无限阻止，但当前范围或历史不可验证则拒绝继续。
3. Candidate 在加入方验证完整基础历史并持久暂存目标安全状态前不进入正式历史。
4. Sponsor 正式提交 `AddDevice`、加入记录、可靠消息、Prepared 效果和 revision 必须原子完成。
5. 正式提交后不回滚；断线、重放和重启继续同一记录。
6. Joiner 完成目标 Space 提升后才进入活动成员范围。
7. Cancel 只在正式提交前产生 Rejected(Cancelled)；提交后返回现有 Pending/Active。
8. 完成帮助和 ACK 补送由 `RecoverSpaceAdmissionsUseCase` 处理，facade 不拨号或发送协议帧。

### Maintenance

1. runtime 对同一 Space 同时最多运行一轮维护。
2. Startup/Resume/StateChanged 顺序固定为：恢复 admission -> 恢复成员效果 -> 送达受限决定 -> 同步当前历史 -> 整理无用旧资料。
3. Periodic 顺序固定为：恢复 admission -> 恢复成员效果 -> 送达受限决定；只有存在请求或关系未确认时同步历史。
4. PeerOnline 只对位于当前授权范围或精确受限计划的对端执行对应单设备动作。
5. 每步暂时失败记录 deferred 并继续不依赖它的后续步骤；Corrupt 停止本轮会扩大权限的步骤。
6. Pause 中止网络任务并等待状态提交完成；Resume 立即安排一轮；Shutdown 最多等待 5 秒后取消网络任务，不能中断正在提交的本地事务。

### Scope consumers

1. 每个普通操作开始时取得一次 `CurrentSpaceMemberScope`。
2. 列表、地址、在线状态、偏好或传输资料只与 `usable_peer_device_ids` 做交集。
3. 同一操作不混用不同 revision 的快照。
4. scope 不可用时不选择任何普通目标。
5. 受限成员决定投递只使用 ledger 中的精确计划，不经过普通 scope。

# 6. Implementation Plan

本计划是一个切换单元。步骤可以在同一分支中依次完成，但不得把中间状态合入主分支或发布。

## Step 1: 固定失败测试和行为夹具

**File:** `crates/uc-application/src/space/*/tests.rs`
**Change:** 从现有 application 测试提取产品结果、恢复、历史分页、加入、移除、拒绝分叉、范围和运行期行为；先让目标接口测试失败。测试只通过目标用例或 facade，不构造旧总对象。
**Risk:** 旧测试可能断言内部阶段。只保留用户可见结果、持久事实、接收端实际应用和稳定协议行为。

## Step 2: 建立 application ledger 模型与 ports

**File:** `crates/uc-application/src/space/membership/ledger/`
**Change:** 定义新运行资料、窄加载/原子提交 ports 和 application 内存 adapter。成员历史规则继续调用现有 Core；本步骤不修改 Core 或提供 Infra adapter。
**Risk:** Port 不能照搬现有宽仓储的全部方法。它只表达目标用例真正需要的一次加载、一次条件提交和一次联合提交。

## Step 3: 重写正式成员用例

**File:** `crates/uc-application/src/space/membership/query_device_trust/`
**File:** `crates/uc-application/src/space/membership/remove_space_member/`
**File:** `crates/uc-application/src/space/membership/decide_device_trust_change/`
**Change:** 以 `rebuild_space` 的单入口、执行锁、阶段方法、持久提交和收尾模式重写三个正式用例。
**Risk:** 决定提交后效果失败不能返回成“未发生”；结果和查询必须表达已提交但暂时受限。

## Step 4: 重写历史收发

**File:** `crates/uc-application/src/space/membership/synchronize_history/`
**File:** `crates/uc-application/src/space/membership/handle_history_message/`
**Change:** 把 outbound 和 inbound 从旧总对象移入两个完整用例；复用既有 V2 编解码和上限。
**Risk:** 必须保留每页先保存后 ACK、重复页幂等、乱序继续和整体验证后提交。

## Step 5: 重写 admission 与成员提交协作

**File:** `crates/uc-application/src/space/admission/`
**Change:** 删除 `SpaceAdmission` 和步骤级 owner ports；Join、Cancel、Recover 和入站消息分别成为完整入口。历史、加入记录、效果和 revision 使用同一 ledger adapter 联合提交。
**Risk:** S2/J2 正式提交点不能改变；任何中断只能向前恢复同一记录。

## Step 6: 合并维护与 runtime

**File:** `crates/uc-application/src/space/membership/maintenance/`
**File:** `crates/uc-application/src/space/lifecycle/session/activity.rs`
**Change:** 合并旧 gossip、成员维护和成员连接 runtime；固定维护顺序、退避、暂停、恢复和关闭。
**Risk:** 暂停必须阻止新网络工作，但不能取消正在提交的本地事务；关闭不能无限等待离线对端。

## Step 7: 一次切换 application facade 和内部消费者

**File:** `crates/uc-application/src/facade/`
**File:** `crates/uc-application/src/clipboard/`
**File:** `crates/uc-application/src/transfer/`
**Change:** 只保留一个 application `SpaceFacade`；所有 application 内普通消费者改用最终 scope。Facade 不再暴露旧总对象、旧综合查询或内部 runtime。
**Risk:** 必须搜索所有成员表、可信关系和地址枚举，证明没有绕过 scope 的普通路径。

## Step 8: 删除 application 旧实现

**File:** 见本节删除清单。
**Change:** 在同一切换中删除 application 旧模块、旧 ports、旧 runtime、旧测试和转发导出。Core/Infra/Engine/绑定保持不动，即使暂时不兼容。
**Risk:** 不得为了让外层继续编译而在 application 保留旧别名或第二条实现；外层适配属于后续工作。

## Step 9: Application 文档与验收

**File:** `docs/architecture/architecture-bible.md`
**File:** `docs/exec-plans/completed/027-application-space-membership-one-shot-rewrite.md`
**Change:** 更新 application 的唯一入口、最终目录和禁止旧符号；运行 `uc-application` 范围的测试、格式和差异检查。
**Risk:** 全工作区因外层尚未适配而失败不能反向改变 application 设计；必须同时报告 application 自身结果和外层未接入状态。

## Mandatory Deletion Checklist

### Application

- `crates/uc-application/src/space/membership_state_coordinator.rs`
- `crates/uc-application/src/space/membership_state_coordinator_tests.rs`
- `crates/uc-application/src/space/membership_state/`
- `crates/uc-application/src/space/membership_convergence/`
- `crates/uc-application/src/space/membership_runtime/`
- `crates/uc-application/src/space/recover_pending_membership_effects/`
- `crates/uc-application/src/space/current_membership_scope.rs`
- `crates/uc-application/src/space/decide_membership_removal_legacy/`
- `crates/uc-application/src/space/query_workspace_membership_diagnostics/`
- 旧 `SpaceAdmission`、`SponsorAdmissionOwnerPort`、`JoinerAdmissionOwnerPort` 及默认失败方法。
- `SpaceModules`、`SpaceModulesDeps` 和内部 owner accessor。
- `SpaceMembershipFacade`、`SpaceJoinFacade` 及 `MemberRosterFacade` 中的成员决定/旧收敛入口。
- facade 内 `deliver_join_completion_ack`、直接持久读取、runtime spawn 和成员候选枚举。
- 所有指向 `workspace_membership`、`membership_state_coordinator`、`membership_convergence` 的兼容路径、再导出和测试辅助。

### Explicitly retained outside this spec

- `crates/uc-core/` 的现有成员类型、规则和 ports。
- `crates/uc-infra/` 的现有 repositories、数据库表、migration、网络 adapter 和协议注册。
- `crates/uc-engine/` 的现有操作、结果、组装和运行期。
- `bindings/` 与 `tests/hosts/`。

这些外层可能在 application 一次性替换后暂时无法编译或接入。不得因此把 application 旧总对象、旧 runtime、兼容别名或双路径加回来。

## Deletion Check

完成后设想删除 `membership_ledger`：历史验证、原子提交、当前 scope 和 revision 规则会重新散到至少查询、移除、决定、准入和历史接收五个调用者，说明该模块具有深度。

设想删除任一标准用例：该完整用户动作或网络动作的顺序会回到一个明确 facade/endpoint，而不会散到多个调用方。任何只转发一个同名内部方法且删除后行为不变的目标模块都不得保留。

# 7. Edge Cases

```text
Scenario: 全新 profile 没有当前 Space
Expected behavior: 设备信任查询返回明确空状态；普通 scope 不可用；Join 仍可由 profile 级入口开始。
Implementation: 查询使用 profile 元数据和加入投影，不构造假成员历史。
```

```text
Scenario: Space 已锁定
Expected behavior: 普通成员动作和 scope 失败关闭；加入终态查询和允许的取消仍可读取 profile 加密资料。
Implementation: 区分 Space MasterKey 与 ProfileAdmissionMasterKey 能力。
```

```text
Scenario: V2 历史缺失但旧成员表完整
Expected behavior: 当前 Space 不运行普通流程，要求重建；绝不从旧表恢复授权。
Implementation: ledger 返回 RecoveryRequired。
```

```text
Scenario: 两个本机成员命令并发
Expected behavior: 只有一个基于给定 revision 提交；另一个重读后返回 StateChanged 或基于新状态重新验证。
Implementation: 共享写锁加持久 CAS，不能只依赖进程锁。
```

```text
Scenario: 同一来源发送重复历史页
Expected behavior: 相同页返回同一 Continue/最终 ACK；不同内容使用同页号返回 Invalid。
Implementation: transfer_id、page_index 和规范页摘要共同验证。
```

```text
Scenario: 历史页乱序或超过上限
Expected behavior: 乱序请求缺失页；超限在分配和业务解码前拒绝。
Implementation: 保留 256 条和 4 MiB 上限及网络层前置检查。
```

```text
Scenario: 用户接受会移除本机的变化
Expected behavior: 未明确确认时不写；确认后先持久停止普通权限，再返回已应用结果。
Implementation: confirm_local_removal 只绑定当前 change_id 和 revision。
```

```text
Scenario: 用户拒绝移除
Expected behavior: 本机分支成员不变；只与提议方关系 Diverged；其他一致设备继续普通工作。
Implementation: 决定提交不运行 RemoveDevice 效果。
```

```text
Scenario: 已移除设备需要收到决定
Expected behavior: 普通 scope 排除该设备，受限计划仍可发送指定决定；不能取得名单、内容或在线状态。
Implementation: RestrictedDelivery 只读取精确加密计划。
```

```text
Scenario: 正式 AddDevice 已提交后网络中断
Expected behavior: 不回滚；双方继续同一记录，新增设备在激活和效果完成前不进入普通 scope。
Implementation: admission、history、effect 和 outbox 同次提交，恢复只向前。
```

```text
Scenario: effect 完成前进程崩溃
Expected behavior: 重启后从持久 phase 继续；非幂等效果不重复；相关设备继续失败关闭。
Implementation: 以 event_id 和 phase 作为幂等键。
```

```text
Scenario: revision 达到 u64::MAX
Expected behavior: 拒绝新变化并进入明确恢复错误，不回绕或重置。
Implementation: 所有推进使用 checked_add。
```

```text
Scenario: 旧 profile 升级
Expected behavior: 保留允许的本机数据，重建单设备 Space，删除旧关系并提示全部重新配对。
Implementation: 只调用 RebuildSpaceUseCase，不读取旧成员表生成新历史。
```

```text
Scenario: 日志包含敏感输入
Expected behavior: 测试失败；生产日志不出现邀请码、设备标识原文、成员实例、地址、签名、文件名或路径。
Implementation: 使用稳定类别、计数和不可逆短关联值，并增加捕获日志测试。
```

# 8. Testing Strategy

## Unit Test

1. `MembershipLedger`：根历史、加入、移除、决定、关系、transfer、effect 和 revision 的条件提交。
2. 当前 scope：活动、待决定、分叉、无效、本机移除、加入未激活、effect 未完成和读取失败。
3. `RemoveSpaceMemberUseCase`：成功、自身目标、目标不存在、本机已移除、并发冲突和重复请求。
4. `DecideDeviceTrustChangeUseCase`：接受、拒绝、本机确认、重复、过期、并发变化和效果恢复。
5. 历史发送：固定总预算、排序去重、单设备 deferred、ACK 不一致和页数不推进。
6. 历史接收：重复、乱序、冲突、超限、完整验证、待决定、分叉和无效。
7. admission：J0-S3/J3、取消边界、正式提交后不回滚、重复消息、帮助完成和压缩。
8. maintenance：五种 trigger、顺序、失败分流、单轮互斥、pause/resume/shutdown。
9. 日志：代表性邀请码、设备标识、成员实例、地址和文件路径均不出现在捕获输出。

每个测试通过目标用例接口验证。旧浅模块测试在对应目标测试建立后删除，不叠加保留。

## Integration Test

1. 使用 application 内存持久 adapter 关闭并重建全部用例，验证恢复只依赖 port 中的持久事实。
2. 在每个联合提交点注入失败，证明用例不会先后调用多个写入口或对外报告半完成成功。
3. 两个 application 实例基于同一内存 adapter 并发提交，证明 Conflict 不会覆盖较新历史。
4. 使用内存网络 adapter 跑完整分页、断线、重放和受限决定投递。
5. 只经 `SpaceFacade` 完成创建、加入、查询、移除、决定、重置和恢复；测试不能取得内部 use case。
6. Application 内剪贴板、文件、名单、连接和恢复消费者都使用同一 scope，并排除已移除或分叉设备。
7. 用只实现目标窄 ports 的测试 adapter 构造完整 `SpaceApplication`，证明没有隐含旧总对象依赖。

## Regression Test

1. 创建、解锁、锁定失败恢复、普通重置、彻底重置和旧资料重建。
2. 两个 application 实例通过内存网络加入后，双方状态和普通 scope 一致。
3. 三个 application 实例模拟离线移除后接受：保留设备继续进入彼此 scope，被移除设备双向排除。
4. 三个 application 实例模拟拒绝移除：本机分支范围不变，只隔离提议分支。
5. 本机被移除、重启后仍无普通权限；以新成员实例重新加入后才恢复。
6. 加入各阶段断线和重启只继续同一 join，不产生第二成员实例。
7. 当前成员地址缺失仍保留成员事实，但不成为拨号目标。
8. 网络 port 失败只产生 deferred，不触发任何 application 备用传输。

## Repository Checks

必须运行并确认非零测试数量：

```bash
cargo test -p uc-application --lib space --locked -- --list
cargo test -p uc-application --lib space --locked -- --test-threads=1
cargo check -p uc-application --all-targets --locked
```

先确认列出的测试数量非零，再运行完整 application 测试、格式和差异检查。Core、Infra、Engine、绑定、真实数据库、真实 P2P 和设备矩阵全部记为“不在本规格范围”，不能记为“通过”。

# 9. Acceptance Criteria

* [x] `SpaceFacade` 是 application 唯一公开 Space 业务入口；内部用例、owner、coordinator 和 runtime 不公开。
* [x] 每个正式用户动作、查询、网络消息和后台维护都只有一个完整 `execute()` 入口。
* [x] `MembershipStateCoordinator`、`SpaceAdmission` 和 `MembershipConvergence` 已删除。
* [x] `SpaceMembershipState` 和 `WorkspaceSnapshot` 不再作为 application 的业务状态、输入或结果存在。
* [x] V2 成员历史是唯一正向成员资格来源；旧成员表、地址、在线状态和可信关系不能授予权限。
* [x] history、admission、relationship、transfer、effect 和 device trust revision 只通过一个 application 原子提交 port 更新。
* [x] Application 不存在旧 store 接口双写、旧路径转发、兼容别名、回退或 feature 控制的第二实现。
* [x] Application 内普通消费者全部使用同一最终授权 scope；不再组合第二个内容门禁。
* [x] 入站历史每页先保存后 ACK，完整验证前不替换正式历史。
* [x] 接受、拒绝、重复、过期和本机移除确认均返回稳定结果和最新设备信任状态。
* [x] 正式 AddDevice/RemoveDevice 提交后不回滚，未完成效果可跨重启恢复。
* [x] 被移除设备只可完成精确受限决定投递，不能取得普通权限或资料。
* [x] 一个成员 runtime 完成启动、在线触发、定时、唤醒、暂停、恢复和关闭。
* [x] Application facade 不再导出旧综合查询、旧决定或旧总对象结果。
* [x] Application 内旧 gossip、candidate、announcement、outbox 和 applied-security-update 编排已删除。
* [x] Core、Infra、Engine、绑定和数据库均未纳入本规格修改。
* [x] 所有 application 持久资料均按敏感负载传给持久 port，未新增明文缓存或文件。
* [x] 日志不含邀请码、完整设备标识、成员实例、地址、签名、密钥、文件名、路径或内容。
* [x] Application 的模块可见性测试能阻止旧符号、内部实现公开和普通消费者绕过 scope 回归。
* [x] Application 单元、内存集成、格式和差异检查全部通过；外层未接入状态明确记录。

# 10. Risks and Trade-offs

## Single cutover size

一次切换会修改 application 的 Space、facade 及其 application 内消费者，评审面仍然较大。代价是避免长期双轨、转发层和错误的中间结构。风险通过先建立目标接口测试、在独立分支持续保持证据矩阵、最终只合入完整 application 切换来控制。

## External adapter incompatibility

目标 application ports 可能无法由当前 Core/Infra/Engine 直接满足。本规格有意接受这种暂时不兼容，避免根据现有适配器形状反向设计 application。后续接入规格必须实现目标 ports，不能要求 application 恢复旧总对象或步骤级接口。

## Runtime consolidation

单 runtime 内部会包含多类调度，但外部接口显著缩小。内部按 trigger、维护用例和连接调度分文件；业务顺序只在维护用例中，runtime 不复制规则。

## Persistence contract

Application 要求一次条件提交完整保存业务事实，但本规格不决定数据库布局、密文封装或 transaction 实现。这样能先固定业务原子性，同时把具体存储选择留给 Infra 接入规格。代价是 application 验收只能证明调用契约和重启模型，不能证明真实数据库行为。

## Old gossip removal

Application 旧 gossip 同时编排候选确认、公告、地址提升和安全更新传播。删除前必须由正式 admission、history exchange、effect recovery 和普通连接资料更新用例逐项证明接管。Core 类型、Infra transport 和存储即使继续存在，也不属于本规格删除范围，application 不得继续调用它们作为回退。

## No legacy authorization migration

旧 profile 不转换成员授权，会要求用户重新配对。这是规格 026 已确认的产品代价，换取不从不可验证旧资料生成当前权限。

# 11. Open Questions

没有阻止实现的开放问题。以下决定已经固定：

- 使用一次性切换，不接受渐进兼容路径。
- 只重写 application；Core、Infra、Engine、绑定和数据库适配另行接入。
- Application 通过一个原子提交 port 表达所需能力，不指定其具体存储实现。
- 旧 profile 重建单设备 Space，不迁移旧成员授权。
- Application 旧 gossip 的必要行为由正式准入、历史同步、效果恢复和连接资料更新用例接管；外层实现不在本规格删除。
