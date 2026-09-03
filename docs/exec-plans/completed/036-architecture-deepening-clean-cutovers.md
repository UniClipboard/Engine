# 规格 036：关键模块深化与退役路径 clean cutover（已完成）

## 状态

- **状态**：已完成
- **日期**：2026-09-03
- **前置规格**：[031 Application 依赖表面深化](../completed/031-application-dependency-surface-deepening.md)、[035 Space 观测装配 interface 收敛](../completed/035-space-domain-observability-assembly.md)
- **相关规格**：[034 确定性虚拟 Peer Network 测试套件](034-deterministic-virtual-peer-network-test-suite.md)可为 Iroh contract 提供后续安全网，但不是本规格的实施前置
- **完整负责人**：五个切片分别由 `uc-application::clipboard`、`uc-infra::db::repositories`、`uc-infra::network::iroh`、`uc-engine::runtime::session_supervisor` 和 `uc-infra::space::security` 完整负责；不存在跨五项工作的运行期总负责人
- **调用方唯一动作**：每个切片的调用者只提交一个完整意图或调用一个窄内部 interface；不得继续编排该切片隐藏的步骤，也不得建立覆盖五项工作的通用 facade
- **成功结果**：五个切片分别 clean cutover，原有 Engine 稳定操作、绑定合同、持久格式、网络协议和安全语义不变；每个切片可独立验收和停止
- **失败结果**：下层失败保持稳定分类和完整 source chain；best-effort 行为以 typed outcome 表达，不把失败字符串化、吞掉或升级成不相关的业务失败
- **重试与重启责任**：Clipboard 由既有领域 runtime 负责；membership ledger 与现有持久恢复负责人不变；Iroh adapter 仍决定协议重试；SessionSupervisor 负责 session 重建和回滚；Space security 继续由现有持久状态与升级流程恢复

## 实施结果

- Clipboard 本机观察与显式发送已统一为 Application 的 `process_local_clipboard` 完整意图入口；7 个处理器单元测试覆盖空输入、去重、索引失败、dispatch 失败及三种宿主 dispatch 模式。
- 五个退役 membership persistence port、四个 repository adapter 及其 codec/store branch 已删除；relationship reset 继续按 profile scope 删除整表关系行，并以不透明退役密文行验证无需旧 decoder。
- Iroh 地址读取与解码已收口到私有 `PeerAddressResolver`；有效、缺失、repository 失败和损坏编码四类结果均有测试，全部 Iroh library 回归为 202 passed、2 ignored。
- 生产 session 的 factory、构造、恢复、安装、关闭与 storage 均由 `session_supervisor` module 拥有；父 runtime 只负责配置与能力投影，operation gate 和事件投影测试通过。
- Space access 已拆为完整依赖的 `RuntimeSpaceAccessAdapter` 与仅实现初始化 port 的 `MigrationSpaceAccessAdapter`；runtime 安全依赖均为非可选字段，旧类型、旧构造器和 capability-unavailable 分支已删除。
- 架构脚本新增五组负向门禁并通过自验证。真实设备、移动平台宿主和发布产物矩阵本次未执行，记为“跳过”，不记为“通过”。

# 1. Overview

[ADR-018](../../design-docs/decisions/018-domain-oriented-application-layout.md) 与规格 031 已把 Application
对象图和生命周期收回领域模块，但当前代码仍有五处复杂度泄漏或已退役 surface：

1. 本机 Clipboard capture 的 capture、active register、事件、去重、实时索引和 dispatch 仍由两个 Engine
   caller 分别编排，且显式发送与宿主观察路径已经形成不同顺序。
2. 旧 membership candidate、announcement、outbox 和 applied security update repository 已无生产消费者，
   但 Core port、Infra adapter、加密 codec、store branch 和测试仍保留。
3. 多个 Iroh adapter 重复读取并 `postcard` 解码 `EndpointAddr`，对缺失、损坏和 repository 失败采用不一致
   分类，部分日志还输出设备或目标身份。
4. `SessionSupervisor` 名义上拥有 session 生命周期，但 `SessionFactory`、`ProductionSession`、构造、回滚与
   shutdown implementation 分散在 `runtime/mod.rs`，形成 owner 对 `ProductionRuntime::build_session` 的回指。
5. `DefaultSpaceAccessAdapter` 用多个 optional capability 表达正常 runtime 与迁移模式，合法组合知识散落在
   三个构造器、几十个方法和测试 fixture 中。

这些问题不能通过增加转发 facade、合并所有 port 或移动文件解决。本规格以五个独立 clean-cutover 切片深化现有
module：调用者只表达意图，implementation 隐藏顺序、格式和生命周期；已经没有生产价值的 interface 则直接删除。

实施顺序固定为 Clipboard capture、退役 membership persistence、Iroh peer-address resolution、生产 session
生命周期、Space security mode。每个切片必须独立通过验收后才能进入下一项，禁止把五项堆入一个不可审查的大提交。

# 2. Goals

- 让 Engine 对一次本机 Clipboard 处理只提交一个语义化意图，并取得一个稳定结果；Application 唯一拥有 capture、
  active register、dedup、best-effort index 与 dispatch 顺序。
- 删除无生产消费者的四组 membership repository interface、Infra adapter、加密 payload codec、store branch、
  re-export 和只验证退役路径的测试，同时保证遗留密文行仍能被 relationship reset 清除。
- 在 Iroh Infra 内建立唯一 peer-address resolution module，统一 repository 读取、`EndpointAddr` 解码和失败事实，
  同时保留各协议 adapter 对 offline、unreachable 或 internal 的既有映射。
- 让 `session_supervisor` module 完整拥有生产 session 的 factory、build、install、失败回滚、恢复和 shutdown，
  删除其对 `ProductionRuntime::build_session` 的回指。
