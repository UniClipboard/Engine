# 规格 023：可持续验证的成员历史与准入激活

## 状态

- **状态**：成员历史与激活语义已实现；准入 wire/runtime 已由规格 028 取代，实体设备验收待补
- **日期**：2026-08-15
- **最新修订**：2026-08-18，完成 ADR-022 的用户明确加入与安全取代规则
- **实施规格**：`docs/specs/025-user-initiated-join-supersession.md`
- **问题基线**：`3eb6fee5a9e7d9f2b03660d5916e338274436c60`
- **细化**：ADR-017 的准入完成边界、ADR-020 的成员历史验证规则、规格 022 的新增成员运行门禁
- **相关文档**：`docs/adr/017-pairing-as-workspace-admission.md`、
  `docs/adr/020-membership-reconciliation-and-user-decisions.md`、
  `docs/adr/022-user-initiated-join-supersession.md`、
  `docs/specs/017-pairing-as-workspace-admission.md`、
  `docs/specs/022-current-member-runtime-scope.md`、
  `docs/specs/025-user-initiated-join-supersession.md`、
  `docs/architecture/architecture-bible.md`

> **后续修订**：ADR-024 已取代本文关于普通 `ResetSpace` 的静止门禁、投影水位推进和轻量清理规则。
> 本文其余准入、防重放、加入取代和 `FactoryResetSpace` 规则继续有效。

本文是以下两项行为的唯一实现与验收依据：

1. 成员被移除后，其过去合法签署的成员历史如何继续被新设备验证；
2. 新设备在双方保存同一成员历史和安全状态后，何时才取得普通成员权限。

规格 017 继续定义“配对只是工作空间内部通信通道”，ADR-020 继续定义离线分支、移除决定和分叉，
规格 022 继续定义普通运行范围。上述文档不得各自复制本文的验证算法、准入状态机或迁移规则。

# 1. Overview

当前签名成员历史把事件作者记录为 `MemberInstanceId`，但 `AddDevice` 没有保存用于验证该成员后续签名的
长期凭据。应用层验证历史事件时，先从当前 `applied_head` 而不是事件的精确父位置解析作者设备，再通过
应用层定义的 `CurrentMemberSignaturePort` 从当前 OpenMLS 成员状态中查找验证身份。成员一旦被后续 `RemoveDevice`
移出当前已应用分支或当前 OpenMLS 树，新加入设备即使收到完整、连续且当时合法的历史，也不再保证能
确认该作者当时有资格或取得其旧公钥。

典型失败链如下：A 加入 B，B 加入 C，A 移除 B，之后 C 加入 D。D 必须验证 B 当时有权签署的“加入 C”
事件，才能验证后续历史；当前实现却要求 B 仍存在于 D 安装后的当前 OpenMLS 树。于是移除操作会破坏
过去历史的可验证性，D 把合法历史判为无效并关闭内容同步。

当前准入顺序还有第二个缺口：发起方先生成并应用组安全更新，加入方在 `Ready` 前安装该安全状态；发起方
随后保存 `AddDevice` 并发送只含历史摘要和发起方事实的 `AdmissionSaved`，加入方到这之后才拉取并验证
完整成员历史。如果最后核对拒绝、断网、崩溃或永久失联，发起方已经形成正式成员事实，加入方却没有
同一已验证历史；双方已安装的安全状态也缺少一个可共同恢复的完成边界。重试还可能重新生成成员实例或
安全状态，无法从一个稳定位置恢复。

现场的直接触发点是当前 Sponsor 从完整成员资料列表生成安全更新接收者，只排除本机和正在加入的设备，
没有先按已接受历史和激活状态筛选。成员资料在移除后为历史验证和受限恢复继续保留本身是正确行为，也
不会恢复成员资格；错误在于准入流程把这些历史资料误当成当前授权范围，因此已移除设备仍可能被选入
新成员的安全更新接收者。删除资料会破坏历史验证，正确修复必须改为只从当前已激活成员投影选择接收者。

本规格用同一方案关闭两个缺口：

- 每次成员身份首次进入历史时，永久保存版本化验证凭据；验证事件时只看精确父历史中的作者资格，并用
  该历史保存的精确公钥验签。移除只取消后继权限，不删除过去的验证材料。
- `AddDevice` 在加入方验证完整历史、持久暂存同一候选和目标安全状态之前不进入正式历史。正式写入后只
  向前完成同一事件和同一安全状态，不回滚、不改写、不生成第二成员实例。
- `AddDevice` 仍是成员资格的唯一正向事实。激活证明只是一道减少权限的临时门禁，不能单独授予成员资格。

完整结果由 `WorkspaceConvergence` 唯一负责。调用方只执行一次 `JoinSpace`；成功、等待和稳定拒绝分别
返回 `Active`、`Pending` 和 `Rejected`。后台恢复、消息重发和进程重启不需要产品端继续任何内部步骤。

# 2. Goals

- 任意设备都能仅凭一条连续成员历史、其版本化验证材料和必要的迁移检查点，验证每位事件或决定作者
  在对应历史位置是否有资格以及签名是否正确，不查询当前 OpenMLS 成员树、成员资料表或在线状态。
- 移除成员后永久保留其旧身份验证材料；该成员可以证明过去合法签署的事件，但不能签署包含其移除的
  当前分支后继。
- 加入方在正式 `AddDevice` 写入前验证完整基础历史，并持久暂存与精确候选绑定的目标安全状态。
- 正式 `AddDevice` 写入后，任意断网、崩溃、丢包、重复或乱序都只恢复同一尝试、事件、成员实例和安全
  状态，不回滚正式历史。
- 新成员在激活完成前不能进入普通设备列表、在线状态、邀请、成员公告、内容、文件、补送或恢复范围。
- 准入目标安全状态的既有接收者只来自候选父历史中已经激活的当前成员；永久历史凭据和残留成员资料
  不会让已移除设备重新取得安全更新。
- `WorkspaceConvergence` 从一次 `JoinSpace` 负责到 `Active`、`Pending` 或 `Rejected`，产品端、绑定和
  `uc-engine` 不编排候选、历史分页、暂存、提交、确认或重试。
- 普通 ResetSpace 不能取消、拒绝或清除未收束准入；FactoryResetSpace 作为现有的明确本机销毁入口，在
  没有活动 Space 时也能按密钥优先、只向前恢复的顺序清除完整 profile。
- 旧数据迁移诚实区分逐条完整验证、接受精确旧前缀和必须人工恢复；不清空数据、不补签旧事件、不把
  未验证历史标成已验证。
- 事件/决定格式、网络协议、加密存储格式和签名算法分别版本化；任何未知版本都返回需要升级，不误报签名
  无效、历史分叉或资料损坏。
- 所有新增持久化负载使用 MasterKey AEAD；数据库、WAL、SHM、缓存、索引和日志不出现敏感明文。
- 桌面、iOS、Android 和 HarmonyOS 依赖同一 `uc-engine` 契约，并对三类加入结果保持一致映射。

# 3. Non-Goals

- 不改变 ADR-020 的移除接受、拒绝、离线分支或分叉规则。
- 不引入服务器、固定主设备、OpenRaft、投票或多数确认；一个当前分支中的有效成员仍可在其他成员离线时
  发起加入。
- 不把成员资料、可信关系、地址、在线状态或当前 OpenMLS 树变成第二份成员事实来源。
- 不重新设计设备身份。首版继续使用现有 Ed25519 准入凭据，并继续由设备标识和该次凭据派生
  `MemberInstanceId`。
- 不改变 P2P 默认能力，也不因 P2P 失败自动切换 LAN 兼容线。
- 不在本规格设计不同 Space 之间退出来源 Space 的产品决策；目标 Space 激活前不得提前破坏来源 Space。
- 不让候选读取普通内容、无关成员地址目录或不受限的历史；正式提交后可以只提供完成同一尝试所需的
  有界、受限恢复路由。
- 不长期双写旧版和新版成员事件，不保留可绕过新版验证的旧成功路径。
- 不以删除旧成员资料、清空数据库或重新配对作为迁移方案。
- 不在本规格定义产品页面的视觉和文案，只定义产品必须收到的稳定事实。

# 4. Current Architecture Context

```text
Component: VersionedMembershipHistory
Path: crates/uc-core/src/membership/membership_history.rs
Responsibility: 保存单父事件历史、known_head、applied_head、成员操作、决定和有效成员计算。
Relationship: V1 事件保存作者成员实例和签名，但 AddDevice 未保存成员历史验签公钥；receive_verified
              假定调用方已完成签名验证。
```

```text
Component: WorkspaceConvergenceState
Path: crates/uc-core/src/membership/workspace_convergence.rs
Responsibility: 保存工作空间历史关系、待完成成员效果和待处理准入。
Relationship: AdmissionChangeFacts 不含成员历史验证凭据；PendingAdmissionRecord 只绑定设备、邀请代次
              和时间，不能恢复精确候选、安全状态或双方确认阶段。
```

```text
Component: CurrentMemberSignaturePort
Path: crates/uc-core/src/membership/ports.rs
Responsibility: 使用当前成员安全状态签名或验证成员资料。
Relationship: 接口按 member/device 从当前状态解析身份，既承担当前签名，又被历史验证复用，导致过去
              历史的可验证性依赖当前 OpenMLS 树。
```

```text
Component: WorkspaceConvergence
Path: crates/uc-application/src/space/convergence/mod.rs
Responsibility: 成员历史核对、用户决定、成员效果、准入记录、恢复和完整查询的唯一负责人。
Relationship: 当前 verify_membership_history_event 通过 applied_head 解析作者设备，而不是事件父位置，
              再调用当前成员签名 Port；发起方先提交新增，加入方之后才核对完整历史。
```

```text
Component: Space admission
Path: crates/uc-application/src/space/admission/
Responsibility: 发行邀请、发起方握手、加入方赎回和空间切换接线。
Relationship: 当前加入方在 Ready 前已安装安全状态，发起方之后保存 AddDevice；AdmissionSaved 只通知
              摘要和发起方事实，加入方再拉取并验证完整历史。Sponsor handshake 还通过 member_repo.list()
              枚举全部成员资料生成安全更新接收者，没有使用当前成员投影；残留资料因此会扩大接收范围。
              内部逐步 WorkspaceAdmissionOwnerPort 使调用顺序泄漏到准入子模块。
```

```text
Component: MlsGroupEngine and security adapters
Path: crates/uc-infra/src/space/security/mls_group.rs
Responsibility: 建立、加入、更新和持久化 OpenMLS 安全状态。
Relationship: OpenMLS 内部已有 staged commit 概念，但当前 admit/apply 路径立即 merge 并返回活动状态，
              没有成为 WorkspaceConvergence 可持久恢复的“暂存、激活、放弃”能力。
```

```text
Component: WorkspaceConvergenceStore
Path: crates/uc-infra/src/db/repositories/workspace_convergence_store.rs
Responsibility: 用 MasterKey AEAD 整体保存工作空间收敛状态，并解码多个旧布局。
Relationship: 已有版本迁移框架；更早的移除因果资料曾保存 signing_public_key，说明部分旧档案可恢复
              公钥，但当前 V1 成员事件本身没有永久携带该材料。旧布局在 load 中会直接改写当前行，
              不满足本规格的旁路写、重开验证和降级保护。
```

```text
Component: Active Space session and key material
Path: crates/uc-infra/src/space/security/session.rs, crates/uc-infra/src/space/security/access.rs
Responsibility: 保存当前唯一活动 Space 的 MasterKey、keyslot 和 OpenMLS 安全状态。
Relationship: 当前 install_group_join 在完整历史核对前替换活动 session/keyslot；收敛仓储也只能从活动
              session 取 Space 和 MasterKey，不能在来源 Space 保持活动时冷恢复目标 Space 尝试。
```

```text
Component: SwitchSpaceUseCase / MigrationPhase
Path: crates/uc-application/src/space/lifecycle/switch_space/,
      crates/uc-core/src/setup/migration.rs, crates/uc-infra/src/migration_state.rs
Responsibility: 以 Prepared -> HandshakeDone -> Swapped -> None 备份和重封装本机剪贴板历史。
Relationship: 这是独立于 WorkspaceConvergence 的第二个恢复负责人。HandshakeDone 前的 phase 2 已经安装
              目标 keyslot/session，并清空来源关系和来源安全状态；phase 3 逐行覆写主表；phase 4 又分别
              写 setup 状态、清备份和删迁移密钥。迁移状态、setup 状态、SQLite、keyslot 和系统安全存储
              不在同一事务中，不能直接作为本规格 J3 的原子实现。
```

```text
Component: Engine contract and native bindings
Path: crates/uc-engine/src/contract/, crates/uc-engine/src/operations/space/, bindings/
Responsibility: 公开一次 JoinSpace 并映射到桌面和移动宿主。
Relationship: 当前成功只有 SpaceJoined，等待和稳定拒绝主要表现为错误或调用未完成，尚无统一的
              Active / Pending / Rejected 结果对象。
```

当前数据流为：

```text
加入方准备 KeyPackage
  -> 发起方生成并应用安全更新，在 Confirm 中发送 Welcome / 安全材料
  -> 加入方安装安全状态并发送 Ready
  -> 发起方把 AddDevice 写入成员历史
  -> 发起方发送仅含摘要和发起方事实的 AdmissionSaved
  -> 加入方再拉取完整历史，并使用已安装的当前 OpenMLS 树验证
  -> 加入方记录完成并返回 SpaceJoined
```

这个顺序同时产生“旧作者从当前树消失”和“正式成员先于加入方完整验证”两个故障窗口。本规格不在
现有流程上增加过滤器，而是更换验证依据和正式提交边界。

# 5. Proposed Design

## Invariants and ownership

以下规则是实现和评审的首要检查项：

1. `WorkspaceConvergence` 对一次准入的完整结果负唯一责任。其应用层运行期以 profile 为生命周期边界，
   即使没有活动 Space 也必须常驻；活动来源 Space 和暂存目标 Space 都只是它显式打开的完整子上下文。
   删除该模块后，候选、验证、提交、门禁、重试和恢复会散落到配对、空间切换与产品调用方，证明该职责
   不能再拆给调用方。`uc-engine` 只组装并转发操作、结果和事件，不成为第二个准入负责人。
2. `AddDevice` 是新增成员资格的唯一正向事实。`PreparedAdmissionProof`、
   `AdmissionActivationReceipt` 和 `AdmissionCompletion` 都不能在没有对应 `AddDevice` 时授予权限。
3. 历史证明和当前权限分离。历史凭据回答“过去这条记录由谁合法签署”；当前已应用分支和激活门禁
   回答“这个成员现在能做什么”。
4. 事件作者资格只从精确父历史计算；决定作者资格只从被决定移除的精确父历史计算。当前 OpenMLS 树、
   成员资料、可信关系、地址和在线状态不得参与。
5. 移除只禁止被移除实例扩展包含该移除的分支；其验证凭据永久保留，且不得被同设备后续重新加入的
   新凭据覆盖。
6. 候选不是成员事件。发起方是提交或拒绝的唯一裁决者；加入方取消只是一项提交前请求。发起方在 S2 前
   保存该请求时结果为 `Rejected`；S2 先保存时取消已经太晚，同一尝试必须继续到 `Active`，不能自动追加
   RemoveDevice。用户之后仍要退出时，必须另行使用现有明确移除操作。正式提交后只能向前交付同一事件
   和回执，不能回滚或重建。
7. 每个会引起网络回复的阶段先持久化本阶段事实和对应 outbox，再发送消息。重复消息只重放已保存结果。
8. 准入未完成时失败关闭。Presence、普通历史核对、成员公告、邀请、内容、文件和补送不得绕过门禁。
9. 暂存目标安全状态只包含候选父历史中已激活的当前成员和本次加入方。永久凭据表、成员资料表、可信
   关系和地址表只能补充验证或路由资料，不能扩大安全接收者集合。
10. 每个 V2 AddDevice 的唯一 `AdmissionActivationReceipt` 进入版本化成员历史并永久保留；Completion 只
    保存在加入方本机尝试或压缩终态中。终态压缩不能破坏未来设备取得回执并验证成员权限的能力。
11. S2 正式提交后，到 Applied 永久入账并与 Complete outbox 一同保存前，本分支不接受另一项本地加入、
    移除或成员安全提交。唯一例外是明确移除永久失联的本次待激活候选；远端竞争后继按 ADR-020 保存为
    分叉，不能自动应用。
12. 同一 `SpaceJoinRecord` 也是本机加入方空间迁移的唯一恢复记录。旧 `MigrationPhase`、setup 状态、
    keyslot、session、关系清理和内容重封装不得各自推进；J3 之前来源 Space 保持完整活动，J3 开始后
    运行入口保持关闭，直到一个目标活动世代完整可用。
13. 邀请的本机加密 claim 是一次性受理的唯一裁决。本机 claim、尝试身份绑定和 consume outbox 一次提交；
    rendezvous / mDNS consume 只是可重试的渠道清理，不能决定或回滚本机准入。
14. ResetSpace 只允许在准入、outbox、写前恢复、Space transition 和清理全部静止时执行，并保留所有幂等和
    防重放事实；FactoryResetSpace 是唯一可绕过远端等待的本机销毁操作，但不能伪造远端成员结论。

开工前的四个答案固定如下：

| 问题 | 答案 |
| --- | --- |
| 谁负责完整结果 | `WorkspaceConvergence` |
| 调用方唯一需要执行什么 | 调用一次 `JoinSpace`，之后查询或订阅完整状态 |
| 成功和失败返回什么 | `Active`、`Pending`、`Rejected`；存储损坏等 Engine 故障仍返回稳定错误 |
| 重启或重试由谁负责 | `WorkspaceConvergence` 扫描非终态、仍有 outbox 或仍有写前恢复工作的加密尝试并继续 |

## Components

### Versioned membership history

- **职责**：解码明确版本的成员事件和决定，保存成员验证凭据，从精确历史位置计算作者资格，验证签名
  和结果摘要，再把已验证事实交给现有移除决定规则。
- **输入**：版本化事件或决定、已验证历史、按事件保存的激活回执和可选旧前缀检查点。
- **输出**：已验证新历史位置、仍缺历史/证明依赖，或结构、授权、签名、摘要、分叉、迁移、版本类别中的
  一个明确结果。
- **关系**：属于 `uc-core` 业务规则；密码算法由 `uc-infra` 的显式公钥验证能力实现。

### Admission transaction owner

- **职责**：从已验证邀请创建唯一尝试，驱动候选准备、正式提交、应用证明、完成通知、取消和恢复。
- **输入**：一次 `JoinSpace`、内部准入通道消息、持久尝试和当前工作空间快照。
- **输出**：`Active`、`Pending` 或 `Rejected`，以及完整工作空间变化通知。
- **关系**：是 `WorkspaceConvergence` 的 profile 级常驻应用运行期；它拥有准入仓储和零或一个完整活动
  Space 上下文，不依赖当前 `AppFacade` 已经存在。产品、绑定、配对和 `uc-engine` 不调用中间步骤。

### Historical signature adapter

- **职责**：按事件声明的算法版本，使用事件历史给出的精确公钥验证规范载荷和签名。
- **输入**：算法版本、公钥、规范载荷、签名。
- **输出**：有效、无效或不支持版本。
- **关系**：不查询当前成员、OpenMLS 树、设备资料或网络身份；现有当前成员签名能力只负责本机签名和
  当前会话证明，不再为旧事件查钥。

### Activation receipts in versioned membership history

- **职责**：按 `AddDevice.event_id` 永久保存唯一的加入方应用回执，并让成员历史核对按缺失事件标识有界补齐。
- **输入**：已经验证的 V2 `AddDevice` 和对应 `AdmissionActivationReceipt`。
- **输出**：已保存、缺少对应事件、幂等重复或冲突回执。
- **关系**：回执是 `VersionedMembershipHistory` 聚合内的永久配套事实，不属于可压缩的
  `SpaceJoinRecord`，也不建立独立证明账本、完成证明游标或产品接口。它只能解除已有 `AddDevice` 的
  负门禁，不能创建成员、改变分支或恢复已移除成员。

成员核对摘要除事件头和决定摘要外，增加对按 `event_id` 排序的 `(event_id, receipt_id)` 求得的
`activation_receipts_digest`。摘要不同时，接收方只请求本机已验证 `AddDevice` 所缺的回执标识，每批最多
256 个且编码后不超过 4 MiB；发送方先补齐对应事件，再发送回执。回执早于事件到达时返回
`MissingMembershipEvent(event_id)`，不保存为正式事实或另建延迟页；发送方永久持有回执并在事件获得持久
确认后重发。历史页中出现由尚未激活候选签署的后继时，沿用一页加密待验证历史缓冲并先补拉该候选回执，
依赖到达后从原游标继续。完成消息不参与跨成员权限计算，不进入该摘要或全网传播。

### Staged security transition adapter

- **职责**：为精确候选生成可持久恢复的暂存安全状态，在正式提交后激活同一状态，或在提交前放弃。
- **输入**：基础安全状态、由候选父历史派生的已激活成员实例、加入方 KeyPackage、候选摘要和尝试标识。
- **输出**：暂存状态引用、双方可独立重算的公共安全承诺、交付材料，以及幂等激活或放弃结果。
- **关系**：`uc-infra` 提供能力，不决定何时提交或何时返回 Active；所有调用只来自准入负责人。仓储必须
  保存精确 staged commit、Welcome、目标 epoch 和安全交付输出，或保存能在不取得任何新随机数的前提下
  逐字节重放这些输出的完整确定性记录；只保存输入或摘要后重新执行 OpenMLS 不构成恢复。

