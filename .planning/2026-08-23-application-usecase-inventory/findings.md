# 发现：Space Application 用例全量梳理

## 2026-08-25：成员历史同步标准用例

- 原 `synchronize_membership_history/use_case.rs` 同时实现网络入站、单设备同步和整 Space 同步，不是标准用例。
- 主业务动作确定为 `SynchronizeMembershipHistoryUseCase::execute()`：在一个固定总预算内，按稳定顺序与当前全部有效成员同步本机历史。
- 当前成员范围不可用是唯一整次失败；单设备离线、拒绝、协议失败或超时只延后该设备，保持原有尽力同步语义。
- 单设备完整交换由 `MembershipHistoryPeerSynchronizerPort` 隐藏，调用方不理解分页、历史导出、确认或关系更新。
- 网络入站处理保留在独立 `exchange.rs`，不属于主动同步用例。
- Space 运行期的上线事件、恢复与显式请求，以及邀请方提交准入前的同步，均复用同一个 UseCase；旧总对象不再公开 `synchronize_chain()`。
## 2026-08-25：Workspace Membership 标准用例梳理

- 当前面向产品的三个动作已经存在标准用例：`QuerySpaceMembershipStatusUseCase`、`InitiateSpaceMemberRemovalUseCase`、`DecidePendingMembershipRemovalUseCase`。Engine 经 `SpaceMembershipFacade` 调用它们，方向正确。
- `WorkspaceMembership` 仍公开 `query()`、`decide_membership_removal()`、`synchronize_chain()`、`handle_membership_history()` 等入口，其中查询和决定与标准用例形成第二套行为表面；dev-tools 仍通过旧 Facade 路径直接使用旧查询与旧决定。
- `WorkspaceMembership` 同时持有状态仓储、加入记录、成员历史、签名、安全更新、成员资料、在线状态、发现通信、重试请求和通知，接口要求调用方理解的知识远大于单一成员用例。
- `membership/bootstrap.rs` 是新建或重建 Space 时的内部生命周期步骤，不是独立用户用例；应由 initialize/reset/rebuild 的完整流程调用，保留内部接口。
- `membership/history.rs` 的入站处理和对端同步是后台通信流程；`synchronize_chain()` 是运行期恢复动作，不是用户用例。应由一个成员历史同步负责人隐藏收发、验证、合并、确认和重试。
- `membership/effects.rs` 的待处理决定投递是重启可恢复的后台效果，不是用户用例；应由成员运行期调用一个完整恢复动作，不暴露逐项步骤。
- `projection/current_scope.rs` 提供内容交换门禁和当前成员范围，是共享查询模块；它服务剪贴板、传输和准入，不能改造成产品用例。
- `projection/snapshot.rs::query()` 是旧开发工具快照，与产品的成员状态查询重复；应让 dev-tools 使用标准查询或明确的诊断查询用例，随后删除旧公开查询。
- `discovery/` 中的候选发现、交换和重试属于另一套事件驱动后台运行期 `MembershipConvergence`，不应拆成用户用例，也不应并入三个成员命令用例。

### 推荐的标准用例表面

1. `QuerySpaceMembershipStatusUseCase::execute()`：唯一产品成员状态查询，组合当前成员资格、成员资料、在线状态、待处理移除、当前加入和待加入成员；只读，不推进流程。
2. `InitiateSpaceMemberRemovalUseCase::execute(target_device)`：唯一发起成员移除动作，负责校验、签名、保存、通知恢复运行期并返回已保存结果。
3. `DecidePendingMembershipRemovalUseCase::execute(change_id, choice, confirm_self_removal)`：唯一决定待处理移除动作，负责幂等、并发变化、自移除确认、签名、原子保存、后续效果和最终状态。
4. 建议新增内部 `RecoverPendingMembershipEffectsUseCase::execute()`：后台唯一恢复动作，完成待投递决定、待发安全更新与必要重试；运行期只触发一次，不理解内部项目。
5. 建议新增内部 `SynchronizeMembershipHistoryUseCase::execute(trigger)`：后台唯一历史同步动作，统一入站处理、主动同步、验证、合并、确认和失败分类；通信端点与运行期均委托它。

### 保留为内部共享模块

- `MembershipHistoryStore`：可靠读取、验证和原子提交成员历史。
- 当前成员范围投影：向内容交换和准入提供稳定成员事实。
- 新 Space 成员基线建立：只服务 initialize/reset/rebuild。
- `MembershipConvergence`：候选发现与成员传播的独立后台运行期。

### 删除检查

- 删除三个标准用例会把产品结果解释、并发、幂等和通知重新散回 Facade，说明它们有真实深度，应保留。
- 删除 `WorkspaceMembership::query` 与 `decide_membership_removal` 后，复杂度不会消失，只会由现有标准用例继续完整承担，说明这两个旧入口是重复表面，应删除。
- 删除当前 `WorkspaceMembership` 总对象会把历史同步、后台效果、投影和生命周期步骤散回多个调用方，说明其内部能力需要保留，但应拆成少量深模块，不应继续作为一个多用途公开对象。
- 最终边界已验证可编译：加入方三个可靠推进阶段由 `JoinerAdmissionProgression` 完整负责；双方共享的持久消息格式、顺序校验、确认构造和错误映射位于 `durable/protocol.rs`。邀请方只依赖共享协议规则，不依赖加入方推进对象。
- `DurableAdmissionTransaction` 的最后一项邀请方行为是成员提交前拒绝；它只依赖加入记录仓储和邀请方候选阶段，已归入邀请方取消与拒绝模块。该总对象现在只剩加入方验证准备、应用提交和完成激活三段角色流程。

## 初始发现