- 将正常 runtime Space security 与 migration-only Space access 构造成两个合法模式；正常 runtime implementation
  不再以 `Option` 保存必需的安全 session 和 repositories。
- 所有切片保持 `uc-engine` 稳定操作、错误码、事件、iOS/Android/HarmonyOS 绑定和 LAN compatibility 显式选择语义。
- 所有新增或改变的下层失败转换保留 typed source；日志只使用固定分类、阶段、计数和耗时，不包含身份、地址、
  文件名、路径、内容或错误文本。
- 为每个 clean cutover 增加架构负向检查，阻止旧步骤入口、退役类型、重复 address decode、生命周期回指和
  optional runtime capability 回流。

# 3. Non-Goals

- 不建立覆盖 Clipboard、membership、Iroh、session 与 Space security 的通用 manager、provider 或 facade。
- 不改变剪贴板去重规则、snapshot hash、索引内容、目标选择、dispatch wire message 或宿主可见发送报告语义。
- 不把宿主事件投递、平台 clipboard 读取或 Engine 操作错误码的最终映射移入 Core。
- 不删除当前 `MembershipLedger`、ACK watermark、持久重试状态、历史反熵或当前 membership announcement 能力；
  本规格只删除已退出生产对象图的旧 repository 路径。
- 不解密、导出、迁移到明文或重新解释退役 relationship payload；遗留行只允许由明确 reset/cleanup 责任删除。
- 不创建 Application 级总 `TransportProvider`，不改变 Iroh ALPN、认证身份、frame bounds、连接策略或协议重试。
- 不把 `SessionSupervisor` 移到 Application；它继续负责 Engine 生产资源和网络 session 生命周期。
- 不修改 Space 密码算法、MasterKey AEAD、profile content key vault、control generation、AAD、SQLite schema 或
  V3 upgrade 语义。
- 不借本规格迁移规格 035 留下的 Clipboard/Blob observability 后继项，也不把两阶段 adapter seam 合并成虚构的
  Engine 侧完整 bundle。
- 不机械合并仍有真实消费者的小 port，不改造已经具备深度的 `SearchCoordinator` 或 `MembershipLedger`。
- 不在同一提交中保留新旧入口、兼容 alias、deprecated wrapper 或双模式 fallback。

# 4. Current Architecture Context

```text
Module: ApplicationRuntime Clipboard step methods
Path: crates/uc-application/src/application.rs
Responsibility: 当前分别公开 capture、live index、普通 dispatch 和 targeted dispatch。
Relationship: host_operations 与 host_clipboard 必须理解内部顺序，违反 Application 完整负责人约束。
```

```text
Module: Explicit clipboard send
Path: crates/uc-engine/src/runtime/host_operations.rs
Responsibility: 当前执行 capture、按 dedup 决定 best-effort index，再按目标 dispatch 并映射 SendReportSummary。
Relationship: 与宿主 clipboard observer 重复编排相同动作，但不推进 active register 或产生同一宿主事件。
```

```text
Module: Host clipboard observer
Path: crates/uc-engine/src/runtime/host_clipboard.rs
Responsibility: 读取平台 snapshot、判断来源，再执行 capture、active register、宿主事件、索引和按模式 dispatch。
Relationship: 同时承担合法的宿主观察职责和不应承担的 Application 业务步骤顺序。
```

```text
Module: Retired membership repository ports
Path: crates/uc-core/src/membership/ports.rs、crates/uc-core/src/membership/error.rs、crates/uc-core/src/membership/mod.rs
Responsibility: 定义 candidate、announcement、outbox、applied security update 和 verified-peer promotion 的旧持久能力。
Relationship: 当前无生产 Core/Application 消费者；仅由退役 Infra adapter 与测试引用。
```

```text
Module: EncryptedRelationshipStore legacy branches
Path: crates/uc-infra/src/db/repositories/relationship_store.rs、crates/uc-infra/src/db/repositories/membership_*_repo.rs
Responsibility: 当前仍保存四种旧 RelationshipKind、payload codec、CRUD 和浅 adapter。
Relationship: 生产 wire 只构造 member、trusted-peer、peer-address；旧 branch 不进入生产对象图。
```

```text
Module: Iroh peer address consumers
Path: crates/uc-infra/src/network/iroh/
Responsibility: membership history、group update、branch recovery、presence、transfer progress 与 active clipboard 各自读取并解码地址。
Relationship: 同一个持久格式和损坏事实被复制到多个 adapter，协议结果映射与底层事实混在一起。
```

```text
Module: SessionSupervisor
Path: crates/uc-engine/src/runtime/session_supervisor.rs
Responsibility: suspend、resume、reset、transition、stop 和 install 当前生产 session。
Relationship: 保存 factory/session，却依赖 runtime/mod.rs 中的 factory 类型、session 类型、build 和 shutdown implementation。
```

```text
Module: Production session implementation
Path: crates/uc-engine/src/runtime/mod.rs
Responsibility: 创建 Iroh/Application runtime、注册后台任务，并按顺序关闭任务、Application 与 SyncEngine。
Relationship: lifecycle 知识与 supervisor 双向分布，构造失败和 rollback 主要依赖慢 E2E 间接验证。
```

```text
Module: DefaultSpaceAccessAdapter
Path: crates/uc-infra/src/space/security/access.rs
Responsibility: 同一 concrete adapter 实现初始化、解锁、恢复、派生、准入、成员分支恢复、撤销、bootstrap 和 protection ports。
Relationship: 三个构造器用 optional security session/repositories 表达正常 runtime、部分测试与 migration-only 模式。
```

