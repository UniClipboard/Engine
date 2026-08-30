# 规格 028：单一 Space 准入协议

## 状态

- **状态**：主体实现与自动化验收完成；clean-cutover 未完成，实体设备和 Release bundle 本次跳过
- **日期**：2026-08-26
- **实施方式**：Core、Application、Infra、Engine、数据库、绑定与验收一次性切换；中间状态不得发布
- **取代范围**：取代规格 017、023、025、027 中关于 pairing wire、准入消息、准入仓储、准入 runtime 和外层接入的实现设计；成员历史、AddDevice、激活回执、取消、用户新加入取代旧加入、Space transition、ResetSpace 和 FactoryResetSpace 的业务不变条件继续有效
- **不兼容决定**：不读取、翻译、发送或回退到任何旧准入协议、旧 ALPN、旧消息、旧准入记录或旧邀请
- **相关决策**：`docs/adr/017-pairing-as-workspace-admission.md`、`docs/adr/022-user-initiated-join-supersession.md`、`docs/adr/025-application-space-membership-one-shot-rewrite.md`

# 1. Overview

当前分支在提交 `098d806` 中删除了 sponsor 侧 `PairingInboundOrchestrator`，同时新增
`HandleSpaceAdmissionMessageUseCase`。新 use case 被构造并从 `SpaceFacade` 暴露，但没有任何生产网络调用者。
`IrohPairingSessionAdapter` 仍然产生 `PairingSessionEvent`，Engine 仍传入 event port，Application 却没有订阅者；
真实入站连接会落到“没有订阅者，消息被丢弃”的分支。

这不是单个接线遗漏。当前准入纵向链路同时存在以下断点：

- `PrepareJoinSpacePort` 与 `PrepareSpaceAdmissionMessagePort` 只有测试实现，没有生产实现；
- 入站 endpoint 丢失消息种类和 predecessor，只接收不透明 `payload`，并返回无法直接路由的 `Vec<u8>`；
- `PairingAdmissionOutboxDelivery` 只发送 `CancelRequested`，其余可靠消息全部 `Deferred`；
- `PairingEventPort`、`PairingSessionPort`、`PairingSessionMessage`、`DurableAdmissionFrame`、
  `AdmissionOutboxPurpose` 分别表达同一协议的一部分，没有一个唯一消息契约；
- `admission_repository_state` 与新的 application membership ledger 都试图保存准入记录，但没有一套完整生产接入；
- Iroh Router 在 `SpaceFacade` 和 application endpoint 构造前启动，无法一次性安装真实准入 handler；
- `cargo check -p uc-engine --lib --locked` 当前在 Infra 的未完成迁移处失败，不能证明 Engine 组装闭环。

本规格不恢复被删除的旧协调器，也不完成任何兼容适配。它定义一个新的、唯一的 Space 准入协议：

1. Core 只保留一套类型化消息和一个按角色/阶段封闭的数据模型；
2. Application 的 `SpaceAdmissionProtocol` 对用户加入、入站消息、取消、持久恢复和最终结果负唯一责任；
3. Infra 使用标准 OPAQUE 完成口令认证，使用 OpenMLS 完成 Add + Commit + Welcome，使用 Iroh 新 ALPN 传输；
4. Iroh `ProtocolHandler` 在 Infra 内完成连接、认证和 wire 转换，再把一条已认证 typed message 交给 application endpoint；
5. 每个可靠业务消息通过一个新的双向流完成一次受认证的请求—回复交换；
6. 每个回复在发送前持久保存，连接、进程和设备重启只重放同一消息；
7. Engine 只按固定顺序构造、绑定、启动和关闭；产品与绑定不接触协议阶段；
8. 升级时按 ADR-025/规格 027 重建为新的单设备 Space，全部设备重新配对，不导入旧准入或旧成员分支。

标准依据：