- Space 当前有 11 个标准 `use_case.rs`：initialize、unlock、lock session、recover session、reset、rebuild、upgrade、query access、query membership、initiate removal、decide removal。
- Space 的完整行为数量大于 11：issue/redeem/cancel invitation、join/cancel join、ensure reachable、network recovery、presence refresh、旧开发工具查询与决定仍定义在其他文件形态中。
- admission handshake、durable transaction、session activity、membership runtime 属于内部流程或后台运行期候选，不能因为有公开方法就直接提取成用例。
- “有 `UseCase` 后缀”不是唯一判断标准；需要从 Engine Space 操作、Space Facade、Profile Space Facade 和后台入口反向建立行为清单。
- 当前只稳定 Space application 行为和接口，其他 application 领域与 infra 实现暂缓。
- 刚才的架构检查会隐式执行 `openmls-validation` 构建；已停止相关进程，后续在用户解除限制前不再运行。

## 2026-08-24：Space 加入记录命名

- `AdmissionAttemptV1` 实际是加入方与邀请方共同保存、可跨重启恢复的一次 Space 加入记录，不是临时的准入尝试。
- 记录自身已有 `format_version`，读取路径也负责校验和兼容旧格式；类型名中的 `V1` 不承担兼容作用。
- 本次统一 `SpaceJoinRecord`、`SpaceJoinRecordId`、`CompletedSpaceJoinRecord`、`SpaceJoinRecordStorePort` 和 `SpaceJoinRecordStoreError`。
- 配套格式常量和存储内部类型同步改名，但数值、字段布局和编码顺序不变。
- 普通局部变量和字段 `attempt` / `attempt_id` 保留，避免业务流程表达变得冗长；它们在 Space Join 上下文中已经明确。
- 第二轮分类：`AdmissionCompletionRecovery*V1` 与 `AdmissionIdentityBindingV1` 有独立编码、版本校验和升级拒绝，继续保留版本后缀。
- 加入阶段、角色状态、待发/已收消息、最终结果、拒绝原因和 profile 元数据只是稳定业务状态，由外层记录格式负责兼容，删除 `V1`。
- application 中的本机加入变更、当前加入视图和投递结果只在进程内使用，也删除 `V1`。

## 2026-08-24：准入消息送达结果收口设计

- `RecoverPendingAdmissionsUseCase` 当前同时负责扫描、路由选择、传输、按消息类型解释结果、调用加入方/邀请方/取消规则、统计和终态整理。
- 第 185-283 行实际上是一张完整的“传输结果 + 消息类型 -> 状态推进规则”路由表；恢复流程知道十种消息及每种合法返回组合。
- 多个状态推进规则同时被实时握手调用，不能复制或搬回恢复流程；需要保留单一规则，同时从恢复调用方隐藏逐项路由。
- `durable/settlement.rs` 当前只负责终态压缩检查，是扩展为完整送达结果结算负责人的候选，但仍需核对实时调用、路由准备和传输返回语义。
- 正式 `PairingAdmissionOutboxDelivery` 当前只发送 `CancelRequested`，并只返回远端 `Rejected`；其他消息直接返回暂缓。
- `Persisted`、邀请消费和多种普通消息确认分支目前主要由测试替身驱动，不能据此声称生产恢复已经支持所有消息。
- 实时握手会直接调用邀请方/加入方的同一状态推进规则，因此新的结算模块应复用这些规则，而不是取代或复制实时协议流程。
- `AdmissionOutboxDeliveryPort` 有一个正式实现和多个测试替身；正式实现当前只恢复旧加入取消，测试替身模拟完整消息确认矩阵。
- `AdmissionRecoveryReportV1` 不进入正式结果，只被内部协议回归读取；正式 `execute()` 只返回成功或失败。
- 新负责人不需要新增 port：它是 application 内部具体模块，持有记录存储、安全切换和空间切换能力，生产与测试通过同一具体入口。
- 恢复用例应只负责扫描、选择路由、调用传输和统计；“传输结果是否合法、怎样推进状态、能否整理记录”全部归新负责人。

### 推荐设计

#### `recover_pending_admissions/outbox_recovery.rs`

- 具体模块 `AdmissionOutboxRecovery`，不定义 port。
- 构造时持有记录存储、消息传输和 `AdmissionDeliverySettlement`。
- 唯一动作 `execute() -> Result<AdmissionOutboxRecoveryReport, WorkspaceConvergenceError>`。
- 内部负责扫描可恢复记录、选择取消消息的 continuation/invitation 路由、逐条调用传输、把成功返回交给 settlement、忽略暂时传输失败并汇总 attempted/confirmed/compacted。
- 不解释任何消息结果，不直接调用 joiner/sponsor/cancel 规则，不判断终态整理条件。

#### `durable/delivery_settlement.rs`

- 具体模块 `AdmissionDeliverySettlement`，不定义 port。
- 构造时持有记录存储、安全切换和 Space 切换能力。
- 唯一动作：`settle(attempt_id, sent_message, delivery_result) -> Result<AdmissionDeliverySettlementResult, WorkspaceConvergenceError>`。
- 输入必须包含原发送消息，确保返回类型与消息用途的组合可验证；不能只凭返回结果推进状态。
- 返回只有 `Deferred` 与 `Confirmed { compacted: bool }`，调用方不看到加入方、邀请方或取消内部状态。
- 内部唯一持有“传输结果 + 消息用途 -> 状态规则”的路由表：邀请消费交给加入方消费规则，普通确认记录精确消息，拒绝/完成/后续消息交给邀请方规则，旧加入取消确认交给旧加入清理规则，远端拒绝交给加入方拒绝规则。
- 确认成功后由同一动作重新读取最新记录；满足终态整理条件时调用现有严格整理能力并返回 `compacted: true`，未满足时正常返回 false。
- outbox recovery 收到 `compacted: true` 后立即停止处理该记录的旧消息快照，避免记录已整理后继续发送过期消息，并保证整理计数只增加一次。
- `Deferred` 不读取或修改记录；非法结果组合继续失败关闭，不降级为暂缓。