```text
Module: Space access assembly
Path: crates/uc-engine/src/assembly/wire/infra.rs、crates/uc-application/src/deps.rs、crates/uc-infra/src/config_migration/mod.rs
Responsibility: Engine 将完整 adapter 投影为 `SpaceAccessPorts`；config migration 构造无 security repositories 的访问器。
Relationship: 正常 runtime 始终提供完整依赖，但 adapter 内部仍按每次调用检查 capability 是否存在。
```

当前关键数据流：

```text
Host observer / explicit send
    → ApplicationRuntime.capture_clipboard
    → caller 决定 active register、event、dedup、index
    → caller 选择 dispatch 方法
    → caller 映射 Engine 结果
```

```text
Iroh protocol adapter
    → PeerAddressRepositoryPort::get
    → adapter-local postcard decode
    → adapter-local warn/swallow/error mapping
    → dial and protocol exchange
```

```text
SessionSupervisor
    → ProductionRuntime::build_session
    → ProductionSession
    → SessionSupervisor install/rollback
    → ProductionSession::shutdown
```

# 5. Proposed Design

## Components

### 5.1 Clipboard `LocalClipboardProcessor`

- **位置**：`crates/uc-application/src/clipboard/local/`；由 Clipboard assembly 私有构造，稳定请求/结果从
  `crates/uc-application/src/facade/clipboard.rs` 精确导出。
- **职责**：一次完成本机 snapshot 的 capture、必要的 active register 推进、dedup 判断、best-effort live index
  和 dispatch；把宿主需要映射的事实作为稳定 outcome 返回。
- **输入**：`LocalClipboardRequest { snapshot, origin, intent, source_started_at }`。
- **输出**：`LocalClipboardOutcome::Empty` 或 `Completed(LocalClipboardCompletion)`。
- **关系**：`ApplicationRuntime` 只转发一个 `process_local_clipboard`；Engine 不再取得 capture/live-index/sync owner。

意图区分业务来源，不暴露步骤开关：

```rust
pub enum LocalClipboardIntent {
    ObservedHostChange { dispatch: HostClipboardDispatch },
    ExplicitSend { targets: Vec<DeviceId> },
}

pub enum HostClipboardDispatch {
    CaptureOnly,
    AwaitReport,
    Background,
}
```

`ObservedHostChange` 保持现有 active register 和宿主新内容通知语义；`ExplicitSend` 保持现有目标过滤与发送报告
语义。不得替换为 `advance_active: bool`、`index: bool`、`emit_event: bool` 等步骤布尔值。

### 5.2 Retired membership persistence deletion

- **位置**：Core membership ports/errors/re-export，Infra repository modules 与 `relationship_store.rs`。
- **职责**：删除无生产消费者的 interface 和 implementation；不新增替代 repository。
- **输入/输出**：无新运行期接口。
- **关系**：当前 `MembershipLedger` 继续是 membership 历史、ACK、effects 和 retry state 的唯一持久事实来源。

必须删除：

- `MembershipCandidateRepositoryPort`
- `MembershipAnnouncementRepositoryPort`
- `MembershipOutboxRepositoryPort`
- `MembershipAppliedSecurityUpdateRepositoryPort`
- `VerifiedPeerPromotionPort`
- 对应 repository errors、Infra adapter、module export、store CRUD、payload codec 和只验证这些路径的测试

`RelationshipStateResetPort` 必须继续删除整个 profile scope 下的 relationship rows，包括未知或退役 kind；不得因删掉
enum variant 而只清除当前三种 kind。`SpaceMembershipCandidate` 等领域模型只有在生产可达性检查证明无其他真实 owner
后才能随切片删除；否则保留模型并记录剩余消费者。

### 5.3 Infra `PeerAddressResolver`

- **位置**：新增 `crates/uc-infra/src/network/iroh/peer_address_resolver.rs`，仅在 Iroh module 内可见。
- **职责**：从 `PeerAddressRepositoryPort` 读取 opaque blob 并解码 `EndpointAddr`；统一底层事实和安全诊断。
- **输入**：目标 `DeviceId`，只用于 repository lookup，不进入日志或错误文本。
- **输出**：`Result<Option<EndpointAddr>, PeerAddressResolutionError>`。
- **关系**：Iroh protocol adapter 注入同一个 resolver 或共享其 repository-backed instance，再将结果映射到既有
  protocol error；resolver 不连接网络、不重试、不决定 offline 业务语义。

目标错误：

```rust
enum PeerAddressResolutionError {
    Repository { source: PeerAddressRepositoryError },
    InvalidEncoding { source: postcard::Error },
}
```

`Ok(None)` 仅代表没有地址记录。repository 失败与损坏不得再通过 `.ok().flatten()` 合并成缺失。需要降级为
offline/unreachable 的 adapter 可以在映射时降级，但必须产生无身份、无错误文本的稳定分类事件；Presence 等已有
合同要求 Internal 的路径继续保留 source。

### 5.4 Production session lifecycle module

- **位置**：`crates/uc-engine/src/runtime/session_supervisor.rs`，必要时拆为
  `runtime/session_supervisor/{mod,factory,session}.rs`，但对 `runtime` 父模块只暴露 `SessionSupervisor`。
- **职责**：完整拥有 `ProductionSessionFactory`、`ProductionSession`、build、install、pending transition recovery、
  rollback、suspend/resume 和 shutdown 顺序。
- **输入**：现有 `WiredDependencies`、运行期配置和生命周期意图。
- **输出**：当前 session 或现有稳定 `EngineError` 分类。
- **关系**：`ProductionRuntime` 只创建/configure supervisor，并通过它投影当前 facade/port；不得保留
  `ProductionRuntime::build_session`。

目标内部 interface：

```rust
impl ProductionSessionFactory {
    async fn build(&self) -> Result<ProductionSession, EngineError>;
}

impl ProductionSession {
    async fn shutdown(self, reason: FileTransferCancellationReason);
}
```