### Attempt-bound Space transition

- **职责**：在同一Space 加入记录内保存来源快照、暂存目标世代，并在 J3 排空来源写入、补齐最终快照、重封装
  数据和一次性提升目标活动世代。
- **输入**：`attempt_id`、来源活动世代、目标暂存世代、Complete 和当前持久恢复步骤。
- **输出**：未切换、可恢复切换中或已激活的唯一结果，以及迁移/保留记录计数。
- **关系**：由 `WorkspaceConvergence` 调用一个完整能力；现有 `SwitchSpaceUseCase::resume_pending` 和
  `MigrationStatePort` 在兼容迁移完成后删除，不能成为第二个启动恢复器。

### Encrypted admission repository and outbox

- **职责**：原子保存尝试阶段、候选、暂存状态引用、证明事务副本、门禁、已消费消息、终态和待发消息。
- **输入**：当前尝试版本和预期前一阶段。
- **输出**：唯一推进、幂等重放或并发冲突。
- **关系**：整个业务负载经 MasterKey AEAD 加密；若历史、OpenMLS 状态和 outbox 不能共用一次 SQLite
  事务，则使用同一加密写前记录恢复，恢复完成前普通流程保持关闭。

加入方 J0 时还没有目标 Space MasterKey。profile 第一次建立 Engine 安全状态时必须通过现有
`SecureStoragePort` 生成并保存一个 32-byte `ProfileAdmissionMasterKey`，固定标识为
profile 安全存储命名空间内的 `profile_admission_master_key:v1`。它跨 ResetSpace、Space 切换和多次加入
保留，只能由 FactoryReset 的 profile 密钥清除阶段删除。FactoryReset 先在现有 `SecureStoragePort` 保存
与准入密钥分开的 profile 生命周期标记、
停止旧运行入口并幂等清除 profile 加密密钥命名空间内明确列出的活动/暂存 Space keyslot、KEK、迁移 key
和该 profile key；确认这些固定别名均不存在后，才按固定存储命名空间删除已经不可解密的密文、设置、关系
和邀请。不得顺带删除该清单外的系统安全存储项目。任一密钥尚未确认删除时
不得提前清除设置或重新开放旧 profile。profile 级 revision、下一个 local ordinal、已消费邀请索引和压缩终态都直接使用
该 MasterKey 的域分离 AEAD 子密钥加密，因此 Fresh、Pending 和 Fresh Rejected 都不依赖目标 Space 密钥。

每个 `attempt_id` 另生成随机 `SpaceJoinRecordDataKey`，由 `ProfileAdmissionMasterKey` 使用带 profile、
attempt、版本和用途的 AEAD envelope 包裹后保存在 SQLite；它只加密未压缩尝试、KeyPackage、续接私钥、
来源备份和其他大体积临时资料。目标 Space 和目标 MasterKey 可用后，历史、证明和安全状态写入按
`(attempt_id, target_space_id, target_generation)` 显式寻址、由目标 Space MasterKey 加密的暂存槽，不得从
当前活动 session 猜 Space 或密钥。不同用途的包裹密钥和数据密钥必须域分离，不能跨用途复用。

来源 Space 的 session、keyslot、活动清单和普通业务在 J3 前保持不变；目标 keyslot、目标安全状态、目标
收敛状态和目标关系只能写入按 `attempt_id` 隔离的暂存世代。J3 通过一个持久激活日志提升目标活动世代；
只有活动清单、目标 keyslot、数据库世代和内存 session 指向同一目标且目标运行入口恢复后才保存 Active。
Active 或 Rejected 压缩时，必须先在一个提交或可恢复写前记录中把 `join_id`、ordinal、revision、邀请
防重放索引、终态结果和 Ack 重建资料重封装到 `ProfileAdmissionMasterKey`，再删除临时负载及其 wrapped
attempt data key。删除失败只形成可重试清理，不能回退活动世代或丢失终态。密钥丢失或安全存储不可用时
进入稳定恢复错误，不能重新生成同名 key、改用明文或提前调用现有 `set_master_key_for_space`。

### Device-trust admission projection

- **职责**：在现有 `DeviceTrustSnapshot` 中投影本机最近一次加入结果和当前 Space 唯一的入站候选。
- **输入**：持久Space 加入记录、当前活动 Space、现有设备关系事实和 profile 级单调 revision。
- **输出**：`current_join`、`pending_inbound_member` 和更新后的完整设备信任快照。
- **关系**：它只是现有正式快照的一部分，不建立独立状态查询、事件或成员事实来源；
  `QueryWorkspaceConvergence` 及其事件继续只用于 `dev-tools` 诊断。

每个本机加入尝试保存不可回退的 `local_join_ordinal`。profile 级准入互斥保证非终态本机加入和非终态
入站准入不会并存。有非终态本机尝试时，`current_join` 必须选择它；
否则选择 ordinal 最大且不小于 `join_projection_floor_ordinal` 的终态本机尝试。新的本机 `JoinSpace` 创建
成功时自然以更大 ordinal 替换旧结果；普通 ResetSpace 只把该加密 floor 推进到下一个待分配 ordinal，
从公开快照隐藏此前结果，不删除终态、幂等索引或防重放事实。FactoryReset 才销毁整个 profile 状态。
当前 Space 同时最多有一个非终态入站尝试；只有该尝试投影为
`pending_inbound_member`，终态入站结果不保留在公开快照，候选也不进入 `devices`。

`DeviceTrustSnapshot.revision` 改为同一 profile 内跨 Space 单调递增的 `u64`；创建尝试、阶段或取消状态
变化、入站候选变化、证明入账、Space 提升和现有设备关系变化都在保存业务事实的同一提交或写前恢复中
checked increment。跨 Space 切换不得从目标 Space 自己的较小 revision 重新开始。该 revision 和下一个
local ordinal、`join_projection_floor_ordinal` 作为准入仓储元数据加密保存，不包含业务判断，也不能独立
重建或覆盖尝试事实；计数溢出或 floor 大于下一个 ordinal 时进入 `RecoveryRequired`。明确 FactoryReset
开始后旧 Engine 会话和订阅已经失效；完成后客户端必须建立新会话，并从新 profile generation 重新查询。

### Profile reset coordination

- **职责**：在 profile 级准入负责人内统一判断 ResetSpace 是否静止，并把 FactoryResetSpace 作为可恢复的
  本机强制销毁流程执行。
- **输入**：全部Space 加入记录、outbox、写前记录、Space transition、终态清理状态、当前 profile generation 和
  现有 Space 生命周期能力。
- **输出**：普通重置完成、无副作用冲突，或只能向前恢复的彻底重置结果。
- **关系**：Engine 把现有 ResetSpace 和 FactoryResetSpace 先路由给 profile 级 `WorkspaceConvergence`；它
  负责准入边界，现有 Space lifecycle 和 secure-storage adapter 仍负责暂停运行、清密钥、设置、关系和邀请。
  Fresh profile 没有活动 `AppFacade` 时也必须可执行 FactoryResetSpace，不能构造空 facade 或绕过负责人。

### Current-member activation projection

- **职责**：把规格 022 的当前成员范围与本规格的待激活门禁合并成一次完整快照。
- **输入**：已应用成员历史、迁移基线、历史内永久激活回执和本地准入完成状态。
- **输出**：普通运行允许使用的成员实例集合。
- **关系**：门禁只能从历史有效成员中排除候选，不能把历史之外的设备加入范围。

投影按以下固定顺序计算，不允许调用方自行推断：

1. 先从 `applied_head` 得到当前有效成员集合；不在集合中的实例始终排除。
2. V2 创世 `AddDevice` 的创建者固有激活。
4. 根历史只接受当前本机已验证身份与永久成员凭据；旧资料必须重置。
5. 其他 V2 `AddDevice` 对发起方和第三方观察者都必须有成员历史中按 event_id 保存的唯一有效
   `AdmissionActivationReceipt`。这证明加入方已经持久保存事件和目标安全状态；它即使尚未运行，也可像
   离线成员一样进入这些观察者的当前范围。
6. 若本机正是该 V2 `AddDevice` 的加入方，还必须在本机尝试或压缩终态中保存一份有效
   `AdmissionCompletion` 并完成 J3；
   在此之前本机所有普通入口继续关闭。这个短暂差异只表示各设备的持久证据到达时间不同，不产生第二份
   成员事实。

任何迁移基线、回执或完成证明都只能从第一步集合中排除尚未满足条件的实例，不能添加成员。

## Data Model

### Profile lifecycle and admission metadata

`ProfileLifecycleMarkerV1` 通过现有 `SecureStoragePort` 的固定别名
`profile_lifecycle_marker:v1` 保存，只包含：

| 字段 | 含义 |
| --- | --- |
| `marker_format_version` | 生命周期标记版本；未知版本失败关闭 |
| `profile_generation` | 本机随机 128-bit 代次；只用于把新旧本机状态隔离，不是设备、Space 或用户标识 |
| `factory_reset_phase` | `None`、`WipingKeys` 或 `ClearingState` |

该记录按敏感数据处理，不得包含设备、Space、attempt、join、邀请、时间、路径或密钥摘要，不进入网络、
日志或遥测，也不得回退到数据库或文件明文。它与待删除的 `ProfileAdmissionMasterKey` 使用不同固定别名，
且不在 FactoryReset 的加密密钥清除清单中，因此准入密钥删除后仍可读取并区分“尚未初始化”和“密钥已删、
清理未完成”。`WipingKeys` 持久化成功前不得删除任何密钥，`ClearingState` 只在全部固定密钥别名确认不存在
后写入。清理完成时原子生成新 generation 并把 phase 置为 None；phase 非 None 时禁止创建 profile key 或
打开普通运行。生命周期标记损坏或安全存储不可用时失败关闭，不能删除磁盘状态后猜测阶段。

`AdmissionProfileMetadata` 使用 `ProfileAdmissionMasterKey` 加密，AEAD associated data 绑定
`profile_generation`，至少保存 `next_local_join_ordinal`、`join_projection_floor_ordinal`、
`device_trust_revision` 和已消费邀请索引引用。floor 必须小于或等于 next ordinal；普通 ResetSpace 只把它
推进到当时的 next ordinal，任何字段倒退、重复代次、溢出或密文与 generation 不符都进入
`RecoveryRequired`。压缩终态和其他准入密文同样绑定该 generation，旧代次文件不能被新 profile 接受。

### `MembershipCredential`

| 字段 | 含义 |
| --- | --- |
| `credential_format_version` | 凭据结构版本；首版为 V1 |
| `signature_algorithm_version` | 签名算法版本；首版使用现有 Ed25519 |
| `public_key` | 该成员实例验证成员历史签名的公钥 |
| `credential_id` | 对版本、算法和公钥的域分离摘要 |

`MembershipCredential` 在对应 `AddDevice` 中首次出现并永久保留。`MemberInstanceId` 继续由设备标识和
该凭据派生；重新加入必须生成新凭据和新实例，旧实例的凭据继续验证旧历史，不恢复当前权限。
这里的 `public_key` 是现有 OpenMLS signer / 成员历史签名公钥；不得复用
`AdmissionChangeFacts.transport_public_key`、identity fingerprint 或连接公钥，即使底层算法同为
Ed25519。AddDevice 中的该公钥和算法必须逐字节等于本次 KeyPackage / OpenMLS credential 的 signer
公钥和算法，并进入 candidate_event_id、admission bundle digest 和目标安全摘要；任一不一致都在 S1/J1
拒绝，不能等到后续事件验签才发现。

### `MembershipEventV2` 与 `MembershipDecisionV2`

成员历史只保存 V2 事件与 V2 决定。事件绑定沿革、父位置、作者永久凭据、操作、结果成员摘要、安全状态、准入摘要和签名；决定绑定被引用的移除事件、决定者永久凭据、接受或拒绝、观察位置、结果摘要和签名。不存在旧事件包装、旧决定包装或旧证据回退。

### `AdmissionSecurityCommitmentV1`

Sponsor 暂存提交和 joiner 处理 Welcome 后的本地 OpenMLS 序列化状态包含不同私钥，禁止直接对本地
snapshot、密文文件或 serde 字节求摘要后比较。双方必须从各自已验证的本地状态独立导出以下规范公共
承诺：

| 字段 | 含义 |
| --- | --- |
| `commitment_format_version` | 安全承诺格式；首版 V1 |
| `lineage_id` / `mls_group_id` | Space 沿革和 OpenMLS group id |
| `attempt_id` | 防止跨准入复用同一承诺 |
| `base_history_position` | 候选扩展的精确父历史 |
| `candidate_core_digest` | 对不含安全承诺、event_id 和签名的候选核心做域分离摘要，避免循环引用 |
| `ciphersuite` | OpenMLS ciphersuite 的稳定编号 |
| `base_epoch` / `target_epoch` | 暂存提交前后 epoch |
| `commit_digest` | 对 TLS 规范编码 MLS Commit 的域分离摘要 |
| `group_context_digest` | 对目标 epoch TLS 规范编码 GroupContext 的域分离摘要 |
| `member_credentials_digest` | 按 MemberInstanceId、credential_id 规范排序的目标叶成员集合摘要 |
| `key_catalog_digest` | 对解密后规范化内容密钥目录的摘要；不得使用本机密文或本机序列化布局 |
| `admission_bundle_digest` | 对绑定 candidate_core_digest 的版本化 Welcome、目录和既有成员交付清单求摘要 |
| `security_commitment_id` | 对以上字段以 `uniclipboard/admission-security-commitment/v1` 域分离求摘要 |

`member_credentials_digest` 的集合只能是候选父历史中已激活的当前成员加本次加入方。目录条目和成员条目
都按规范字节升序排序，长度使用固定大端整数编码；不允许依赖 map 迭代顺序、JSON 字段顺序或平台整数。
`admission_bundle_digest` 只承诺版本化公共/加密交付字节和接收者 credential_id，不包含 Sponsor 私钥状态。

Sponsor 的 `prepare` 和 joiner 的 `stage` 都必须返回完整承诺及其 id；负责人逐字段比较，并在 S2、J2 和
激活前重新从持久暂存状态导出一次。任何字段不等都拒绝提交或进入 RecoveryRequired；不能只比较 epoch。
V2 AddDevice 保存 `security_commitment_id` 和 `admission_bundle_digest`，event_id 最后对完整事件求摘要，
从而不存在 event_id 与安全承诺互相包含的循环。

### `SpaceJoinRecord`

| 字段 | 含义 |
| --- | --- |
| `attempt_id` | 加入方在首次发送前生成并保存的 256-bit 随机稳定标识；发起方把邀请消费原子绑定到该标识 |
| `join_id` | 加入方生成的独立 128-bit 随机公开关联标识；只进入正式结果和取消命令，不用于协议认证 |
| `local_join_ordinal` | 仅加入方保存的 profile 级单调序号；用于从尝试事实选择 `current_join` |
| `role` | `Sponsor`、`Joiner` 或只用于 S3 接管的 `CompletionHelper` |
| `stage` | `Initiated`、`Accepted`、`Candidate`、`Prepared`、`Committed`、`Applied`、`Completed`、`Rejected`、`Superseded` |
| `lineage_id` | 目标 Space 沿革 |
| `base_history_position` | 候选唯一允许扩展的父事件、深度和摘要 |
| `candidate_event` | 正式提交时必须原样写入的 V2 AddDevice |
| `target_members_digest` | 候选结果成员摘要 |
| `security_commitment` | 双方可独立重算的 `AdmissionSecurityCommitmentV1` |
| `staged_security_state` | 受保护的暂存状态引用或密文 |
| `invitation_claim` | 邀请代次、加密 code 绑定、过期时间、双方身份和远端 consume outbox |
| `space_transition` | `Fresh`、`SameSpace` 或与本 attempt 绑定的 `CrossSpaceTransitionV2` |
| `prepared_proof` | 加入方提交前准备证明 |
| `activation_receipt` | 加入方正式应用证明的事务副本；永久原件属于版本化成员历史 |
| `completion` | 完成者发送、只供加入方 J3 和最终回复恢复的本机证明；不参与全网权限投影 |
| `completion_recovery_routes` | Commit 后可用于提交同一 Applied 的有界不透明当前成员路由 |
| `completion_recovery_deliveries` | 按父历史当前成员加密、只可由对应成员应用的目标安全更新 |
| `cancel_request` | 加入方已持久发送的取消请求及消息编号；它本身不改变终态 |
| `cancel_outcome` | Sponsor 在 S2 前保存的 Rejected，或由已保存 Commit 证明的 `TooLateCommitted` |
| `resume_public_key` / `resume_private_key_ref` | 专用于准入续接的 Ed25519 验证公钥和加入方加密私钥引用 |
| `resume_peers` | 按 Sponsor 或接管成员实例保存挑战计数、双方最后持久消息号和认证传输身份摘要 |
| `inbox_dedup` | 按消息编号和摘要保存的已消费消息及 Ack 重建依据 |
| `outboxes` | 按 `(purpose, recipient, message_id)` 索引的待发集合；阶段消息、取消、邀请消费和成员更新可并存 |
| `terminal_result` | Joiner 可重复查询的 `Active`/`Rejected` 或内部 `SupersededByNewJoin`，Sponsor 的 `Completed`/`Rejected`，或 CompletionHelper 的 `Completed` |

该表是角色和阶段状态的字段并集，不是每条记录都必须同时具有全部字段。持久格式必须是
`SponsorAdmissionState`、`JoinerAdmissionState` 和 `CompletionHelperState` 三个带阶段标签的版本化变体：`Initiated` 和早期
`Rejected` 可以没有 candidate、event、proof 或目标 Space；`local_join_ordinal` 只存在于 Joiner；Sponsor
的 `Completed` 保存内部完成终态而不是伪造 Joiner 的 `Active`。CompletionHelper 只能从已认证续接创建
`Applied -> Completed` 记录，保存原 Sponsor、joiner、同一 event/receipt、自己的 security delivery、挑战
位置和 Complete outbox；它不能保存邀请、Candidate、Prepared、Commit、Rejected、取消裁决、join_id 投影
或 Space transition。进入某阶段时一次写齐该变体的必需字段，缺字段、出现不允许字段或阶段倒退均为损坏，
不能靠默认值补齐。

每个 profile 同时最多占用一个准入槽，不区分 Sponsor、Joiner 或 CompletionHelper。统一的 `admission_slot_held` 在存在非终态
尝试、共享 profile 写前恢复、Space transition，或仍会改变活动世代的清理时为真；业务已终态且只剩按
attempt 隔离的消息重发或终态压缩时为假，这些工作继续由恢复扫描处理但不阻塞新准入。最先持久保存
`Initiated`、`Accepted` 或已认证 helper Applied 记录取得该槽；另一方向在创建任何尝试、候选、备份、
helper security update 或目标状态前返回稳定冲突或 AdmissionUnavailable。这也意味着每个 Space 同时最多一个未收束正式准入，并保证本机 Cross-Space J3
不会遗弃来源 Space 的入站尝试。相同
`attempt_id` 和消息编号幂等；不同
尝试不得复用 `join_id`、候选、KeyPackage、暂存状态、凭据或证明。`join_id` 不能由 `attempt_id`、设备、
Space 或 Engine 单次请求的 `operation_id` 编码得到。

`AdmissionUnavailable` 只是远端 profile 槽正在被另一尝试占用的可重试忙碌回复，不是 Rejected 或
DeliveryAck。接收方不得消费邀请、创建 Sponsor attempt 或保存候选；发送方保留原 JoinRequest outbox、
同一 attempt 和 Pending，只按负责人内部有界退避重试。对本机再次明确发起的 `JoinSpace`，负责人按
ADR-022 裁决：旧本机 Joiner 尚未持久保存 Prepared 时，原子保存旧 `SupersededByNewJoin` 终态并创建
全新尝试；Prepared 或更晚阶段返回稳定 `PreviousJoinCannotBeSuperseded` Engine Conflict 且不创建尝试。
入站准入、共享写前恢复、Space transition 或其他不可取代工作继续返回 `JoinOperationInProgress`。
任何分支都不能让调用方编排取消、重置和加入步骤。

`Initiated` 由加入方在任何网络发送前保存；`Accepted` 由发起方在原子消费邀请并绑定 `attempt_id`、
加入方身份和本次凭据后保存。相同邀请和相同 `attempt_id` 重放同一尝试；相同邀请绑定另一
`attempt_id` 或身份时稳定拒绝。

这里的“原子消费”只指发起方本机：加密 `invitation_claim`、Sponsor SpaceJoinRecord 和远端 consume
outbox 在同一提交中保存。本机提交后该 claim 是唯一裁决，即使 rendezvous 尚可解析旧 code，后续连接
也只能恢复同一 attempt 或稳定拒绝。`PairingInvitationPort::consume_invitation` 由 outbox 重试；204、404
或 409 都只表示渠道已经关闭，5xx/网络失败继续重试，任何结果都不能撤销本机 claim。

J0 同时生成只用于续接的独立 Ed25519 密钥，并把其公钥摘要绑定进候选核心和正式 `AddDevice`。发起方
保存公钥，加入方私钥只存在于 `SpaceJoinRecordDataKey` 加密负载中。每次恢复由对端先持久保存新挑战；
签名载荷域分离绑定协议版本、attempt、lineage、event、原 Sponsor、joiner、当前对端成员实例、双方认证
传输身份摘要、单调挑战计数、随机 nonce 和双方最后持久消息编号。加入方用 resume key 响应；对端用其
精确历史成员凭据签署挑战和结果。双方在发送回复前先保存计数和消息位置，旧计数、身份变化、跨尝试或
跨成员重放全部拒绝。