#### 顶层调用

- `RecoverPendingAdmissionsUseCase` 继续负责旧资料恢复、完成帮助恢复和邀请方激活恢复，然后只调用一次 `outbox_recovery.execute()`。
- `SpaceAdmission` 不新增送达结果方法，避免继续扩张总负责人。
- `SpaceModules` 组装时从现有依赖一次构造 settlement 和 outbox recovery；不让 Facade 或 Engine 看见内部步骤。

#### 删除与迁移

- 从 `recover_pending_admissions/use_case.rs` 删除 `recover_outbox_deliveries`、完整结果 match、`record_protocol_message_delivered`、路由函数和终态整理判断。
- `AdmissionRecoveryReportV1` 改为 `AdmissionOutboxRecoveryReport` 并只属于 outbox recovery。
- `recover_pending_admissions/mod.rs` 不再为测试导出内部自由函数。
- 现有 joiner/sponsor/cancel 状态函数保留为 settlement 的内部依赖，同时继续服务实时握手；待调用面稳定后再收紧可见范围，不在本次复制或合并它们。

#### 验收标准

- 顶层恢复文件不出现 `AdmissionOutboxDeliveryResult` 或跨 joiner/sponsor/cancel 的逐项送达结果调用；邀请方激活恢复仍可使用 Commit/Applied 用途完成其独立恢复步骤。
- outbox recovery 不出现 joiner/sponsor/cancel 状态规则调用，只调用一次 settlement。
- settlement 是唯一结果路由表；新增消息用途只修改该表及其接口级测试。
- 用 settlement 表格回归覆盖全部合法组合与非法组合；用 outbox recovery 回归覆盖暂缓、传输失败、路由回退、确认计数和整理计数。
- 保留实时握手的现有状态回归；删除只验证旧自由函数内部步骤的重复测试。
- 当前正式适配仍仅对 `CancelRequested` 执行真实发送，其他消息保持 `Deferred`。

## 2026-08-24：成员历史 V1 完全删除评估

- 本次评估范围限定为成员历史 V1，不包括仓库中其他独立协议的 `V1` 格式。
- V1 不是只有 `MembershipEventV1Evidence` / `MembershipDecisionV1Evidence`：旧模型以无后缀的 `MembershipEvent`、`MembershipOperation`、`MembershipDecision` 和旧成员历史存在。
- 旧模型仍被生产代码使用：workspace convergence、旧资料 bootstrap、成员效果计算、current scope 投影和 admission base history 都直接读取或创建旧事件。
- `VersionedMembershipEvent::V1Evidence` 与 `VersionedMembershipDecision::V1Evidence` 保存旧签名原文和原始编号，用于在 V2 历史中保留旧授权证据。
- 因此完全删除不是死代码清理，而是移除一条仍在运行的旧资料/旧空间兼容线；必须先确认持久数据迁移和产品支持策略。
- 进一步区分后确认：`MembershipEventV1Evidence`、`MembershipDecisionV1Evidence` 及 `VersionedMembershipEvent/Decision` 的 V1 分支目前没有生产调用，只有定义、导出、测试和规格引用，可作为独立死代码删除候选。
- `VersionedMembershipHistory` 当前实际保存的是 V2 事件，旧资料通过 `MembershipActivationBaselineV2::LegacyAccepted` 的检查点进入，而不是把 V1Evidence 继续放在事件集合中。
- 但无后缀的旧 `MembershipHistory` 仍被生产代码读取和创建，并负责生成该 LegacyAccepted 基线；不能与无调用的 V1Evidence 包装一起直接删除。
- 当前加密 `WorkspaceConvergenceStateV3Payload` 仍序列化 `membership_reconciliation: Option<MembershipReconciliation>`；即使值为空，字段也属于 V3 布局。
- 直接删除旧类型和 V3 读取结构会让现有 V3 工作空间状态无法解码，影响面不是只有尚未升级的旧资料。
- 安全完全删除需要两阶段发布：先引入只保存 V2 成员历史的 V4 工作空间状态并保留 V3 -> V4 读取迁移；确认迁移覆盖后，后续版本才能删除 V3 读取器和旧成员类型。
- 若要求一次提交内完全删除所有旧代码，只能同时宣布现有工作空间状态不兼容，并为所有既有资料提供重置/重新配对结果；这不是内部重构。
- 旧多成员历史无法仅凭当前本地事实自动提升为完整 V2 授权历史；现有安全路径只允许严格的单成员、身份和签名一致场景形成 `FullyVerifiedMigration`，其余保持恢复失败。

### 评估结论

- 可以立即删除：无生产调用的 V1Evidence / VersionedMembershipEvent/Decision 包装及专用测试、导出和规格段落。
- 不应立即删除：`MembershipHistory`、`MembershipEvent`、`MembershipOperation`、`MembershipDecision`、`MembershipReconciliation` 及 V3 状态读取；它们仍是生产兼容和持久格式的一部分。
- 推荐目标是 V2-only，但实施顺序必须是“停止创建旧状态 -> 写入 V4 并迁移 -> 验证真实升级 -> 后续版本删除旧读取器”，不能在当前整理中直接全删。

### 加入“升级必须重置 Space”后的修正