这两个类型保持 Engine crate-private，不进入稳定 `uc-engine` 入口。测试失败注入只能是 module-private 或
`cfg(test)` seam，不能为了 fake 暴露新的生产 trait。

### 5.5 Mode-specific Space access modules

- **位置**：`crates/uc-infra/src/space/security/access/`；共享密码和 session 操作留在私有 implementation。
- **职责**：分别表达完整 runtime 能力与 migration-only 能力，构造后不可能缺少该模式必需依赖。
- **输入**：完整 runtime 接受 key material、current profile、session、revocation repository、legacy bootstrap
  repository 和 profile content key vault；migration-only 只接受其真实调用所需依赖。
- **输出**：实现各自合法 port 集合的 concrete adapter。
- **关系**：Engine 只构造 `RuntimeSpaceAccessAdapter` 并投影 `SpaceAccessPorts`；config migration 与相关升级路径构造
  `MigrationSpaceAccessAdapter`。共享密码 implementation 不公开为新上层 seam。

目标构造器：

```rust
impl RuntimeSpaceAccessAdapter {
    pub fn new(
        key_material: Arc<KeyMaterialStore>,
        current_profile: Arc<dyn CurrentProfilePort>,
        session: Arc<InMemorySession>,
        revocation_repository: Arc<dyn RevocationRepositoryPort>,
        legacy_bootstrap_repository: Arc<dyn LegacyBootstrapRepositoryPort>,
        profile_content_key_vault: Arc<ProfileContentKeyVault>,
    ) -> Self;
}

impl MigrationSpaceAccessAdapter {
    pub fn new(
        key_material: Arc<KeyMaterialStore>,
        current_profile: Arc<dyn CurrentProfilePort>,
        session: Arc<InMemorySession>,
    ) -> Self;
}
```

完整 adapter 的 `active_security_session`、`revocation_repository` 和 `legacy_bootstrap_repository` 为非 optional
字段。若某项 port 只在 migration/upgrade 路径需要，应由 migration-specific implementation 实现，而不是在完整
adapter 中恢复 optional capability。

## Data Model

本规格不新增 SQLite 表、列、磁盘文件、搜索字段或 wire message。

新增的 Clipboard request/outcome 只存在于内存和一次调用期间：

- `LocalClipboardRequest` 按值拥有 snapshot 与语义化 intent；完成或失败后释放。
- `LocalClipboardCompletion` 保存 `entry_id`、`snapshot_hash`、dedup 结果、索引 outcome 和可选 dispatch outcome；
  不保存原始内容的额外副本，不序列化。
- best-effort index 失败以脱敏 `LocalIndexStatus::Failed { kind }` 或等价稳定分类存在于一次返回值中；不得携带
  错误显示文本。

退役 relationship rows 保持原有密文，不迁移、不解密。runtime 删除旧 kind 解码能力后，这些行仅对
`RelationshipStateResetPort` 的 scope cleanup 可见。若仓库现有升级合同要求主动清理，必须以直接密文行删除完成，
不能恢复旧 payload 的业务解释能力。

`PeerAddressResolver` 不缓存地址，避免建立第二事实来源。`EndpointAddr` 生命周期仍限于一次 Iroh 连接尝试。

Session 与 Space mode 拆分只改变内存对象图。不得改变 key material、control generation、profile content key vault
或 session 持久生命周期。

## API / Interface

Clipboard 最终公开面：

```rust
impl ApplicationRuntime {
    pub async fn process_local_clipboard(
        &self,
        request: LocalClipboardRequest,
    ) -> Result<LocalClipboardOutcome, LocalClipboardProcessError>;
}
```

旧 `capture_clipboard`、`index_clipboard_capture`、`dispatch_clipboard_capture` 和
`dispatch_clipboard_capture_to_targets` 在 caller 迁移的同一切片删除，不保留 wrapper。

错误必须区分：

- runtime unavailable；
- capture 的 typed source；
- active register 的 typed source（若现有 port 具备失败）；
- dispatch 的 typed source；
- index 属于 best-effort outcome，不把一次成功 capture 改判为整体失败。

Engine 继续拥有平台 clipboard read error、宿主事件发送和稳定 operation code 映射。Application outcome 必须足以让
Engine 直接映射，不允许 Engine 根据 dedup 再决定是否调用其他 Application 方法。

Iroh resolver 为 Infra private interface；Space adapters 与 Session types 为各自 crate-private/concrete interface。
除 Clipboard 完整意图外，本规格不扩大 Application 或 Engine 稳定公开面。

## Workflow

### Clipboard 宿主观察

1. Engine 读取平台 snapshot 并完成 origin attribution；空 snapshot 和 remote-push 回环继续在宿主观察 owner 处终止。
2. Engine 提交一次 `ObservedHostChange` intent。
3. Clipboard processor capture；无内容变化返回 `Empty`。
4. Application 按现有语义推进 active register，并形成宿主通知所需的稳定 completion。
5. Application 仅在非 deduplicated 时执行 best-effort live index。
6. Application 按 `HostClipboardDispatch` 执行 capture-only、await-report 或 background dispatch。
7. Engine 只映射 outcome 为宿主事件、日志和可选发送报告。

### Clipboard 显式发送

1. Engine 解析目标设备并提交一次 `ExplicitSend` intent。
2. Application capture，按既有语义跳过重复内容的 index，但仍执行目标 dispatch。
3. Application 返回完整 dispatch outcome；Engine 只调用既有 `send_report_summary` 映射稳定操作结果。

### Membership persistence clean cutover

1. 先用静态引用检查和启动/升级测试证明旧 repository 不进入生产对象图。
2. 以原始加密 relationship row 注入测试证明 reset 能删除未知/退役 kind，而无需旧 codec。
3. 删除 Core port/error/re-export、Infra adapters、store methods/kinds/codecs 和只验证旧路径的测试。
4. 删除或保留关联领域模型，依据逐类型生产可达性结果决定并在实施记录中列出。
5. 增加架构检查禁止旧文件、类型名、kind 字符串和构造器回流。