第三方接管时，当前成员必须先从连续历史验证候选中的 resume 公钥摘要、自己的父位置资格和当前资格，
再进行上述双向挑战；仅持有路由密文、知道 `attempt_id` 或建立到同一地址的连接都不构成认证。续接凭据
只允许查询或发送该尝试的 Candidate、Prepared、Commit、Applied、Complete、Rejected 和 Ack，不授予
普通成员权限。Joiner Rejected、Active 或 Superseded 后不再建立新续接会话；已经持久保存的最终回复或
被取代事实仍可幂等重放。
日志不得记录公钥、挑战、nonce、签名或身份摘要。

### `CrossSpaceTransitionV2`

已经存在活动 Space 的加入方只有在 Candidate 明确证明目标沿革不同后才创建该记录；同 Space 和全新
设备不创建来源备份。记录属于 `SpaceJoinRecord`，不能通过独立 migration id 启动第二条恢复流程。

| 字段 | 含义 |
| --- | --- |
| `transition_format_version` | 首版 V2；与旧 `MigrationPhase` 格式隔离 |
| `attempt_id` | 必须等于所属Space 加入记录 |
| `source_space_id` / `source_generation` | J1 时仍活动的来源 Space 和活动世代 |
| `source_backup_ref` / `source_backup_digest` | 尝试子密钥保护的完整来源备份及规范清单摘要 |
| `source_revision_at_backup` | 初次备份完成时的数据版本 |
| `target_space_id` / `target_generation` | 目标暂存 Space 和不可复用世代 |
| `target_keyslot_ref` | 未设为活动的目标 keyslot/KEK 引用 |
| `target_workspace_ref` | 目标历史、证明、关系和安全状态的隔离暂存槽 |
| `phase` | `SourcePrepared`、`TargetStaged`、`ActivationStarted`、`SourceFinalized`、`DataRewrapped`、`TargetPromoted`、`CleanupPending` |
| `final_source_revision` / `final_manifest_digest` | J3 排空写入后固定的最终来源版本和数据清单 |
| `migrated_records` / `preserved_unreadable_records` | 最终 Active 结果沿用的计数 |

初次来源备份只是 J3 前的安全基线，不冻结用户活动。J3 必须先取得唯一 Space 切换租约，暂停并排空来源
Space 的本地捕获、接收、文件传输、成员后台任务、搜索写入和其他 MasterKey 写入，再把备份追到一个固定
`final_source_revision`。新建、修改和删除都进入最终清单；不能只处理 J1 时已经存在的行。每个迁移条目
使用稳定记录标识和来源修订去重，逐条重封装可以重放，但运行入口在全部条目和清单摘要核对前保持关闭。

目标 keyslot、安全状态、收敛状态和关系先按 `target_generation` 保存并完整重开验证。`ActiveSpaceGenerationManifestV2`
是唯一活动指针，至少绑定 SpaceId、keyslot generation、数据库 generation、安全 generation 和 manifest
digest；所有被引用世代先持久且可读，再以一次原子替换提升该 manifest。setup 状态只从 manifest 投影，
不能再独立决定活动 Space。内存 session 从已提升 manifest 重新加载，加载成功并恢复目标运行入口后才可
保存 Active。

`ActivationStarted` 之前收到正式 Rejected 时，先保存 Rejected outbox；收到 RejectedAck 并把终态重封装
到 profile key 后，才删除目标暂存、来源备份和 wrapped attempt data key，来源 manifest 与运行活动始终
不变。`ActivationStarted` 之后已经存在有效 Complete，只能从持久 phase 向前完成；进程启动
必须先恢复该 transition，再开放任何 Space 网络和写入。`TargetPromoted` 后清理失败只停在
`CleanupPending`，来源世代不能重新激活。

### `PreparedAdmissionProof`

加入方在完整验证历史并持久暂存状态后，用本次成员凭据签署：

```text
protocol_version
attempt_id
lineage_id
base_history_position
candidate_event_id
target_members_digest
security_commitment_id
joiner_member_instance_id
joiner_credential_id
```

该证明只允许发起方正式提交精确候选，不授予成员资格。发起方收到后必须再次确认本机历史仍等于
`base_history_position`；任何变化都稳定拒绝本次尝试，不能自动改绑新父事件。

### `AdmissionActivationReceipt`

加入方正式保存同一 `AddDevice` 和目标安全状态后，用本次成员凭据签署：

```text
protocol_version
attempt_id
event_id
applied_history_digest
installed_security_commitment_id
joiner_member_instance_id
```

该回执证明正式成员事实已经在加入方落盘。它只能解除对应 `AddDevice` 的待激活负门禁；没有事件、
事件不在当前分支、摘要不符或成员已经被移除时，回执不能产生权限。

### `MembershipActivationReceiptRecord`

| 字段 | 含义 |
| --- | --- |
| `receipt_record_format_version` | 永久回执记录版本；首版为 V1 |
| `event_id` | 唯一对应的 V2 `AddDevice` |
| `attempt_id` | 产生该事件的稳定尝试 |
| `activation_receipt` | 加入方签署的不可变应用回执 |
| `receipt_id` | 对回执规范载荷和签名的域分离摘要 |

该记录属于 `VersionedMembershipHistory.activation_receipts`，按 `event_id` 唯一保存。相同 `event_id`
出现不同 `attempt_id`、回执或摘要时进入 `RecoveryRequired`，不能按时间选择。成员历史核对通过事件标识
补齐该记录，不另建 proof cursor；Space 加入记录结束后，验证与传播不得再读取尝试记录。

### `AdmissionCompletion`

原发起方保存 `AdmissionActivationReceipt` 后，优先签署并持久化完成消息。若它离线，任何仍在当前
已应用分支中有效、已保存同一 `AddDevice` 和同一回执、应用了属于自己的目标安全更新、从本地状态重算
出相同 `security_commitment_id`，且不是本次加入方的成员都可以签署等价完成消息。消息至少绑定
`attempt_id`、`event_id`、回执摘要、安全承诺、完成者实例/凭据和完成者当前历史位置。

加入方验证完成者当前有资格且其分支包含同一事件，再把首份有效完成消息保存到本机尝试并进入 J3；只有
目标活动 manifest、安全状态和运行入口都验证成功时，才在保存 `Active` 的同一恢复边界清除本机门禁。
任何本机 CancelRequested 在已验证 Commit 后都已稳定归类为 `TooLateCommitted`，不会改变 J3。后续有效
完成消息只幂等返回同一 Active；相同 attempt/event 出现不同回执或安全承诺时进入 `RecoveryRequired`。
完成消息不进入成员历史、激活回执记录或全网传播，也不授予其他设备权限。

`CompleteAck` 只确认加入方已持久保存 `AdmissionCompletion`、完成 J3 并进入 `Active`，不是第六个业务
阶段。它绑定原 Complete message id 和摘要；加入方从 Active 终态确定性重建同一 Ack。Ack 本身不进入
durable outbox，也不需要 ack-of-ack；完成消息的发送者收到它后才能清除自己的 Complete outbox。

### Terminal attempt compaction

业务进入 Sponsor/CompletionHelper `Completed`、Joiner `Active`、`SupersededByNewJoin` 或任一 `Rejected` 后，不立即删除尝试。只在 outbox 已确认
清空、写前恢复完成，且实际存在 AddDevice 时所需永久回执已经写入版本化成员历史后，才删除 KeyPackage、暂存安全状态、历史页和逐
消息 inbox 等大负载。压缩必须按上文先把终态原子重封装到 `ProfileAdmissionMasterKey`，再删除 wrapped
attempt data key。终态记录至少永久保留 `attempt_id`、`join_id`、Joiner 的 local ordinal、邀请消费摘要、
双方身份绑定、可选候选 `event_id`、终态、稳定拒绝类别、取消是否在提交前生效或已太晚，以及重放最终
回复所需的证明引用；S2 前 Rejected 的 `event_id` 和证明引用为空，Sponsor Completed 也不能写成 Joiner
Active。

首版不自动删除终态记录。同一 `attempt_id`、同一 `join_id`、同一已消费邀请或同一协议消息的重复
JoinRequest、Applied、Complete 或 Ack 从终态记录和版本化成员历史返回同一结果，不能重新消费邀请或
创建另一实例。每次新的明确 `JoinSpace` 按 ADR-022 使用新 attempt；一次 Rejected 或
SupersededByNewJoin 不得永久封死该设备以后加入。以后如需保留期限，必须
先另行定义跨设备最大重试期和已消费邀请的永久防重放依据。

### V2 单成员根历史

新建或重置 Space 直接以本机已验证身份和永久成员凭据建立唯一 V2 单成员根历史。升级资料不得从旧成员状态生成检查点或继续成员集合；产品必须先重置 Space，再重新配对其他设备。

## API / Interface

### Stable Engine contract

```text
IssueInvitation() -> IssuedInvitation

JoinSpace(input) -> JoinSpaceStatusSummary
CancelJoinSpace(join_id) -> JoinSpaceStatusSummary
RemoveMember(device_id) -> existing member-removal result

QueryDeviceTrust() -> DeviceTrustSnapshotSummary
DeviceTrustChanged { revision }
```

公开结构固定为：

```text
JoinId(String) // 128-bit 随机值的 base64url，无填充

JoinedSpaceSummary {
  sponsor_device_id,
  sponsor_identity_fingerprint,
  space_id,
  self_device_id,
  self_identity_fingerprint,
  migrated_records: Option<u64>,
  preserved_unreadable_records: Option<u64>
}

JoinSpaceStatusSummary
  Active {
    join_id,
    joined_space: JoinedSpaceSummary
  }
  Pending {
    join_id,
    target_space_id: Option<String>,
    sponsor_device_id: Option<String>,
    sponsor_identity_fingerprint: Option<String>,
    cancel_requested: bool
  }
  Rejected {
    join_id,
    reason: JoinSpaceRejectionReasonSummary
  }

PendingInboundMemberSummary {
  device_id,
  display_name
}

DeviceTrustSnapshotSummary {
  revision,
  local_device_id,
  local_membership,
  current_change,
  devices,
  current_join: Option<JoinSpaceStatusSummary>,
  pending_inbound_member: Option<PendingInboundMemberSummary>,
  recovery,
  allowed_actions,
  blocked_reason,
  updated_at_ms
}
```

`JoinedSpaceSummary` 逐字段保留当前 `OperationResult::SpaceJoined` 的全部内容；Fresh join 的两个计数为 None，
Cross-Space join 返回最终 J3 计数。绑定必须对 device name 和 fingerprint 使用现有脱敏 Debug 规则。

`Pending` 不公开 Candidate、Prepared、Commit、Applied、J3 迁移 phase 或安全摘要。同步调用超时、网络暂
不可达或重启恢复都由负责人继续同一尝试；调用方从 `QueryDeviceTrust` 取得当前 `current_join`，不得为
恢复再次调用 `JoinSpace`。新的明确 `JoinSpace` 在旧本机 Joiner 处于 Initiated/Candidate 时原子取代旧尝试
并返回新 Pending；旧尝试已经持久保存 Prepared 时返回 `PreviousJoinCannotBeSuperseded`，原 Pending 不变。
本机入站尝试或其他不可取代的 profile 工作仍返回 `JoinOperationInProgress`，不创建 Rejected 尝试；
调用方从 `QueryDeviceTrust` 取得当前 `current_join` 或 `pending_inbound_member`。本机已有非终态 Joiner
尝试时收到入站请求，则在保存 Accepted 前返回 AdmissionUnavailable，不消费邀请或创建 Sponsor attempt；
远端 Joiner 保持同一 Pending 和 JoinRequest outbox，稍后重试。

`join_id` 只用于正式结果、快照和 `CancelJoinSpace` 的本机关联，不能认证续接或推进内部阶段。它与
`attempt_id` 一起生成并保存，但两者相互独立；Engine 已有的 `operation_id` 继续只表示一次可能很短的
Engine 请求，不能复用为跨重启的加入标识。产品不提供按编号查询任意历史加入的第二入口。

`current_join` 有非终态本机尝试时选择该尝试；否则选择不小于 `join_projection_floor_ordinal` 且
`local_join_ordinal` 最大的可公开终态尝试。Active 或 Rejected 一直显示到下一次本机 JoinSpace 创建成功或普通
ResetSpace 推进 floor；SupersededByNewJoin 从不单独投影，原子提交后只显示取代它的新尝试。Fresh Pending/Rejected 时设备关系字段使用无活动 Space 的空值，但 `current_join`
仍可查询；Cross-Space J3 前设备关系继续描述来源 Space，J3 后一次切换为目标 Space。被 floor 隐藏的旧
终态仍在内部用于幂等和防重放，但不与当前结果并列公开。

`pending_inbound_member` 只返回当前活动 Space 唯一的非终态入站候选，字段固定为 device id 和 display
name。候选不进入 `devices`，也不公开内部阶段、指纹、候选安全摘要或“可取消/可移除”推断字段。入站尝试
一旦 Rejected 或完成，该字段立即为 None；产品不保留一份入站终态列表。

`DeviceTrustSnapshot.revision` 在同一 profile 内跨 Space 严格递增。任何快照字段变化先持久保存业务事实
和新 revision，再发送 `DeviceTrustChanged { revision }`。事件只是一条失效提醒，消费者收到比本地更新的
revision 或通用 `RefreshRequired` 后重新调用 `QueryDeviceTrust`；相同或更小 revision 不覆盖已查询结果。
事件丢失不影响恢复，第一次 JoinSpace 返回值丢失后也能从快照找回 Pending。Fresh profile 的查询必须由
profile 级常驻 `WorkspaceConvergence` 提供，不能因为还没有活动 Space `AppFacade` 而返回“工作空间不
存在”；`uc-engine` 只把查询路由给该应用层负责人。

`QuerySetupState` 继续只表示设置是否完成、设备名和邀请。`QueryWorkspaceConvergence` 与
`WorkspaceConvergenceChanged` 继续受 `dev-tools` 限制，只观察内部收敛诊断，不承载正式准入状态，也不
进入桌面、iOS、Android 或 HarmonyOS 的产品契约。

正常加入仍只要求调用一次 `JoinSpace`；后续推进和恢复不需要产品动作。每一次公开 `JoinSpace` 调用都表示
新的用户操作，不是恢复命令；相同邀请码也生成新的 attempt、join、ordinal、成员实例和安全材料。调用方
丢失首次结果时查询 `current_join`，不能重放公开操作。`CancelJoinSpace` 是用户明确放弃
本机待加入时的可选命令，只保存 CancelRequested；返回 Pending 表示发起方尚未裁决。未知或不属于本机的
`join_id` 返回稳定 NotFound；Active/Rejected 返回已保存终态，不能创建反向流程。

新的用户操作只在旧本机加入尚未持久保存 Prepared、没有相关写前恢复或 Space transition 且记录完整时
可以取代旧加入。负责人必须在同一事务或加密写前记录中保存旧 `SupersededByNewJoin`、停止旧提交前
outbox、保留迟到消息与隔离取消所需事实、创建全新尝试并推进 DeviceTrust revision；提交后才发送新请求。
旧终态不公开为第四类 JoinSpace 结果，`current_join` 只显示新尝试。崩溃恢复只能得到完整旧状态或完整
新状态，不能出现两个当前加入或丢失当前加入。

Prepared 一旦持久保存，就必须假定邀请方可能已经正式提交。新的 `JoinSpace` 返回专用
`PreviousJoinCannotBeSuperseded` Engine Conflict，且不生成新 attempt、ordinal、材料、邀请消费或目标
Space 副作用；原加入继续向前恢复。该错误必须使用独立稳定错误码，不得映射为 1238。被取代尝试的迟到
Candidate、重复请求和取消回复只重放被取代事实或推进旧清理；若收到 Commit 或后继消息则说明协议或
持久状态矛盾，进入 RecoveryRequired，不能重新打开旧尝试或覆盖新尝试。

相同邀请码的新尝试仍遵守 Sponsor 一次性 claim：旧尝试尚未消费时可以继续，已经绑定旧 attempt 或身份时
稳定拒绝，不能改绑或复用旧材料。取代不调用 ResetSpace/FactoryResetSpace，也不清当前 Space、历史、
设备身份、设置、关系、搜索或文件；Cross-Space 仍在新尝试完成 J3 前保持来源 Space 活动。

取消和 S2 只在 Sponsor 的持久版本上决定先后。取消先保存时最终为 `Rejected(Cancelled)`，没有
AddDevice；S2 先保存时 Commit 本身就是 `TooLateCommitted` 的稳定依据，Joiner 停止重发取消并继续保持
Pending，直到同一尝试完成为 Active。该路径不自动追加 RemoveDevice，也不把完成者变成取消裁决者；用户
随后仍要退出时，必须在 Active 后从另一台当前成员设备通过现有 RemoveMember 和确认流程发起一项独立
成员变化；本规格不新增本机自移除规则。

Sponsor 使用现有 `RemoveMember(device_id)` 处理 `pending_inbound_member`，产品沿用现有的二次确认交互。
准入负责人按持久阶段隐藏差异：S2 前，移除与 S2 原子竞争，胜出后保存 Rejected 而不创建 AddDevice；S2
后、S3 前，移除追加真实 `RemoveDevice` 后继，永久保留 Add/Remove，再以 RemovedBeforeActivation 收束。
若 S3 已经先胜出，现有 RemoveMember 继续作为普通当前成员移除，不需要调用方改换接口。未知设备仍返回
现有 member-removal NotFound；产品只能使用快照给出的候选 device id，不能从地址或展示名猜测。

现有 `ResetSpace` 和 `FactoryResetSpace` 不新增产品入口，但实施规格 023 后必须先由 profile 级负责人处理：

- `ResetSpace` 不是取消。只要统一 `admission_slot_held` 为真，或仍有任一终态 outbox、终态重封装或物理
  清理未收束，就使用现有
  `RESET_SPACE_UNAVAILABLE_CODE` 返回 Conflict，
  且设置、邀请、尝试和公开 revision 均不得变化。静止时继续执行现有“清设置和未消费邀请、保留 keyslot”
  行为；对准入只原子推进 `join_projection_floor_ordinal` 到下一个待分配 ordinal，并推进 DeviceTrust revision。
  它不删除终态、已消费邀请、防重放索引、ordinal、revision、永久激活回执或 `ProfileAdmissionMasterKey`，同一
  join_id 或已消费邀请重放仍返回原内部结果，也不生成 Rejected、Cancelled 或 RemoveDevice。
- `FactoryResetSpace` 是唯一不等待远端裁决的本机强制销毁边界，可在 Pending 时执行，但不表示 Sponsor
  拒绝、取消或回滚了可能已保存的 AddDevice。负责人先锁住 profile、停止全部运行和接收入口，并通过
  `SecureStoragePort` 持久保存独立的 factory-reset phase；随后幂等清除当前、暂存和 profile 准入全部密钥，
  但保留该生命周期标记。只有确认密钥清除
  成功后，才按固定命名空间删除设置、关系、准入密文、成员历史与永久激活回执密文、已消费邀请、待处理邀请、搜索文件以及
  受管缓存和 blob 目录中的实际文件，最后清 intent。文件删除按经校验的 profile 固定根目录执行，不依赖
  已删除密钥解出逐文件路径，也不能越过该根目录。
  密钥清除失败不得提前清设置；密钥已清后的任何失败只能在启动时继续向前清理，不能重开旧 profile、
  生成同名 key 或处理旧协议消息。完成后从新的 profile generation 开始，旧会话和订阅永久失效。

`Rejected.reason` 首版固定为：

| 类别 | 含义 |
| --- | --- |
| `InvitationUnavailable` | 邀请无效、过期、取消、已消费或不能再使用；不细分可枚举原因 |
| `AuthenticationRejected` | 口令或已认证身份不符合本次邀请 |
| `IdentityConflict` | 本机持久身份、成员实例或凭据与目标绑定冲突 |
| `BaseHistoryChanged` | 发起方在 S2 前不再位于候选基础历史 |
| `JoinerHistoryAhead` | 加入方已有同沿革的更新历史，不能由旧候选覆盖 |
| `HistoryConflict` | 双方历史不可按本次候选安全连续，且未进入 Engine RecoveryRequired |
| `PeerUpgradeRequired` | 同一已认证对端明确只支持旧协议，不能完成本次提交 |
| `Cancelled` | 发起方在 S2 前持久接受取消请求 |
| `RemovedBeforeActivation` | Sponsor 对待激活候选执行 RemoveMember；提交前无 Add，提交后保留 Add+Remove |

`Rejected` 通常只用于正式提交前可证明没有 `AddDevice` 的稳定业务拒绝，例如邀请无效、身份冲突、基础
历史变化、目标历史更旧或已确认旧协议。`RemoveMember` 在提交前也可得到
`Rejected(RemovedBeforeActivation)`，此时没有 AddDevice；提交后得到同一产品原因时，原 AddDevice 和
后继 RemoveDevice 都永久保留，不能伪装成回滚。存储损坏、加密状态不可读、不可恢复摘要矛盾和内部
不变量破坏是 Engine 稳定错误或 `RecoveryRequired`，不得伪装成普通拒绝。

下列边界不进入 `JoinSpaceRejectionReasonSummary`：