- [RFC 9420](https://www.rfc-editor.org/rfc/rfc9420.html) 定义 MLS 的 KeyPackage、Add、Commit 和 Welcome；本项目继续通过 OpenMLS 使用这些能力，不自行实现组密钥变化。
- [RFC 9807](https://www.rfc-editor.org/rfc/rfc9807.html) 定义 OPAQUE 双向口令认证；本规格选择 `opaque-ke` 的固定版本和 Argon2 支持，不保留现有自定义 challenge/HMAC 口令握手。
- [RFC 9106](https://www.rfc-editor.org/rfc/rfc9106.html) 定义 Argon2id；本规格使用其内存受限环境第二推荐参数，避免实现阶段自行调低口令成本。
- [Iroh Protocol 文档](https://docs.iroh.computer/concepts/protocols)定义 ALPN 到 `ProtocolHandler` 的直接路由；本规格使用直接 handler，不使用中间 event subscriber。
- [Stripe 幂等请求](https://docs.stripe.com/api/idempotent_requests)体现“同一操作重试复用同一身份，新业务操作使用新身份”；本项目继续遵守 ADR-022 的相同区分。

# 2. Goals

- 建立一个且仅一个生产 Space 准入协议和 ALPN。
- 让 `SpaceAdmissionProtocol` 成为加入开始、消息推进、取消、重试、重启恢复和完成判定的唯一负责人。
- 让产品调用方只执行 `JoinSpace`、`CancelJoinSpace`、邀请操作和状态查询，不接触任何协议阶段。
- 让 Infra Iroh handler 对每条认证完成的业务消息只调用一次 application message endpoint。
- 用类型化消息取代消息 kind、purpose、frame、payload 和 reply bytes 的多重表达。
- 用按角色和阶段携带合法字段的 Core aggregate 取代几十个可选字段组成的 `SpaceJoinRecord`。
- 每次用户明确 `JoinSpace` 生成新的 admission、join、ordinal、成员实例、KeyPackage 和恢复凭据。
- 同一 admission 的网络重试、响应丢失、进程重启和设备重启保持原身份和原消息。
- 使用 RFC 9807 OPAQUE 验证共享 Space 口令，短邀请码只用于发现和邀请选择。
- 使用 OpenMLS 生成并验证 Add + Commit + Welcome，不在 Application 或 Core 重写 MLS。
- 保持 AddDevice 是唯一正向成员资格，Complete 前普通权限始终失败关闭。
- 每个回复先与状态原子保存，再发送；重复请求逐字节重放已保存回复。
- 让初始认证成功后生成 admission-bound continuation key，使后续恢复不依赖邀请码、口令输入或原连接。
- 使用一个真实加密 membership ledger 原子保存历史、准入、关系、效果和 revision。
- 明确单设备重建和重新配对切换，删除全部旧协议、旧记录读取和旧回退。
- 保持现有 Engine 用户动作、成功结果、DeviceTrust 投影和绑定方法，不公开新的步骤式操作。
- 在 Core、Application、Infra、Engine、真实 SQLite、真实 Iroh 和实体设备层分别给出非零验收证据。

# 3. Non-Goals

- 不支持旧 pairing ALPN、旧 wire version、旧 `PairingSessionMessage` 或旧准入 frame。
- 不增加版本协商、旧协议 translator、fallback、双写、feature flag 或并行实现。
- 不导入旧 admission attempt、旧 invitation claim、旧 outbox、旧 continuation 或旧 membership branch。
- 不恢复 `PairingInboundOrchestrator`、Sponsor/Joiner owner ports 或旧 `DurableAdmissionTransaction`。
- 不让 P2P 失败自动切换 LAN compatibility 线；用户明确选择的独立 LAN 产品线不在本规格内。
- 不改变剪贴板、文件、搜索和普通成员历史同步协议，只改变它们何时因新成员激活而获得最终 scope。
- 不把 Reachability、Iroh 连接、OPAQUE 成功、MLS Welcome 处理或消息发送成功单独视为加入成功。
- 不引入多数提交、中心协调者或要求所有旧成员在线。
- 不把口令、邀请码、continuation key、成员历史、MLS 状态、设备名或地址写入日志或明文存储。
- 不自行实现 PAKE、Argon2、MLS、QUIC、AEAD、HKDF、HMAC 或签名算法。
- 不新增产品端“继续第 N 步”“重试消息”“消费邀请”“完成迁移”等操作。
- 不在实现阶段重新选择完成边界、协议阶段、持久原子性或旧数据策略。

# 4. Current Architecture Context

```text
Component: 当前 Core 准入记录
Path: crates/uc-core/src/membership/space_join_record.rs
Responsibility: 以 SpaceJoinRecord、role stage、可选安全材料、inbox 和 outbox 表达准入。
Relationship: 约五十个可选字段允许大量不可能组合；decode_persisted 还保留旧补零读取。新 aggregate 完成后整体删除。
```

```text
Component: 当前 Application 入站准入 use case
Path: crates/uc-application/src/space/admission/handle_space_admission_message/
Responsibility: 读取 ledger、验证邀请、调用无生产实现的 preparation、保存后返回 reply bytes。
Relationship: 没有生产 caller；消息输入缺少 kind/predecessor，输出无类型。新 protocol 完成后整体删除。
```

```text
Component: 当前 Application 用户加入与恢复
Path: crates/uc-application/src/space/admission/join_space/
Path: crates/uc-application/src/space/admission/recover_space_admissions/
Responsibility: Join 保存 prepared record；Recover 扫描 outbox 并解释送达结果。
Relationship: PrepareJoinSpacePort 无生产实现；恢复逻辑知道逐条 outbox 结果。由 SpaceAdmissionProtocol 的完整动作取代。
```

```text
Component: 当前 Application membership ledger
Path: crates/uc-application/src/space/membership/ledger/
Responsibility: 在一个概念快照中联合历史、准入记录、关系、效果和 revision。
Relationship: 责任方向正确，但没有 Infra 的生产 Load/Commit 实现，且与旧 admission repository 重复。作为新协议唯一持久入口重写并完成接入。
```

```text
Component: 当前 Iroh pairing session/event adapter
Path: crates/uc-infra/src/pairing/session.rs
Path: crates/uc-infra/src/pairing/wire.rs
Responsibility: 注册 /uniclipboard/pairing/2，管理长会话，产生 PairingSessionEvent，并编解码 wire v10。
Relationship: Application 无 subscriber，入站消息可直接丢失。由 Infra 新 ALPN direct handler 和抽象 authenticated exchange transport 取代。
```

```text
Component: 当前 outbox delivery
Path: crates/uc-infra/src/pairing/admission_outbox_delivery.rs
Responsibility: 只发送 CancelRequested 并等待 Rejected。
Relationship: 其他 purpose 永久 Deferred，不能形成完整协议。由 protocol-owned request-response delivery 删除并取代。
```

```text
Component: 当前独立 admission repository
Path: crates/uc-infra/src/db/repositories/space_join_record_store.rs
Path: crates/uc-infra/migrations/2026-08-16-000001_create_workspace_convergence_v3/up.sql
Responsibility: 在 admission_repository_state 保存加密仓储、attempt、terminal 和一份 membership history。
Relationship: 与 application membership ledger 重复且当前 imports 已失效。切换 migration 删除表和生产实现，不保留 reader。
```

```text
Component: 当前 Engine 组装
Path: crates/uc-engine/src/assembly/sync_engine.rs
Path: crates/uc-engine/src/assembly/wire/infra.rs
Responsibility: 安装 pairing handler、启动 Iroh router，再构造 SpaceFacade。
Relationship: endpoint 出现得比 router 晚；event/session 仍传给已删除调用者。新组装先构造 dormant application message endpoint，再注入 Infra handler、spawn router、启动 runtime。
```

```text
Component: 稳定产品与绑定契约
Path: crates/uc-engine/src/contract/operation.rs
Path: crates/uc-engine/src/contract/result.rs
Path: crates/uc-engine/src/operations/space/join_space.rs
Path: bindings/uc-engine-uniffi/src/runtime.rs
Path: bindings/uc-ohos-napi/src/runtime.rs
Responsibility: 暴露 JoinSpace、CancelJoinSpace、邀请、DeviceTrust 和 Active/Pending/Rejected。
Relationship: 保持用户动作与结果；清理只属于旧同步握手的不可达错误分支，不公开 wire 类型。
```

当前数据流：

```text
Iroh pairing handler
  -> PairingSessionEvent
  -> no subscriber
  -> message dropped

SpaceFacade
  -> exposes HandleSpaceAdmissionMessagePort
  -> no Infra/Engine caller

Membership runtime
  -> RecoverSpaceAdmissionsUseCase
  -> PairingAdmissionOutboxDelivery
  -> CancelRequested only; every other message Deferred
```

# 5. Proposed Design

## Invariants and ownership

1. `SpaceAdmissionProtocol` 对一次准入从用户提交到 Active/Rejected 的完整结果负唯一责任。
2. 调用方只执行一个完整产品动作或交付一条连接；不读取 stage、outbox、token、history digest 或 record version。
3. `AddDevice` 是成员资格的唯一正向事实；激活回执只解除该事实的负门禁。
4. Sponsor 是 Commit/Reject 的唯一裁决者；Commit 保存后只能向前完成或正常追加 RemoveDevice，不能回滚。
5. 每个 reply-producing transition 把新状态、输入证据和 exact reply 一次保存后才允许发送。
6. 同一 `message_id + digest` 是重放；同一 id 不同 digest 是攻击或损坏；更高 sender sequence 是乱序。
7. 网络连接、OPAQUE 中间状态和 Iroh stream 都不是业务事实；连接消失后从加密 record 和 pending exchange 恢复。
8. 邀请 claim、admission 绑定和远端 invitation-consume cleanup 一次本地提交；远端 consume 不决定本机资格。
9. OPAQUE shared secret 只认证准入通道并派生 continuation key，不作为 Space MasterKey、MLS secret 或内容 key。
10. OpenMLS staged state 必须逐字节持久或由完整确定性材料重放；恢复不得取得新随机数后生成不同 Commit/Welcome。
11. 同一 profile 同时只有一个会改变活动 Space 或成员历史的准入槽；终态 replay cleanup 不占槽。
12. Complete 前 joiner 普通入口关闭；S2 后 sponsor 只在 S3 验证 Applied 并保存 receipt 后解除本机观察门禁。
13. 新公开 JoinSpace 总是新 admission；自动恢复总是旧 admission；Prepared 及以后不能被新 JoinSpace 取代。
14. P2P 不可用只产生 Pending 和退避，不自动切换其他协议。
15. 任一密文、stage、sequence、predecessor、credential、history、OpenMLS commitment 或 counter 不一致均失败关闭。

开工前固定答案：

| 问题 | 答案 |
| --- | --- |
| 谁负责完整结果 | Application `SpaceAdmissionProtocol` |
| 产品调用方做什么 | 一次 JoinSpace/CancelJoinSpace/邀请操作，之后查询或订阅 DeviceTrust |
| 网络调用方做什么 | Infra 完成连接/认证/wire 后，把一条已认证 typed message 交给 Application endpoint，再发送 typed reply |
| 成功和失败是什么 | Active、Pending、Rejected；本机不可恢复故障使用稳定 Engine error |
| 谁负责重启恢复 | SpaceAdmissionProtocol 从唯一 encrypted membership ledger 继续同一 admission |

## Components

### Core `SpaceAdmissionAggregate`

- **Path:** `crates/uc-core/src/membership/admission/`
- **职责:** 定义唯一消息、角色/阶段、合法 transition、重复/乱序判断、取消、取代和终态压缩规则。
- **输入:** 当前 aggregate、已验证消息证据和 transition-specific domain facts。
- **输出:** 新 aggregate 和需要原样保存的 typed reply，或一个确定性 domain error。
- **关系:** 不执行 I/O、密码算法、时间、日志或存储；不使用 serde 作为公开 wire contract。

### Application `SpaceAdmissionProtocol`

- **Path:** `crates/uc-application/src/space/admission/protocol/`
- **职责:** 唯一负责 `start_join`、`cancel_join`、`handle_authenticated_message`、`recover_pending`、状态投影和 Space transition 收尾。
- **输入:** 产品命令、已认证 typed message、唯一 membership ledger snapshot 和 transport-agnostic Infra capabilities。
- **输出:** 稳定产品结果、已发送/待恢复的 typed exchange、维护报告和 DeviceTrust revision。
- **关系:** 内部调用 Core aggregate；通过 Infra ports 使用 OPAQUE、OpenMLS、连接和邀请服务；不暴露中间步骤。

### Application `MembershipLedger`

- **Path:** `crates/uc-application/src/space/membership/ledger/`
- **职责:** 一次加载并验证 profile/Space 事实；按 expected revision/history digest 条件提交 admission、history、receipt、relationship、effect 和 projection。
- **输入:** 一个完整 transition mutation，而不是按字段 repository 方法列表。
- **输出:** committed snapshot、Conflict、Locked、RecoveryRequired 或 Unavailable。
- **关系:** 只负责事实验证和原子保存，不决定发送哪条消息或何时返回成功。

### Application authenticated message endpoint

- **Path:** `crates/uc-application/src/space/admission/protocol/endpoint.rs`
- **职责:** 核对 channel peer binding 与消息业务身份，处理一条完整 envelope，原子保存 stage/evidence/exact reply，再返回 typed reply。
- **输入:** `AuthenticatedSpaceAdmissionMessage { channel_peer_id, envelope, newly_established_continuation? }`。
- **输出:** `SpaceAdmissionMessageReply` 或 admission-facing typed error；不发送网络字节。
- **关系:** Infra handler 在认证和 wire decode 后每条业务消息调用一次；endpoint 不认识 Iroh、ALPN、connection、stream、frame、timeout 或 transport-specific identity type。

### Infra OPAQUE/OpenMLS adapters

- **Path:** `crates/uc-infra/src/security/space_admission_auth.rs`
- **Path:** `crates/uc-infra/src/space/admission/security/transition.rs`
- **职责:** Infra channel adapter 使用固定 `opaque-ke` + Argon2 ciphersuite 完成 OPAQUE/continuation authentication；现有 OpenMLS adapter 完成 KeyPackage/Add/Commit/Welcome/stage/apply/export commitment。
- **输入:** application 给出的明确身份 binding、加密 verifier/client material 或 current Space security snapshot。
- **输出:** authenticated channel secrets 或 staged MLS result；不决定业务 stage。
- **关系:** 不持有 application aggregate，不产生产品结果，不从当前成员表推断资格。

### Infra Iroh admission transport

- **Path:** `crates/uc-infra/src/network/iroh/space_admission.rs`
- **Path:** `crates/uc-infra/src/network/iroh/space_admission_wire.rs`
- **职责:** 注册新 ALPN、连接/接受、打开 bi stream、验证长度、完成 OPAQUE/continuation auth、把 Iroh remote identity 转成 opaque `AdmissionChannelPeerId`、转换 wire mirror 与 Core type、执行 deadline 和 close。
- **输入:** Application 的 transport-agnostic `AdmissionRoute`/auth material，或 accepted Iroh connection。
- **输出:** 出站 `AuthenticatedAdmissionExchangePort`，或向 Application endpoint 提交一条 `AuthenticatedSpaceAdmissionMessage` 并发送其 typed reply。
- **关系:** Iroh 类型只存在于 Infra；不保存业务 stage、不生成业务 reply、不产生 event channel。

### Infra encrypted membership ledger store

- **Path:** `crates/uc-infra/src/db/repositories/membership_ledger_store.rs`
- **Path:** 新 migration `crates/uc-infra/migrations/<timestamp>_single_space_admission/`
- **职责:** 在一个 SQLite immediate transaction 中加载、解密、核对 revision 并保存 profile payload、current/target Space payload、admission rows、effects 和 terminal replay facts。
- **输入:** expected revision/history digest 与完整 replacement/mutation。
- **输出:** committed encrypted state 或 typed store error。
- **关系:** profile payload 使用 ProfileAdmissionMasterKey；Space payload 使用对应 Space MasterKey；无明文镜像。

### Engine assembly

- **Path:** `crates/uc-engine/src/assembly/sync_engine.rs`
- **Path:** `crates/uc-engine/src/assembly/wire/infra.rs`
- **职责:** 构造 passive Infra admission transport/handler builder、构造 dormant SpaceApplication、取得 typed message endpoint、注入 Infra handler、spawn router、再启动 application runtime；shutdown 反序执行。
- **输入:** 一次完整依赖包。
- **输出:** 完整可运行 Engine；任一 endpoint 未绑定或重复绑定均构造失败。
- **关系:** 不处理消息、不映射 stage、不持有 outbox、不启动第二准入 runtime。

## Data Model

### Identity types

| 类型 | 规则 |
| --- | --- |
| `SpaceAdmissionId` | 每次公开 JoinSpace 生成随机 256-bit；网络、持久和恢复唯一身份 |
| `JoinId` | 每次公开 JoinSpace 生成独立随机 128-bit；只用于产品查询/取消 |
| `AdmissionMessageId` | 每条 durable message 随机 256-bit，生成后持久，不从敏感 payload 直接充当数据库 key |
| `InvitationId` | 随邀请生成的随机 256-bit 内部身份；完整邀请直接携带，短 code 只查询到同一份完整邀请 |
| `AdmissionProtocolVersion` | 只接受 `1`；未知值关闭连接并记录固定分类 |
| `AdmissionChannelPeerId` | transport-agnostic opaque 认证通道身份；Infra 从 Iroh remote identity 规范派生，Application 不可反解 |

这些类型的 `Debug` 只输出类型名和 `[REDACTED]`。它们不实现 `Display`，不进入日志字段、指标 label 或遥测属性。

### Core aggregate

`SpaceAdmissionAggregate` 使用公共 header 加一个封闭 state enum：

```text
SpaceAdmissionAggregate
  format_version = 1
  record_version
  admission_id
  join_id?
  local_join_ordinal?
  peer_binding
  continuation_credential
  last_received
  pending_exchange
  state: Joiner | Sponsor | CompletionHelper | Terminal
```

禁止继续使用一个包含所有阶段可选字段的结构。每个 enum variant 只携带该阶段合法数据：

```text
JoinerState
  Initiated        { start_material, encrypted_password_equivalent, join_request }
  Candidate        { candidate, base_history, staged_target_input }
  Prepared         { verified_history, staged_target, prepared_proof, prepared_request }
  Committed        { exact_commit, staged_target }
  Applied          { exact_commit, activation_receipt, applied_request }
  Activating       { completion, space_transition }

SponsorState
  Accepted         { invitation_claim, join_request, base_snapshot, peer_binding }
  Candidate        { fixed_candidate, staged_security, candidate_reply }
  Committed        { fixed_candidate, committed_history, sealed_security, commit_reply }
  Applied          { activation_receipt, activated_security, complete_reply }

CompletionHelperState
  Challenged       { helper_binding, challenge_counter, nonce, last_message_ids }
  Applied          { verified_commit, receipt, helper_security, complete_reply }

TerminalState
  Active | Completed | Rejected(reason) | SupersededByNewJoin | RecoveryRequired(category)
```

Stage transition 只能由 Core 方法创建。Application 和 Infra 禁止直接赋值 stage 或拼装 terminal。

### Replay and pending exchange

```text
AdmissionMessageEvidence
  sender_role
  sender_sequence
  message_id
  predecessor_message_id
  canonical_digest

PendingAdmissionExchange
  route
  request_envelope
  exact_expected_reply_kind
  retry_state { attempt_count, next_attempt_at_ms }

SavedAdmissionReply
  inbound_evidence
  exact_reply_envelope
```

- `retry_state` 只影响调度，不影响业务 stage；时间和次数均 checked。
- 相同 id/digest 重放 `SavedAdmissionReply`，不重新调用 OpenMLS 或生成随机数。
- 相同 id 不同 digest、sender sequence 回退但非已知重放、predecessor 不匹配均进入稳定协议拒绝或 RecoveryRequired。
- Terminal compaction 保留 admission/join/peer binding、continuation credential、最后双方 sequence、必要 reply 和防重放 digest；删除大块 staged/security/password payload。

### Authentication material

| 数据 | 生命周期 | 持久规则 |
| --- | --- | --- |
| OPAQUE server setup | profile 生命周期 | SecureStorage 固定别名；FactoryReset 删除 |
| Space OPAQUE registration record | Space 生命周期 | Space MasterKey AEAD；Space rebuild 重建 |
| joiner password-equivalent | Joiner Initiated 到 continuation 保存 | ProfileAdmissionMasterKey AEAD；随后 zeroize 并删除 |
| OPAQUE ephemeral state | 一个 connection | 只在内存 `Zeroizing`，断线丢弃 |
| continuation key | admission 生命周期及 terminal replay retention | 每个 admission data key 加密；绑定双方 channel peer id 和 admission id |
| invitation claim digest | profile 生命周期防重放 | ProfileAdmissionMasterKey AEAD；不能改绑 |

OPAQUE transcript/AAD 固定包含：协议名与版本、admission id、invitation id、joiner/sponsor channel peer id、角色和 ciphersuite id。Infra 从 Iroh remote identity 派生 peer id 并完成绑定；Application 只保存 opaque peer id。OPAQUE shared secret 通过 HKDF 域分离派生 `continuation_key` 和 channel confirmation key，随后丢弃原 shared secret。

后续连接的 `ContinuationHello` 包含随机 256-bit nonce 和对 canonical envelope digest 的 HMAC。MAC 输入固定包含协议版本、admission id、双方 channel peer id、方向、nonce、sender sequence、message id、predecessor 和 envelope digest。Infra 常量时间验证；Application 用 aggregate evidence 裁决业务重放。

### Typed message protocol

Core 只定义一个业务 envelope：

```text
SpaceAdmissionEnvelopeV1
  protocol_version = 1
  admission_id
  sender_role
  sender_sequence
  message_id
  predecessor_message_id
  body
```

`body` 只允许：

| Body | 方向 | 业务含义 |
| --- | --- | --- |
| `JoinRequest` | Joiner -> Sponsor | 新 admission、邀请、身份签名、MembershipCredential、KeyPackage、恢复公钥和来源策略 |
| `Candidate` | Sponsor -> Joiner | 基础历史、固定 AddDevice、MLS Commit/Welcome、公共安全承诺和 continuation route |
| `Prepared` | Joiner -> Sponsor | 对完整历史、固定 candidate 和安全承诺的已签名准备证明 |
| `Commit` | Sponsor -> Joiner | Sponsor 已正式提交的原样 candidate、目标历史和封存恢复材料 |
| `Applied` | Joiner -> Sponsor | Joiner 已持久保存 event/target state 的 AdmissionActivationReceipt |
| `Complete` | Sponsor/Helper -> Joiner | 已保存 receipt 并激活安全状态的 AdmissionCompletion |
| `CompleteAck` | Joiner -> Sponsor/Helper | Joiner 已完成 J3 并保存 Active |
| `Settled` | Sponsor/Helper -> Joiner | CompleteAck 已保存，双方可压缩终态 |
| `CancelRequested` | Joiner -> Sponsor | 正式 Commit 前请求拒绝；不创建下一 admission |
| `Rejected` | Sponsor/Helper -> Joiner | 稳定拒绝及固定 reason；不含内部错误文本 |

每个角色维护独立 `sender_sequence`，从 0 开始 checked increment。`predecessor_message_id` 必须是对端最后一条持久消息；第一条 JoinRequest 为 None。CancelRequested 使用 joiner 的下一 sequence，Sponsor 以 Rejected 或当前已保存的后继回复，不能跳回旧 stage。

Wire 只增加认证控制帧，不增加第二套业务消息：

```text
AdmissionWireFrameV1
  ChannelHello { InitialOpaque | Continuation, ids, auth bytes }
  ChannelChallenge { auth bytes }
  ChannelFinish { auth bytes }
  BusinessMessage { envelope, auth_tag }
  BusinessReply { envelope, auth_tag }
  RetryLater { category, retry_after_ms }
  AuthenticationRejected
```

`RetryLater` 只用于尚未改变业务状态的 Locked/Busy/Unavailable，不进入 aggregate。已有稳定业务拒绝必须保存并使用 `Rejected` envelope。

### Limits and deadlines

| 项目 | 固定上限 |
| --- | --- |
| Auth/control frame | 64 KiB |
| 普通 durable message | 256 KiB |
| Candidate/Commit/Complete helper bundle | 4 MiB |
| 单 admission replay evidence | 64 条；超出前必须 terminal compact 或 RecoveryRequired |
| 同时入站连接 | profile 8 条；同 admission 1 条 |
| accept/open/read/write 单步 deadline | 30 秒 |
| 单次 exchange 总 deadline | 120 秒 |
| 自动 retry | 1s、2s、5s、10s、30s、60s，上限 60s，带确定性 admission-id jitter |

长度在分配前验证。超过上限、零长、未知 body、未知 enum、整数溢出或 deadline 失败不保存部分 payload。上限不是配置项。

### Persistence layout

新 migration 创建：

```text
space_membership_ledger_v1
  singleton_id = 1
  format_version = 1
  profile_payload_ciphertext
  current_space_payload_ciphertext?
  target_space_payload_ciphertext?

space_admission_record_v1
  lookup_key_hmac
  wrapped_record_data_key
  encrypted_payload

space_admission_terminal_v1
  lookup_key_hmac
  encrypted_replay_payload
```

- lookup key 使用 ProfileAdmissionMasterKey 域分离 HMAC，不保存 admission id 明文。
- profile payload 保存 profile generation、ledger revision、next ordinal、projection floor、active admission slot、invitation claims 和 terminal 索引。
- current/target Space payload 保存历史、receipt、relationship、effect、门禁和 generation manifest，由对应 Space MasterKey 加密。
- record row 保存 role/stage payload、continuation、pending exchange 和恢复材料，由随机 record data key 加密；data key 由 ProfileAdmissionMasterKey 包裹。
- 一次 SQLite immediate transaction 解密并核对 expected revision/history digest，写入全部相关 ciphertext 后提交。
- Application 只看到一个 `VerifiedMembershipLedgerSnapshot` 和一个 `commit_transition`；不得取得表级 repository 列表。
- migration 删除 `admission_repository_state` 和其他旧准入表/触发器；不复制旧记录。
- down migration 以明确“new admission protocol state cannot be downgraded”失败，不生成旧表或旧 payload。

### Public projection

`DeviceTrustSnapshot.current_join` 和 `pending_inbound_member` 保持现有产品含义：

- `JoinSpace` 完成本机持久创建后返回 `Pending`；不等待网络握手。
- 远端 passphrase、邀请、历史或协议稳定拒绝写入 `Rejected`，再由查询/事件公开。
- 只有 J3 完成、目标运行入口打开并保存 `Active` 后返回 Active。
- `pending_inbound_member` 只来自当前 Sponsor non-terminal aggregate，不进入普通 devices。
- 每次公开变化与业务事实同一事务推进 DeviceTrust revision，再发送不含敏感字段的状态变化事件。

## API / Interface

稳定 Engine 操作保持：

```text
JoinSpace(input) -> Active | Pending | Rejected
CancelJoinSpace(join_id) -> Active | Pending | Rejected
IssueInvitation / CancelInvitation / QueryInvitationAddresses
QueryDeviceTrust -> current_join + pending_inbound_member
```

Application 唯一负责人：

```text
SpaceAdmissionProtocol
  start_join(JoinSpaceInput) -> JoinSpaceResult
  cancel_join(JoinId) -> CurrentJoinStatus
  handle_authenticated_message(AuthenticatedSpaceAdmissionMessage) -> SpaceAdmissionMessageReply
  recover_pending(AdmissionRecoveryTrigger) -> AdmissionRecoveryReport
  complete_pending_space_transition() -> CurrentJoinStatus
```

这些方法属于同一 module 和同一 profile execution lock。Facade 只选择一个完整方法；不能先后调用 prepare、save、send、settle。

Infra Iroh handler 只依赖 Application 两个窄入口：

```text
HandleAuthenticatedSpaceAdmissionMessagePort
  handle(AuthenticatedSpaceAdmissionMessage) -> SpaceAdmissionMessageReply

SpaceAdmissionChannelCredentialPort
  resolve_initial(InvitationId, AdmissionId) -> SponsorOpaqueMaterial
  load_continuation(AdmissionId) -> Zeroizing<ContinuationCredential>
```

credential port 只提供认证材料，不决定 stage 或 reply。初始 OPAQUE 成功产生的新 continuation 随
`AuthenticatedSpaceAdmissionMessage` 交给同一次业务提交保存；Infra 不单独持久化。

Application 出站只依赖 transport-agnostic Infra port：

```text
SpaceAdmissionTransportPort
  establish_initial(AdmissionRoute, JoinerOpaqueMaterial, AdmissionChannelBinding)
    -> Box<dyn AuthenticatedAdmissionExchangePort>
  resume(AdmissionRoute, ContinuationCredential, AdmissionChannelBinding)
    -> Box<dyn AuthenticatedAdmissionExchangePort>

AuthenticatedAdmissionExchangePort
  channel_peer_id() -> AdmissionChannelPeerId
  newly_established_continuation() -> Option<ContinuationCredential>
  exchange(SpaceAdmissionEnvelopeV1) -> SpaceAdmissionMessageReply
```

`AuthenticatedAdmissionExchangePort` 只允许一次业务 exchange，然后消费自身并关闭。它不实现
Clone/Debug/Serialize。Application 只认识 route、binding、credential、typed envelope 和 typed reply；
所有 Iroh/ALPN/connection/stream/frame/deadline 均隐藏在 Infra。

Core transition interface 只表达完整结果：

```text
SpaceAdmissionAggregate::start_join(...)
SpaceAdmissionAggregate::accept_join_request(...)
SpaceAdmissionAggregate::prepare_candidate(...)
SpaceAdmissionAggregate::accept_candidate(...)
SpaceAdmissionAggregate::commit_prepared(...)
SpaceAdmissionAggregate::apply_commit(...)
SpaceAdmissionAggregate::complete_applied(...)
SpaceAdmissionAggregate::activate_complete(...)
SpaceAdmissionAggregate::settle_complete_ack(...)
SpaceAdmissionAggregate::cancel(...)
SpaceAdmissionAggregate::supersede(...)
SpaceAdmissionAggregate::replay_or_reject(evidence)
```

每个方法返回 `AdmissionTransition { replacement, exact_reply?, effects }`；Application 将 replacement、history/effect mutation 和 exact reply 一次交给 ledger。Core 不提供字段 setter。

错误分层：

| 层 | 稳定分类 | 规则 |
| --- | --- | --- |
| Core | Invalid, Duplicate, OutOfOrder, Conflict, UnsafeCancellation, RecoveryRequired | 无字符串化内部错误 |
| Application endpoint | Locked, Busy, AuthenticationRejected, ProtocolRejected, StateChanged, RecoveryRequired, Unavailable | 只映射为 wire action/close |
| Infra transport | Offline, Timeout, Closed, Oversized, Decode, Authentication | 不生成业务 Rejected |
| Engine product | InvalidInput, NotFound, Unauthorized, Conflict, Unavailable, DeadlineExceeded, Internal | 保持稳定 code/category/retryable |

`JoinSpace` 在本机 attempt 保存成功后只返回 Pending/Active/Rejected。之后发生的 PassphraseMismatch、InvitationUnavailable、HistoryConflict、PeerUpgradeRequired 等远端稳定结果通过 `current_join = Rejected` 和 DeviceTrust 事件公开，不从第二条同步调用路径返回。保存前的本机输入/锁定/密钥/存储错误仍同步返回 Engine error。

## Workflow

### Build and startup

1. Engine 构造 Iroh endpoint、rendezvous invitation adapter、实现 `SpaceAdmissionTransportPort` 的 Infra adapter 和尚未注册的 handler builder；Router 尚未 spawn。
2. Engine 构造 dormant `SpaceApplication`，其中 `SpaceAdmissionProtocol` 只有 transport-agnostic transport、OpenMLS、ledger 和 invitation capabilities，没有 Iroh 类型或后台任务。
3. Engine 取得 typed message endpoint 与 channel credential port，把它们一次注入新 ALPN Infra handler。
4. 构造过程检查 endpoint 已绑定一次、所有 required port 非空；失败则 Engine 创建失败。
5. Engine spawn Iroh Router，确认 admission ALPN 已注册。
6. Engine 启动 `SpaceApplicationRuntime`；runtime 首轮先恢复 admission，再打开普通成员/内容活动。
7. shutdown 先停止 application 新 admission work 并等待当前 SQLite commit，再 shutdown Router；不保留 detached task。

### Space credential preparation

1. InitializeSpace 或强制单设备 rebuild 成功取得 passphrase 后，Infra 使用固定 `opaque-ke` ciphersuite 在本机完成 OPAQUE registration，保存 server setup 和该 Space registration record。
2. UnlockSpace 若记录缺失且当前 Space 是新协议 format，使用本次正确 passphrase 原子补齐；存在密文但打不开时 RecoveryRequired，不重建同名凭据。
3. registration record 与 Space generation 绑定；ResetSpace/rebuild 创建新记录，FactoryReset 删除 profile server setup。
4. 任何原始/规范化 passphrase 和临时 OPAQUE state 使用 Zeroizing，禁止 Debug/Clone/日志。

### Invitation

1. IssueInvitation 检查当前 Space active、OPAQUE record 可读、没有占用 admission slot。
2. 生成随机 InvitationId、短 code、expiry 和 sponsor Iroh route，再编码一份不含口令或私钥的版本化完整邀请。holder 以 InvitationId 和短 code 索引同一 invitation。
3. rendezvous/mDNS 将完整邀请作为不透明内容发布；短 code 只是查询别名，完整邀请可从二维码、链接或直接文本在本地解码。两条路径必须得到同一 InvitationId、route 和 expiry。
4. code 和完整邀请都不进入 Debug 或日志。未知/过期/损坏邀请不返回 Space、成员、移除或 admission slot 细节。
5. 本机受理 JoinRequest 时才原子保存 claim；渠道 consume 在提交后后台清理。

### J0 and initial authenticated JoinRequest

1. 用户调用 JoinSpace。Application 完成设备名、来源历史和 preserve choice 预检。
2. 按 ADR-022 分类当前本机 admission；Initiated/Candidate 可原子 supersede，Prepared 及以后返回 PreviousJoinCannotBeSuperseded。
3. 生成新 AdmissionId、JoinId、ordinal、成员 credential、KeyPackage、record data key 和初始 JoinRequest。
4. 完整邀请在本地验证并保存。短 code 则先与不透明开始上下文一同加密保存为 Ready，不进行网络 I/O。
5. 返回 Pending 并唤醒恢复。短 code Ready 先原子提交 Started，持久记录在此时删除 code，然后只发起一次云端/局域网解析。
6. 解析成功后先保存 Resolved 完整邀请。超时、响应丢失、保存失败或 Started 重启都稳定 Rejected(InvitationUnavailable)，不再使用短 code。
7. 只有完整邀请已保存才可打开新 ALPN stream，发送 ChannelHello 并完成 OPAQUE。
8. OPAQUE 成功后双方派生 continuation key。Joiner 先保存 continuation 并删除 password-equivalent，再发送 JoinRequest。

### S0/S1 Candidate

1. Infra Iroh handler 接受 stream，读取 hello，通过 channel credential port 取得内存 invitation 对应的 OPAQUE record，并在 Infra 内完成认证。
2. Infra 将 remote identity 规范化为 opaque channel peer id，构造完整 typed `AuthenticatedSpaceAdmissionMessage`，调用 Application message endpoint 一次。Application 验证 JoinRequest 的 credential/member instance、transport key/facts 签名、KeyPackage 和当前 admission slot。
3. 在一个提交中保存 invitation claim、peer binding、continuation、Accepted record 和远端 invitation-consume cleanup。
4. 从精确基础历史和已激活成员集合调用 OpenMLS prepare Add + Commit + Welcome；生成固定 candidate event 和公共安全承诺。
5. 保存 Sponsor Candidate state 和 exact Candidate reply，再发送。此时不写正式 AddDevice。
6. 同一 JoinRequest 重试只重放 Candidate；相同 code 改绑另一 admission/identity 稳定 Rejected。

### J1 Prepared

1. Joiner 验证 Candidate predecessor、完整签名历史、candidate AddDevice、MembershipCredential、OpenMLS Welcome/Commit 和公共安全承诺。
2. Fresh/SameSpace/CrossSpace 按规格 023 保存对应来源和目标暂存世代；目标入口保持关闭。
3. 生成并签署 Prepared proof。
4. 原子保存 Joiner Prepared state、staged target 和 exact Prepared request，再发送。
5. Candidate 重试返回同一 Prepared；验证失败保存稳定 Rejected/RecoveryRequired，不生成第二 candidate。

### S2 Commit

1. Sponsor 验证 Prepared proof 和基础历史仍逐字节/摘要匹配。
2. 在一个 SQLite transaction 或同一加密 write-ahead recovery 中取得 commit guard，保存原样 AddDevice、目标历史、sealed security transition、pending effect 和 exact Commit reply。
3. 这是唯一正式成员提交点。提交后 cancel 只能变成 TooLateCommitted，任何失败不能删除 AddDevice。
4. 提交成功后发送 Commit；重复 Prepared 重放同一 Commit。

### J2 Applied

1. Joiner 验证 Commit 与 Prepared candidate 完全一致。
2. 从本地 staged target 重新导出同一安全承诺，不重新生成 Welcome/Commit/member instance。
3. 原子保存同一 AddDevice、目标暂存状态、负门禁、永久 AdmissionActivationReceipt 和 exact Applied request。
4. 发送 Applied；joiner 普通入口仍关闭。

### S3 Complete

1. Sponsor 验证 Applied receipt。
2. 原子把 receipt 加入版本化成员历史、激活 sealed security state、建立正式成员效果/传播、清 commit guard，并保存 exact Complete reply。
3. 此后 Sponsor 可把 joiner 视为暂时离线成员；Joiner 自身仍等待 J3。
4. 发送 Complete；重复 Applied 重放同一 Complete。

### J3 Active and settlement

1. Joiner 持久保存首份有效 Complete，执行规格 023 的 Fresh/SameSpace/CrossSpace 完整 transition。
2. 目标 manifest、keyslot、history、OpenMLS state、database generation 和 runtime 全部一致后，保存 Active 和 exact CompleteAck。
3. 发送 CompleteAck。Sponsor/Helper 保存 ack、结清 Complete 并返回 Settled。
4. Joiner 保存 Settled 后 terminal compact。Settled 丢失时重发同一 CompleteAck；Sponsor 重放同一 Settled。
5. Active 事件只在本机 Active 提交后发出。

### Cancellation and supersession

1. CancelJoinSpace 只针对 current join id。Joiner 保存 CancelRequested 并返回 Pending。
2. Sponsor 在 S2 前原子保存 Rejected(Cancelled) 并回复；S2 后回复当前 Commit/Complete，Joiner 保存 TooLateCommitted 并继续。
3. 新 JoinSpace 是新用户操作。旧 Initiated/Candidate 可在一个 ledger commit 中保存 SupersededByNewJoin 并创建全新 admission。
4. 旧 Prepared 及以后不可 supersede；新调用零新身份、零新密钥、零新 outbox。
5. 迟到旧消息只命中旧 admission replay/terminal；有效 Commit 命中已 superseded admission 时 RecoveryRequired。

### Recovery and completion helper

1. Startup/Resume/Periodic/StateChanged/PeerOnline 只调用 `SpaceAdmissionProtocol::recover_pending`。
2. Protocol 按 profile lock 和 admission id 排序，每个 admission 同时最多一个 exchange。
3. pending request 从保存 route 打开新 stream，使用 continuation auth，发送同一 envelope 并保存 reply。
4. response 丢失不推进 sender；receiver 已保存 exact reply，重试可重放。
5. Sponsor 在 Applied 后长期离线时，Joiner 按规格 023 选择一个可达且在父历史有效的当前成员，完成 challenge counter、成员签名、continuation response、历史/receipt/security 验证后取得等价 Complete。
6. helper 不能创建 AddDevice、改变 candidate、接受 cancel 或扩大 receiver 集合。
7. Corrupt/RecoveryRequired 阻止后续扩大权限的 membership maintenance；普通 Unavailable 只 Deferred。

### Clean cutover

1. 升级检测到旧 profile/Space format 时执行现有 rebuild 流程，建立新的单设备 root、Space generation 和 OPAQUE registration。
2. 迁移不读取旧 admission payload 内容，不映射旧 stage，不重发旧 outbox，不保留旧 invitation。
3. migration 删除旧 admission repository、pairing session 持久引用和旧表；用户重新邀请并配对所有设备。
4. 旧 ALPN 不注册。旧 binary 打开新 schema 时在写入前明确失败，不生成旧表或清空新状态。
5. 切换前已有普通本机内容按现有 rebuild/rewrap 规则处理；本规格不以删除本机内容代替 Space rebuild。

# 6. Implementation Plan

整个计划是一个不可发布的切换单元。每一步先写失败测试，再完成最小实现；任何阶段不得通过别名、默认失败 adapter 或旧实现让全仓临时“变绿”后合入。

## Step 1: 固定目标行为和删除基线

**File:** `crates/uc-core/src/membership/space_join_record.rs`

**File:** `crates/uc-application/src/space/admission/`

**File:** `crates/uc-infra/src/pairing/`

**File:** `crates/uc-engine/tests/space_membership_auto_pairing_e2e.rs`

**Change:**

- 从规格 023/025 提取 J0—J3、取消、supersession、重复、乱序、崩溃和 completion helper 的可观察结果。
- 建立目标测试名与非零数量清单；测试只使用新消息和新 aggregate 名称。
- 增加失败的依赖防火墙，先证明旧符号仍存在。
- 增加一个最小真实症状测试：Iroh 收到 admission stream 后必须调用 endpoint 并返回保存后的 reply；没有 endpoint 必须在 Router spawn 前构造失败，不能运行后丢消息。

**Risk:** 复制旧内部字段断言会把旧结构带入新设计。测试只断言用户结果、Core transition、持久事实、exact replay、权限门禁和 wire type。

**Exit Gate:** 目标测试因新类型/endpoint 尚不存在而确定性失败；旧 application 662 测试结果作为行为参考记录，但不要求旧实现继续编译到最终阶段。

## Step 2: 建立 Core typed protocol 和封闭 aggregate

**File:** `crates/uc-core/src/membership/admission/mod.rs`

**File:** `crates/uc-core/src/membership/admission/id.rs`

**File:** `crates/uc-core/src/membership/admission/message.rs`

**File:** `crates/uc-core/src/membership/admission/state.rs`

**File:** `crates/uc-core/src/membership/admission/transition.rs`

**File:** `crates/uc-core/src/membership/mod.rs`

**Change:**

- 新增红acted identity types、`SpaceAdmissionEnvelopeV1`、body、evidence、pending exchange 和 role/stage aggregate。
- 为 J0—J3、cancel、supersede、replay、out-of-order、Rejected、Settled、helper 和 terminal compact 建立纯领域 transition。
- 所有计数使用 checked arithmetic；所有集合 canonical sort；所有摘要使用固定域分离和长度编码。
- Core type 不 derive serde 作为 wire；测试可以使用专用 fixture builder。

**Risk:** stage variant 仍可能包含跨阶段可选字段。每个 variant 的构造器必须要求完整合法载荷，禁止 `Option` 表示“以后才需要”的业务字段。

**Exit Gate:** Core 单元/属性测试覆盖每条合法边和每条非法跳转；删除某个 stage payload 会让对应 transition 无法编译，而不是运行时返回“缺字段”。

## Step 3: 验证并接入 OPAQUE 与 OpenMLS capabilities

**File:** `crates/uc-infra/Cargo.toml`

**File:** `crates/uc-infra/src/security/space_admission_auth.rs`

**File:** `crates/uc-infra/src/space/admission/security/transition.rs`

**File:** `tests/openmls-validation/`

**Change:**

- 固定 `opaque-ke = "=4.0.1"`（features: `argon2`, `ristretto255`）；提交前运行 advisory/license/audit 检查，若该精确版本不能通过则本步骤阻塞，不得换自制协议。
- 使用 RFC 9807 官方向量覆盖 registration、KE1/KE2/KE3、wrong password、wrong identifiers、corrupt record 和 zeroization；测试位于 Infra，不把 OPAQUE frame 暴露给 Application。
- 固定 ciphersuite：Ristretto255、SHA-512/HKDF-SHA-512/HMAC-SHA-512。
- 固定 KSF：Argon2id v=0x13、m=65536 KiB、t=3、p=4，SHA-512 ciphersuite 下输出 64 bytes，对应 RFC 9106 的内存受限环境第二推荐参数。按 RFC 9807 §10.11，OPAQUE 的 OPRF key 已充当秘密 salt，不额外生成或持久化 salt；参数由协议版本固定，不是用户配置。Android/iOS/HarmonyOS 代表设备必须记录实际耗时和峰值内存，不能因超时私自降低。
- 将 protocol/admission/invitation/endpoint ids 和角色放入 OPAQUE identifiers/context。
- 复用 OpenMLS 0.8.1 当前生产 adapter 生成 Add + Commit + Welcome；新增 standard vectors/fixture 验证双方导出相同公共 commitment。
- 暂存结果必须可逐字节恢复；测试注入进程中断后不得生成新 Commit/Welcome。

**Risk:** `opaque-ke` 早期审计不等于当前版本全部代码已审计。固定版本、advisory 检查、RFC vectors、错误输入和移动性能门槛均为进入后续步骤的前置条件。

**Exit Gate:** OPAQUE 正确/错误口令与 identity-binding 测试非零通过；OpenMLS Add/Commit/Welcome、staged restore 和 public commitment 测试非零通过；日志/Debug 不含口令或共享秘密。

## Step 4: 建立唯一加密 membership ledger store

**File:** `crates/uc-application/src/space/membership/ledger/`

**File:** `crates/uc-infra/src/db/repositories/membership_ledger_store.rs`

**File:** `crates/uc-infra/src/security/admission_key_manager.rs`

**File:** `crates/uc-infra/migrations/<timestamp>_single_space_admission/up.sql`

**File:** `crates/uc-infra/migrations/<timestamp>_single_space_admission/down.sql`

**Change:**

- 定义一个 application verified snapshot 和一个完整 conditional transition commit。
- 实现 profile/Space/record/terminal 分域加密、HMAC lookup、wrapped record key 和 SQLite immediate transaction。
- 在同一事务核对 revision、history digest、active slot、record version、message evidence、invitation claim 和 target generation。
- 实现 write-ahead recovery，覆盖不能在一次存储调用中完成的 OpenMLS/manifest 变化；恢复前普通入口关闭。
- migration 创建新表并删除 `admission_repository_state`；不解码旧 attempt。
- 下行 migration 明确失败，不创建旧 schema。

**Risk:** 先写历史后写 record、先推进 projection 后写事实，或以多次 repository 调用拼事务，都会产生半提交。

**Exit Gate:** 真实加密 SQLite 故障注入在每个写点只得到完整旧快照或完整新快照；明文探针扫描无命中；并发 CAS 只有一个胜者。

## Step 5: 实现 Application `SpaceAdmissionProtocol` tracer bullet

**File:** `crates/uc-application/src/space/admission/protocol/mod.rs`

**File:** `crates/uc-application/src/space/admission/protocol/model.rs`

**File:** `crates/uc-application/src/space/admission/protocol/error.rs`

**File:** `crates/uc-application/src/space/admission/protocol/ports.rs`

**File:** `crates/uc-application/src/space/admission/protocol/coordinator.rs`

**File:** `crates/uc-application/src/space/admission/protocol/tests.rs`

**Change:**

- 先实现 Fresh Joiner J0 -> Sponsor Accepted/Candidate -> Joiner Prepared 的最小纵向切片。
- 用户 start 先保存后网络；Sponsor claim 先保存后 Candidate reply；duplicate JoinRequest exact replay。
- 实现 typed authenticated-message endpoint 和 continuation 原子保存；Application 测试只使用 in-memory typed transport，不构造 duplex/Iroh。
- `SpaceFacade` 的 Join/Cancel/Query 直接委托 protocol 完整方法，不调用 preparation/store/send 组合。
- `SpaceMembershipMaintenanceRuntime` 只持有 `RecoverSpaceAdmissionsPort`，其 adapter 是 protocol 自身；报告不解释 stage。

**Risk:** 为了 tracer bullet 暂时保留现有 `Prepare*Port` 会形成第二入口。旧 use case 可以在工作分支短暂存在但不得被新代码调用，Step 8 必须删除且中间不可发布。

**Exit Gate:** in-memory 双端从 JoinSpace 得到 Pending，Sponsor 保存 Candidate，Joiner 保存 Prepared；每个发送前都有持久证据，断线重试 exact replay。

## Step 6: 完成 Commit、Applied、Complete、Cancel、Supersession 和 Helper

**File:** 上述 protocol/Core/ledger 文件

**File:** `crates/uc-application/src/space/admission/space_transition/`

**File:** `crates/uc-application/src/space/membership/effect_executor.rs`

**Change:**

- 完成 S2/J2/S3/J3/Settled 全阶段和门禁。
- 将现有 Space transition 变为 protocol 内部完整能力；Active 前验证 runtime 实际打开。
- 实现 Cancel 与 S2 原子竞争、Prepared supersession boundary、迟到消息隔离和 terminal replay。
- 实现 completion helper challenge counter、成员签名、历史/receipt/security 复核和等价 Complete。
- 正式提交后的成员传播进入 membership history/effect，不留在 admission outbox。

**Risk:** Cancel 或新 JoinSpace 回滚正式 AddDevice；Helper 越权创建事实；S3 前传播候选；J3 前开放普通权限。

**Exit Gate:** 完整 in-memory 双端 Active；取消两种竞态、supersession、Sponsor 离线 helper、Fresh/SameSpace/CrossSpace 和每阶段崩溃测试通过。

## Step 7: 实现新 Iroh direct handler 和 connector

**File:** `crates/uc-infra/src/network/iroh/space_admission.rs`

**File:** `crates/uc-infra/src/network/iroh/space_admission_wire.rs`

**File:** `crates/uc-infra/src/network/iroh/node.rs`

**Change:**

- 注册唯一 `SPACE_ADMISSION_ALPN = b"/uniclipboard/space-admission/1"`。
- 实现 bounded frame、typed mirror、deadline、opaque channel peer id、explicit close code 和最多 8 个连接 semaphore。
- handler 在 Infra 内完成认证，对每条业务消息调用 typed Application endpoint 一次；无 endpoint/credential port 时 Router builder 不能 spawn。
- connector 每次 exchange 打开新 connection/bi stream；不依赖旧 session map 或 recv pump。
- handler 自身不再 spawn 子任务；每个 `accept` future 由 Iroh Router 管理。`ProtocolHandler::shutdown` 先停止 application 新 exchange，返回后 Router 才允许中止仍未结束的 accept future。
- 完成真实本机双 endpoint：Initial OPAQUE + JoinRequest/Candidate，以及 continuation Prepared/Commit exchange。

**Risk:** 在 Router task 外再次 detached spawn 会丢失 shutdown；decode 前分配 payload 会导致内存攻击；Infra 解释 body 会产生第二协议实现。

**Exit Gate:** 真实 Iroh loopback 从新 ALPN 进入 endpoint 并返回保存后的 typed reply；没有 subscriber/event；oversize、timeout、wrong remote id、wrong MAC 均确定性关闭。

## Step 8: 重排 Engine 组装和稳定产品契约

**File:** `crates/uc-engine/src/assembly/sync_engine.rs`

**File:** `crates/uc-engine/src/assembly/wire/infra.rs`

**File:** `crates/uc-engine/src/operations/space/join_space.rs`

**File:** `crates/uc-engine/src/contract/error_codes.rs`

**File:** `crates/uc-engine/tests/public_contract.rs`

**File:** `bindings/uc-engine-uniffi/tests/public_contract.rs`

**File:** `bindings/uc-ohos-napi/tests/public_contract.rs`

**Change:**

- 将 application 构造成 dormant，绑定 admission endpoint，spawn Router，再启动 runtime。
- shutdown 先停 application admission work，再停 Iroh。
- 保持 JoinSpace/Cancel/Invitation/DeviceTrust 方法和 Active/Pending/Rejected 结构。
- 审计 1233—1295 错误：仅保留仍可从新同步保存前路径返回的 code；异步远端结果通过 Rejected projection。删除不可达映射和测试，不用别名保号。
- bindings 只透传稳定操作/结果/error，不新增协议方法。

**Risk:** runtime 在 endpoint 绑定或 Router ready 前发送；Engine 开始解释 stage；删除错误码导致平台仍引用旧常量。

**Exit Gate:** Engine/binding public contract 测试通过；构造顺序测试证明 endpoint-before-router、router-before-runtime；dispatch 不引用协议类型。

## Step 9: 一次性删除旧实现并执行 clean cutover

**File:** 见 Mandatory Deletion Checklist

**File:** `scripts/architecture/check-engine-repository.mjs`

**File:** `docs/architecture/architecture-bible.md`

**Change:**

- 删除所有旧 protocol/session/event/preparation/outbox/store/completion recovery 代码和导出。
- 删除旧 ALPN、wire version、legacy probe、fallback、tests 和 migration reader。
- 扩展架构脚本，在生产和测试源码中拒绝旧符号；历史 docs 可按明确 allowlist 引用。
- 升级路径强制单设备 Space rebuild 和重新配对；不导入旧 admission/membership branch。
- 更新规格 017/023/025/027 状态和交叉引用，说明其业务不变条件由规格 028 实现、旧实现章节已被取代。

**Risk:** 只删除调用而保留 adapter/port 会再次形成死代码；只删除表而未完成 rebuild 会丢失本机活动状态。

**Exit Gate:** deletion searches 零生产命中；全工作区编译；旧 ALPN 连接失败且不尝试 fallback；新 profile/升级 profile 都只存在一套协议。

## Mandatory Deletion Checklist

- `crates/uc-core/src/pairing/session_message.rs`
- Core `PairingSessionMessage`、`DurableAdmissionFrame`、`DurableAdmissionMessageKind`
- Core 当前 `SpaceJoinRecord` 可选字段 bag、`AdmissionOutboxPurpose`、`AdmissionOutboxMessage` 和旧 persisted decoder
- `PairingEventPort`、`PairingSessionPort`、`PairingSessionEvent`、`PairingSessionId`
- `crates/uc-infra/src/pairing/session.rs`
- `crates/uc-infra/src/pairing/wire.rs`
- `crates/uc-infra/src/pairing/admission_outbox_delivery.rs`
- `/uniclipboard/pairing/2`、`/uniclipboard/pairing/1`、`PAIRING_ALPN`、`LEGACY_PAIRING_ALPN`
- 所有 legacy pairing reachability probe、version negotiation 和 fallback
- `crates/uc-infra/src/network/iroh/admission_completion_recovery_adapter.rs`
- `crates/uc-infra/src/db/repositories/space_join_record_store.rs`
- `admission_repository_state` 表和旧 repository triggers
- `PrepareJoinSpacePort`、`PreparedJoinSpace`
- `PrepareSpaceAdmissionMessagePort`、`PreparedSpaceAdmissionMessage`、`PreparedSpaceAdmissionCommit`
- `HandleSpaceAdmissionMessagePort` 的 `Vec<u8>` contract
- `crates/uc-application/src/space/admission/handle_space_admission_message/`
- 现有独立 `RecoverSpaceAdmissionsUseCase` 对 outbox result 的逐项解释
- `AdmissionOutboxDeliveryPort`、`AdmissionOutboxDeliveryResult`、`AdmissionOutboxDeliveryRoute`

### Core 旧准入清理清单

以下 Core 内容已标记为 deprecated，只允许未迁移的 Application、Infra 和 Engine 代码继续引用。它们不是兼容层，不能新增调用者、字段、阶段、消息或持久化用途；新代码只能使用 `SpaceAdmissionAggregate`、`AdmissionTransition` 和类型化消息。

迁移完成后一次删除：

- `crates/uc-core/src/membership/space_join_record.rs` 整个文件和 `membership/mod.rs` 的全部对应导出。
- `SpaceJoinRecord`、`SpaceJoinRecordId`、`SpaceJoinRoleState` 和旧 persisted decoder。
- `SponsorAdmissionState` / `SponsorAdmissionStage`、`JoinerAdmissionState` / `JoinerAdmissionStage`、`CompletionHelperAdmissionState` / `CompletionHelperAdmissionStage`。
- `AdmissionOutboxPurpose`、`AdmissionOutboxMessage`、`AdmissionInboxRecord`。
- `AdmissionTerminalResult`、旧 `AdmissionRejectionReason`、`SupersedeSpaceJoinError`、`CancelSpaceJoinRecordError`、`SpaceJoinTransitionError`。
- `AdmissionProfileMetadata`、`CompletedSpaceJoinRecord` 及其格式常量。
- `AdmissionIdentityBindingV1` 及其验证错误和格式常量；新协议改用 `AdmissionPeerBinding` 与 continuation credential。
- `AdmissionCompletionRecoveryHello`、challenge、response、bundle、peer、transport binding、validation error 和格式常量；新协议改用 Core CompletionHelper 状态与新 Infra handler。
- `SponsorAdmissionSecurityDelivery`；新协议只保存类型化 Commit/Complete 和帮助方安全材料。

删除前必须全部满足：

- Application 的 start/cancel/handle/recover/complete 只使用 `SpaceAdmissionAggregate` 与 `AdmissionTransition`。
- 新加密 membership ledger 已能保存完整 Aggregate、消息证据、原样回复和一次性影响。
- Infra 不再编解码旧 record/outbox/completion-recovery 结构，不再读写旧表或旧加密记录。
- Engine 和绑定不再导入任何上述符号，产品查询已改读新投影。
- 全仓搜索上述符号只剩本清单和历史文档；随后删除 deprecated 标记本身，不保留别名或空壳。
- `SpaceFacade::space_admission_endpoint()` 作为无人管理的 accessor；新 endpoint 只交给 Engine assembly 安装
- Engine `pairing_events`、`pairing_session` deps 和 sponsor inbound handle 注释/生命周期残留
- 所有旧兼容导出、默认失败 adapter、no-op test dependency 和 feature flag

# 7. Edge Cases

## Scenario: 空帧、零长字段或超过上限

**Expected behavior:** 在分配/反序列化业务 payload 前关闭连接；不创建 admission、不写 inbox、不输出 payload。

**Implementation:** Infra length-prefix validator 使用 frame-kind 上限；checked conversion；固定 close category `invalid_frame`。

## Scenario: 未知 ALPN、version、body、enum 或 ciphersuite

**Expected behavior:** 不协商旧版本、不回退；连接失败或固定 ProtocolRejected；本机已有 attempt 保持 Pending。

**Implementation:** 只注册一个 ALPN；Core/wire exhaustive conversion；unknown discriminant 不进入 Application transition。

## Scenario: 邀请不存在或过期

**Expected behavior:** 不泄露 Space/成员/slot；不创建 Sponsor record；Joiner 保存 Rejected(InvitationUnavailable)。

**Implementation:** 初始 hello 只查内存 InvitationId/code binding；统一 auth rejection body；rendezvous 结果仅用于发现。

## Scenario: 错误口令或 OPAQUE identity/context 不匹配

**Expected behavior:** OPAQUE key confirmation 失败；不发送 JoinRequest，不创建 Sponsor claim；Joiner 稳定 Rejected(AuthenticationRejected)。

**Implementation:** `opaque-ke` error 固定映射；不记录错误正文；Infra 按 invitation/channel peer id 限速在线猜测。

## Scenario: OPAQUE connection 在 KE1/KE2/KE3 任一点断开

**Expected behavior:** ephemeral state 丢弃；业务 stage 不变；Joiner 以同一 admission 重新认证。

**Implementation:** OPAQUE state 只在连接栈；password-equivalent 在 continuation 持久前保持加密；不复用 half handshake。

## Scenario: continuation 已保存但初始回复丢失

**Expected behavior:** Joiner 使用同一 continuation 和 JoinRequest 重连；Sponsor 重放同一 Candidate。

**Implementation:** 双方在发送前保存 continuation；JoinRequest id/digest 命中 SavedAdmissionReply。

## Scenario: 相同 message id 与相同 digest

**Expected behavior:** 逐字节返回已保存 reply；不重新执行 OpenMLS、签名、随机数或 effect。

**Implementation:** evidence lookup 发生在 transition/crypto 前。

## Scenario: 相同 message id 与不同 digest

**Expected behavior:** 稳定 ProtocolRejected/RecoveryRequired；不覆盖旧证据，不返回新 reply。

**Implementation:** canonical digest 常量时间比较；保存固定攻击/损坏分类，不保存 payload 到日志。

## Scenario: sender sequence 跳跃、回退或 predecessor 不匹配

**Expected behavior:** 已知旧 evidence 可 replay；否则 OutOfOrder，不跳 stage。

**Implementation:** Core `replay_or_reject` 在任何 side effect 前执行。

## Scenario: 两个 JoinRequest 并发消费同一邀请

**Expected behavior:** SQLite claim/CAS 只有一个成功；失败者稳定 Rejected(InvitationUnavailable/IdentityConflict)。

**Implementation:** claim + admission binding + active slot 同一 transaction；内存 holder 不是最终裁决。

## Scenario: 本地 stage 保存成功，reply 写网络失败

**Expected behavior:** 保持新 stage/Pending；重试同一 input 得到 exact reply。

**Implementation:** reply ciphertext 是 transaction 的一部分；send error 只更新 scheduler，不改业务事实。

## Scenario: reply 到达但 sender 保存前崩溃

**Expected behavior:** sender 重发原 request；receiver 重放 reply；sender保存一次。

**Implementation:** request 保持 pending，直到 reply 通过 ledger commit 结清。

## Scenario: Candidate 生成中崩溃

**Expected behavior:** 恢复已保存 Accepted 和固定 preparation intent；不得 claim 第二次或创建另一 candidate。

**Implementation:** OpenMLS staged output 通过 write-ahead recovery 逐字节完成；完成前 JoinRequest 重试 RetryLater。

## Scenario: 基础历史在 Candidate 后、Prepared 前推进

**Expected behavior:** Sponsor 在 S2 拒绝 BaseHistoryChanged；不提交旧 candidate。

**Implementation:** S2 transaction 再次核对 exact base history digest；Rejected reply 先保存后发。

## Scenario: CancelRequested 与 S2 并发

**Expected behavior:** 同一 revision CAS 决定唯一结果。Cancel 先胜出则无 AddDevice；S2 先胜出则 TooLateCommitted 并继续。

**Implementation:** 两个 transition 在同一 ledger transaction 核对 Sponsor state。

## Scenario: 新 JoinSpace 命中 Initiated/Candidate

**Expected behavior:** 原子 SupersededByNewJoin + 全新 admission；旧清理与新 current join 隔离。

**Implementation:** Core supersede transition + 一个 profile transaction；新旧全部身份/材料不同。

## Scenario: 新 JoinSpace 命中 Prepared 或以后

**Expected behavior:** 返回 PreviousJoinCannotBeSuperseded；零新 id/key/outbox/network。

**Implementation:** Application 预检与 ledger transaction 双重检查。

## Scenario: Sponsor 在 Commit 后永久离线

**Expected behavior:** Joiner 保持 Pending；Applied 后可向一个合格当前成员请求 helper Complete。

**Implementation:** 使用规格 023 challenge counter、成员签名、sealed delivery、history/receipt/security 完整验证；helper 不创建事实。

## Scenario: Complete 长期丢失

**Expected behavior:** Sponsor/Helper 重放 exact Complete；Joiner不超时变 Active。

**Implementation:** Complete 保留 pending reply，直到 CompleteAck/Settled。

## Scenario: CompleteAck 或 Settled 丢失

**Expected behavior:** 重发同一 CompleteAck，重放同一 Settled；不重复 J3 或 terminal compact。

**Implementation:** Active/terminal replay facts 先保存；ack settlement 幂等。

## Scenario: Fresh、SameSpace、CrossSpace J3 中崩溃

**Expected behavior:** 普通入口关闭；从同一 Space transition phase 向前恢复；只出现一个 active manifest。

**Implementation:** 沿用规格 023 manifest/write-ahead 不变条件；不从 setup 或 session 猜 phase。

## Scenario: 认证通道身份在 admission 中途变化

**Expected behavior:** continuation auth 失败，保持 Pending/RecoveryRequired；不得静默改绑新 endpoint。

**Implementation:** Infra 从当前 transport identity 规范派生 channel peer id；continuation MAC 和 aggregate peer binding 包含双方 peer id。新的用户操作生成新 admission。

## Scenario: Profile/Space key 缺失或密文损坏

**Expected behavior:** RecoveryRequired；不重建同名 key、不清状态、不发送 reply、不开放权限。

**Implementation:** key-first lifecycle marker 和 AEAD failure classification；普通维护停止扩权步骤。

## Scenario: revision、ordinal、record version、sequence 或 helper counter 溢出

**Expected behavior:** RecoveryRequired，零写入。

**Implementation:** 全部 checked arithmetic；transaction 内再次检查。

## Scenario: P2P 离线、relay 不可用或连接超时

**Expected behavior:** admission 保持 Pending，按固定退避；不自动切换 LAN 或创建新 admission。

**Implementation:** transport error 只更新 retry scheduler；PeerOnline 可提前唤醒。

## Scenario: 入站连接超过并发上限

**Expected behavior:** 尚未改状态时 RetryLater/close；已提交连接继续到保存边界；不饿死 membership commit。

**Implementation:** profile semaphore 8、admission mutex 1；SQLite commit 不可取消。

## Scenario: invitation channel consume 失败、404、409 或进程重启

**Expected behavior:** 本机 claim 不回滚；远端 cleanup 重试；第二 admission 仍被本机 claim 拒绝。

**Implementation:** consume 是独立 cleanup effect，不能修改 Sponsor stage。

## Scenario: 旧二进制或旧协议设备

**Expected behavior:** 旧 ALPN 无 handler；不协商、不 fallback、不读新数据。用户升级并重新配对。

**Implementation:** 新 ALPN only；down migration fail；rendezvous metadata 可显示 upgrade-required，但不承载旧协议。

## Scenario: 升级时存在旧 Pending/邀请/attempt

**Expected behavior:** 单设备 Space rebuild 后全部旧 admission/invitation 不存在；不投影为 Rejected/Active，不自动发送。

**Implementation:** clean-cut migration 删除旧仓储；rebuild 创建新 profile/Space protocol state；架构检查禁止旧 reader。

## Scenario: ResetSpace/FactoryReset 与 admission 并发

**Expected behavior:** ResetSpace 仅在 slot、WAL、exchange、transition、cleanup 静止时执行；FactoryReset 先停 runtime 并删 key，再清密文。

**Implementation:** profile lock + lifecycle marker；不能以 reset 旁路取消。

# 8. Testing Strategy

## Unit Test

### Core state machine

- 每个 role/stage 合法 successor 正好一条测试；每个跳级、错角色、错 predecessor、错 sequence、错 digest 均拒绝。
- Cancel 与 S2、RemoveMember 与 S3、supersede 与 Prepared、CompleteAck 与 terminal compact 使用确定性竞争夹具。
- Aggregate variant 构造不允许缺必要字段；无字段 setter。
- Id/message/domain digest 使用 golden vectors 和跨平台固定字节。

### OPAQUE and authentication

- RFC 9807 registration/login vectors。
- 正确口令、错误口令、corrupt server record、wrong endpoint id、wrong invitation/admission id、wrong role/context。
- Argon2 参数 round-trip、上限和移动性能基准。
- continuation HMAC 的 nonce、方向、sequence、message/predecessor/digest 任一变化均失败。
- secret types 无 Debug/Clone/Serialize；drop 后 zeroize 使用库测试/内存探针。

### Wire codec

- Core <-> infra mirror 双向 round-trip；每个 body 独立 golden vector。
- 0、边界、超过边界、截断、未知 discriminant、整数溢出。
- fuzz target 对任意 bytes 不 panic、不超界分配、不输出 payload。

### Redaction

- Debug/log capture 不含 code、passphrase、device id、transport/channel peer identity、admission id、join id、message id、地址、credential、MLS、history、key 或 payload。

## Integration Test

### Application in-memory dual peer

- Fresh J0—J3—Settled 完整 Active。
- SameSpace 与 CrossSpace 完整 transition。
- 每个 send/receive/crypto/ledger/manifest 边界前后崩溃并重建 protocol。
- response loss、duplicate、out-of-order、same-id-different-digest。
- Cancel before/after S2、supersede before/after Prepared。
- Sponsor offline helper completion。
- active slot、two concurrent JoinRequest、two concurrent public JoinSpace。

### Real encrypted SQLite

- 每个 transaction write point 注错并 reopen。
- profile/Space/record key domain separation 和 wrong-key failure。
- HMAC lookup 不含明文 id；plaintext probe 无命中。
- terminal compact 后 late duplicate exact replay。
- cutover migration 不解码旧 admission，rebuild 后只有新单设备 root。

### Real Iroh loopback

- 两个真实 endpoint、new ALPN、initial OPAQUE、continuation reconnect、relay-disabled direct path。
- handler 在 Infra 完成 auth/wire 后调用 typed message endpoint；没有 event/subscriber，Application 测试无 Iroh 类型。
- endpoint 未绑定时 Router spawn 构造失败。
- shutdown hook 等待当前不可取消的 SQLite commit；其余 active handler 由 Router 受控结束，无 detached task。
- wrong ALPN、old ALPN、oversize、timeout、remote id mismatch、MAC failure。

### Engine dual-instance E2E

- 通过稳定 Operation 创建 Space、issue invitation、JoinSpace，轮询 DeviceTrust 到双方 Active。
- 双向 clipboard/file exact transfer 在 Active 前拒绝、Active 后通过。
- A/B restart 后 membership/history/receipt/continuation terminal state 一致。
- A 离线、C 经 B 加入、B 离线、A/C 经 history/receipt 同步后互通。
- P2P 不可用保持 Pending，不触发 LAN compatibility。

推荐命令（实施后测试名必须按实际模块存在，先 `--list` 验证非零）：

```bash
cargo test -p uc-core membership::admission --lib --locked -- --list
cargo test -p uc-core membership::admission --lib --locked

cargo test -p uc-application space::admission::protocol --lib --locked -- --list
cargo test -p uc-application space::admission::protocol --lib --locked -- --test-threads=1

cargo test -p uc-infra space_admission --lib --locked -- --list
cargo test -p uc-infra space_admission --lib --locked

cargo test -p uc-engine --test space_membership_auto_pairing_e2e \
  --features dev-tools --locked -- --nocapture
```

## Regression Test

- Engine/UniFFI/N-API public operation、result、error category/retryable 和 DeviceTrust contract。
- Create/Unlock/Lock/Recover/Reset/FactoryReset lifecycle。
- V2 membership history、activation receipt、remove/decision/divergence/current scope。
- invitation issue/cancel/query UI-facing behavior。
- clipboard/file/search 普通权限门禁。
- architecture forbidden symbols 和唯一 `uc-engine` 外部入口。
- release artifact 不发布内部 crate 或 binding crate。

## Physical Device Acceptance

| Matrix | Required behavior | Result rule |
| --- | --- | --- |
| Android sponsor -> iOS joiner | Initial OPAQUE、J0—J3、Active、双向内容、双方重启 | 必须实际执行；未执行写“跳过” |
| iOS sponsor -> Android joiner | 同上，验证角色互换 | 必须实际执行；未执行写“跳过” |
| Android sponsor -> HarmonyOS joiner | 同上，验证 N-API/宿主 | 未执行只能写“跳过” |
| Three devices A/B/C | B sponsor C while A offline，B offline 后 A/C 收敛并互传 | 必须至少一组实体设备执行 |
| Wrong passphrase | 无 Sponsor record、Joiner Rejected、日志无 secret | 必须执行 |
| Kill at Candidate/Prepared/Commit/Applied/Complete | 重启继续同一 admission，最终 Active 或稳定 Rejected | 每个阶段至少一次 |
| Old build attempts old ALPN | 无 fallback、明确升级/不可连接、当前状态不变 | 必须执行 |

## Repository Checks

```bash
cargo metadata --locked --format-version 1
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-engine-repository.mjs
git diff --check
node scripts/security/scan-plaintext-probe.sh <representative-probe>
```

设备、真实 Iroh、真实 SQLite、绑定和全工作区是不同证据边界，不能互相替代。

# 9. Acceptance Criteria

截至 2026-08-30，Core/Application/Infra focused tests、真实 SQLite、Iroh loopback、
Engine 双实例、重启传输、绑定 contract、明文探针和完整 workspace tests 已通过。
当前环境没有可用的 Android/iOS/HarmonyOS 实体设备，也没有待核验的 Release bundle；
三平台设备矩阵使用 `scripts/release/device-matrix.skipped.json` 明确记录为“跳过”，
不得据此宣称实体设备或发布产物通过。

清单标记只代表已有仓库证据支持。当前仍有三类缺口：生产装配仍注册
`/uniclipboard/pairing/2`，且保留 `PairingSessionPort`、`PairingEventPort` 和旧 ALPN
兼容探测；三设备与实体设备矩阵未执行；被本规格取代的关联规格状态尚未全部同步。
因此本规格是“主体实现完成”，尚未达到 clean-cutover 和实体交付全部完成。

* [ ] 只有 `/uniclipboard/space-admission/1` 一个生产 Space 准入 ALPN。
* [x] Router spawn 前 endpoint 已一次绑定；缺失/重复绑定构造失败。
* [x] Infra handler 对每条认证完成的业务消息只调用一次 Application typed message endpoint。
* [ ] 不存在 PairingEventPort/PairingSessionPort/subscriber/recv pump 准入路径。
* [x] `SpaceAdmissionProtocol` 是 start/cancel/handle/recover/complete 的唯一完整负责人。
* [x] Facade、Engine、binding 不读取或判断协议 stage。
* [x] Core 只有一套 typed envelope/body/evidence/state aggregate。
* [x] 当前 SpaceJoinRecord optional-field bag 和旧 persisted decoder 已删除。
* [x] 不存在 opaque payload + reply Vec<u8> endpoint。
* [x] 每个 stage 只携带合法字段，无跨阶段可选业务字段。
* [x] 每次公开 JoinSpace 生成全新 admission/join/ordinal/member/KeyPackage/credentials。
* [x] 自动 retry/restart 不生成新身份或消息。
* [x] Prepared 之前 supersede 原子完成；Prepared 以后稳定冲突且零副作用。
* [x] OPAQUE 使用固定 `opaque-ke`/Argon2/Ristretto255 配置并通过 RFC vectors。
* [x] 短 code 只用于 discovery，不作为认证 secret。
* [x] OPAQUE transcript 绑定协议、admission、invitation、角色和双方 channel peer id。
* [x] continuation key 只由 OPAQUE 派生并加密保存；后续恢复不依赖 passphrase 或原连接。
* [x] OpenMLS 是 Add/Commit/Welcome 的唯一实现，Application/Core 无自制 MLS。
* [x] AddDevice 是唯一正向成员资格，receipt/Complete 不单独授权。
* [x] Candidate/Prepared/Commit/Applied/Complete/CompleteAck/Settled 全部类型化且顺序固定。
* [x] 每个 reply 在发送前与 stage/evidence 原子保存。
* [x] 同 id/digest exact replay，不重新执行密码或随机步骤。
* [x] 同 id 不同 digest、乱序、错 predecessor 失败关闭且零业务推进。
* [x] S2 是唯一正式提交点；之后不回滚 AddDevice。
* [x] J3 完成前 joiner 普通入口关闭；Active 只在 runtime 可用后保存。
* [x] Cancel 与 S2 竞态只有 Rejected(Cancelled) 或 TooLateCommitted 两种结果。
* [x] Sponsor 离线 completion helper 不能创建/修改成员事实。
* [x] P2P 失败保持 Pending，不自动走 LAN compatibility。
* [x] 准入 aggregate 与 membership ledger 分别原子保存协议状态，以及 history/receipt/effect/revision。
* [x] profile/Space/record/terminal 使用正确 MasterKey/data-key 域；无明文镜像。
* [x] admission lookup key 是 HMAC，不保存 id 明文。
* [x] cutover 强制单设备 Space rebuild 和重新配对，不导入旧 admission/branch。
* [x] 旧 ledger admission records 与旧独立准入 store 已删除；当前 `admission_repository_state` 只保存新 aggregate。
* [ ] 旧 pairing ALPN/wire/session/event/outbox/preparation/completion-recovery 全部删除。
* [ ] 无 translator、fallback、dual write、feature flag、compat alias 或 no-op production adapter。
* [ ] architecture script 对 Mandatory Deletion Checklist 的旧符号零容忍。
* [x] 稳定 Engine/UniFFI/N-API 产品操作和结果通过 contract tests。
* [x] 异步远端失败通过 Rejected projection，不保留第二同步握手错误路径。
* [x] Core/Application/Infra focused tests 均先确认非零再通过。
* [x] 真实 SQLite 故障注入只恢复完整旧/新状态。
* [x] 真实 Iroh loopback 完成 initial 和 continuation exchange。
* [x] Engine 双实例从 JoinSpace 到 Active 并双向传输通过。
* [ ] 三设备离线加入/恢复/传播通过（跳过：当前无三台实体设备）。
* [ ] Android/iOS 实体双向角色矩阵通过（跳过：当前无实体设备；HarmonyOS 同样跳过）。
* [x] 日志和持久文件明文探针无敏感命中。
* [x] metadata、workspace check、format、architecture、diff checks 全部通过。
* [ ] `docs/architecture/architecture-bible.md` 和规格 017/023/025/027 状态同步更新。

# 10. Risks and Trade-offs

## OPAQUE dependency and temporary password-equivalent

OPAQUE 去除自制口令 challenge，并提供标准 mutual authentication；代价是引入新的密码依赖和一份短期 password-equivalent。风险通过固定版本、RFC vectors、Argon2、advisory/license 检查、ProfileAdmissionMasterKey AEAD、Zeroizing 和 continuation 保存后立即删除来控制。若固定依赖不能通过验证，本规格实施阻塞，不得回退旧 HMAC challenge 或自制 PAKE。

## Mandatory Space rebuild

不兼容切换要求现有设备重新配对，用户成本高。好处是没有旧授权迁移、旧消息重放、旧 ALPN、双 schema 或不可证明的历史提升。rebuild 保留本机业务资料并建立新单设备 root；不能用删除本机内容简化实现。

## Direct handler instead of event channel

直接 handler 消除“生产者存在、订阅者缺失”和 session map 生命周期问题，也让 caller 明确。代价是 Engine 必须重排构造顺序，Application endpoint 必须在 Router spawn 前存在。Dormant application -> bind -> router -> runtime 的一次顺序由构造测试固定。

## One request-response stream per durable exchange

每步重连增加少量 QUIC/relay 延迟，但把正确性从长连接中移除，显著缩小 crash/restart 状态。Iroh 可复用 endpoint/connection path；不为性能恢复长 session 业务事实。只有 profiling 证明不可接受后，才可在不改变 exchange identity 的前提下复用 connection。

## Stage-carrying aggregate size

封闭 enum 会增加类型和文件数量，但删除大量运行时缺字段验证和不可能状态。文件多不是复杂度外泄；调用方只学习完整协议入口，stage knowledge 局限于 Core/Application protocol。

## Single encrypted ledger

联合提交可能重写较大 ciphertext，且 transaction 覆盖多个 key domain。代价换来唯一事实来源和可证明原子性。Infra 可以物理拆行，但不能把提交顺序暴露给 Application。

## 4 MiB candidate bundle

完整历史、Welcome 和 helper deliveries 可能接近上限。固定上限阻止无界内存；历史过大时应在创建 candidate 前稳定拒绝或通过已有有界 history sync 先收敛，不能自动提高配置。

## Public synchronous error cleanup

新 JoinSpace 先保存并返回 Pending，远端结果异步进入 DeviceTrust。部分历史同步错误码会变得不可达。删除它们可消除第二结果路径，但要求产品正确订阅/刷新状态；contract 和实体设备测试必须验证。

## Rejected alternatives

- **恢复旧 PairingInboundOrchestrator**：恢复旧责任和旧 wire，不符合单一新协议。
- **给 PairingEventPort 补 subscriber**：只能修消息丢失，不能补 typed contract、认证、生产 protocol、outbox 和 store。
- **保留 raw bytes endpoint**：caller 无法验证/路由 kind、sequence、predecessor 和 reply。
- **让 Infra 解释准入 stage**：形成第二协议负责人并让持久顺序泄漏到 adapter。
- **让 Application 实现 PAKE/MLS 算法**：重复成熟密码库并扩大审计面。
- **保留 separate admission repository**：历史/receipt/effect 与 attempt 无法一个事务提交，形成双事实来源。
- **继续使用长 pairing session 作为恢复**：进程/网络丢失后没有持久事实，无法满足 restart。
- **支持旧 ALPN 或 wire translator**：形成兼容分支和第二实现，用户已明确拒绝。
- **选择当前 RustCrypto SPAKE2 crate**：其公开 issue 涉及 RFC transcript 和 memory-hardening；不作为本次生产依赖。
- **以 handshake/OPAQUE/Welcome 成功返回 Active**：没有双方持久 AddDevice/receipt/transition，违反权限门禁。

# 11. Open Questions

没有阻止实现的开放产品或架构问题。以下决定已经固定，实施者不得重新选择：

- 单一新 ALPN、无旧协议兼容或 fallback；
- 现有 profile 强制单设备 Space rebuild 并重新配对；
- `SpaceAdmissionProtocol` 是唯一完整负责人；
- Infra Iroh direct handler 完成连接/认证/wire 后调用 typed message endpoint，Application 不认识 Iroh；
- OPAQUE (`opaque-ke` 固定版本 + Argon2) 负责口令认证；OpenMLS 负责 MLS；
- 一条 durable message 一个 request-response exchange；连接不是业务事实；
- 一个 encrypted membership ledger 是唯一持久事实来源；
- AddDevice/S2、Applied receipt/S3、Complete/J3 的成功边界保持规格 023 语义；
- stable product operations/results 保持，协议阶段不进入 binding；
- 全部外层接入和实体设备证据属于同一实施切换，不能留给后续规格。

实施开始前唯一允许的前置验证是确认固定 `opaque-ke` 版本在仓库 Rust toolchain、目标平台和许可证策略下通过。若失败，任务状态是 Blocked，并提交新的密码依赖决策；不得在代码中临时换算法。