### Iroh address resolution

1. Engine 仍将同一 `PeerAddressRepositoryPort` 注入各 Iroh adapter。
2. Iroh assembly 构造一个共享 resolver，并注入每个需要 dial 的 adapter。
3. resolver 返回地址、缺失、repository failure 或 invalid encoding。
4. adapter 按原合同映射结果后执行现有连接、超时、重试和协议交换。
5. 诊断只记录固定 operation/outcome/error kind，不记录 target、device、地址或 source 文本。

### Production session lifecycle

1. ProductionRuntime 构造并配置 SessionSupervisor。
2. supervisor 通过内部 factory build 新 session；build 失败时关闭已建立的 SyncEngine/Application 部分并返回原始失败。
3. pending transition 需要完成时，supervisor 关闭临时 session、完成 transition、重建并恢复 Space 活动。
4. 新 session 完整成功后原子替换当前 session 并 reopen operation gate。
5. suspend/reset/shutdown 都由 supervisor 取得 session 所有权并按唯一顺序关闭。

### Space security mode construction

1. Engine 正常启动只构造完整 runtime adapter；必需 dependency 在构造期全部提供。
2. Engine 将同一 concrete adapter 投影到 `SpaceAccessPorts`，调用时不再出现“repository unavailable”模式检查。
3. config migration/upgrade 路径只构造 migration adapter，并只能访问其实现的窄 ports。
4. 两个 adapter 复用私有密码 implementation，但不互相 fallback，也不通过 optional 字段模拟另一模式。

# 6. Implementation Plan

## Slice 1：Clipboard 完整动作

```text
Step 1.1
File: crates/uc-application/src/clipboard/local/、crates/uc-application/src/clipboard/assembly.rs
Change: 新增 LocalClipboardProcessor、request/intent/outcome/error；把 capture、active register、dedup、index 和 dispatch 顺序收进同一 module。
Risk: 显式发送与宿主观察的 active register/事件语义被意外统一；必须以语义化 intent 保持当前差异。
```

```text
Step 1.2
File: crates/uc-application/src/application.rs、crates/uc-application/src/facade/
Change: 增加唯一 process_local_clipboard 入口，迁移 typed error；删除四个步骤级 runtime 方法和不再需要的公开 re-export。
Risk: best-effort index 被错误升级为整体失败，或 source chain 在新错误映射中丢失。
```

```text
Step 1.3
File: crates/uc-engine/src/runtime/host_clipboard.rs、crates/uc-engine/src/runtime/host_operations.rs
Change: 两个 caller 改为各提交一次 intent，只映射稳定 outcome、宿主事件和 Engine operation result。
Risk: background/await/capture-only 的返回与事件时序发生变化。
```

```text
Step 1.4
File: scripts/architecture/check-engine-repository.mjs
Change: 禁止 Engine 调用旧步骤名，禁止 ApplicationRuntime 重新暴露 capture/index/dispatch 分步入口。
Risk: 规则过宽误伤领域内部合法方法；检查应限定跨 crate 公开面和 Engine source。
```

## Slice 2：退役 membership persistence 删除

```text
Step 2.1
File: crates/uc-core/src/membership/ports.rs、error.rs、mod.rs
Change: 删除五个退役 port、对应错误和 re-export；逐类型记录领域模型的生产可达性并只删除真正无消费者的模型。
Risk: 同名 CurrentMembershipAnnouncementPort 是当前能力，不能与旧 MembershipAnnouncementRepositoryPort 混删。
```

```text
Step 2.2
File: crates/uc-infra/src/db/repositories/mod.rs、membership_*_repo.rs、relationship_store.rs
Change: 删除四个 adapter 文件、旧 CRUD、RelationshipKind variants、payload codecs、verified promotion 和只验证退役路径的测试。
Risk: relationship reset 漏删旧密文行；先建立按 profile scope 删除未知 kind 的回归测试。
```

```text
Step 2.3
File: scripts/architecture/check-engine-repository.mjs
Change: 添加旧文件、类型、kind 字符串和 repository constructor 的负向门禁。
Risk: 文档历史可能包含旧名；检查只扫描生产源码和 module exports。
```

## Slice 3：Iroh peer-address resolution

```text
Step 3.1
File: crates/uc-infra/src/network/iroh/peer_address_resolver.rs、mod.rs
Change: 新增 private resolver 和 typed error，覆盖地址存在、缺失、repository failure、损坏 codec。
Risk: postcard error 若不适合直接作为 source，使用 anyhow::Error 包装但不得字符串化。
```

```text
Step 3.2
File: crates/uc-infra/src/network/iroh/*adapter.rs、active_clipboard/*adapter.rs
Change: 替换 adapter-local get/decode；逐 adapter 明确映射表并保留连接与协议行为。
Risk: 原来被静默降级的 repository failure 可能改变外部结果；本切片只改善可观测分类，不改变 port outcome。
```

```text
Step 3.3
File: scripts/architecture/check-engine-repository.mjs
Change: 禁止 Iroh adapter 在 resolver 外直接 postcard decode PeerAddressRecord.addr_blob；禁止相关 tracing 字段包含身份或地址。
Risk: 合法 wire codec 也使用 postcard；规则必须同时匹配 PeerAddressRecord/EndpointAddr 上下文。
```

## Slice 4：SessionSupervisor 生命周期深化

```text
Step 4.1
File: crates/uc-engine/src/runtime/session_supervisor.rs、crates/uc-engine/src/runtime/mod.rs
Change: 将 SessionFactory、ProductionSession、build_session、session shutdown 和 rollback helper 移入 supervisor module；ProductionRuntime 只保留构造与投影。
Risk: Rust 可见性和循环引用调整可能暂时扩大 pub(crate)；切片结束必须收回为最窄可见性。
```