| 情形 | 对外结果 |
| --- | --- |
| 输入缺字段、设备名非法、join_id 格式非法 | 现有 InvalidInput 类 Engine error；不创建 attempt |
| 本机未解锁、当前 setup/session 不可用 | InvalidState 类 Engine error；不创建或推进 attempt |
| 旧本机加入尚未保存 Prepared | 按 ADR-022 原子保存旧 SupersededByNewJoin 并创建全新 attempt |
| 旧本机加入已保存 Prepared 或更晚 | 专用 `PreviousJoinCannotBeSuperseded` Engine Conflict；原 attempt 向前恢复，新请求零副作用 |
| 已有非终态入站准入或其他不可取代 profile 工作 | `JoinOperationInProgress` Engine Conflict；原工作不变 |
| 未确认保留不可读来源历史 | 现有确认所需 Engine conflict；不联系 Sponsor、不创建 attempt |
| J0 本机持久化失败、安全存储不可用、目标 keyslot 损坏 | Storage/Internal 类 Engine error；失败关闭 |
| attempt 已存在后的网络断开、超时、对端暂不可达 | 同一 `Pending`；后台继续，不返回 transient error |
| 已保存状态出现不可能摘要、半提交或迁移矛盾 | `RecoveryRequired` 快照加稳定 Engine error |
| `CancelJoinSpace` 使用未知或不属于本机的 join_id | NotFound 类 Engine error，不返回 Rejected |
| `RemoveMember` 与准入阶段竞争 | 负责人返回现有移除结果并发 `DeviceTrustChanged`；调用方重新查询完整快照 |
| 非静止准入期间调用 `ResetSpace` | 现有 `RESET_SPACE_UNAVAILABLE_CODE` 加 Conflict；零副作用 |
| `FactoryResetSpace` 清密钥或后续清理失败 | 现有 FactoryReset 稳定错误；保留 reset intent 并只向前重试 |

桌面、UniFFI 和 HarmonyOS 绑定映射同一 tagged result。绑定不得把 `Pending` 变成超时错误，也不得把
`Rejected` 重新解释为成功或产品端重试步骤。

### Internal capabilities

内部只允许由准入负责人组合以下完整能力，不为每条消息建立产品接口：

```text
MembershipHistoryV2
  verify_and_receive(versioned_event)
    -> Applied | AwaitingDependencies(missing_event_or_receipt_ids) | Invalid(reason)
  verify_and_receive_decision(versioned_decision)
    -> Applied | AwaitingDependencies(missing_event_or_receipt_ids) | Invalid(reason)
  verify_and_receive_activation_receipt(event_id, receipt)
    -> Applied | MissingMembershipEvent(event_id) | Invalid(reason)
  missing_activation_receipt_ids(max_records, max_bytes)
  verify_checkpoint(checkpoint, attestations, authenticated_sponsor)

HistoricalSignaturePort
  verify(algorithm_version, public_key, payload, signature)

AdmissionSecurityTransitionPort
  prepare(base_state, candidate_core, key_package) -> staged_ref + public_commitment
  derive_public_commitment(staged_ref) -> AdmissionSecurityCommitmentV1
  activate(prepared_ref, candidate_event_id, expected_commitment_id)
  discard(prepared_ref)

AdmissionSpaceTransitionPort
  prepare_source(attempt_id, source_generation, unreadable_policy)
  stage_target(attempt_id, target_space, keyslot, workspace_state)
  activate(attempt_id, completion) -> JoinedSpaceSummary
  discard_pre_commit(attempt_id)
  recover(attempt_id)

SpaceJoinRecordStorePort
  load(attempt_id)
  load_by_join_id(join_id)
  compare_and_advance(expected_stage, next_state_with_outbox)
  project_current_local_join()
  project_current_inbound_member(current_space_id)
  advance_device_trust_revision(expected_revision)
  scan_recoverable()  // stage 非终态，或 outbox 非空，或存在写前恢复工作
  compact_terminal(attempt_id, expected_version)

WorkspaceAdmissionChannelPort
  exchange versioned Candidate / Prepared / Commit / Applied / Complete / CancelRequested /
           Rejected / Ack envelopes
```

`MembershipHistoryV2` 必须一次完成“父历史授权、精确凭据选择、签名验证、结构和结果验证”，调用方不能
自行拼接 `resolve -> verify -> receive_verified`。安全适配器只提供暂存、激活和放弃能力，不决定流程。
`AdmissionSpaceTransitionPort::activate` 隐藏来源排空、最终快照、重封装、活动世代提升、session 重开和
清理日志；调用方不能逐步调用 keyslot、setup、关系清理或主表覆盖接口拼出 J3。

激活回执只能由 `MembershipHistoryV2` 在对应事件、凭据、摘要和签名完整验证后写入自己的
`activation_receipts` 映射；尝试仓储不能代替它。回执先到时不另存延迟页，发送方先补事件再重发；只有
等待回执才能验证的后继历史沿用一页加密历史缓冲，不能把未验证字段加入正式索引。
`scan_recoverable()` 必须包含业务已完成但 Complete 或其他 durable outbox 尚未确认的记录。

`outboxes` 是多条持久记录的集合，不是随 stage 覆盖的单个字段。每条记录固定 purpose、recipient、message id、
前置消息和载荷摘要；新增另一条消息不能隐式删除旧记录。终态或后继消息可以携带
`supersedes_message_ids`，但负责人必须在保存该结果的同一事务中逐条校验这些 id 属于同一 attempt、相同
双方和允许替代的因果位置，再把它们标为 Superseded。迟到消息从已保存终态重建相同 Ack 或最终结果，
不能重新打开阶段。所有 outbox 的清理条件固定如下，网络写入成功或连接关闭都不算对端已保存：

| outbox | 唯一可清理证据 |
| --- | --- |
| JoinRequest | 对端匹配 DeliveryAck，或绑定该 request message id 的 Candidate/Rejected |
| Candidate | 对端匹配 DeliveryAck，绑定该 Candidate message id 的 Prepared，或 Sponsor 终态 Rejected 明确 supersede |
| Prepared | 对端匹配 DeliveryAck，或绑定该 Prepared message id 的 Commit/Rejected |
| Commit | 对端匹配 DeliveryAck，或绑定该 Commit message id 的 Applied/Rejected |
| Applied | 对端匹配 DeliveryAck，或绑定该 Applied message id 的 Complete/Rejected |
| CancelRequested | 绑定该请求 message id 的 Rejected，或任何已验证 Commit/后继结果将其标为 TooLateCommitted |
| Rejected | 对端从持久 Rejected 终态重建的匹配 RejectedAck |
| Complete | 加入方在 J3 保存 Active 后可重建的匹配 `CompleteAck` |
| invitation consume | 渠道返回 204、404 或 409；5xx、超时和不可达保留重试 |
| 既有成员安全更新 | 目标成员认证确认已经持久应用相同 event、target epoch 和 `security_commitment_id` |
| 历史页或激活回执批次 | 接收方认证确认已经持久保存页摘要/回执标识集合；补拉缺失依赖时历史游标不前进 |

接收方必须先在同一次事务或写前恢复中保存 inbox 去重、业务效果和重建 Ack 所需的 message id/摘要，
再允许发送 Ack。Ack 不进入 durable outbox；如果发送前崩溃或 Ack 丢失，发送方重放原业务消息，接收方
从已保存阶段或终态重建同一 Ack。发送方清理原消息时保存收到的 Ack 或后继消息。终态压缩必须等待自身
全部 durable outbox 按本表清空，但不等待 Ack 的再次确认。

`AdmissionUnavailable` 不在上表的清理证据中。它只安排同一 JoinRequest 的有界重试，不能清 outbox、消费
邀请、增加 ordinal 或把 Pending 改成 Rejected。Sponsor 在 S2 前保存 Rejected 时必须一次 supersede 自己
尚未确认的 Candidate 和 JoinRequest 回复；S2 先保存时 Joiner 用 Commit 或其后继结果原子清理
CancelRequested。两条竞争路径都不能留下永远无法确认的旧消息。

当前 `WorkspaceAdmissionOwnerPort` 中按步骤公开的 `record_local_readiness`、
`record_admission_saved` 等接口在迁移完成后删除。现有 `CurrentMemberSignaturePort` 不再承担历史公钥
解析；若当前会话证明仍需要它，应把签名当前载荷和用历史公钥验旧事件的职责明确分开。

当前 `QueryMigrationProgress`、`MigrationProgressSummary`、`MigrationPhaseKind` 和
`SwitchSpaceUseCase::resume_pending` 同时删除。它们暴露 Prepared/HandshakeDone/Swapped 内部阶段并依赖
独立恢复状态，不能保留为新尝试的投影。空间切换进度只表现为同一 `JoinSpaceStatusSummary::Pending`；最终迁移
计数仍在 Active 中返回。删除范围包括公开 Operation/OperationKind/OperationResult、dispatch、handler、
AppFacade、绑定、宿主 probe、公共接口文档、装配依赖和对应契约/恢复测试；不得保留兼容别名。
旧 `.migration_state` 只允许由 `uc-infra` 内部一次性只读 importer 通过私有 `LegacyMigrationPhaseV1` DTO
解码，导入或诚实收束后删除旧文件。公开 `MigrationStatePort`、`MigrationPhase` 和旧恢复入口不再存在。

### Independent versioning

以下五个版本不得复用一个整数或相互推断：

| 版本 | 决定什么 | 未知版本行为 |
| --- | --- | --- |
| 事件格式 | 规范字段、编码和事件标识 | `UpgradeRequired`，不尝试按已知布局解析 |
| 决定格式 | 决定字段、编码和决定标识 | `UpgradeRequired`，不尝试按已知布局解析 |
| 签名算法 | 公钥和签名验证方法 | `UpgradeRequired`，不报告签名无效 |
| 准入网络协议 | 消息类型、顺序和上限 | 同一已认证对端确认旧版后，提交前 `Rejected(PeerUpgradeRequired)`；提交后保持 Pending 等待升级 |
| 加密存储格式 | 本机状态解码和迁移 | 保留原密文并返回稳定升级错误 |

本规格首次实现固定使用以下兼容边界，不留给实现阶段重新分配：

| 边界 | 当前基线 | 本规格目标 |
| --- | --- | --- |
| 成员事件 / 决定 | V1 | `MembershipEventV2` / `MembershipDecisionV2` |
| 成员历史通道 | `uniclipboard/membership-history/1` | `uniclipboard/membership-history/2` |
| 配对 wire | V9 | V10 |
| 准入续接通道 | 当前无生产通道 | `uniclipboard/workspace-admission-resume/1` |
| 工作空间收敛存储 | `WORKSPACE_STATE_V2_PREFIX` | `WORKSPACE_STATE_V3_PREFIX` |

V10 和新准入续接 `/1` 承载本规格的版本化消息及 Ack。V10 pairing 外层必须是可在不解码 body 的情况下
先读取的固定版本头；新端收到已认证 V9 响应才返回 `PeerUpgradeRequired`，旧端收到 V10 允许只失败关闭，
不能要求旧代码理解新错误。

成员历史新成功路径只监听和使用 `/2`。连接 `/2` 失败、超时或没有 ALPN 共同项只证明当前不可达，必须
保持 Pending/Offline；只有连接同一目标设备身份的旧 `/1` 成功，且认证 transport peer id 与原目标完全
相同，才确认 `PeerUpgradeRequired`。这个 `/1` 连接只做能力探测，不发送或接受 V1 历史，不推进准入；
新运行时不得注册 inbound `/1` 成功处理器，因此两台新版设备之间没有旧成功路径。`/2` 与 `/1` 都失败
时不得猜测版本。不存在协商后回退旧成功语义的分支。

首版固定以下协议上限：单个 pairing / resume / history frame 解码前最多 4 MiB；单页最多 256 条事件或
决定，激活回执请求/响应每批最多 256 个标识，且编码后仍不得超过 4 MiB；每个本机 attempt 只保留首份
有效 Completion；恢复路由最多 256 个。零长度、计数溢出、声明长度与实际不符或任一上限超出都在分配大缓冲区和语义
解码前拒绝。后续调整上限必须升对应网络协议版本，不能仅改本地常量。

## Membership history verification workflow

每条事件按以下固定顺序验证，任何调用方不得省略或调换：

1. 按显式事件版本解码并检查大小上限；未知版本返回 `UpgradeRequired`。
2. 用该版本的规范编码计算事件标识，检查沿革、父引用、深度和操作标识去重。
3. 创世事件只允许无父 `AddDevice`：作者必须等于被加入实例，实例必须由事件内设备标识和凭据派生，
   再用该凭据验证自签名。其他无父事件全部无效。
4. 非创世事件从精确父位置重建或读取已验证快照。作者必须在该父分支有效，并且其准入激活门禁已经
   解除；迁移检查点中的继续成员按检查点规则视为已激活。作者的 V2 AddDevice 已验证、但对应激活回执
   尚未到达时返回 `AwaitingDependencies`，不得记为 Unauthorized、Invalid 或 Diverged。
5. 由作者实例和 `author_credential_id` 在父历史中选择唯一凭据。找不到、找到多个或编号不符都失败；
   禁止查询当前 OpenMLS 树、成员资料或同设备的新实例。
6. 使用事件声明的签名算法和历史公钥验证规范载荷。算法不支持与签名无效是不同结果。
7. 对 `AddDevice` 检查新凭据结构、实例派生、实例未占用，并确认公钥/算法与候选 KeyPackage signer、
   admission bundle 及安全摘要绑定完全一致；对 `RemoveDevice` 检查目标在父位置有效。被移除作者从更旧
   父位置签署的记录只能形成另一可验证分支，不能扩展包含其移除的当前分支。
8. 纯函数应用操作，重新计算结果成员摘要、安全状态摘要、安全更新绑定和准入包摘要；任一不符即拒绝。
9. 只有以上步骤全部成功，才调用现有决定边界保存事件并推进 known/applied head。远端移除仍按
   ADR-020 等待本机用户决定。

成员决定先验证被引用移除存在且结构有效，再从该移除的父位置确认决定者当时是成员、选择唯一凭据并
验签，最后检查观察位置、决定随机数和结果摘要。当前 OpenMLS 树中是否仍有决定者不参与判断；决定只能
影响 ADR-020 允许的本机决定或对端关系，不能创建成员或越过未决定移除。

历史分页继续使用有界连续页。每页必须从接收方已知位置开始，不能跳父节点；凭据随首次需要它们的页
传递并去重。成员核对摘要同时比较永久激活回执摘要，接收方按已验证 AddDevice 的 event_id 分批请求缺失
回执。事件已经存在但回执暂缺时，成员事实可以保存，候选仍被激活投影排除，依赖该候选作为作者的后继页
进入加密延迟区并停止历史游标。回执先到则要求发送方先补事件再重发，不建立第二游标或证明延迟区。
Completion 只在准入续接中由加入方验证完成者的当前历史位置，不进入普通历史核对。不能通过发送完整成员
资料表或Space 加入记录替代成员历史及其激活回执。

## Admission workflow

一次准入使用三段、五条版本化业务消息：

```text
Prepare:  Candidate -> Prepared
Activate: Commit    -> Applied
Confirm:  Complete
```

邀请验证、口令挑战、已有身份认证以及 CancelRequested、Rejected 和各类 Ack 控制
消息不计入这五条。每条消息都绑定 `attempt_id`、单调消息号、前一条已持久消息的 `message_id`、前一阶段
摘要和消息版本；重复只返回已保存的下一消息，乱序不能跳阶段。

| 阶段 | 发起方持久状态 | 加入方持久状态 | 普通权限 |
| --- | --- | --- | --- |
| `Candidate` | 精确候选、基础历史、公共安全承诺、暂存目标安全状态、Candidate outbox | 已保存本次凭据和 KeyPackage | 双方仅准入通道 |
| `Prepared` | 保持同一暂存状态，等待提交 | 已验证完整历史、重算相同安全承诺并暂存目标世代；Cross-Space 已保存来源备份，Prepared outbox | 双方仅准入通道，可安全拒绝 |
| `Commit` | 原子保存 AddDevice、封存目标安全状态/旧成员更新、待激活门禁和 Commit outbox | 收到精确正式事件，尚未应用 | 候选有正式事件但不发布，既有成员继续旧安全状态 |
| `Applied` | 永久保存回执，激活目标状态，建立 Complete 与旧成员更新 outbox，解除本机观察门禁并释放提交保护 | 原子保存同一事件、目标暂存状态、门禁、永久回执和 Applied outbox | 发起方和取得相同证明/安全更新的观察者可把候选视为离线成员；加入方自身仍关闭 |
| `Complete` | 重发同一完成消息直到 CompleteAck | 永久保存完成消息；J3 恢复同一 Space transition 并保存 Active | 双方进入普通范围；提交后到达的取消已是 TooLateCommitted |

详细顺序：

1. **J0 加入方准备**：先完成输入、解锁状态和不可读来源历史确认预检；通过后生成 256-bit 随机
   `attempt_id`、独立公开 `join_id`、持久 `local_join_ordinal`、本次成员凭据和 KeyPackage 私有状态，
   把初始请求、profile 级新 DeviceTrust revision 和 outbox 与恢复资料一起加密保存，再发送。同一次加入的
   重启、断线和后台恢复不得生成另一标识、序号或凭据；新的公开 `JoinSpace` 调用按 ADR-022 创建新尝试，
   并只在旧尝试未持久保存 Prepared 时安全取代它。
2. **S0 发起方受理**：验证邀请、口令、双方身份、当前分支和本机资格；保存邀请消费绑定、加入方身份和
   基础历史位置，并把本机加密 claim、`attempt_id`、双方身份和远端 consume outbox 一次提交。存在待决定
   移除或 `RecoveryRequired` 时不创建候选；同一邀请改绑另一尝试或身份时稳定拒绝。渠道 consume 在提交
   后异步重试，不影响本机裁决。
3. **S1 形成 Candidate**：从基础历史生成最终不再变化的 V2 `AddDevice`，只以父历史中已激活的当前成员
   和本次加入方暂存精确 OpenMLS 下一状态。Sponsor 从 staged commit 导出
   `AdmissionSecurityCommitmentV1`，完成无循环的候选 event_id 后，保存 Candidate outbox，再发送基础历史、
   候选和只供本次暂存的安全交付资料。此时不写成员事件。
4. **J1 返回 Prepared**：加入方逐条验证完整历史或诚实接受迁移检查点，处理 Welcome 后从本地 staged
   状态独立导出相同公共安全承诺并逐字段比较。Fresh 设备只暂存目标世代；Same-Space 不备份内容；
   Cross-Space 先保存来源初始备份，再暂存目标 keyslot、安全状态、收敛状态和关系世代。任何分支都不启用
   目标状态；最后保存 `PreparedAdmissionProof` 和 outbox 后发送，来源 Space 保持活动。
5. **S2 正式 Commit**：发起方验证 Prepared，并再次比较基础历史。完全相同时，在一个本地事务或可恢复
   写前记录中取得本分支提交保护，保存原样候选、封存同一目标安全状态及其既有成员更新、增加待激活
   门禁和 Commit outbox。Sponsor 普通运行仍使用旧安全状态；普通历史导出和 group-update outbox 不得在
   S3 前泄露该 AddDevice 或新 epoch。这里是唯一正式提交点；之后取消和网络失败都不能删除事件或回滚。
6. **J2 返回 Applied**：加入方验证 Commit 就是已准备候选，从目标暂存世代再次导出相同安全承诺，原子
   保存同一事件、目标暂存状态、本地待激活门禁和永久 `AdmissionActivationReceipt`，并保存 Applied
   outbox 后发送回执。此时目标仍不是活动 Space，不能重建 Welcome、成员实例或目标世代。
7. **S3 发出 Complete**：发起方验证 Applied，在同一事务或写前恢复中把回执追加到版本化成员历史、激活
   封存的目标安全状态、把既有成员更新移入正式 outbox、清除本机门禁、保存
   `AdmissionCompletion` 和 Complete outbox，然后释放本分支提交保护和普通历史导出门。其他成员以后
   只通过有界历史、该事件的激活回执和正式安全更新取得同一结果；首次传播不属于 S3 原子完成条件，崩溃后重试。
8. **J3 激活本地目标**：加入方把首份有效 Complete 保存到本机尝试，再调用一次完整的 Space transition。
   Fresh 创建首个目标活动世代，Same-Space 原子提升已暂存的目标安全/历史状态，Cross-Space 按下节排空
   来源、完成数据重封装并提升目标活动世代。目标 manifest、keyslot、数据库、安全状态和内存 session
   全部一致且运行入口恢复后，负责人清除门禁并原子保存 `Active` 终态和 CompleteAck 重建材料。任何已保存
   CancelRequested 已在 Commit 到达时归类为 `TooLateCommitted` 并清除对应 outbox，不与 J3 再次竞争。
   Ack 只清理发送者的 Complete outbox，不进入本机 outbox；重复调用返回同一结果。

Commit 可以携带只在正式提交后使用的有界不透明恢复路由，其集合只来自候选父历史中的已激活成员，
不含设备名、普通偏好或无关资料，并作为敏感负载加密保存。Commit 同时携带 Sponsor 在 S1 已生成并在
S2 封存的 `completion_recovery_deliveries`；每份只绑定一个父历史 member_instance/credential_id，只能由
该成员使用自己的旧 epoch 状态解密和应用，加入方只能转发不透明密文。全部 recipient 和 ciphertext 摘要
进入 `admission_bundle_digest`，不能在接管时临时重建。Commit 还必须携带完整 resume public key；接收方
先核对它与 AddDevice 中的 `resume_public_key_digest` 一致，再允许保存。只携带摘要不能用于第三方验签。