- 不再需要 V3 -> V4 的旧历史迁移，也不需要保留旧资料加入/恢复；可以把旧状态直接判定为必须重置。
- 但新建 Space 当前仍调用 `initialize_legacy_space_membership(true)` 并创建旧 `MembershipEvent`，因此删除前必须先让新建/重置直接建立并保存 V2 单设备根历史。
- 完成 V2 根历史替代后，application 的旧升级入口、旧事件创建、旧效果队列处理、旧移除决定和旧当前范围回退可以整体删除。
- 持久层仍需新状态格式或明确拒绝旧 V3；既然产品要求重置，可以不实现旧历史迁移，但必须让重置入口在旧状态无法进入普通运行前仍可执行。
- core 的 `membership_history.rs` 混放旧状态机与 V2 仍复用的通用类型，不能整文件盲删；应先迁出事件/决定编号、移除选择、关系状态和当前网络消息外壳，再删除旧状态机主体。

### 可清理的 application 代码

- `membership/bootstrap.rs`：用“直接建立并持久化 V2 单设备根历史”替换 `initialize_legacy_space_membership`；随后删除 `initialize_upgraded_legacy_space`、`complete_upgraded_legacy_join`、`record_local_admission_history`、`record_local_removal_history` 和旧事件签名创建代码。
- `membership/admission_base_history.rs`：删除 `verified_legacy_admission_base_history` 以及 `verified_admission_base_history` 的旧历史回退；没有 V2 历史时直接要求重置/恢复。
- `membership/effects.rs`：删除基于旧 `MembershipEvent` 的 `enqueue_applied_membership_effects`、`execute_pending_membership_effects` 和 `recover_pending_membership_effects`；V2 历史接收继续直接保存成员事实和应用安全更新。
- `membership/removal.rs`：删除 `decide_membership_removal_locked` 中 V2 未命中后进入旧 `MembershipDecision` 的整段回退，以及 `submit_legacy_removal_for_test`；产品移除决定只走现有 V2 用例。
- `projection/current_scope.rs`：删除 `membership_reconciliation` 回退，`snapshot()` 只接受 V2 当前历史；缺失历史直接要求重置。
- `membership/history.rs`：`synchronize_chain` 的候选设备改从 V2 active members/成员资料取得，删除从旧 reconciliation 枚举设备的分支。
- `workspace_membership/mod.rs` 及测试辅助：删除旧事件/操作/决定/reconciliation 导入、旧查询和只服务旧历史的测试夹具。
- 删除所有只验证旧升级、旧多成员历史、旧移除决定和 V1 当前范围回退的 application 回归；保留“旧资料只能重置”和“重置后生成 V2 根历史”的产品回归。

### application 清理后可删除的 core 死代码

- `MembershipEventV1Evidence`、`MembershipDecisionV1Evidence`、`VersionedMembershipEvent`、`VersionedMembershipDecision` 及 `InvalidLegacyEvidence` 分支。
- `MembershipOperation`、`MembershipEvent`、`MembershipDecision`。
- `MembershipReconciliation`、`MembershipReconciliationOutcome`、`MembershipHistoryError`。
- 旧状态使用的 `PendingAppliedMembershipEffect`、`PendingMembershipDecisionDelivery` 以及 `SpaceMembershipState` 中对应队列。
- `SpaceMembershipState.membership_reconciliation` 字段，以及 `effective_members`、旧设备查找/移除判断等只从该字段推导的方法。
- `workspace_convergence.rs` 中创建和推进旧成员历史的事件分支与回归。

### 需要保留但应迁出旧文件的 core 类型

- `MembershipEventId`、`MembershipDecisionId`：V2 仍使用。
- `RemovalDecision`：V2 移除决定仍使用。
- `MembershipHistoryRelationship`：当前 V2 peer 关系仍使用。
- `MembershipHistoryMessage`：当前只含 `HistoryPageV2` / `AckV2`，仍是正式网络外壳。
- `PendingRemovalFacts`：当前成员状态结果仍使用；可保留或改为更贴近产品结果的名称。

### 必须同步处理的持久层条件

- 新状态格式不再包含 `membership_reconciliation` 和两个旧队列；旧 V3 状态不得进入普通恢复。
- 强制重置判断必须位于旧状态完整解码之前，且重置操作必须能在不解析旧成员历史的情况下清除旧状态并建立全新 V2 Space。
- `MembershipCredential`、签名算法、准入消息等其他仍在使用的 `V1` 格式不属于本次成员历史清理，不能因名称带 V1 一并删除。

### 推荐实施顺序

1. 删除无生产调用的 V1Evidence 包装。
2. 让新建/重置 Space 原子建立 V2 根历史，并以它作为唯一成员事实。
3. 删除 application 的旧初始化、旧回退、旧效果和旧移除路径。
4. 删除 `SpaceMembershipState` 旧字段并调整持久层为重置后新格式。
5. 迁出 V2 共用类型，删除 core 旧状态机主体和旧测试。
6. 验证旧资料只进入重置、新资料可正常建立、加入、移除、重启和多设备同步。

## 分类口径

- 标准用例：一个入口负责一个完整结果。
- 内部流程：只服务某个用例，不应独立暴露。
- 后台运行期：负责触发、合并、重试、暂停和关闭。
- 共享模块：隐藏多个用例共同需要的复杂规则或可靠访问。
- Facade：对外提供完整动作和稳定结果，不逐步编排。
- 纯转发：删除后行为几乎不变，应合并或重新划分。

## Space 生命周期第一批清单