```text
Step 4.2
File: crates/uc-engine/src/runtime/session_supervisor.rs tests 或相邻 cfg(test) modules
Change: 增加 build 各阶段失败、pending transition 双 build、install 原子性、shutdown 顺序和 operation gate 的确定性测试。
Risk: 为注入失败建立长期生产 trait；测试 seam 必须 module-private 或 cfg(test)。
```

```text
Step 4.3
File: scripts/architecture/check-engine-repository.mjs
Change: 禁止 ProductionRuntime::build_session、runtime/mod.rs 中的 ProductionSession implementation 和 supervisor 对父 runtime 的反向调用回流。
Risk: 禁止模式不得阻止 ProductionRuntime 合法创建 supervisor。
```

## Slice 5：Space security mode 构造即合法

```text
Step 5.1
File: crates/uc-infra/src/space/security/access.rs 或 access/ 子模块
Change: 提取私有共享密码 implementation，建立 RuntimeSpaceAccessAdapter 与 MigrationSpaceAccessAdapter；完整模式依赖改为非 optional。
Risk: 大文件拆分时错误改变密码、generation、AAD、会话刷新或 keychain 访问次数。
```

```text
Step 5.2
File: crates/uc-engine/src/assembly/wire/infra.rs、crates/uc-infra/src/config_migration/mod.rs、升级模块与测试
Change: 正常生产对象图只用 runtime adapter，迁移/升级 fixture 使用 mode-specific adapter；删除三个旧构造器和旧类型。
Risk: profile storage upgrade 可能同时需要 legacy bootstrap 与完整 runtime 能力；实施前按真实调用拆分 owner，禁止猜测。
```

```text
Step 5.3
File: scripts/architecture/check-engine-repository.mjs
Change: 禁止 `DefaultSpaceAccessAdapter`、旧构造器和 runtime adapter 中 optional security dependencies 回流。
Risk: 可选业务数据仍可能合法使用 Option；检查仅限定三个 capability 字段。
```

## Documentation closeout

每个切片完成后更新本文状态与实际证据。全部完成后：

1. 将稳定的 Clipboard owner、Iroh resolver、session 生命周期和 Space mode 事实回写对应 design docs；删除型工作只在
   仍需长期防回流时记录结论。
2. 更新 `docs/architecture/architecture-bible.md` 正文和维护记录。
3. 将本文从 `active/` 移入 `completed/`，同步两个 index，不保留 active/completed 双副本。

# 7. Edge Cases

```text
Scenario: 平台 snapshot 为空，或 capture 返回无变化
Expected behavior: 返回 Empty；不推进 active register、不索引、不 dispatch、不生成虚假发送报告。
Implementation: 由 LocalClipboardProcessor 在任何副作用前短路。
```

```text
Scenario: capture 命中 dedup
Expected behavior: 保持当前 resurfacing 语义，跳过 live index；宿主观察与显式发送仍按各自当前 intent 执行后续合法动作。
Implementation: dedup 只影响 index policy，不由 Engine 再次判断步骤。
```

```text
Scenario: live index 失败
Expected behavior: capture 与 dispatch 结果不被改判失败；返回脱敏 best-effort 状态并保留内部诊断 source。
Implementation: processor 捕获 typed index error，按既有 policy 记录固定分类，不返回错误文本。
```

```text
Scenario: dispatch 或 active register 失败
Expected behavior: 返回对应稳定失败，保留 source；不得重做已经提交的 capture。
Implementation: 恢复责任沿用现有 spool/register/runtime，Spec 不增加内存重试。
```

```text
Scenario: watcher 与 explicit send 并发处理相同 snapshot
Expected behavior: 由现有 capture/dedup/存储原子性产生一个稳定 identity；两次调用不得因不同 snapshot hash 复制条目。
Implementation: 两个 intent 共用同一 processor 和 capture owner，不增加 Engine 锁。
```

```text
Scenario: 数据库仍含退役 membership kind 的密文行
Expected behavior: 正常启动和升级不读取或恢复旧业务状态；relationship reset 能按 scope 删除这些行。
Implementation: reset 使用 scope 级删除或 opaque kind 清理，不依赖已删除 codec。
```

```text
Scenario: 误把 CurrentMembershipAnnouncementPort 当作旧 announcement repository 删除
Expected behavior: 编译或架构测试失败；当前 membership initializer 能力必须保留。
Implementation: 删除清单按完整类型名匹配，并为当前 port 保留生产引用断言。
```

```text
Scenario: peer address 不存在
Expected behavior: resolver 返回 Ok(None)，adapter 映射为其既有 NoAddress/Offline/Unreachable 结果。
Implementation: 缺失不记录 warning，不伪造 repository error。
```

```text
Scenario: peer address ciphertext repository 读取失败或 EndpointAddr blob 损坏
Expected behavior: resolver 返回不同 typed error；adapter 保持原 port outcome，同时产生无身份稳定分类诊断。
Implementation: source chain 仅留在内部错误；日志不格式化 source，不输出 device/target/address。
```

```text
Scenario: 地址在 resolve 后、dial 前变旧
Expected behavior: 保持现有 dial failure 与 staggered retry 语义；resolver 不建立 cache 或自动回写。
Implementation: 一次调用只解析一次当前记录，刷新仍由现有地址写入 owner 负责。
```

```text
Scenario: session build 在 Iroh 建立后、ApplicationRuntime start 前后失败
Expected behavior: 已创建资源按反向顺序关闭，旧 session 不被替换，operation gate 保持与当前状态一致。
Implementation: factory build 使用显式局部 ownership/rollback，不把半成品安装到 supervisor。
```