原发起方在 J2 后不可达时，加入方可通过受限准入续接向任一可达当前成员提交同一 Commit、Applied 和该
成员的精确 delivery。双方先完成固定顺序的持久挑战：帮助者先生成 nonce、递增并保存该 attempt 的计数，
再使用精确当前成员凭据签署协议版本、attempt、lineage、event、原 Sponsor、joiner、帮助者实例、双方
认证 transport identity 摘要、新 nonce、计数和双方最后持久消息号；加入方验证后先保存挑战和自己的最后
消息号，再使用 Commit 携带且由 AddDevice 摘要绑定的 resume key 对完全相同载荷响应；帮助者验证响应并
保存已认证 session 后，才接收 Commit/Applied。任一身份、当前成员资格、计数、nonce、消息号或签名不符
都拒绝，不能以网络重连或 transport identity 变化重置计数。

挑战通过后，该成员仍必须从有界连续历史证明一个包含 AddDevice、且自己在父位置和签署位置都有效并已
激活的分支；随后应用自己的封存更新，从本地目标 GroupContext、成员集合和目录重算出同一公共安全承诺，
并在一个本地写前恢复中保存事件、回执、目标安全状态、Completion 和 outbox，才返回等价 Complete。只
保存事件/回执而没有应用同一安全状态的成员不得完成。该接管不开放普通成员目录或内容；没有任何其他
合格且能应用自己 delivery 的成员可达时保持 Pending。

在有限消息协议中，S3 与 J3 之间存在不可消除的短暂观察差异：发起方已证明加入方完成落盘，加入方在
收到 Complete 前仍为 Pending。此时加入方继续失败关闭普通入口，发起方重发 Complete；加入方持久保存
后回复只用于 outbox 清理的 CompleteAck。发起方和已取得同一 Applied/安全更新的其他成员可以把加入方
视为暂时离线成员；加入方自己在 J3 前仍没有普通权限。Ack 不移动成功边界，也不得把网络发送成功当作
加入方 Active。

## Space transition within J3

> 后续规格 033 取代本节中 CrossSpace 对本机历史执行 source final snapshot、目标 MasterKey 重封装和数据库/blob generation 替换的规则。完成一次 V3 profile 存储升级后，CrossSpace 复用 profile data generation，只切换目标 Space control generation 与新写入保护上下文；本节其余准入门禁、持久 phase 和失败恢复语义继续有效。

J3 是准入负责人的内部子事务，不是新的产品流程。三类本机状态固定如下：

| 类型 | J3 行为 |
| --- | --- |
| `Fresh` | 验证目标暂存世代，创建首个 `ActiveSpaceGenerationManifestV2`，从 manifest 加载 session 和运行入口 |
| `SameSpace` | 不备份或重封装本地历史；在当前 Space 内提升同一 AddDevice、证明和目标安全世代，仍遵守 Complete 前门禁 |
| `CrossSpace` | 恢复 transition、排空来源运行期、安装目标 catalog，复用 profile data generation 提升目标 control manifest，再恢复目标运行入口；不重封装本机历史 |

Cross-Space 的固定执行顺序：

1. 在保存 Complete 后把 phase 推进为 `ActivationStarted`，取得唯一 Space 切换租约；新的本地操作返回
   Pending/InvalidState，后台网络和全部来源写入暂停并排空。崩溃重启默认没有运行入口，先从该 phase 恢复。
2. 验证 profile 已完成规格 033 的 V3 storage upgrade，读取并固定当前 `profile_data_generation`。若仍是
   V1/V2 profile，先退出 J3 并由启动升级负责人完成升级；CrossSpace 自身不得调用旧 reader 或 payload rewrap。
3. 验证目标 keyslot、Space control generation、MLS group/epoch、安全承诺、成员关系和完整 content key catalog；把目标 catalog 原子安装到
   profile 加密 key vault。重复安装必须按保护组与 catalog 摘要幂等，冲突失败关闭。
4. 结束、取消或隔离来源 Space 的在途发送、接收和目录发布，构造复用同一 `profile_data_generation`、只替换
   SpaceId/keyslot/control generation 的 V3 manifest。验证完成后原子提升 manifest，进入 `TargetPromoted`；
   SQLite、blob 和搜索历史在该过程中不复制、不扫描、不重加密。
5. 从新 manifest 加载目标 MLS/security session，把目标 protection group 设置为新写入上下文。确认本机取消已在
   Commit 时稳定归类为 `TooLateCommitted` 后，恢复目标接收、成员、搜索和内容活动，并在同一恢复边界保存
   Active、Ack 重建材料和 `CleanupPending`；这之前不能向产品或网络暴露可运行状态。
6. 幂等删除退役 Space control generation、transition 暂存和 wrapped attempt data key。
   清理失败不回滚 manifest 或 Active，后台继续；不得删除仍用于本机历史读取的来源 protection group catalog。

profile 级 `WorkspaceConvergence::scan_recoverable()` 是新状态的唯一启动恢复入口，即使没有活动 Space
也必须先构造。启动顺序固定为：解锁 ProfileAdmissionMasterKey 和其他恢复所需安全存储引用，导入一次
旧空间迁移状态，恢复非终态准入/Space transition，重开并验证活动 manifest，最后
才启动成员发现、Presence、内容接收、文件、搜索和其他后台任务。旧
`SwitchSpaceUseCase::resume_pending()` 不得再从 `try_resume_session` 或 `unlock_space` 单独推进。

## Branch serialization after commit

S2 的同一持久操作设置 `admission_commit_guard { attempt_id, event_id }`。在永久 Applied 已写入成员历史且
Complete 与既有成员更新 outbox 已建立前，`WorkspaceConvergence` 的所有本地加入、其他目标移除和成员
安全提交入口都返回同一“成员变化进行中”结果，不生成候选或安全状态。发起方与既有成员继续使用旧安全
状态交换普通内容；只有候选与任何成员之间的普通通道保持关闭。

若用户明确确认本次候选永久失联，产品把 `pending_inbound_member.device_id` 交给现有 `RemoveMember`。
S2 后该操作通过正常 `RemoveDevice` 流程移除这个精确待激活实例；RemoveDevice 作为 AddDevice 的后继永久
保留，不是回滚；原尝试完成为 `Rejected(RemovedBeforeActivation)`。它与 S3 保存 Applied 在发起方同一
持久版本上原子竞争：移除先胜出时，保存 Add 后继 Remove、最终安全状态、发给加入方的受限 Rejected 和
既有成员更新 outbox，再释放 guard；Applied 后到只重放该结果。S3 先胜出时，同一 `RemoveMember` 继续
按普通当前成员移除，不需要另一公共命令。除此之外不得绕过 guard。网络收到的竞争后继可以验证并
保存为 known 分支，但不能在 guard 期间自动应用；与当前分支竞争时按 ADR-020 保持分叉，不能为了让
准入继续而改写基础位置。

## Cancellation and forward-only recovery

- 用户新的 `JoinSpace` 与 `CancelJoinSpace` 是不同动作。前者按 ADR-022 在本机 Prepared 之前原子保存旧
  `SupersededByNewJoin` 并创建全新尝试，不等待 Sponsor 裁决；后者请求 Sponsor 把当前尝试正式拒绝，且不
  自动创建下一尝试。两者不得共享产品端步骤或把本机被取代伪装成 Sponsor Rejected。
- 发起方是本次尝试提交或拒绝的唯一裁决者。加入方在初始请求已经发出后，取消只保存并发送
  `CancelRequested`，并继续保持 `Pending`、凭据、KeyPackage 和暂存安全状态。发起方只有在 S2 前保存该
  请求时才生成 `Rejected(Cancelled)`；加入方收到并持久保存该终态后才清理暂存状态。
- 加入方失联不会永久占住 Sponsor 的准入槽。Sponsor 对快照中 `pending_inbound_member.device_id` 调用
  现有 `RemoveMember`：Accepted、Candidate 或 Prepared 阶段由负责人保存 Rejected 而不创建 AddDevice。
  未受理邀请按原 `expires_at` 自动失效；已经 Accepted 的 attempt 不因本机时钟超时静默消失，只能由该
  命令、收到 joiner CancelRequested 或既定邀请撤销动作持久化明确裁决。
- 发起方用自己的尝试版本原子裁决 joiner CancelRequested、Sponsor RemoveMember 和 S2。joiner 取消只有
  两种结果：取消先于 S2 保存时没有 `AddDevice` 并得到 Rejected；S2 先保存时取消为
  `TooLateCommitted`，Commit 或任一后继消息明确替代 CancelRequested，同一尝试继续到 Active。Sponsor
  主动 RemoveMember 仍保持原三阶段含义：S2 前无 Add 的 Rejected；S2 后、S3 前永久 Add+Remove 并返回
  `Rejected(RemovedBeforeActivation)`；S3 后是独立普通移除。取消不得复用后两条路径，也不自动触发移除。
- 本机 invitation claim 保存后，远端 consume 丢失、5xx 或进程崩溃只重试 consume outbox。即使旧 code
  暂时还能从 rendezvous 解析，第二个 attempt 也必须由本机 claim 稳定拒绝；不能把远端消费和本机尝试
  伪装成一个跨系统事务。
- Candidate 发送前后崩溃，发起方重发同一 Candidate；Prepared 保存后丢失，加入方重发同一证明。
- S2 或 J2 跨多个存储写入时崩溃，重启先完成或核对同一写前记录；恢复完成前历史、安全状态和门禁整体
  不可运行，不能暴露半提交快照。
- Commit、Applied、Complete 或任一 Ack 丢失时只重发同一业务消息，或从持久去重记录重建回复。
  相同消息幂等；乱序消息返回当前阶段或要求
  重发前置消息，不产生副作用。
- 加入方已持久保存 Prepared 后，原发起方在 S2 前或保存 S2 后尚未把 Commit 交给加入方即永久离线时，
  其他设备无法证明它是否提交；加入方只能保持同一 `Pending`，直到发起方持久状态恢复，不能超时变成
  Rejected 或创建新实例。Prepared 之前则允许后续明确 `JoinSpace` 按 ADR-022 安全取代，不能用超时自动取代。
- 加入方已保存 Commit 和 Applied 后，才可把同一事件和回执提交给其他合格当前成员；首个验证并永久
  保存成功的成员返回等价 Complete。加入方只保留首份有效 Completion；其他完成者不能产生成员事实、
  取消结果或全网证明。
- 双方同时崩溃时，各自用 `scan_recoverable()` 从最后持久阶段继续，包括业务已完成但 Complete outbox
  尚未收到 CompleteAck 的记录。Ack 从持久阶段或终态重建，从不进入 outbox。同步调用超时返回
  `Pending`，后台不停用同一尝试。
- P2P 暂时不可用时保持 `Pending`，不得自动切换 LAN。
- S2 后出现磁盘损坏、不可能的摘要冲突或无法恢复的半提交状态时进入 `RecoveryRequired`，不能伪装成
  `Rejected` 或创建另一尝试覆盖。

## Reset and profile destruction

普通 ResetSpace 使用一次 profile 级 compare-and-reset：负责人先在同一快照确认没有非终态、outbox、
写前恢复、Space transition 或清理工作；不满足时返回 Conflict，不能先调用现有 setup reset。满足时通过
同一事务或加密写前记录推进 `join_projection_floor_ordinal` 和 DeviceTrust revision，并执行现有设置与未
消费邀请清理。任一步失败都恢复到完整旧状态或继续同一 reset 记录，不能出现“设置已清但旧 current_join
仍公开”或相反状态。重置完成后发送一次新 revision；被隐藏终态、已消费邀请、永久激活回执和全部单调元数据
仍可解密并用于幂等重放。

FactoryResetSpace 不运行上述静止检查。它按以下唯一顺序执行：

1. profile 级负责人拒绝新操作，停止活动、网络监听和接收入口，并通过 `SecureStoragePort` 的独立固定别名
   持久保存不含 Space、设备、attempt 或其他业务标识的 `ProfileLifecycleMarkerV1::WipingKeys`；该别名不在
   后续加密密钥清除清单中。
2. secure-storage adapter 幂等清除整个 profile 的活动/暂存 Space keyslot、KEK、迁移 key、
   `ProfileAdmissionMasterKey` 和仍存在的 wrapped-key 根，并重新查询确认这些固定别名全部不存在。删除一部
   分后失败时保持 intent 和锁定状态；启动只能继续擦除，不能尝试恢复旧 session。
3. 密钥确认全部不存在后，把 intent 推进为 `ClearingState`，按固定命名空间删除 setup、关系、准入密文、
   历史/证明密文、outbox、邀请、搜索文件，以及受管文件缓存和 blob 目录中的实际文件。存储适配器从已经
   校验并固定的 profile 根目录删除整个受管命名空间，不从已不可解密的数据库引用推导路径；清理前后都
   拒绝符号链接或路径逃逸。该步骤不需要解密；任一实际文件删除失败时保留 `ClearingState`，重启继续同一
   清理，不能创建新 profile 或返回成功。
4. 全部命名空间确认为空后创建新的 profile generation，把 marker 原子替换为新 generation 加 None 并返回
   成功。新安全状态以后生成新的
   profile key；旧 join_id、邀请、订阅、续接凭据和协议消息都没有可恢复目标。

这条本机销毁路径不向远端发送 Rejected、Cancelled 或历史回滚。若 Sponsor 已保存 AddDevice，其他设备仍
保留该事实并把本机视为离线，之后只能按正常 RemoveDevice 或既有恢复规则收束。FactoryReset 的成功只
证明本机旧状态已经销毁，不能作为远端准入结果。

## Legacy migration

### Legacy switch-space recovery gate

升级时可能已经存在旧 `MigrationPhase`，它不含 attempt、候选、公共安全承诺、回执或 outbox，禁止伪造
`SpaceJoinRecord` 或补造激活证明。新运行时在 V2 -> V3 成员状态迁移和任何网络监听之前只运行一次
`LegacySwitchSpaceRecovery`：

| 旧状态 | 唯一处理 |
| --- | --- |
| `None` 且 backup 为空 | 无操作 |
| `Prepared` | 先证明来源 setup/session/keyslot 和全部主记录仍由来源 key 一致可读，再幂等清 backup、旧迁移 key 和旧状态；不得导入为新 attempt |
| `HandshakeDone` | 目标 keyslot/session 已替换且旧准入已经发生，只能按旧语义向前完成全量重封装和 setup 切换，再对完整目标状态做迁移分类；不得回滚或补造 Applied |
| `Swapped` | 先用目标 key 重开验证全部主记录，再幂等完成 setup/identity/cleanup，随后对完整目标状态做迁移分类 |
| 状态不可解析、run key 缺失、backup/phase 不符、target/setup/keyslot 不一致 | 原样保留全部工件并进入 `RecoveryRequired` |

旧 phase 3 逐行覆写且 J1 备份之后仍允许来源写入。恢复 HandshakeDone/Swapped 时必须枚举全量主记录并用
目标 key 重新打开；不在 backup 中且目标 key 也不可读的行证明旧窗口已经丢失一致性，必须保留原密文并
进入 RecoveryRequired，不能把记录标为“不可读但迁移成功”。

`None` 但 backup 非空是旧 phase 1 在写 Prepared 前崩溃的孤儿窗口。只有来源状态和全部主记录一致可读、
且不存在目标 keyslot/安全状态副作用时才可清 backup；旧 backup 不含 run_id 且系统安全存储不可枚举，无法
定位的孤儿密钥记为旧版不可逆安全存储残留，不猜测或删除其他 key。旧状态成功收束后删除独立恢复接线；
之后所有新切换只由 `SpaceJoinRecord.space_transition` 恢复。

### Workspace state reset policy

下版不读取或迁移旧成员状态。旧资料在进入普通运行前被判定为必须重置；重置流程无需解码旧成员历史即可清除旧状态，并建立不含旧成员字段的新状态与 V2 单成员根历史。缺少 V2 历史时，准入、普通交换和成员操作全部失败关闭。

# 6. Implementation Plan

实施按可运行的纵向切片推进，而不是先把每一层全部铺开：先在内部测试入口跑通 Fresh/Same-Space 的
Candidate -> Complete、精确历史验签和当前成员接收者；再补每个持久点重启、永久激活回执与单 Sponsor 恢复；
随后完成旧数据迁移、第三方接管和 Cross-Space J3；最后统一公开契约与全部绑定。每个切片结束都必须有
真实接收方落盘证据，但在迁移、降级保护、生产入口删除和完整故障矩阵全部通过前，不发布或默认启用 V2
成功路径，也不保留可回退的 V1 普通加入。

## Step 1: 固定失败测试和格式夹具

**File:** `crates/uc-core/tests/`、`crates/uc-application/src/space/convergence/admission/tests.rs`、
`crates/uc-application/src/space/convergence/membership/tests.rs`、`crates/uc-engine/tests/`

**Change:** 增加 A 加 B、B 加 C、A 移除 B、C 加 D 的红测；保存当前 V1、可恢复公钥 V1、缺少已移除
作者公钥 V1 和损坏 V1 的不可变夹具；四种可读 workspace layout 分别使用真实历史密文，不允许由当前 DTO
现场重编码替代；同时保存 None+孤儿 backup、Prepared、HandshakeDone、Swapped 和损坏旧 MigrationPhase
夹具。断言当前失败发生在旧作者验签、提前准入边界和独立 Space Switch 恢复窗口。

**Risk:** 测试必须实际执行并报告数量；不能只构造已经预先标记 verified 的事件绕过真实验证。

## Step 2: 在 `uc-core` 建立 V2 历史规则

**File:** `crates/uc-core/src/membership/membership_history.rs`、
`crates/uc-core/src/membership/workspace_convergence.rs`、`crates/uc-core/src/membership/ports.rs`、
`crates/uc-core/src/membership/mod.rs`

**Change:** 增加 `MembershipCredential`、`VersionedMembershipEvent`、`VersionedMembershipDecision`、
`AdmissionSecurityCommitmentV1`、历史内永久 `MembershipActivationReceiptRecord`、迁移检查点、激活基线、
`SpaceJoinRecord` 和 `CrossSpaceTransitionV2` 领域状态；实现父历史授权、精确凭据选择、事件/决定结果
验证和固定激活投影。把事件、决定和激活回执验证收成一个完整历史入口，保留 ADR-020 的移除决定边界。

**Risk:** 创世、自移除、重新加入和旧父分支必须明确验证；不能让公钥存在本身等于当前授权。

## Step 3: 在 `uc-infra` 实现显式公钥验证、暂存安全状态和加密迁移

**File:** `crates/uc-infra/src/space/security/mls_group.rs`、现有安全适配器、
`crates/uc-infra/src/db/repositories/workspace_convergence_store.rs`、相关 migration 和测试

**Change:** 提供显式算法和公钥验签；把 OpenMLS admission 拆成可冷恢复的 prepare/activate/discard，并
从 Sponsor staged commit 与 joiner staged Welcome 独立导出相同规范公共安全承诺；
加密保存精确 staged commit、Welcome、目标 epoch 和 delivery 输出；如果 OpenMLS 类型不能直接序列化，
则保存包含全部随机输入、密钥派生版本和规范输出的确定性重放记录，恢复时不得取得新随机数或只重算摘要；
在版本化成员历史中增加唯一激活回执映射、回执摘要和按 event_id 有界补拉；把收敛负载升级到
`WORKSPACE_STATE_V3_PREFIX`，新增 V3 slots、active pointer、加密 migration record 和旧单行 guard，
实现 V2 原密文旁路保留、
三类迁移结果、激活基线、加密尝试、outbox、终态压缩和写前恢复。迁移器必须先旁路写 V3，再用生产
入口重新打开验证，最后原子发布 pointer 与 guard。复用 `SecureStoragePort` 的一次性迁移密钥模式实现
`ProfileAdmissionMasterKey` 加 wrapped `SpaceJoinRecordDataKey`，增加世代化目标 keyslot/workspace 暂存
和 `ActiveSpaceGenerationManifestV2`，并让目标
Space 仓储显式按 attempt/Space/generation/key 访问，不依赖活动 session。准入仓储同时加密保存
`local_join_ordinal`、`join_projection_floor_ordinal` 和跨 Space 单调的 DeviceTrust revision；它们只选择或
标记现有尝试投影，不形成独立状态索引。扩展 secure-storage 清除能力，使其能按固定 profile 加密密钥
清单幂等擦除全部活动/暂存/ProfileAdmission 密钥并验证不存在，同时保留独立
`ProfileLifecycleMarkerV1`；增加按命名空间清理接口。

**Risk:** 不得把 OpenMLS pending commit 内部类型泄漏到核心或应用接口；只保存输入、epoch 或摘要不能
证明重启后恢复的是同一安全状态；激活回执不能只留在尝试记录，Completion 不得扩张成全网证明；旧
二进制打开 V3 必须在写入前失败且不能覆盖任何新记录；数据库/WAL/SHM 扫描不得出现身份或安全明文。
全新设备 J0 也必须可恢复；J3 前不得替换来源 Space 的 session、keyslot 或活动指针。

## Step 4: 由 `uc-application` 完整接管准入事务

**File:** `crates/uc-application/src/space/convergence/`、
`crates/uc-application/src/space/admission/`