| 行为 | 当前负责人 | 当前判断 | 后续动作 |
| --- | --- | --- | --- |
| 初始化 Space | `InitializeSpaceUseCase` + `SpaceFacade` + `AppFacade` | 标准入口已存在，但完整成功仍由三层拼接 | 收口存储就绪与 Space 活动恢复，Facade 只转换输入输出 |
| 解锁 Space | `UnlockSpaceUseCase` + `AppFacade` | 用例已负责解锁与数据准备，但 Space 活动恢复仍在 Facade | 将活动恢复纳入完整解锁结果 |
| 锁定 Space 会话 | `LockSpaceSessionUseCase` | 完整用例，包含暂停、锁定失败回滚和稳定错误 | 保持 |
| 静默恢复 Space 会话 | `RecoverSpaceSessionUseCase` | 完整用例，包含恢复、数据准备和活动恢复 | 保持 |
| 重置 Space | `ResetSpaceUseCase` | 完整用户命令，取消邀请后执行可恢复重建 | 保持 |
| 重建 Space | `RebuildSpaceUseCase` | 完整可恢复内部流程，不是独立产品动作 | 保持为 reset/upgrade 的内部用例 |
| Engine 版本升级 | `UpgradeSpaceUseCase` | 完整内部迁移流程，由会话就绪阶段调用 | 保持内部，不作为 Facade 用户动作 |
| 查询 Space 访问状态 | `QuerySpaceAccessStateUseCase` | 完整只读用例 | 保持 |
| 查询 Setup 状态 | `QuerySpaceSetupStateUseCase` | 已提取完整产品查询 | 保持，Facade 只调用 `execute()` |
| 查询重置是否已提交 | `SpaceFacade::has_committed_device_management_reset` | Session Supervisor 使用的完整内部查询藏在 Facade | 迁入 reset 模块，单独评估名称和结果 |
| 会话数据准备 | `PostSessionReadiness` | unlock/recover 共用的内部流程，不是用户用例 | 保持内部，避免独立暴露 |
| Space 活动开关 | `SpaceSessionActivity` | lock/recover/initialize/unlock 共用的运行协调 | 保持内部，由完整生命周期用例调用 |

## 生命周期关键发现

- `InitializeSpaceUseCase::execute()` 完成加密 Space、身份、成员基线和当前 Space 激活后，`SpaceFacade` 仍执行关系存储读取与 presence 激活，`AppFacade` 再恢复 search、receive 和 membership 活动；对外成功由三层共同完成。
- `UnlockSpaceUseCase::execute()` 已负责升级、回填、成员仓储检查和 presence 激活，但 `AppFacade` 仍在返回前恢复 search、receive 和 membership 活动。
- `RecoverSpaceSessionUseCase` 已把会话恢复、数据准备和 Space 活动恢复放在同一入口，可作为 initialize/unlock 的完整性参考。
- `LockSpaceSessionUseCase` 在锁定失败时恢复已暂停活动，隐藏了调用顺序和补偿逻辑，属于有效的深模块。
- `QuerySpaceAccessStateUseCase` 只回答是否初始化与会话是否就绪；`query_setup_state` 还回答当前邀请、设备名和重新配对要求，两者不是重复查询。

## 生命周期整改顺序

1. `QuerySpaceSetupStateUseCase` 已完成提取。
2. 收口 initialize 的关系就绪与活动恢复，删除两个 Facade 的后处理。
3. 收口 unlock 的活动恢复，参照 recover 的完整入口。
4. 将重置提交状态查询迁入 reset 模块。
5. 最后复核 `PostSessionReadiness`、`SpaceSessionActivity` 的命名和可见范围，不把它们提取成用户用例。

## 已完成：Query Space Setup State

- 模型、错误和四项读取编排已从 `SpaceFacade` 迁入 `space/query_space_setup_state`。
- 用例唯一入口为 `execute()`，输入为空，成功返回当前 Space、邀请、设备名和重新配对要求，读取失败返回稳定查询错误。
- `SpaceFacade` 只保留一次转发，原 `uc_application::facade` 导出路径保持兼容。
- 新增全新安装、已完成状态和待处理邀请三个用例级回归；按当前约束未运行构建或测试。

## Space 准入第一批清单

| 行为 | 当前负责人 | 当前判断 | 后续动作 |
| --- | --- | --- | --- |
| 签发普通邀请 | `admission/invitation/issue` | 完整用户用例 | 保持，后续迁出 Facade 命令模型 |
| 按地址签发邀请 | `admission/invitation/issue_for_address` | 已提取独立 dev 用例 | 保持内部，不向正常产品暴露 |
| 查询邀请地址 | `admission/invitation/query_addresses` | 已提取独立查询用例 | 保持，普通 issue 不再依赖查询 Port |
| 取消邀请 | `admission/invitation/cancel` | 已提取完整用户命令 | 保持，Facade 只调用 `execute()` |
| 加入 Space | `SpaceAdmissionCoordinator::join_space` + `RedeemPairingInvitationUseCase` + Engine | 完整结果跨 application 与 Engine 拼接 | 建立 `JoinSpaceUseCase`，收口设备名、准入与持久结果 |
| 执行邀请兑换 | `RedeemPairingInvitationUseCase` | Join Space 内部通信与持久化流程 | 保持为 Join Space 内部用例，不直接暴露 |
| 取消本机加入 | `CancelSpaceJoinUseCase` | 已提取完整用户命令 | 保持 |
| 查询当前加入 | `QuerySpaceJoinStatusUseCase` | 已提取持久准入投影查询 | 保持；普通 Join 直接返回自身结果 |
| 完成跨 Space 会话切换 | Application transition recovery + Engine Session Supervisor | Engine 必须拥有会话 drain/rebuild，application 拥有持久转换 | 保留跨层协作，但让 Join 结果明确是否需要切换，避免完成后再次探测 |
| 重建并发送完成确认 | `pending_joiner_complete_ack` + `deliver_join_completion_ack` + Session Supervisor | 重启可恢复的会话安装后动作，当前分散在两个 Facade | 后续收口为一个明确的会话安装后准入恢复入口 |
| Sponsor 入站处理 | `SponsorAdmissionOrchestrator` | 后台运行期，不是用户用例 | 保持内部 Runtime |
| Joiner/Sponsor handshake | 两个 handshake coordinator | Join/入站准入的内部协议流程 | 保持内部，不独立暴露 |
| Durable admission transaction | `DurableAdmissionTransaction` | 保存检查点、幂等推进和恢复的共享 durable workflow | 保持深层内部模块，不把每个阶段提取成用例或 Port |