```text
Scenario: pending transition 需要关闭临时 session 后第二次 build 失败
Expected behavior: 当前 session 为空且 gate 关闭，返回第二次 build 的原始失败；下次显式恢复可重试。
Implementation: supervisor 独占 install 流程，不在两个 build 间 reopen gate。
```

```text
Scenario: suspend、reset 和 shutdown 并发
Expected behavior: session 只被 take 和 shutdown 一次；后续动作得到幂等成功或稳定 unavailable，不发生双重资源关闭。
Implementation: 继续用 supervisor 的 session mutex 和 operation gate 串行化 ownership 转移。
```

```text
Scenario: migration adapter 被用于正常 runtime port
Expected behavior: 在编译/组装期不可表达，不在运行期返回“repository unavailable”。
Implementation: 两个 concrete type 只实现各自合法 ports，Engine builder 签名要求 RuntimeSpaceAccessAdapter。
```

```text
Scenario: profile upgrade 同时需要 legacy bootstrap 与 content key vault
Expected behavior: 由明确的 upgrade-specific owner 接收完整必需依赖，不通过 optional 字段临时恢复万能 adapter。
Implementation: 实施前列出 upgrade 调用的 ports，并选择 runtime adapter 或独立 upgrade adapter；结果记录入本文。
```

```text
Scenario: 极端目标列表、最大 clipboard payload 或大型成员历史
Expected behavior: 保持既有大小上限、目标去重、分页和 frame bounds；本规格不增加与输入规模成倍的副本或无界集合。
Implementation: request 按值移动既有 snapshot/targets，resolver 不缓存，session/module 拆分不复制业务数据。
```

# 8. Testing Strategy

## Unit Test

### Clipboard processor

- **输入**：空 snapshot、首次 capture、dedup capture、index failure、dispatch failure、三种 host dispatch mode、空与多目标 explicit send。
- **操作**：只调用一次 `process_local_clipboard`。
- **预期结果**：调用顺序和次数固定；Engine 不再触发第二个 Application 方法；best-effort index 不改变整体成功；
  capture/dispatch source 可追溯。

### Membership deletion guard

- **输入**：含当前 kind 与任意退役/未知 kind 的加密 relationship rows。
- **操作**：执行 profile relationship reset。
- **预期结果**：scope 内全部关系行删除，其他 profile 不受影响；测试不需要旧 payload decoder。

### PeerAddressResolver

- **输入**：有效 `EndpointAddr`、缺失记录、repository fake error、损坏 postcard bytes。
- **操作**：调用一次 resolve。
- **预期结果**：分别得到 `Some`、`None`、Repository source、InvalidEncoding source；错误显示不含地址或身份。

### SessionSupervisor

- **输入**：各 build 阶段失败、pending transition、恢复 locked/unlocked、重复 shutdown、并发 suspend/reset。
- **操作**：通过 module-private fixture 触发生命周期意图。
- **预期结果**：rollback 逆序、session 原子安装、gate 状态正确、资源只关闭一次、source 保留。

### Space security modes

- **输入**：完整 runtime dependencies 与 migration-only dependencies。
- **操作**：分别构造两个 adapter 并调用其合法 ports。
- **预期结果**：无需 capability unavailable 分支；非法模式组合不能构造或不能满足 trait bound。

## Integration Test

- **Clipboard Engine integration**：通过现有公开 observe/send operation 提交相同 snapshot，验证 entry identity、宿主事件、
  active register、index 调用和 send report 与切片前一致。
- **Encrypted relationship integration**：以真实 SQLite/MasterKey scope 注入退役 kind 密文，重启确认不读取，reset 后确认
  行被删除且没有明文探针。
- **Iroh adapter contracts**：对 history、group update、presence、transfer progress、active pull/dispatch 逐项验证 resolver
  四类结果到既有协议结果的映射，不需要运行完整复杂拓扑。
- **Session lifecycle integration**：使用真实 ApplicationRuntime/SyncEngine 小型 fixture 验证 build failure cleanup、pending
  transition 双 build 和 shutdown 顺序；真实慢 E2E 保留为回归层。
- **Space security integration**：运行 config migration、profile storage upgrade、V3 admission transition、membership branch
  transition、device management reset 和 control-generation 测试。

## Regression Test

每个切片至少运行所属 crate 定向测试；全部完成时运行：