**Change:** 在 profile 级常驻的 `WorkspaceConvergence` 内实现 Initiated、Accepted、Candidate、Prepared、
Committed、Applied、Sponsor Completed、Joiner Active、Rejected 和 Superseded 状态机及可重建 Ack；即使没有活动 Space，也先构造该负责人和
准入仓储，再按需接入零或一个完整活动 Space 上下文。每次回复前持久化状态和 outbox；发起方独占提交/
拒绝裁决；S2 到永久
Applied 加 Complete/既有成员更新 outbox 之间持有本分支 guard，并封存 AddDevice、新 epoch 和更新，S3
后才开放普通导出。基础历史变化时提交前稳定拒绝，提交后只向前恢复。加入方在 Candidate 阶段完成
完整历史验证，不再在发起方正式保存后才开始验证。本机邀请 claim、身份绑定和远端 consume outbox 一次
提交；实现 joiner CancelRequested 与 S2 的两结果裁决，并分别实现 Sponsor `RemoveMember(device_id)` 与
S2/S3 的三结果裁决。每次创建本机
加入或改变 DeviceTrust 完整快照时，在同一事务或写前恢复中推进 local ordinal/revision 后再发送失效
提醒；Commit 或后继结果把未确认取消稳定收束为 TooLateCommitted，CompleteAck 只确认 Active；
AdmissionUnavailable 只保留原 Pending 和 JoinRequest。实现按 purpose/recipient/message id 索引的多 outbox、
终态 supersede 和本节每类持久确认条件，并由
同一负责人执行 ResetSpace 静止检查、投影 floor 推进和 FactoryReset 只向前销毁编排。

**Risk:** S2/J2 涉及多个持久化能力时必须用真实事务或可恢复写前记录；同步函数结束不能取消后台尝试；
加入方取消不能自行删除 Prepared；业务终态但 outbox 未清空的尝试仍必须被恢复扫描发现。

## Step 5: 把 Space Switch 并入同一准入恢复事务

**File:** `crates/uc-application/src/space/lifecycle/switch_space/`、
`crates/uc-application/src/space/lifecycle/session.rs`、`crates/uc-core/src/setup/migration.rs`、
`crates/uc-core/src/ports/clipboard/blob_migration_repo.rs`、`crates/uc-infra/src/migration_state.rs`、
`crates/uc-infra/src/db/repositories/migration_repo.rs`、setup/keyslot/session/relationship/security 仓储

**Change:** 先实现一次性 `LegacySwitchSpaceRecovery`，按旧 phase 收束并重开验证已有状态；再以
`AdmissionSpaceTransitionPort` 替换独立 `MigrationPhase` 写入。J1 保存来源初始备份和目标暂存世代；J3
由一个深模块取得 Space 切换租约，排空所有来源写入、补齐最终 revision、幂等重封装、验证目标世代、
一次性提升 ActiveSpaceGenerationManifest、重开 session/运行入口并继续清理。删除 `SwitchSpaceUseCase::resume_pending`
从 try_resume/unlock 的独立接线。旧 `.migration_state` 的布局解析只保留在 `uc-infra` 私有只读 importer；
导入结束后删除公开 `MigrationStatePort`、`MigrationPhase`、装配依赖和旧恢复测试，不保留兼容别名。

**Risk:** 现有 backup 不含 run_id，旧 phase 3 逐行写且没有捕获备份后的新增行。旧状态证据不完整时必须
RecoveryRequired；新流程在 final manifest 验证前不能开放任何网络或写入，也不能把 setup JSON、keyslot
或关系清理当作活动 Space 的单独真相。

## Step 6: 接入规格 022 的统一运行门禁

**File:** `crates/uc-application/src/space/convergence/mod.rs`、
`crates/uc-core/src/membership/ports.rs`、规格 022 列出的普通成员消费者及架构检查

**Change:** 从 V2 单成员根和后续已验证事件派生范围：根成员和
成员获得明确基线；其他 V2 Add 必须有历史内永久 Applied 回执，本机候选还必须有本机 Complete。Presence、邀请、公告、
普通历史、内容、文件、活动状态和补送使用同一快照；准入安全更新的既有接收者也只能从该快照派生，
删除 sponsor handshake 对完整成员资料仓储的接收者枚举。

**Risk:** 门禁只能收窄范围。受限准入消息和已移除设备决定传递继续使用各自精确计划，不能为了恢复而
开放普通通道。

## Step 7: 统一 `uc-engine` 和全部绑定结果

**File:** `crates/uc-engine/src/contract/`、`crates/uc-engine/src/operations/space/`、
`bindings/uc-engine-uniffi/`、`bindings/uc-ohos-napi/`、公开契约测试和宿主验收代码

**Change:** 组装并长期持有 profile 级 `WorkspaceConvergence`，无活动 Space 时也把 Join、Cancel、
DeviceTrust 查询、现有 ResetSpace 和 FactoryResetSpace 路由给它；活动 Space `AppFacade` 仍完整构造后作为一个上下文接入。用本节精确 tagged
`JoinSpaceStatusSummary` 取代只返回 `SpaceJoined` 的成功形态，保留全部
joined_space 字段；新增 `CancelJoinSpace(join_id)`，复用现有 `RemoveMember(device_id)`；给正式
`DeviceTrustSnapshotSummary` 增加 `current_join` 和 `pending_inbound_member`，所有变化只发送现有
`DeviceTrustChanged { revision }` 并由绑定重新查询。`QueryWorkspaceConvergence` 及其事件继续只在
`dev-tools` 存在，不进入产品绑定。删除
`QueryMigrationProgress` 的 Operation、OperationKind、
OperationResult、dispatch、handler、AppFacade、绑定、宿主 probe、接口文档和契约测试，以及三种公开旧
迁移阶段。`QuerySetupState` 不增加准入投影，也不新增按 join id 查询任意历史操作的接口。

**Risk:** 这是跨平台契约变化，版本和生成绑定必须同时推进；任何平台不得保留旧成功解释。

## Step 8: 隔离协议版本并删除旧路径和双重事实来源

**File:** `crates/uc-infra/src/pairing/wire.rs`、
`crates/uc-infra/src/network/iroh/membership_history_exchange_adapter.rs`、准入续接通道、当前 `Ready` /
`AdmissionSaved` wire、`WorkspaceAdmissionOwnerPort`、`MigrationStatePort` 生产写入、历史验签接线、
V1 写入路径、旧测试和架构检查

**Change:** 把成员历史通道升到 `/2`、pairing wire 升到 V10，并新增准入续接 `/1`；所有通道先检查固定
外层版本和 4 MiB 上限，再解业务载荷。`/2` 失败只保持不可达；只有同一认证设备成功响应 `/1` 探测才
报告 `PeerUpgradeRequired`，新版运行时不监听 `/1` 成功处理。删除发起方先提交再由加入方验证、当前 OpenMLS 树验旧历史、
逐步准入 Port、长期 V1 双写和旧 `SpaceJoined` 映射。更新文档和架构检查，搜索证明生产路径只剩 V2
负责人。`MigrationStatePort` 和 `MigrationPhase` 的公开 core 类型、facade 依赖、Engine assembly 接线和
infra adapter 一并删除；仅保留私有、只读、一次性的旧布局 importer。

**Risk:** 不允许以兼容名保留旧实现；迁移器是只读版本入口，不是第二套普通运行路径。旧 `/1` 和 V9
只能用于同一认证设备的版本确认或失败关闭，不得交换业务数据或协商回旧成功语义。

## Step 9: 执行故障矩阵和跨平台验收

**File:** 核心、应用、Engine、绑定测试和多 profile/设备验收宿主

**Change:** 对五条业务消息、普通 DeliveryAck、RejectedAck、CompleteAck、每类 outbox、每个持久化点、
取消/S2 竞争、主动移除/S2/S3 竞争、
第三方接管挑战、本分支 guard、公共安全承诺、证明先于事件、激活基线、旧 MigrationPhase 导入、J3 final
revision/manifest 每步、AdmissionUnavailable 重试、统一槽占用判断、ResetSpace 静止/投影事务、
FactoryReset key-first 和实际受管文件清理每步、
认证 `/1` 版本探测、迁移/降级分支、当前成员门禁和接收方内容落盘执行完整矩阵；
扫描持久化与日志明文。

**Risk:** 模拟器、真实设备、CI 和发布是不同证据；未执行的桌面或移动设备项必须写“跳过”。

# 7. Edge Cases

### Scenario: 被移除作者签署过当前历史的合法祖先事件
**Expected behavior:** 使用该作者在自身 `AddDevice` 中保存的公钥验证旧事件；移除不影响旧签名。
**Implementation:** 从事件父历史解析成员实例和 credential_id，不查询当前 OpenMLS 树。

### Scenario: 被移除作者从包含其移除的父位置签署后继
**Expected behavior:** 返回未授权，不进入 known_head。
**Implementation:** 父位置有效成员集合不含作者，即使公钥和签名有效也拒绝。

### Scenario: 被移除作者从移除之前的旧父位置签署后继
**Expected behavior:** 可以验证为另一分支，但不能自动应用到当前分支。
**Implementation:** 签名验证与分支选择分离，按 ADR-020 标记 Diverged。

### Scenario: 被移除目标返回移除决定
**Expected behavior:** 使用被引用移除的父历史凭据验证决定；当前 OpenMLS 树已无该成员也不影响验证。
**Implementation:** 决定验证使用 removal.parent_event_id，不复用当前成员验签入口。

### Scenario: 同一设备移除后重新加入
**Expected behavior:** 新凭据派生新实例；旧凭据只验证旧事件，不能恢复旧实例权限。
**Implementation:** 凭据按成员实例索引，不按 DeviceId 覆盖。

### Scenario: Candidate 后基础历史推进
**Expected behavior:** S2 前稳定 Rejected，放弃暂存状态，不在新父位置自动重建候选。
**Implementation:** Prepared 和 S2 的 compare-and-advance 都比较精确基础历史。

### Scenario: 取消与 S2 同时发生
**Expected behavior:** 恰好得到“无 AddDevice 的 Rejected”或“已正式提交并继续 Pending”之一。
**Implementation:** 加入方只保存 CancelRequested 并等待；发起方在自己的 SpaceJoinRecord 上原子裁决，
收到正式 Rejected 前不得删除 Prepared 或暂存状态。若 Commit 先到，它证明 S2 已先保存，加入方把取消
归类为 TooLateCommitted、清理 CancelRequested outbox，并继续恢复同一尝试直到 Active。

### Scenario: S2 后再次调用 CancelJoinSpace
**Expected behavior:** 返回同一 Pending 或已保存 Active，不产生 Rejected、RemoveDevice 或第二次成员变化。
**Implementation:** 已验证 Commit 或其后继结果是 TooLateCommitted 的持久依据；J3 不再与取消竞争，用户
若在 Active 后仍要退出，从另一台当前成员设备另行调用现有 RemoveMember 并完成现有确认。

### Scenario: 对端因另一准入返回 AdmissionUnavailable
**Expected behavior:** 当前 JoinSpace 保持同一 Pending；邀请、attempt、ordinal 和 JoinRequest outbox 均不
变化，对端释放 profile 槽后自动重试。
**Implementation:** AdmissionUnavailable 不是 DeliveryAck 或 Rejected，只更新内存退避期限；重启从原
outbox 继续，不能创建另一请求。

### Scenario: 已受理加入方在 S2 前永久失联
**Expected behavior:** Sponsor 的 DeviceTrust 快照持续显示 `pending_inbound_member`；用户可以移除并释放准入槽，不产生
AddDevice。仅邀请码到期不会让已 Accepted attempt 从磁盘静默消失。
**Implementation:** 现有 `RemoveMember(device_id)` 与 S2 做 Sponsor 本地 compare-and-advance；移除先胜出
时保存 Rejected/outbox 并清理 staged target，S2 先胜出时追加真实 RemoveDevice。

### Scenario: 本机 invitation claim 后远端 consume 失败或崩溃
**Expected behavior:** 同一 joiner 只能恢复原 attempt，其他 joiner 稳定拒绝；渠道恢复后最终关闭旧 code。
**Implementation:** 本机 claim、身份绑定和 consume outbox 同事务；`PairingInvitationPort` 结果只推进
outbox，不改变 claim 或 attempt。

### Scenario: Sponsor 与 joiner 本地 OpenMLS snapshot 字节不同
**Expected behavior:** 只要两者映射到同一 Commit、GroupContext、成员凭据集合和 key catalog，公共安全承诺
相同；任一公共输入变化则在 Prepared/Applied 前拒绝。
**Implementation:** 两端从本地 staged state 导出 `AdmissionSecurityCommitmentV1`，禁止 hash 私有 client
snapshot 或本机密文布局。

### Scenario: S2 后发生另一项本地成员变化
**Expected behavior:** 在 Applied 永久入账和 Complete outbox 建立前返回成员变化进行中，不生成另一
安全提交；永久失联候选的精确 RemoveDevice 例外。
**Implementation:** 所有本地加入、移除和成员安全入口检查持久 `admission_commit_guard`；远端竞争后继
只保存为 ADR-020 分叉，不自动应用。

### Scenario: Commit 后加入方永久失联
**Expected behavior:** 正式事件不回滚，候选保持无普通权限；以后由用户明确执行正常 RemoveDevice，原
尝试稳定返回 `Rejected(RemovedBeforeActivation)`。
**Implementation:** 重发同一 Commit，不允许超时删除事件或重建成员；只有正式后继 RemoveDevice 能结束。

### Scenario: Applied 与移除待激活候选同时到达
**Expected behavior:** 移除先保存时永久得到 Add+Remove、门禁始终关闭并返回 RemovedBeforeActivation；
S3 先保存时原加入先完成，同一 RemoveMember 随后按普通当前成员移除，不能把已完成加入重写为 Rejected。
**Implementation:** 发起方在同一 attempt 版本和 `admission_commit_guard` 上原子裁决；失败一方只重放
胜出结果，不生成第二安全状态或把 Add 当作不存在。

### Scenario: Complete 长时间丢失
**Expected behavior:** 发起方保存 Applied，加入方仍为 Pending 且普通权限关闭；重发后进入同一 Active。
**Implementation:** Complete outbox 与永久 Applied 同事务保存；Completed 且 outbox 非空的尝试继续被
扫描。加入方 J3 幂等保存 Complete、Active 和 CompleteAck 重建材料；发起方收到 Ack 后
才清理 Complete outbox。Ack 丢失时发起方重发 Complete，加入方从同一持久状态重建同一 Ack。

### Scenario: 原发起方在 Applied 前后永久离线
**Expected behavior:** 加入方尚未收到 Commit 时只能保持 Pending，等待发起方持久状态恢复；加入方已经
保存 Commit 和 Applied 后，有其他合格当前成员可达时可由其验证并保存同一事件和回执后完成。
**Implementation:** 不猜测远端是否完成 S2。受限续接先完成绑定 attempt、双方和帮助者身份、双方
transport identity、计数、nonce 及最后持久消息号的双向签名挑战，之后只允许携带已收到的精确 Commit、
Applied 和完成者资格历史；加入方只保存首份有效 Completion，其他完成者不能产生新的全网事实。

### Scenario: 终态尝试压缩后新设备请求历史
**Expected behavior:** 新设备仍能取得 AddDevice 和对应 Applied 回执；Completion 只由原加入方的压缩终态
保留，普通新设备不请求它；重复加入仍返回原终态。
**Implementation:** 激活回执随版本化成员历史永久保留；加密终态记录保留本机 Completion 和重放索引，
首版不自动删除。

### Scenario: 完成恢复路由过期或全部不可达
**Expected behavior:** 保持同一 Pending，通过 P2P 地址刷新或当前成员重新可达后继续；不自动切换 LAN。
**Implementation:** 路由只授权发送精确 Commit/Applied，不授予普通权限，刷新结果不改变成员事实。

### Scenario: 任意消息重复或乱序
**Expected behavior:** 不重复应用、不跳阶段、不生成第二事件、成员实例或安全状态。
**Implementation:** 每条消息绑定 attempt、event、阶段、编号和前一持久 message id；inbox 去重后重放已
保存回复，outbox 只按清理表中的持久证据删除。

### Scenario: 激活回执早于对应 AddDevice 到达
**Expected behavior:** 不解除门禁，也不因到达顺序判 Invalid、Unauthorized 或 Diverged；接收方返回缺少
事件，发送方先补事件并在其持久确认后重发同一回执。
**Implementation:** 接收方不保存无事件回执或建立第二延迟页；发送方永久回执 outbox 保持不变。由候选
签署的后继历史仍使用现有一页加密历史缓冲，等待回执后从原历史游标继续。

### Scenario: 任一 durable outbox 的发送成功但确认丢失
**Expected behavior:** 重启后继续发送同一消息；对端重复处理只重放 Ack 或后继消息，副作用不重复。
**Implementation:** 业务消息等待可重建 DeliveryAck 或绑定它的后继消息；Ack 不进入 outbox。邀请
consume、安全更新、历史页和激活回执批次分别使用本规格清理表中的精确持久证据。

### Scenario: S2 或 J2 写入中进程退出
**Expected behavior:** 重启后得到完整前一状态或完整后一状态；半状态不进入普通运行。
**Implementation:** 单事务或加密写前记录，启动恢复先于运行期和网络监听。

### Scenario: 加入方已有相同、落后、更新或分叉的同 Space 历史
**Expected behavior:** 相同且原准入已完成时幂等 Active，相同但激活未完成时保持 Pending；可验证落后时
先按同一候选追赶；加入方更新时 Rejected；不可比较历史进入分叉或恢复，不执行 Space Switch 副作用。
**Implementation:** Candidate 准备前比较沿革和精确历史位置。

### Scenario: 发起方有待决定移除
**Expected behavior:** 不从尚未决定的 known_head 创建候选；先完成本机决定。
**Implementation:** 只允许稳定 applied_head 且无阻塞决定的分支发起准入。

### Scenario: 发起方与另一设备已分叉，但本分支自身有效
**Expected behavior:** 可以在本分支加入；不读取、更新或自动合并另一分支。
**Implementation:** 候选只绑定发起方 applied_head，不要求全局多数或全部设备在线。

### Scenario: 成员资料仓储仍保留已移除设备
**Expected behavior:** 历史验证继续可用，但该设备不在准入目标安全状态、拨号、公告或普通内容范围内。
**Implementation:** 安全接收者先从候选父历史和激活门禁派生，再与所需资料相交；不得反向枚举仓储。

### Scenario: 升级资料尚未重置 Space
**Expected behavior:** 只报告必须重置，不恢复旧成员关系，也不开放普通运行。
**Implementation:** 重置清除旧状态后建立 V2 单成员根历史，其他设备必须重新配对。

### Scenario: 收到未知事件、算法、协议或存储版本
**Expected behavior:** UpgradeRequired，不标记 Invalid 或 Diverged。
**Implementation:** 在语义解码或签名验证前检查独立版本。

### Scenario: 成员历史 `/2` 连接失败
**Expected behavior:** 仅标记暂不可达并重试；只有同一认证设备成功响应旧 `/1` 探测时才报告
PeerUpgradeRequired。
**Implementation:** `/1` 只验证相同 transport peer id 和旧能力，不交换历史；新版运行时不监听 inbound
`/1` 成功处理，`/2` 与 `/1` 都失败时不猜测版本。

### Scenario: 迁移写入或 V3 重新打开验证失败
**Expected behavior:** 原活动 V2 行和密文逐字节不变，只作为下次迁移输入；新版返回可重试迁移错误或
RecoveryRequired，并关闭普通写入、准入和旧协议，不留下半活动 V3，也不继续旧版成功流程。
**Implementation:** 保存加密 migration record 并旁路写 V3 slot，生产入口重开验证后才在一个事务中发布
active pointer 和旧行 guard；指针存在时不得回退备份。

### Scenario: 旧二进制降级打开活动 V3
**Expected behavior:** 旧二进制失败关闭，活动 V3、激活回执、指针和只读 V2 备份逐字节不变；当前二进制随后
仍能打开同一状态。
**Implementation:** 旧单行记录在 V3 激活事务中替换为未知 `WORKSPACE_STATE_V3_GUARD_PREFIX`；仓储在
成功解码前禁止任何 upsert，兼容测试运行问题基线旧读取/保存路径并比较前后字节。

### Scenario: 加入另一个 Space 时准入 Pending
**Expected behavior:** 原 Space 保持活动且 J1 后的新写入不会丢失；只有 J3 验证完整目标活动世代后才切换。
**Implementation:** J1 保存初始备份；J3 排空来源写入并追到 final_source_revision，再重封装和提升 manifest。

### Scenario: 本机入站准入与本机 JoinSpace 竞争
**Expected behavior:** 只有最先持久取得 profile 准入槽的一方继续。远端入站请求在任何消费邀请或创建尝试
前收到 AdmissionUnavailable 并保持原 Pending；本机已有入站尝试或其他不可取代工作时，新的 JoinSpace
在备份来源或暂存目标前收到 JoinOperationInProgress 冲突。已有本机 Joiner 时，Prepared 前按 ADR-022
原子取代，Prepared 后返回 PreviousJoinCannotBeSuperseded。任何 Cross-Space J3 都不会遗弃原 Space 的
Accepted/Candidate/Prepared 入站尝试；上述冲突都不是 Rejected。
**Implementation:** `Initiated` 与 `Accepted` 在同一 profile 元数据版本上 compare-and-advance；统一
`admission_slot_held` 覆盖非终态、共享写前恢复、transition 和改变活动世代的清理。业务终态且只剩按
attempt 隔离的消息重发或压缩时释放槽，但恢复扫描继续处理旧记录。