## 准入关键发现

- `SpaceAdmissionCoordinator` 只有 `join_space` 包含真实编排；其余五个方法只是转发到 `SpaceFacade`。应替换为单一 `JoinSpaceUseCase`，而不是保留“准入总协调器”。
- `IssuePairingInvitationUseCase` 同时暴露普通签发、按地址签发和地址查询三个入口，混合产品命令、开发工具命令和查询，应拆成三个行为。
- `cancel_invitation` 已完整实现“没有邀请时报冲突、存在时全部取消”，但仍写在 Facade。
- `cancel_join_space` 已完成持久取消、profile 修订读取和事件通知，但仍写在 Profile Facade。
- Engine 当前丢弃 application `join_space` 的返回值，再通过 `ProfileSpaceAdmission::current_join` 查询一次持久状态；application 的 Join 结果没有成为单一事实出口。
- 跨 Space 加入成功后，Engine 再调用 `requires_session_transition()` 决定是否 drain 当前会话。该事实在 `RedeemPairingInvitationUseCase` 内已经计算过，应随 Join 结果返回，避免成功后重复探测。
- Engine 必须负责关闭和重建完整运行会话，因此跨 Space 切换不可能完全藏进 application；application 应只暴露“是否需要切换”和“会话已 drain 后继续恢复”的明确接口。
- Port 迁入 application 时不能只迁 trait：仓储错误、原子变更、查询投影、投递结果，以及安全和空间切换的请求/结果也属于该接口，继续放在 core 会让 application 的接口仍由 core 塑形。
- `SponsorAdmissionSecurityDelivery` 是例外：它被序列化进可恢复准入记录，属于持久领域状态，不是单纯的 Port 返回值，因此留在 core 的准入记录模块，由 application 的安全接口复用。

## 准入整改顺序

1. Cancel、地址查询和按地址签发均已完成拆分。
2. 已将 `SpaceAdmissionCoordinator` 收敛为 `JoinSpaceUseCase`，并删除其余转发。
3. 让 Join Space 直接返回持久 `CurrentJoinStatus` 和是否需要会话切换，删除 Engine 成功后的重复查询与探测。
4. 提取 `CancelJoinSpaceUseCase`，移出 Profile Facade。
5. 收口会话安装后的完成确认恢复入口。
6. 保持 handshake、durable transaction 和 sponsor orchestrator 为内部流程，不按阶段拆成公开用例。

## 已完成：Cancel Pairing Invitation

- 用例唯一入口为 `execute()`，输入为空；至少一个待处理邀请被清除时成功，holder 为空时返回 `NotIssued`。
- 错误归用例模块所有，原 `uc_application::facade` 导出路径保持兼容。
- `SpaceFacade` 不再直接操作或持有取消所需 holder，只保留用例字段和一次转发。
- Facade 回归改为通过 Setup 状态观察邀请是否清除，不再读取内部字段。
- 新增 holder 为空与多个邀请全部取消两个用例级回归；按约束未运行构建或测试。

## 已完成：Invitation 子域内聚

- `issue` 与 `cancel` 已迁入 `space/admission/invitation/`，与共享 `holder` 放在同一子域。
- admission 根目录旧 `issue_invitation.rs` 和 Space 顶层旧 `cancel_pairing_invitation/` 已删除，不保留转发模块。
- 后续 invitation address query 直接建立在同一子域。
- `QuerySpaceSetupStateUseCase` 继续位于 Space 顶层：它虽然读取 holder，同时还组合当前 Space、设置和重新配对要求，不属于纯邀请行为。

## 已完成：Query Pairing Invitation Addresses

- 查询用例位于 `admission/invitation/query_addresses`，唯一入口为 `execute()`。
- 输入为空，成功返回当前可签发地址候选；网络未启动、服务不可用、地址不可用和内部失败使用独立查询错误。
- 普通 `IssuePairingInvitationUseCase` 已删除地址查询 Port、字段、构造参数和 `list_addresses()`。
- Space/Application Facade 改为返回查询专用错误；Engine dev 操作仍只转换地址结果或稳定内部错误。
- 新增候选返回和网络未启动错误两个用例级回归；按约束未运行构建或测试。

## 已完成：Issue Pairing Invitation For Address

- dev 用例位于 `admission/invitation/issue_for_address`，唯一入口接收明确 IP 并返回现有邀请结果。
- 普通 issue 用例已删除 by-address Port、字段、构造参数和第二个 execute 入口。
- 两个签发用例共享内部 `PairingInvitationIssuer`，统一负责准入门禁、分析事件、领域邀请生成和 holder 保存，没有复制完整流程。
- 既有指定地址回归已改为直接执行 dev 用例；Engine `DevOperation::IssueInvitationForAddress` 外部行为不变。

## 已完成：Join Space 用例入口