```bash
cargo metadata --locked --format-version 1
cargo test -p uc-application --locked
cargo test -p uc-infra --locked
cargo test -p uc-engine --locked
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

对受平台或设备限制未执行的矩阵项明确记录“跳过”，不得写成“通过”。涉及加密持久路径的 Slice 2 与 Slice 5 必须
额外执行相关真实 SQLite、重启恢复和明文探针测试。

# 9. Acceptance Criteria

## Slice 1

* [x] Engine 的 host clipboard 与 explicit send 各只调用一次 Application 完整意图入口。
* [x] `ApplicationRuntime` 不再公开四个 capture/index/dispatch 步骤方法，且不存在兼容 wrapper。
* [x] active register、dedup、best-effort index 与 dispatch 顺序只在 Clipboard module 的一个主要入口可读。
* [x] 空、dedup、index failure、dispatch failure、capture-only、background、await-report 和 targeted send 均有单元测试。
* [x] Engine 稳定操作结果、错误码、宿主事件和 snapshot identity 与切片前一致。

## Slice 2

* [x] 五个退役 Core port、对应 errors/re-export、四个 Infra repository adapter 与旧 store branches 已删除。
* [x] 当前 `CurrentMembershipAnnouncementPort`、`MembershipLedger`、ACK watermark、effects 和 retry state 未被删除或复制。
* [x] 生产启动与升级路径没有旧 repository 类型、构造器、kind 字符串或 decoder 可达性。
* [x] relationship reset 能删除当前、退役和未知 kind 的 scope 内密文行，不读取旧 payload。
* [x] 架构检查阻止退役路径回流。

## Slice 3

* [x] Iroh 只有一个 module 负责 PeerAddressRecord 到 EndpointAddr 的读取与解码。
* [x] missing、repository failure 和 invalid encoding 是三个可区分事实，后两者保留 source。
* [x] 每个 adapter 的外部 outcome、连接重试、timeout 与 wire 行为保持不变。
* [x] 相关日志和事件不含 device、peer、target、address 或 source 显示文本。
* [x] 架构检查阻止 resolver 外的重复 peer-address postcard decode。

## Slice 4

* [x] factory、session、build、install、rollback 和 shutdown 都位于 session_supervisor module。
* [x] `ProductionRuntime::build_session` 和 supervisor 对父 runtime 的反向调用已删除。
* [x] 半构造 session 不会安装；失败后资源反向关闭、gate 状态正确且 source 保留。
* [x] 重复或并发生命周期动作不会双重 shutdown 或暴露半初始化 facade。
* [x] 未为单一 production implementation 增加公开 trait seam。

## Slice 5

* [x] 正常 runtime 与 migration-only 使用不同 concrete adapter，并只能构造合法 dependency 组合。
* [x] 正常 runtime adapter 的 active security session、revocation repository 与 legacy bootstrap repository 不是 Option。
* [x] `DefaultSpaceAccessAdapter` 和三个旧构造器已删除，不保留 alias/fallback。
* [x] 密码、MasterKey AEAD、control generation、AAD、keychain 访问次数和 V3 upgrade 语义通过回归测试。
* [x] 完整生产 `SpaceAccessPorts` 仍由同一 runtime adapter 投影，绑定和 Engine 稳定入口不变。

## 全局

* [x] 五个切片按顺序独立提交和验收，没有跨切片的大爆炸提交。
* [x] Application 下层失败保持完整 source chain；best-effort 结果没有字符串化或吞错。
* [x] 没有新增明文持久字段、敏感日志、P2P 自动降级或内部 crate 发布面。
* [x] 所有相对链接有效，active index、架构圣经和本文实施状态同步。
* [x] 全工作区 check、fmt、架构检查和 diff check 通过；未执行矩阵明确记录为“跳过”。

# 10. Risks and Trade-offs

- **Clipboard 行为漂移**：把两条已分叉路径收口时最容易无意改变 active register、事件或 dedup 后发送语义。代价是
  request 需要语义化 intent，但它比步骤布尔值更能保持 owner 和测试清晰。
- **删除旧持久路径的升级风险**：静态无 caller 不等于旧数据无需处理。选择保留 opaque scope cleanup、删除 runtime
  decoder，可减少攻击面和维护成本，同时避免把旧业务状态重新带回当前事实来源。
- **Resolver 抽象过宽**：若 helper 同时决定 offline 或连接重试，会吞掉协议 adapter 职责。本设计只集中持久格式与
  解码事实，保留各 adapter 的结果映射，深度有限但职责明确。
- **Session 移动造成大 diff**：生命周期代码集中会触发 Rust 可见性与 import 调整。收益是 build/rollback/shutdown
  locality 和可测试性；禁止借机改业务生命周期顺序可控制风险。
- **Space mode 类型数量增加**：两个 concrete adapter 比一个 optional adapter 多类型，但删除了无效组合和几十个
  runtime capability check，长期认知成本更低。
- **测试时间**：真实 Infra 和 Engine 回归较慢。各切片以单元/contract 测试提供快速反馈，最终仍保留真实 SQLite、
  Iroh smoke 和安全升级验证，不能用 fake 取代。
- **替代方案：只增加 facade wrapper**：会保留旧步骤入口和 caller 知识，不采用。
- **替代方案：建立通用 repository/transport/lifecycle manager**：会把无关职责集中为浅 interface，不采用。
- **替代方案：保留旧类型并标 deprecated**：与仓库 clean cutover 和单一事实来源规则冲突，不采用。

# 11. Open Questions

以下问题必须在对应切片开工时以代码和测试证据回答；不能用猜测阻塞整份规格：

1. Slice 1：宿主新内容事件最终应继续由 Engine 根据 completion 发出，还是已有脱敏 HostEvent port 可以作为
   Clipboard processor 的注入能力？默认保持 Engine 映射，除非现有 port ownership 证明 Application 已是消费者。
2. Slice 2：当前发布版本的 profile upgrade 是否存在仓库外或 feature-gated 路径读取四种旧 relationship kind？需要
   对全部 features、升级 tests 和发布说明做最终可达性确认。
3. Slice 2：`SpaceMembershipCandidate`、旧 gossip message 与相关 errors 是否全部无生产消费者，还是仍被测试工具或
   compatibility feature 使用？逐类型 reachability 结果决定删除范围。
4. Slice 3：membership branch recovery adapter 是否通过共享 helper 或不同文件名间接解码地址？实施时需完成全 Iroh
   `PeerAddressRecord.addr_blob` 调用点清单，不能只迁移当前已知六类。
5. Slice 4：现有测试基础设施能否在不引入 production trait 的情况下注入 build 阶段失败？若不能，只允许添加
   crate-private seam，并在实现 PR 说明其唯一替代实现是测试 fake。
6. Slice 5：profile storage upgrade 是完整 runtime mode、migration mode，还是需要独立 upgrade-specific adapter？必须按
   实际 port 调用清单决定，禁止重新引入 optional capability。
7. Slice 5：拆分后 `SpaceAccessPorts` 是否仍应由一个 runtime adapter 全量投影，还是已有消费者分组足以进一步缩窄？
   本规格默认保持现有 bundle，避免同时重开 Application dependency design；任何进一步拆分需单独证据。