### Scenario: Cross-Space J3 任一步崩溃
**Expected behavior:** 重启时普通网络和写入保持关闭，只从所属 SpaceJoinRecord 的持久 phase 继续；最终
只存在一个活动 Space，不能由旧 migration 恢复器同时推进。
**Implementation:** `ActivationStarted` 后前向恢复 SourceFinalized、DataRewrapped、TargetPromoted 和
CleanupPending；活动 manifest 是唯一选择点。

### Scenario: 准入未静止时调用 ResetSpace
**Expected behavior:** 返回现有 ResetSpace 冲突，设置、邀请、attempt、outbox、revision、Space 和网络状态
逐字节或逐字段不变；它不代表取消或拒绝。
**Implementation:** profile 负责人先检查非终态、outbox、写前记录、transition 和 cleanup，再允许调用现有
reset 能力；检查与开始 reset 使用同一版本。

### Scenario: 终态后调用 ResetSpace
**Expected behavior:** 设置和未消费邀请按现有语义清除，`current_join` 从公开快照消失；同一 join_id、已消费
邀请和旧消息仍幂等返回原结果，下一次新 JoinSpace 使用更大 ordinal。
**Implementation:** 原子推进加密 `join_projection_floor_ordinal` 和 revision，不删除终态、防重放索引、
永久激活回执、profile key 或单调计数。

### Scenario: Pending 时 FactoryReset 或清理中崩溃
**Expected behavior:** 本机旧状态最终全部销毁，但不向远端伪造 Rejected、Cancelled 或回滚；密钥删除失败
时设置不提前清除，密钥已删后崩溃只继续清状态；数据库、搜索文件、受管缓存和 blob 实际文件全部消失，
旧 profile 永不重开。
**Implementation:** profile 级不透明 reset intent 串行 `WipingKeys -> ClearingState -> new generation`；
ClearingState 从经校验的固定 profile 根删除完整受管命名空间，不依赖密文引用；任一文件失败都保留该阶段
重试。每步幂等验证，Fresh Pending 无活动 AppFacade 时使用同一入口。

### Scenario: 升级时存在旧 HandshakeDone / Swapped
**Expected behavior:** 先按旧证据前向收束并全量重开验证，再做 V2 -> V3 迁移；不能伪造新 Applied 或
Completion。缺 key、缺 backup 或出现非备份旧-key行时 RecoveryRequired。
**Implementation:** 一次性 `LegacySwitchSpaceRecovery` 在所有新恢复和网络之前运行，成功后删除旧生产
恢复接线。

# 8. Testing Strategy

## Unit Test

1. 构造 A、B、C 历史，移除 B 后使用 B 的永久凭据验证其旧事件和移除决定，预期通过；让 B 从移除后
   父位置签署事件，预期 Unauthorized。
2. 让 B 从移除前父位置签署另一后继，预期签名有效但关系为 Diverged，不推进当前 applied_head。
3. 同一 DeviceId 使用两份凭据重新加入，预期两个 MemberInstanceId 和两份不可覆盖的历史凭据。
4. 修改事件版本、credential_id、父引用、结果成员摘要、安全摘要、准入包摘要或签名任一字节，预期在
   对应验证阶段失败且状态不变。
5. 让 AddDevice 凭据的算法或公钥与 KeyPackage signer、admission bundle 或安全摘要任一不一致，预期在
   Candidate/Prepared 验证时拒绝，transport key 相同也不能通过。
6. 未知事件格式和未知签名算法分别返回 UpgradeRequired；不能落入 InvalidSignature。
7. 对 SpaceJoinRecord 的每个阶段执行相同消息、前一阶段消息、后一阶段消息和另一 attempt 消息，预期
   幂等重放或稳定拒绝，阶段不跳跃。
8. Candidate 后推进基础历史，再提交 Prepared，预期 Rejected 且没有 AddDevice。
9. 分别把 joiner `CancelRequested` 与 S2、Sponsor 对 `pending_inbound_member.device_id` 的 `RemoveMember`
   与 S2/S3 做并发调度。取消只能得到“无 Add 的 Rejected”或“TooLateCommitted 后 Active”；主动移除仍只
   得到三阶段真值表之一。两类操作不得互相复用结果或自动生成额外 RemoveDevice。
   的 V2 投影，预期只得到固定规则允许的集合，任何回执都不能添加历史外成员。
11. 对同一 event_id 追加相同和冲突 Applied 回执，并让多个有效 Completion 到达同一 Joiner；预期回执
    幂等或 RecoveryRequired，Joiner 只保留首份有效 Completion，后续同事实消息不进入共享历史。
12. 压缩 Sponsor Completed、Joiner Active、Rejected 和 SupersededByNewJoin 尝试，预期 profile key 下的终态、邀请防重放和必要永久回执仍存在，wrapped
    attempt data key 与大负载已删除；outbox 非空或终态重封装未提交时拒绝压缩。Fresh Rejected 也能重启查询。
    原 V2 外层密文逐字节不变，V1 规范证据摘要一致。
14. 对原 Sponsor 和第三方续接签名逐项修改 attempt、lineage、event、Sponsor/joiner/helper 任一成员身份、
    任一 transport identity、nonce、挑战计数或双方最后消息编号，预期拒绝；旧挑战重放不推进状态。
15. 对 4 MiB frame、256 条 history page/receipt batch/route 的边界值和超限值测试，预期边界值可解码，超限在大
    缓冲区分配和业务解码前拒绝。
16. 用 Sponsor 和 joiner 含不同私钥字节的 staged OpenMLS 状态导出公共安全承诺，预期全部公共字段和 id
    相同；逐项修改 group、epoch、Commit、GroupContext、成员集合、目录或交付包，预期明确失败。
17. 保存本机 invitation claim 后，依次返回远端 consume 204、404、409、5xx、网络失败和重启，预期前
    三者只清理 outbox，后两者保留重试，任何结果都不撤销或改绑本机 claim。
18. 构造一份非终态本机尝试和多份终态，预期 `current_join` 选择非终态；终态后选择最大
    `local_join_ordinal`，新 JoinSpace 替换旧终态，ResetSpace 推进 floor 后只隐藏旧终态。制造 ordinal、
    floor 或 revision 重复、倒退和溢出，预期 RecoveryRequired，不能按时间猜测。
19. 让激活回执早于 AddDevice，预期返回 MissingMembershipEvent 且不落库；发送方补事件确认后重发回执。
    再让由该新成员签署的历史页早于激活回执，预期一页加密历史缓冲停止原游标并请求回执，依赖到达后
    继续。Completion 引用未知完成者历史位置时只在受限准入续接中补拉资格历史，不写普通历史证明。
20. 对每类 outbox 分别提供网络发送、错误 Ack、正确 DeliveryAck、绑定后继和终态 supersede，预期只有本
    规格清理表允许的证据删除消息；让 Candidate 与 CancelRequested、S3 的 Complete 与多成员更新并存，
    终态一次标记全部合法旧消息且迟到重放不重新打开阶段。Ack 由持久阶段重建且不进入 outbox；邀请
    consume 的 204/404/409 与 5xx/不可达结果分别验证。
21. 并发保存本机 Joiner Initiated 和当前 Space Sponsor Accepted，预期只有一个取得 profile 准入槽；失败方
    不消费邀请、不创建候选、不备份来源，也不写目标暂存状态。
22. 在 Sponsor 反复交错 CancelRequested 与 S2：取消先保存只得 Rejected(Cancelled) 且没有 Add；
    S2 先保存时 Commit 原子把取消标为 TooLateCommitted，随后只得 Active。J3 前后重复 CancelJoinSpace 都
    不生成 RemoveDevice，CompleteAck 始终只证明 Active。
23. 对同一 JoinRequest 连续返回 AdmissionUnavailable、重启后再返回 Candidate，预期 attempt、ordinal、
    邀请和 outbox 不变；忙碌回复不满足任何 outbox 清理条件。
24. 枚举 ResetSpace 静止判断的每个阻塞项，并枚举 FactoryReset 的 WipingKeys/ClearingState phase；预期
    普通重置冲突零副作用，彻底重置任何失败都只从同一 intent 向前且不会生成远端业务结果。创建实际受管
    cache/blob 文件并逐个注入删除失败，只有文件本体和命名空间均不存在后才能进入新 generation。

## Integration Test

1. 使用真实历史验证适配器证明事件验签不读取当前 OpenMLS 成员树；删除当前树中的 B 后，D 仍完成历史
   验证。
2. 对 Candidate、Prepared、Commit、Applied 及各自可重建 DeliveryAck，以及 Complete/CompleteAck 依次
   丢弃一次，再恢复网络；预期只产生一个 AddDevice、一个成员实例和一个安全状态，最终 Active 且相关
   outbox 清空。Rejected/RejectedAck 另按失败分支逐项执行。
3. 每条消息重复两次并乱序发送；预期 outbox 重放同一回复，没有重复副作用。
4. 在 J0、S0、S1、J1、S2、J2、S3、J3 每个持久化点前后注入崩溃，单方和双方重启后继续同一 attempt。
5. 在 S2/J2 的历史、安全状态、门禁和 outbox 每个底层写入点注入失败；恢复前普通快照关闭，恢复后各
   摘要一致。
6. 在 Candidate、Prepared、Commit 阶段让候选与任一成员尝试 Presence、邀请、公告、普通历史、双向内容、
   文件和补送，预期全部被统一门禁拒绝；Applied 后发起方可把它视为离线成员，但加入方本机入口仍关闭，
   直到 J3 完整完成后才可实际接收和发送普通内容。发起方与既有成员全程继续可用。
7. 让其他当前成员离线，发起方和加入方仍完成；其他成员上线后取得同一 AddDevice 和 Applied 回执，再
   进入普通范围。
8. 对同一 attempt 重复网络消息、断线并重启，预期恢复同一 attempt；随后再次明确执行 JoinSpace，
   Initiated/Candidate 创建全新 attempt，Prepared 及以后返回 PreviousJoinCannotBeSuperseded 且原尝试不变。
9. P2P 失败时保持 Pending，观测不到 LAN 自动连接。
10. 保留已移除成员的资料、公钥和地址，再创建候选；暂存安全状态和全部普通消费者都不包含该成员。
11. 发起方业务已 Completed 但 Complete outbox 未清空时重启；`scan_recoverable()` 必须重发。Ack 后压缩
    尝试，再重放 Applied，仍从压缩终态和历史内永久回执返回同一 Complete。
12. S2 后分别请求另一加入、移除另一目标和成员安全提交，预期都不产生副作用；远端竞争后继只形成分叉；
    Applied 永久入账且 Complete outbox 建立后 guard 才释放。
13. 原发起方在 Commit 送达前永久离线，加入方持续 Pending；在 J2 后永久离线，第三个合格当前成员提供
    有界资格历史、保存同一事件和 Applied、应用属于自己的封存安全更新并从本地状态重算相同公共承诺后
    返回 Complete，加入方 Active。只保存事件/Applied 而未应用安全更新时必须继续 Pending。
14. 压缩所有Space 加入记录后让新设备分页取得历史并按 event_id 补齐永久激活回执，预期仍正确激活 V2 成员，
    不读取尝试仓储，也不请求其他设备的 Completion。
15. A、C 从相同 V1 前缀独立迁移；checkpoint_id 相同、attestation 可并存，不出现伪分叉，激活基线一致。
    migration record、V3 slot、生产重开、`TargetVerified`、active pointer 加 guard 事务和 cleanup 前后逐点
    注入失败，预期只能打开完整旧行或完整 V3，原密文备份不丢且不自动回退。
17. 用问题基线旧二进制打开 guard 并尝试正常保存；预期在 upsert 前失败关闭且 V3 slot、active pointer、
    激活回执和备份字节不变，当前二进制随后成功重开同一 V3。
18. 新旧双向协议夹具验证：新端先读 pairing V10 外层版本再解 body；成员历史 `/2` 超时、拒绝和不可达
    均只保持 Pending。只有同一 transport peer id 成功响应 `/1` 探测才报告 PeerUpgradeRequired；探测不
    交换 V1 历史，新版不接受 inbound `/1`。任何方向都不把版本错误误报为签名无效或历史分叉。
19. 全新设备没有活动 Space 或 keyslot 时，先建立 profile admission MasterKey，再包裹本次 attempt data
    key 并保存后崩溃；重启恢复同一 attempt、凭据、KeyPackage、resume key、ordinal 和 revision，
    SQLite/WAL 不含明文。Fresh Rejected 压缩并删除 attempt data key 后仍能查询和防重放。
20. 已有来源 Space 的设备加入另一 Space，在 J0-J2 每阶段重启并读写来源内容；来源 session/keyslot/
    活动指针始终不变，J3 只在完整目标世代验证后一次性提升活动 manifest，失败恢复不产生混合 Space 状态。
21. 每个业务阶段关闭原 pairing stream，再向原 Sponsor 和第三方当前成员分别发起新挑战；逐项改变
    Sponsor/joiner/helper 身份、任一 transport identity、nonce、计数和双方最后消息号都拒绝，正确续接只
    取得该 attempt 的受限消息，重启后计数不回退。帮助者只在旧父位置曾是成员但当前已移除时也必须拒绝；
    transport identity 漂移不能靠新连接自动接受。
22. 在 S2 后、S3 前反复触发普通历史导出和 group-update delivery，并在封存与发布的每个写入点崩溃；
    其他成员不得看到缺永久 Applied 的 AddDevice 或新 epoch。S3 后事件、激活回执和同一批更新可跨重启补发。
23. 分开并发验证两类裁决：CancelRequested 与 S2 只能得到无 Add Rejected 或 TooLateCommitted 后 Active；
    `RemoveMember` 与 S2/S3 只能得到无 Add Rejected、Add+Remove 后 RemovedBeforeActivation，或 Completed
    后普通 Remove。再让原 Sponsor 与多个帮助者并发发送 Completion，证明它们只能完成同一加入，不能因
    取消产生任何 RemoveDevice。历史连续、guard 可恢复释放，终态不改写。
24. 本机已有非终态 JoinSpace 时，用另一邀请加入另一 Space：旧尝试在 Initiated/Candidate 时原子保存
    SupersededByNewJoin，并以更大 ordinal 和全新 join_id 投影新 Pending；旧尝试在 Prepared 及以后时新请求
    返回 PreviousJoinCannotBeSuperseded，`current_join` 仍是原 join_id。原尝试终态后，新有效邀请同样可
    创建新 join_id，不受旧 Rejected 或 SupersededByNewJoin 永久阻塞。
25. Sponsor 在 Accepted、Candidate、Prepared 分别对快照候选调用 `RemoveMember`，并与 S2 反复交错；
    S2 前移除先保存时双方最终只见 Rejected(RemovedBeforeActivation) 且没有 AddDevice，S2 先保存时必须
    追加真实 RemoveDevice，不能删除 AddDevice。
26. 本机 invitation claim 保存后，在远端 consume 超时、5xx、404 和进程重启处逐点中断；同一 code 只能
    恢复原 attempt 或稳定拒绝另一身份，渠道状态不能形成第二次受理。
27. Cross-Space J3 在 `ActivationStarted`、`SourceFinalized`、`DataRewrapped`、`TargetPromoted` 和
    `CleanupPending` 前后分别崩溃；每次重启只能从同一 phase 向前，`final_source_revision` 覆盖 J1 后的
    新增、修改和删除，活动 manifest 始终只指向完整来源或完整目标世代。
28. 对旧 None+孤儿 backup、Prepared、HandshakeDone、Swapped 和损坏 `MigrationPhase` 夹具逐项启动；预期
    私有 importer 按旧语义清理、向前收束或 RecoveryRequired，且从不伪造 Candidate、Applied 或 Complete。
29. 先查询 DeviceTrust revision N+1，再投递延迟的 `DeviceTrustChanged { revision: N }`；绑定必须保留
    N+1。跨 Space J3 后 revision 继续增大而不从目标 Space 重置；事件丢失或 `RefreshRequired` 后重新查询
    得到唯一最新快照。
30. 创建多份历史 Active/Rejected 和一份当前 Pending；快照优先选择 Pending，终态后按固定本地 ordinal
    选择最近结果并保留到下一次 JoinSpace 或 ResetSpace 推进 floor。Fresh Pending 时关系字段为空但
    `current_join` 存在；Cross-Space J3 前关系仍描述来源、之后描述目标。当前入站候选只出现在
    `pending_inbound_member`，不进入 devices。
31. 在业务事实与 profile DeviceTrust revision 写入之间逐点崩溃；重启必须先完成同一写前恢复，查询和
    事件只能同时看到完整旧状态或完整新状态。SQLite/WAL 中没有 ordinal、revision 或尝试负载明文。
32. 双向测试依赖乱序：先发送未知 event_id 的激活回执，接收方不落库并要求事件；再补 AddDevice 和重发
    回执；随后发送由已知候选签署的后继历史但暂不发送其 Applied。接收方保持门禁和原历史游标，依赖到达
    后继续。回执永久不可达时历史事实保留，但候选及其后继不激活、不误报无效。
33. 对业务消息、邀请 consume、既有成员安全更新、历史页和激活回执批次逐类注入“发送成功后崩溃”；只有收到
    各自持久确认后才清理 outbox，重启重放不重复副作用。
34. 在 Sponsor Accepted/Candidate/Prepared 与本机 Cross-Space JoinSpace 的 Initiated 处反复并发；只有
    profile 槽胜出者继续。远端失败方收到 AdmissionUnavailable 并保持 Pending，本机失败方收到冲突，两者
    都无副作用。胜出尝试进入业务终态且不再持有共享写前恢复/transition 后即可释放槽；旧终态 outbox
    继续重试且不被新 attempt 覆盖。
35. Fresh profile 无活动 Space 启动、J0 后崩溃并重启；profile 级 WorkspaceConvergence 必须先恢复，再让
    QueryDeviceTrust 返回同一 current_join。Engine 不创建空 AppFacade，也不接管准入阶段。
36. 对每一种非终态、未清 outbox、写前恢复、Cross-Space transition 和终态重封装状态调用
    ResetSpace，预期现有 unavailable code 加 Conflict，setup、邀请、revision 和所有密文字节不变。
37. 在 Active/Rejected 静止终态调用 ResetSpace，并在 floor/revision、setup 和邀请清理之间逐点崩溃；重启
    只能看到完整旧投影或已隐藏投影，同一 join_id/邀请仍幂等，下一次新 Join 使用更大 ordinal。
38. Fresh Pending 和 Cross-Space Pending 分别调用 FactoryResetSpace；在停止运行、保存 intent、每个密钥
    删除、密钥确认、数据库/搜索/受管 cache/blob 实际文件清理和新 generation 前后崩溃。旧会话永不恢复，
    key wipe 失败不清 setup，任一实际文件失败保留 ClearingState，全部消失后才成功；远端历史不出现伪造
    Rejected/Cancelled。
39. 本机拥有 Joiner 槽时，对端收到 AdmissionUnavailable 后跨重启重试同一 JoinRequest；释放槽后进入同一
    attempt。反向 Sponsor 槽占用时本机第二 JoinSpace 返回冲突，两边都不消费第二邀请或创建第二 ordinal。

## Regression Test

1. ADR-020 的远端新增自动应用、移除待决定、接受、拒绝、分叉隔离和重新加入行为保持不变。
2. 规格 022 的设备列表、主动连接、内容、文件、活动状态和恢复全部继续使用一个当前成员快照。
3. 已移除设备仍可完成受限决定投递，但不能因永久公钥保留进入普通范围。
4. 邀请一次性、过期、取消、口令失败和设备名校验保持原产品语义。
5. 旧空间安全提升只使用明确的迁移状态，不因新增 V2 读取器恢复旧成员表作为普通事实来源。
6. Engine 公共错误、锁定、损坏、空间切换和会话恢复不会被误映射为 Rejected。
7. `cargo` 精确过滤测试必须报告实际运行数；0 tests 不计为通过。
8. 当前 pairing V9、成员历史 `/1` 和收敛存储 V2 夹具保持只读兼容证据；生产成功路径只使用 V10、
   成员历史 `/2` 和存储 V3。
9. `QuerySetupState` 只保留设置和邀请；`QueryDeviceTrust` 在原设备关系字段之外只增加 `current_join` 和
   `pending_inbound_member`，不公开内部阶段。`QueryWorkspaceConvergence` 及其事件继续只用于 dev-tools。
10. ResetSpace 继续保留 keyslot 并只清设置和未消费邀请，FactoryResetSpace 继续先暂停活动和清密钥、再清
    设置与邀请；规格 023 只增加 profile 级忙碌门禁、投影一致性和可恢复清理，不颠倒既有安全顺序。

## Multi-device Acceptance

按独立 profile 顺序运行 A、B、C、D：

1. A 加入 B，B 加入 C；确认每次接收方实际保存同一历史和安全状态。
2. A 移除 B，C 按场景接受该移除；确认 C 的当前分支不含 B，但仍保存 B 的历史验证凭据。
3. C 邀请 D，D 完整验证 B 过去签署的合法历史并 Active。
4. D 与 A、C 分别发送唯一内容；必须从每个接收方实际持久化历史读回，不能以发起方计数、在线状态或
   日志代替。
5. B 尝试签署移除后的当前后继，所有当前成员拒绝；B 从旧父位置签署时只形成隔离分支。
6. 双方在每个准入持久点重启，并对五条业务消息、普通 DeliveryAck 和 CompleteAck 分别丢失一次；恢复后
   仍只有一个 D 实例，任何 Ack 都不产生自己的 outbox。