- `JoinSpaceUseCase` 是加入 Space 的唯一 application 命令入口，依次负责可选设备名保存、加入前尽力激活连接，以及执行邀请兑换。
- 设备名为空时在联网前失败；设置保存失败时不开始准入。连接预热失败不会遮蔽后续邀请兑换给出的可操作错误。
- `SpaceAdmissionCoordinator` 已删除；邀请签发、地址查询、直接兑换和取消不再经过总协调器转发。
- `AppFacade` 只经 `SpaceFacade` 暴露这些完整动作，不再额外组装准入行为。
- 本步尚未改变 Join 的成功结果。Engine 成功后重复读取持久加入状态、再次判断会话切换的问题，按路线图在下一小步处理。
- 加入方邀请兑换只服务 Joiner，不属于 admission 根级共享流程；已迁入 `admission/joiner/redeem_invitation.rs`，与加入方握手共同由 `joiner` 模块隐藏。外层 `JoinSpaceUseCase` 不再依赖 admission 根级具体文件。
- 加入和跨 Space 切换之间存在必须持久化的中断边界，因此对内拆为两个完整用例：Join 保存并返回当前加入状态和明确切换要求；`CompletePendingSpaceTransitionUseCase` 只在旧会话关闭后推进切换，并要求最终状态为活动。Engine 不再扫描准入记录解释是否切换，也不再在正常加入后补查状态。
- 程序启动时仍需判断是否存在已经准备好的切换，因为此时没有上一次 Join 返回值；该内部查询通过 Space Facade 完成，确认后调用同一个完成切换用例，不恢复 Engine 对准入内部对象的直接依赖。
- `durable/flow.rs` 曾把加入方、邀请方、取消加入和空间切换 17 个入口实现到同一个负责人上；现已按协议角色拆除。正常握手与重启恢复继续复用同一份加入方或邀请方可靠推进规则，不把每个协议阶段伪装成产品用例。
- 加入方与邀请方不再共同依赖一个包含全部协议步骤的接口。两侧接口分别位于自己的模块，测试替身也只实现对应角色所需能力；共享 adapter 文件只保留两侧共用的值和稳定请求绑定。
- `ProfileSpaceAdmission` 已删除。`SpaceJoinFacade` 只组合查询当前加入、取消加入和恢复完成确认三个 profile 范围用例；`SpaceMembershipFacade` 只组合成员状态查询、发起移除、决定移除和活动 Space 接入。Engine 不再通过一个名称模糊的总对象混用两类能力。
- core 中原有准入仓储、投递、完成恢复、空间切换、安全切换和成员材料 Port 均没有 core 领域调用者；它们已迁入 application 并由 infra 直接实现。成员历史验签仍留在 core，因为版本化成员历史的验证规则直接使用它。
- `GroupAdmissionPort` 原有成员接纳和安装方法没有 application 调用者，迁移后删除；新的加入方接口只表达准备加入、读取准备中公开凭据和使用准备中身份签名，避免继续暴露底层 MLS 管理全集。

## Space 成员关系第一批清单

| 行为 | 当前负责人 | 当前判断 | 后续动作 |
| --- | --- | --- | --- |
| 查询成员关系状态 | `QuerySpaceMembershipStatusUseCase` | 完整产品查询 | 保持 |
| 发起成员移除 | `InitiateSpaceMemberRemovalUseCase` | 完整产品命令 | 保持 |
| 决定待处理移除 | `DecidePendingMembershipRemovalUseCase` | 完整产品命令 | 保持 |
| 查询设备列表 | Engine + `MemberRosterFacade::list_with_presence` | 初始化判断和列表查询跨层拼接 | 提取 `QuerySpaceDevicesUseCase`，application 决定未初始化结果 |
| 查询成员同步偏好 | `MemberRosterFacade::get_sync_preferences` | 完整查询藏在 Facade | 提取独立查询用例 |
| 更新成员同步偏好 | `MemberRosterFacade::update_sync_preferences` | 完整命令藏在 Facade | 提取独立命令用例 |
| 查询 Space 保护状态 | `MemberRosterFacade::query_space_protection` | 完整查询藏在 Facade，且使用全量资料投影作为输入 | 提取并重新核对当前成员范围 |
| 查询 peer 快照 | `MemberRosterFacade::list_peer_snapshots` | 完整宿主查询藏在 Facade | 提取独立查询或与设备列表统一产品模型 |
| 订阅 presence | `MemberRosterFacade::subscribe_presence_events` | 事件订阅，不是用例 | 保持 Facade 订阅能力 |
| 旧成员决定 | `WorkspaceMembership::decide_membership_removal` | 仅 dev-tools 使用，与新决定用例并行 | 最终删除或让 dev-tools 复用新用例，不保留第二套生产逻辑 |
| 旧收敛快照查询 | `WorkspaceMembership::query` | 仅 dev-tools 与旧内部流程使用 | 评估诊断需求后删除或改为明确诊断投影 |
| 成员历史交换与接收 | `WorkspaceMembership` history endpoint | 已认证网络端点内部流程 | 保持内部 endpoint，不提取为用户用例 |
| 成员效果恢复与决定投递 | `WorkspaceMembershipRuntime` | 后台恢复流程 | 保持 Runtime，后续随旧负责人拆分迁移 |
| 成员历史可靠访问 | `MembershipHistoryStore` | 多个用例共享的深模块 | 保持 |
| 成员状态与事件 | `membership_state` | 共享状态和进程内失效通知 | 保持 |

## 成员关系关键发现

- 三个新成员用例已形成清晰产品入口，Engine 通过 `ProfileSpaceAdmission` 调用；但该 Facade 同时承载准入投影和成员命令，名称与职责需要在最终 Facade 收口时处理。
- Engine `ListDevices` 先查询加密状态，再调用 Roster Facade；“未初始化返回空列表”仍由 Engine 决定，属于 application 查询结果的一部分。
- Roster Facade 中同步偏好读写和保护状态查询都已经包含完整规则，不是单纯输入输出转换。
- `WorkspaceMembership` 剩余公开方法多数是准入适配、网络 endpoint 或后台恢复步骤，不能逐个提取成用户用例；应跟随它们所属的 Join、Runtime 或共享模块迁移。
- dev-tools 的旧决定入口与新决定用例表达同一用户事实，长期保留会形成两套行为。

## Space 连接与运行期第一批清单