7. 在 D 已 Applied 后停止原 Sponsor，由另一当前成员先取得同一事件和激活回执、应用属于自己的安全更新并
   重算公共承诺，再完成 D；D 必须实际收发内容。未应用安全更新时 D 保持 Pending。
8. 扫描四个 profile 的数据库、WAL、SHM、缓存和日志，不得出现凭据、设备资料、安全状态、地址或历史
   负载明文。

桌面、iOS、Android 和 HarmonyOS 分别验证同一结果契约。没有实际执行的平台必须记录“跳过”，不能
记录“通过”。模拟器结果不能替代物理设备结果。

# 9. Acceptance Criteria

* [ ] V2 AddDevice 永久保存版本化成员验证凭据，移除和重新加入都不会覆盖旧实例凭据。
* [ ] AddDevice 历史凭据与 KeyPackage/OpenMLS signer 的算法和公钥逐字节一致，并绑定候选、准入包和
      安全摘要；transport 或 identity key 不能代替。
* [ ] 每条非创世事件先从精确父历史确认作者资格，再使用父历史选出的精确公钥验签。
* [ ] 生产代码验证旧成员事件时不查询当前 OpenMLS 树、成员资料、可信关系、地址或在线状态。
* [ ] A 加 B、B 加 C、A 移除 B、C 加 D 的测试中，D 能验证 B 过去的合法签名并 Active。
* [ ] B 不能扩展包含其移除的当前分支；从旧父位置签署只能形成不可自动应用的分支。
* [ ] 被移除目标仍能用历史保存凭据返回合法移除决定，不依赖当前 OpenMLS 树。
* [ ] Candidate 不是成员事件；加入方完整验证并持久 Prepared 之前，发起方没有 AddDevice 副作用。
* [ ] S2 后不回滚、不删除正式事件、不重建成员实例或安全状态；永久失联只通过正常 RemoveDevice 处理。
* [ ] 对待激活候选调用现有 RemoveMember：S2 前只保存 Rejected 且没有 AddDevice；S2 后、S3 前永久保留
      Add/Remove 并返回 Rejected(RemovedBeforeActivation)；S3 先胜出则保留 Completed，再走普通 Remove。
      三种结果不能互相伪装、回滚或改写终态。
* [ ] 加入方生成并持久保存内部 attempt_id、公开 join_id 和本地 ordinal；发起方把邀请消费原子绑定到
      attempt，重复请求不能改绑身份或实例，Engine operation_id 不复用为 join_id。
* [ ] 发起方是提交/拒绝唯一裁决者；CancelRequested 与 S2 只有两个结果：取消先保存则
      Rejected(Cancelled) 且没有 AddDevice，S2 先保存则取消为 TooLateCommitted 并恢复同一尝试到
      Active。后者不自动生成 RemoveDevice，CompleteAck 只证明 Active。
* [ ] Sponsor 可在 Accepted、Candidate 或 Prepared 对快照候选调用现有 RemoveMember；它与 S2/S3 的并发
      严格符合“无 Add Rejected”“Add+Remove Rejected”“Completed 后普通 Remove”三结果，不出现本机和
      远端各自裁决；该主动移除流程与 CancelJoinSpace 的两结果语义互不复用。
* [ ] 五条准入消息、普通 DeliveryAck、RejectedAck 和 CompleteAck 逐条丢失、重复
      和乱序后都恢复同一 attempt，且副作用各发生一次；Ack 不进入需要二次确认的 outbox。
* [ ] J0、S0、S1、J1、S2、J2、S3、J3 前后单方或双方崩溃，重启后结果一致。
* [ ] 原发起方在 Commit 送达前永久离线时保持 Pending；J2 后其他合格当前成员必须先完成绑定三方身份、
      双方 transport identity、计数、nonce 和最后消息号的双向挑战，再保存同一激活回执、应用自己的封存安全
      更新并重算相同公共承诺，才可完成同一尝试；已移除帮助者和 identity 漂移都拒绝。
* [ ] S2/J2 任一底层写入失败都不会暴露历史、安全状态、门禁或 outbox 的可运行半状态。
* [ ] Sponsor 和 joiner 从含不同私有资料的本地 OpenMLS 状态导出相同公共承诺；任一公共字段变化均拒绝。
* [ ] 精确 staged commit、Welcome、目标 epoch 和 delivery 输出可跨重启恢复；实现不通过重新取得随机数
      执行 OpenMLS 或只比较摘要来冒充同一安全状态。
* [ ] S2 到永久 Applied 加 Complete outbox 建立前，本分支拒绝其他本地成员/安全变化；远端竞争后继不
      自动应用，只有精确待激活候选的正常 RemoveDevice 可以例外。
* [ ] S2-S3 期间 AddDevice、目标 epoch 和既有成员更新保持封存；发起方与既有成员继续旧状态通信，S3
      持久保存 Applied 后才共同释放历史、激活回执和安全更新。
* [ ] `scan_recoverable()` 覆盖非终态、outbox 非空和写前恢复记录；Completed 状态重启后不会丢失 Complete。
* [ ] Applied 前所有观察者都排除候选；Applied 后发起方/合格第三方最多把它视为离线成员，加入方本机在
      Complete 和 J3 完成前仍没有 Presence、邀请、公告、普通历史、内容、文件、活动状态或补送权限。
* [ ] JoinSpace 对外只返回 Active、Pending 或 Rejected；内部阶段不进入产品和绑定接口。
* [ ] profile 级 WorkspaceConvergence 在没有活动 Space 时也完整构造并独占 Join、Cancel、恢复、加入状态
      投影以及现有 ResetSpace/FactoryResetSpace 的准入边界；活动 AppFacade 仍只完整构造，uc-engine 只组装
      和路由，不保存准入状态或步骤。
* [ ] 本规格新增或改变的准入专用正式接口只有 JoinSpace、CancelJoinSpace(join_id)、现有
      RemoveMember(device_id)、QueryDeviceTrust 和 DeviceTrustChanged；既有 IssueInvitation 保持不变，
      不新增按 join 查询、入站取消或待激活专用移除接口。现有 ResetSpace/FactoryResetSpace 的签名和结果
      类型不变，只把准入门禁与可恢复清理收进 profile 负责人。
* [ ] DeviceTrustSnapshot 在现有字段外只增加 `current_join` 和 `pending_inbound_member`；Pending 带不透明
      join_id，第一次调用结果丢失后仍可重新查询，候选不进入 devices。
* [ ] Fresh Pending 时设备关系字段为空但 current_join 可用；Cross-Space 在目标 manifest 提升前关系继续
      描述来源 Space，提升后只描述目标 Space，不暴露暂存目标为当前状态。
* [ ] CancelJoinSpace 只请求 S2 前拒绝；提交后返回同一 Pending/Active，不自动移除。Sponsor 对快照候选
      复用现有 RemoveMember 并沿用现有二次确认交互，两项操作的结果不会互相冒充。
* [ ] Pending 的断线、重启、消息重放和后台重试恢复同一 attempt；每次新的公开 JoinSpace 调用表示新的
      用户操作，后台恢复不依赖产品端重放该操作或编排步骤。
* [ ] 本机邀请 claim、attempt 和身份一次保存后即为唯一裁决；远端 consume 的成功、已不存在、冲突、
      网络失败或重启都不能撤销 claim、改绑身份或形成第二 attempt。
* [ ] 每个 profile 全局最多一个被占用的准入槽，不区分 Sponsor 或 Joiner；Initiated/Accepted 原子竞争，
      远端失败方得到 AdmissionUnavailable 并保持 Pending。本机再次明确 JoinSpace 时，只有旧 Joiner 在
      Initiated/Candidate 且无相关恢复工作时可原子保存 SupersededByNewJoin 并创建新尝试；Prepared 及以后
      返回 PreviousJoinCannotBeSuperseded；入站尝试和其他不可取代工作返回 JoinOperationInProgress。
      共享写前恢复、Space transition 或改变活动世代的清理存在时槽不释放；业务终态且只剩隔离
      outbox/压缩时可开始新尝试，旧记录仍由恢复扫描继续且不被覆盖。
* [ ] AdmissionUnavailable 不消费邀请、不创建 Sponsor attempt、不清 JoinRequest outbox，也不形成 Rejected；
      Joiner 保持同一 Pending 并跨重启重试。新的本机 JoinSpace 按 ADR-022 创建新 ordinal，或在不可取代时
      使用专用稳定冲突并保持 ordinal 不变；不得再因新旧输入不一致返回 1238。
* [ ] profile 级 ProfileAdmissionMasterKey 在无活动 Space 时也能解密 revision、ordinal、projection floor、邀请防重放索引和
      终态；J0 的 wrapped SpaceJoinRecordDataKey 只保护未压缩尝试。Active/Rejected/SupersededByNewJoin
      先原子重封装终态，
      再删临时 key；普通 ResetSpace 仍保留这些事实，Fresh Rejected、重启、跨 Space 和下一次 Join 仍可
      幂等读取且不出现明文。
* [ ] J3 前目标 Space 暂存不替换来源 Space 的 session、keyslot 或活动指针；J3 只在完整目标世代通过生产
      读取验证后一次性提升活动 manifest，失败不混合状态。
* [ ] Cross-Space J3 排空全部来源写入并固定 final_source_revision；每个持久 phase 前后崩溃都只向前恢复，
      活动 manifest 从未指向混合来源/目标世代。
* [ ] pairing 会话消失后，Commit 携带与 AddDevice 摘要一致的完整 resume public key；固定顺序的域分离
      challenge 只恢复同一 attempt，错误三方身份、transport identity、nonce、计数、消息号和跨尝试重放
      均拒绝。
* [ ] 每个 AddDevice 的唯一 Applied 回执永久保存在版本化成员历史；Completion 只保存在加入方本机终态。
      尝试压缩后新设备仍能按 event_id 补齐回执并激活 V2 成员，不请求全网 Completion。
* [ ] 回执早于 AddDevice 时接收方不落库并返回 MissingMembershipEvent，发送方先补事件再重发；候选后继
      早于 Applied 时只加密暂存一页历史并停止历史游标。依赖补齐后自动验证，不能因乱序误报无效、未授权
      或分叉，也不存在独立 proof cursor 或 Completion 传播页。
      激活回执只能减少已应用历史授予的权限。
      历史版本提取的不可变密文夹具、迁移测试和重启测试；LegacyWorkspaceConvergenceState 不合成 V2 事件。
      不修改、补签或删除原 V1 历史。
* [ ] 相同规范旧前缀由不同成员迁移得到同一 checkpoint_id，成员证明差异不会制造历史分叉。
* [ ] 未知事件、决定、算法、协议和存储版本返回 UpgradeRequired，不误报 Invalid 或 Diverged。
* [ ] 旧 None+孤儿 backup、Prepared、HandshakeDone、Swapped 和损坏 MigrationPhase 都有只读导入夹具；
      导入不会伪造新准入证明，结果只能是诚实清理、向前收束或 RecoveryRequired。
* [ ] 成员历史使用 `/2`、pairing wire 使用 V10、收敛存储使用 V3；外层版本在业务 body 前检查。`/2`
      失败只表示不可达，只有同一认证设备成功响应只读 `/1` 探测才是 PeerUpgradeRequired，新版不监听
      inbound `/1` 且旧成功路径不能被协商恢复。
* [ ] frame、历史分页、激活回执批次和恢复路由固定上限在分配与业务解码前执行，边界与超限测试均实际运行。
* [ ] V3 迁移使用明确的 slots、active pointer 和加密 migration record；先保存原密文备份和旁路 slot，再用
      生产入口重开验证，最后在一个事务中发布 pointer 与旧行 guard。事务前失败保持旧行逐字节不变，事务
      后只读取 V3 且不自动回退。
* [ ] 问题基线旧二进制打开 `WORKSPACE_STATE_V3_GUARD_PREFIX` 时在 upsert 前失败关闭，不能覆写 V3 slot、
      pointer、激活回执或备份；当前二进制随后能重新打开并验证同一数据。
* [ ] 迁移完成后不再写 V1，生产准入和历史核对没有 V1 备用成功路径。
* [ ] `QueryMigrationProgress` 的公开操作、结果、路由、门面、绑定、宿主 probe、测试和接口文档全部删除；
      公开 MigrationStatePort/MigrationPhase 不保留兼容别名，旧布局只由私有只读 importer 读取。
* [ ] current_join 优先唯一非终态，否则选择不小于 projection floor 的最大 local ordinal 终态；新 JoinSpace
      用更大 ordinal 替换，普通 ResetSpace 只推进 floor 并隐藏旧投影。pending_inbound_member 只选择当前
      Space 唯一非终态候选，终态入站不公开。
* [ ] DeviceTrust revision 在同一 profile 内跨 Space 单调递增，与业务事实同事务或写前恢复推进；迟到的较小
      DeviceTrustChanged 不覆盖新查询，ordinal/floor/revision 倒退、重复、溢出和损坏都失败关闭。
* [ ] durable outbox 是按 purpose、recipient 和 message id 索引的集合；Candidate 与 CancelRequested、
      Complete 与多成员更新可以并存。每条只在收到本规格列出的持久证据，或终态原子列入合法
      supersedes_message_ids 后清理；迟到重放不重开阶段。DeliveryAck 从已保存阶段或终态重建，不进入
      outbox，也不存在 ack-of-ack。
* [ ] 任一非终态、outbox、写前恢复、Space transition、终态重封装或其他清理工作存在时，ResetSpace
      使用现有 unavailable code 返回 Conflict 且零副作用；静止时只清现有设置/未消费邀请并推进投影 floor，
      不删除终态、防重放、单调计数、永久激活回执或 profile key，也不伪造取消、拒绝或移除。
* [ ] FactoryResetSpace 可从 Fresh Pending 执行且不需要活动 AppFacade；它先锁 profile 和停止运行、持久化
      无标识 intent、幂等清全部固定密钥并确认不存在，再从经校验的固定 profile 根清设置、关系、准入密文、
      邀请、搜索文件及受管 cache/blob 实际文件。密钥失败不先清设置，任一文件失败保留 ClearingState；全部
      命名空间为空后才成功，完成后旧会话、订阅和协议消息失效且不产生远端成员结果。
* [ ] `ProfileLifecycleMarkerV1` 只通过独立 `SecureStoragePort` 别名保存格式版本、随机 profile generation 和
      三值 reset phase；没有任何业务标识、时间、路径或摘要，不回退到数据库/文件明文，也不被加密密钥
      清理步骤提前删除。其余准入元数据和终态继续加密，并把 generation 绑定进 AEAD associated data。
* [ ] 当前成员范围只由已应用历史产生正向资格，激活门禁只能减少权限。
* [ ] 准入安全状态的既有接收者只来自候选父历史中的已激活成员，残留资料不会带回已移除设备。
* [ ] 数据库、WAL、SHM、缓存、索引和日志扫描无敏感明文。
* [ ] 桌面、iOS、Android 和 HarmonyOS 使用同一结果契约；未执行设备项明确标记“跳过”。
* [ ] 多设备内容验收读取接收方实际保存结果，不以发送成功、计数、配对或在线代替。
* [ ] 精确测试、工作区检查、格式检查、架构检查和 `git diff --check` 实际通过。

# 10. Risks and Trade-offs

## 永久验证材料

保留被移除实例的公钥会增加少量加密存储和身份关联风险，但这是长期验证不可变历史的必要条件。公钥不
等于当前授权，普通范围仍由当前分支和激活门禁决定。立即删除材料会让历史在成员移除后不可验证，因此
不采用。

## 准入状态增加

五条业务消息、可重建 delivery Ack 和持久 outbox 比当前 Ready / AdmissionSaved 更复杂，但复杂度集中在
唯一负责人内部，换来明确提交点、可恢复幂等和不提前成功。减少 Prepared、Applied 或 Complete 中任一步
都会分别失去“加入方已准备”“加入方已落盘”或“加入方知道发起方已收到”的证明；Ack 只负责有限清理
终态 outbox，不增加权限阶段。

## 短暂双端观察差异

有限网络协议无法保证双方在同一瞬间知道对方已经知道。S3 后发起方确认加入方已落盘，J3 前加入方仍
保持 Pending 和普通门禁；Complete 重发最终消除差异。CompleteAck 确认 J3 已完成，并用于清理发起方
outbox，并且始终对应 Active。S2 后到达的取消已经稳定归类为 TooLateCommitted，不再增加完成后的自动
移除流程。继续增加业务确认轮次只会把同一问题后移，不能带来真正同时完成，因此不采用。

## 提交窗口串行化

S2 到永久 Applied/Complete outbox 的短窗口会暂时阻止本分支的其他成员变化。这是为了保证加入方安装的
目标安全状态不会在确认前过期；锁定整个 Candidate/Prepared 等待期会不必要地影响离线使用，因此不采用。
永久失联候选仍可走精确的正常 RemoveDevice，其他竞争变化按 ADR-020 隔离。

## 发起方裁决与可用性

加入方无法仅凭超时判断发起方是否已经提交，所以取消必须由发起方裁决，Commit 尚未送达而发起方永久
丢失时只能保持 Pending。这牺牲了该故障下的自动结束，换取不重复创建成员和不误删已提交事实。若未来
要求在这个窗口自动接管，必须先设计 S2 前的持久授权交接，不能由产品端超时猜测。

取消在 S2 后不自动转成移除，会要求仍想退出的用户在 Active 后从另一台当前成员设备执行现有明确移除并再次确认。这一额外
操作换来唯一且可解释的正式边界，也避免原 Sponsor 与多个完成帮助者各自产生竞争移除。自动继承取消
意图需要跨设备唯一移除裁决，首版不采用。

## 升级重置保证

旧成员状态不再转换为当前授权事实。升级资料必须重置 Space；重置保留产品明确允许保留的本机数据，并删除旧设备关系与旧成员状态，然后建立 V2 单成员根历史。

## 存储和恢复成本

完整历史凭据、永久激活回执映射、终态索引和 outbox 会扩大加密收敛负载。首版不自动删除小型终态记录，
用存储换取跨长期离线的幂等。个人设备成员数量有限，优先使用完整事务和清晰恢复；只有测量证明负载
成为问题后，才可在负责人内部增加版本化索引、快照或明确保留策略，且不能成为第二事实来源。

## 加入前密钥

全新设备在取得目标 Space 密钥前必须保存私钥和恢复进度，因此增加长期 profile 准入 MasterKey，并由它
包裹每次尝试的数据 key。profile key 只保护准入元数据、终态和 key envelope，不授予任何 Space 内容权限；
attempt data key 在终态重封装和临时资料清理后删除。提前切换当前 session 会破坏来源 Space，使用明文或
仅内存又无法冷恢复，因此都不采用。安全存储中的 profile key 缺失而磁盘仍有相关密文时保持恢复错误，
不能静默重建成员凭据、revision 或防重放索引。

## 重置保留与彻底销毁

普通 ResetSpace 保留压缩终态、防重放和 profile key，会占用少量加密存储，但能避免重置被用作取消旁路，
也保证旧请求仍幂等。加密 projection floor 只控制当前快照显示，不形成第二份业务状态。FactoryResetSpace
则允许用户不等待远端而彻底销毁本机状态；代价是远端可能永久保留一个离线成员事实，必须以后正常移除。
key-first 顺序确保不会在密钥尚存时把设备提前标成 Fresh；密钥已经删除后的清理失败必须靠不透明 intent
向前恢复，因此不能复用普通 ResetSpace 或改成先删业务密文。受管文件内容允许按规则保存原始字节，所以
彻底重置必须删除实际 cache/blob 文件和搜索文件，不能只删数据库引用。

## 成员实例与凭据轮换

首版继续沿用“设备标识加准入公钥派生成员实例”，因此更换成员历史公钥会形成一次明确重新加入，而不是
原地换钥。这避免在本次根因修复中同时重做身份模型。实现不得静默替换同一实例的凭据；未来若需要原地
轮换，必须另行定义由旧、新凭据共同绑定的版本化轮换事件及兼容规则。

## 不采用的替代方案

- **继续从当前 OpenMLS 树查旧作者**：移除会再次破坏历史验证，不能解决根因。
- **移除时删除旧资料**：会破坏审计、决定投递和历史验证，只能作为证明无用途后的物理整理。
- **把公钥重复嵌入每条事件**：可验证但增加重复和替换攻击面；由 AddDevice 引入一次、后续引用凭据即可。
- **正式提交后失败就回滚 AddDevice**：会重写不可变历史，并在其他成员已收到事件时制造更大分叉。
- **让加入方自己重试各步骤**：把顺序和恢复复杂度泄漏到产品与四套绑定。
- **长期双写 V1/V2**：形成两个事实来源并允许旧设备绕过新验证；只保留受限读取迁移器。
- **引入多数确认**：违背 ADR-020 的离线分支模型，也不能替代加入方实际落盘证明。

# 11. Open Questions

无待决产品或架构问题。OpenMLS staged state 若不可直接序列化，使用本规格已定义的完整确定性重放记录；
多个存储若不能参加同一 SQLite 事务，使用本规格已定义的加密写前恢复；旧布局能恢复出的实际公钥只决定
布局矩阵中的迁移分类，不能改变分类规则。这三项都由 Step 1/3 的夹具和故障测试裁决，不需要实现者另选
完成边界。

事件/决定 V2、成员历史 `/2`、pairing V10、准入续接通道版本、收敛存储 V3 和 V3 slot/pointer/guard 模型
均已固定；实现不得重新分配、合并版本、删除旧数据、恢复当前 MLS 树验旧历史、提前返回成功或保留第二套
准入流程。