| 行为 | 当前负责人 | 当前判断 | 后续动作 |
| --- | --- | --- | --- |
| 刷新全部当前 peer 可达性 | `EnsureReachableAllUseCase` | 完整命令，返回逐 peer 汇总 | 保持，统一命名与 Facade 入口 |
| 手动恢复网络会话 | `NetworkRecoveryFacade::request_recovery` | 深层命令，包含合并与有界重试 | 保持行为，负责人改名为 Coordinator/Runtime 更准确 |
| 查询网络恢复状态 | `NetworkRecoveryFacade::status` | 同一恢复负责人状态查询 | 保持，不拆出无状态转发用例 |
| 网络变化观察 | `observe_*` | 自动恢复 Runtime 的内部输入 | 保持内部，不作为用户用例 |
| 成员保持连接 | `MembershipConnectivityRuntime` | 后台重试与退避运行期 | 保持 Runtime |
| 启停全部 Space 后台任务 | `SpaceApplicationRuntime` | 聚合启动、活动控制和关闭 | 保持 Runtime |
| 查询配对 peer 编号 | `SpaceFacade::list_paired_peer_device_ids` | 无生产调用者 | 删除候选 |
| 确保单个 peer 可达 | `SpaceFacade::ensure_reachable_one` | 无生产调用者 | 删除候选 |
| Space 关闭清理 | `SpaceApplicationRuntime::shutdown` + `SpaceFacade::on_shutdown` | Runtime 为关闭 pairing orchestrator 反向持有 Facade | 将 pairing orchestrator 句柄归入明确的 admission runtime，移除 Runtime 对 Facade 的依赖 |

## 连接与运行期关键发现

- `EnsureReachableAllUseCase` 同时用于用户刷新和会话就绪后的尽力恢复，核心行为一致；调用方只决定是否传播失败，属于合理复用。
- `NetworkRecoveryFacade` 实际拥有状态机、同轮合并、自动窗口、退避和关闭，不是普通 Facade；拆成多个小用例会泄露状态机，应保留深模块并调整定位。
- Engine 直接持有网络恢复负责人并转换稳定结果，这个入口本身完整；最终只需统一 Space Facade/Runtime 的对外组织，不应把观察方法暴露给产品。
- `SpaceApplicationRuntime` 正确聚合三个后台运行期，但只为调用 `SpaceFacade::on_shutdown` 而持有整个 SpaceFacade，说明 sponsor pairing runtime 的所有权仍不清晰。

## Space 全量梳理后的优先级

1. 生命周期：提取 Query Space Setup State。
2. 准入：提取 Cancel Invitation 与 invitation address query，收敛 Join Space。
3. 成员：提取 Query Space Devices、同步偏好读写与保护状态查询。
4. 生命周期：收口 initialize/unlock 的活动恢复。
5. 准入：提取 Cancel Join，并收口 Join 后会话切换结果。
6. Runtime：移除无调用者 reachability 转发，理顺 pairing shutdown 所有权。
7. 最后清理旧 WorkspaceMembership dev-tools 旁路和已迁出公开方法。

## Durable Admission Transaction 复核

- `durable/transaction.rs` 约 3400 行，同时承担查询投影、本机加入建立、邀请方状态推进、加入方状态推进、取消与拒绝、跨 Space 切换、待发送消息恢复、终态压缩、完成协助、消息编码和错误映射；它已经不是一个单一事务模块。
- 加入方与邀请方的正常协议推进已有各自 `durable_flow.rs`，但状态转换实现仍集中在共享 transaction，维护者必须在角色流程和共享文件之间来回追踪，角色内聚尚未真正完成。
- `DurableAdmissionProjection` 混合只读查询和 `cancel_local_join` 写入；当前加入查询应归查询用例，取消写入应归取消用例，不应继续以 projection 名义组合。
- `start_join`、`start_join_before_network`、`start_join_with_recovery_material`、`request_cancel`、`sponsor_remove_pending_member`、`record_admission_unavailable` 等入口没有生产调用者，只被旧测试直接调用。应先删除或改由测试走真实角色入口，不能继续作为共享模块表面。
- `recover_with` 负责扫描未完成记录并投递待发送消息是合理的共享恢复能力，但它内部直接调用多个 joiner/sponsor 状态转换，仍掌握两侧语义。后续应让恢复调度只判断待发送消息种类，再委托对应角色负责人确认结果。
- 真正应留在 `durable/` 的知识是：按版本原子保存准入记录与成员历史、加载与终态压缩、收件去重、待发送消息和确认的稳定编码、可恢复记录扫描，以及通用错误映射。这些能力是角色流程的内部保存工具，不是另一个公开用例。
- 跨 Space 切换只属于加入方完成阶段，应迁入 joiner 侧内部完成模块；完成协助恢复继续留在 durable 子模块，但只通过窄保存工具读写记录。

### 推荐拆分顺序

1. 先删除或收口没有生产调用者的旧 transaction 入口，并把测试改到真实入口，缩小表面。
2. 将 `DurableAdmissionProjection` 拆到查询加入与取消加入两个现有用例中。
3. 将 `sponsor_*` 状态转换迁入 sponsor 内部流程，将 `joiner_*`、加入准备和跨 Space 完成迁入 joiner 内部流程。
4. 提取仅供内部复用的准入记录保存工具，集中版本递增、原子历史提交、加载和终态压缩。
5. 最后重写恢复调度，使它只扫描和投递，再委托角色负责人处理确认；完成后删除 `DurableAdmissionTransaction` 总对象和 `transaction.rs`。

第一步已完成首批清理：`start_join`、`start_join_before_network` 和 `start_join_with_recovery_material` 及其共用旧建档实现已删除。仍有业务价值的“联网前保存”和“重启恢复材料不变”回归改走真实加入准备入口；其他协议回归通过测试夹具预置初始记录，不再把旧入口当作共享接口。
