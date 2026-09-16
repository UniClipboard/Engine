# Application 依赖表面深化计划

## 状态

已完成（2026-09-02）。九个切片均已 clean cutover；最终架构、回归证据与明确跳过项记录于本文。

本计划是 [ADR-018](../../design-docs/decisions/018-domain-oriented-application-layout.md) 的当前执行计划，
取代已关闭的 [规格 018](018-domain-oriented-application-layout.md) 中尚未完成的迁移步骤。
ADR-018 保存稳定领域归属；031 是剩余实施顺序、门禁和验收的唯一事实来源。

# 1. Overview

`crates/uc-application/src/deps.rs` 当前保存约 110 个 `Arc<dyn Port>`。小 port 数量本身不是问题：
[端口设计](../../design-docs/ports.md)要求消费者拥有窄 query、command 和 capability port，避免宽
repository 耦合无关能力。

问题位于更外层：Application 的对象图、步骤选择和生命周期顺序仍大量由 `uc-engine` 掌握。Engine
直接构造 Application 内部 use case、coordinator、runtime deps 和 session，再从 `AppDeps` 逐字段
投影能力，形成“单个 port 很窄、跨层调用 interface 很宽”的浅模块：

- Engine 必须理解 capture、inbound receive、transfer recovery、search 和 Space 的内部组合；
- facade 接收完整 bundle 后忽略大部分字段；
- Application 内部 coordinator 因 Engine 组装而被公开再导出；
- wiring 期已经消费或已退休的能力仍保存在 `AppDeps`；
- Engine 仍拥有应用领域启动、失败回滚和关闭顺序。

本计划不机械合并小 port，也不只移动文件。目标是在 Application seam 后建立深模块：Engine 只提供
平台、网络、持久化 adapter 和已装饰的领域 port，然后执行一次 Application 组装并操作一个具体
`ApplicationRuntime`；领域内部自行拥有对象图、恢复、重试和关闭知识。

# 2. Goals

- 删除三个 wiring-only `AppDeps` 字段，并用架构检查阻止回流。
- 按 admission、pairing crypto、receive state 三组删除七个零生产消费者 capability；删除前证明替代的
  隐私清理语义仍存在。
- 让 File Transfer、Search、Settings、Clipboard、Space 各自拥有内部对象图，Engine 不再逐步骤组装。
- 建立唯一 Application 组装入口，Engine 不再把 `&AppDeps` 分发给多个 builder。
- 建立一个具体 `ApplicationRuntime`，唯一负责应用领域启动、失败反向回滚和关闭顺序；不为单一实现
  新增 trait seam。
- 将 Active Clipboard startup reconcile 接入 Clipboard runtime，并保证它先于任何读取或广播持久
  active register 的 worker 成功完成。
- 保持 Application 下层失败的完整 source chain，消除迁移路径上的 `error.to_string()` 转换。
- 最终把 `uc-application` 对 Engine 的公开 interface 收敛到 `facade` 与 `deps` 的明确白名单。

# 3. Non-Goals

- 不为减少类型数量而合并仍有真实消费者或 adapter 的 capability ports。
- 不改变 `uc-engine` 对宿主公开的操作、结果、错误码或事件语义。
- 不改变 SQLite schema、MasterKey AEAD、V3 content protection envelope 或 active manifest。
- 不重写文件传输协议、剪贴板同步协议、成员规则或搜索行为。
- 不把 Iroh endpoint、宿主事件、具体 Infra adapter 或持续观测 decorator 移入 Application。
- 不在 029 完成或 030 按 033 control-only generation 重基线前迁移 Space assembly。
- 不保留新旧两套组装路径、兼容 alias 或长期过渡 facade。

# 4. Current Architecture Context

```text
Component: AppDeps 与领域 bundle
Path: crates/uc-application/src/deps.rs
Responsibility: 保存 Engine 构造后交给 Application 的 port 和少量已构造对象。
Relationship: 被多个 Engine builder 重复投影；它是 wiring inventory，不是深模块 interface。
```

```text
Component: Engine application assembly
Path: crates/uc-engine/src/assembly/
Responsibility: 构造 Infra/host adapter，并直接组装多个 Application use case、runtime deps 和 facade。
Relationship: 同时承担合法 composition root 职责和不应承担的应用对象图/步骤编排职责。
```

```text
Component: ProductionSession lifecycle
Path: crates/uc-engine/src/runtime/mod.rs
Responsibility: 启动 search、clipboard、transfer maintenance、Space 活动并逐项关闭。
Relationship: 当前掌握应用生命周期；目标是只保留 Engine/host/network 生命周期并委托 ApplicationRuntime。
```

```text
Component: File Transfer
Path: crates/uc-application/src/transfer/file/
Responsibility: 现有 facade/lifecycle 已负责 readiness、startup reconciliation、timeout 和 privacy maintenance。
Relationship: 模块已有深度，但 FileTransferLifecycleDeps 和 receive 步骤仍由 Engine 拼装。
```

```text
Component: Clipboard 与 Active Clipboard reconcile
Path: crates/uc-application/src/clipboard/
Responsibility: capture、inbound、history、restore、active register、spool 和同步流程。
Relationship: Engine 重复组装多种模式；reconcile 已有实现和测试但未接生产，且必须先于相关 worker。
```

```text
Component: Search 与 Settings
Path: crates/uc-application/src/search/、crates/uc-application/src/settings/
Responsibility: Search 查询/索引/维护；Settings 配置、诊断、存储、迁移和升级。
Relationship: Engine 仍构造 SearchRuntimeDeps 和各 Settings facade；原计划遗漏 Settings 切片。
```

```text
Component: Space
Path: crates/uc-application/src/space/ 与 crates/uc-engine/src/assembly/
Responsibility: admission、membership、anti-entropy、recovery 和 session activity。
Relationship: Application 组装泄漏到 Engine；Iroh/Infra adapter 和 observability decorator 仍应由 Engine 选择。
```

当前证据：`deps.rs` 有约 111 个 `Arc<dyn Port>`；Engine 有 6 个接收 `&AppDeps` 的分发函数，并直接
构造 `CaptureClipboardUseCase`、`ApplyInboundClipboardUseCase`、`InboundReceiveAttemptDeps`、
`FileTransferLifecycleDeps`、`SearchRuntimeDeps`、`SpaceApplicationDeps` 和
`SpaceSessionActivityDeps`。三个 wiring-only 字段及七个 capability 已确认没有生产消费者。Clipboard
Restore 只消费宽 entry bundle 的 4/11 和 representation bundle 的 2/4；单独缩窄会形成一次性过渡
interface，因此改为随 Clipboard 对象图迁移。

# 5. Proposed Design

## Components

### ApplicationAssembly

- 职责：Engine 唯一调用的 Application 对象图构造入口。
- 输入：最终收敛后的 `ApplicationDeps`；迁移期间可内部消费 `AppDeps`，但不得同时保留两个公开 bundle。
- 输出：`ApplicationAssemblyOutput { app_facade, runtime }`；领域 owner 不向 Engine 暴露。
- 关系：实现位于 Application，通过 `uc_application::facade` 只再导出 Engine 必须理解的构造合同。

### ApplicationRuntime

- 职责：拥有应用领域启动依赖、启动失败反向回滚、任务登记和关闭顺序。
- 输入：已构造的私有领域 runtime owner，不接收 Engine task 闭包或步骤级 ports。
- 输出：保留 source 的 `ApplicationStartError` / `ApplicationShutdownError`。
- 关系：具体类型而非 trait；Engine 只调用 `start` 与 `shutdown`，不持有多个领域 handles。

### Domain assemblies

- `transfer` 复用现有 `FileTransferFacade`/`FileTransferLifecycle`，内部组装 receive、readiness、
  reconciliation、session registry、timeout 和 privacy maintenance。
- `search` 内部构造 query/index/projection/maintenance runtime。
- `settings` 内部构造 Settings、Diagnostics、Storage、ConfigMigration、Upgrade；relay 只是注入 adapter。
- `clipboard` 单例拥有 EntryIdentityCoordinator、capture/inbound 模式、active reconcile、spool 和 workers。
- `space` 内部构造 SpaceApplication/session activity；Engine 仍构造 Iroh/Infra adapter 并先装 decorator。

### Engine composition root

Engine 构造 MasterKey/SQLite/Iroh/host adapter、选择具体实现和安装持续观测 decorator。禁止构造
Application use case、coordinator、runtime deps、session 或逐步骤 port 组合。进程 supervisor、宿主
事件桥、网络 endpoint 和非 Application 资源生命周期继续留在 Engine。

## Application 内部目录预览

031 完成后的目标形状如下。目录表示稳定所有权；实施时不为尚无实现的职责创建空目录，也不借此保留
同义旧路径：

```text
crates/uc-application/src/
├─ lib.rs
├─ deps.rs                 # 被动的 adapter 输入
├─ application.rs          # 顶层 assembly 和生命周期协调
├─ facade/
│  ├─ mod.rs
│  ├─ clipboard.rs
│  ├─ transfer.rs
│  ├─ search.rs
│  ├─ settings.rs
│  └─ space.rs
├─ clipboard/
│  ├─ assembly.rs
│  ├─ runtime.rs
│  ├─ capture/
│  ├─ inbound/
│  ├─ active/
│  ├─ history/
│  └─ restore/
├─ transfer/
│  ├─ assembly.rs
│  ├─ runtime.rs
│  ├─ file/
│  └─ receive/
├─ search/
│  ├─ assembly.rs
│  └─ runtime.rs
├─ settings/
│  └─ assembly.rs
└─ space/
   ├─ assembly.rs
   ├─ runtime.rs
   ├─ lifecycle/
   ├─ admission/
   └─ membership/
```

`application.rs` 只实现顶层 `ApplicationAssembly`、`ApplicationRuntime` 和各领域生命周期的依赖协调，
不保存 Clipboard、Transfer、Search、Settings 或 Space 的业务规则。具体规则、恢复、重试和领域内部
顺序继续位于所属领域；`facade/` 只保存 Engine 需要理解的稳定命令、查询、结果和入口类型。

## Clipboard 最终 interface

当前 Engine 必须分别理解普通捕获、交互式入站和 store-only pull 的对象图。最终 Engine 只表达领域
意图，由 Clipboard module 内部选择模式：

```text
ClipboardFacade
    │
    ├─ capture(...)
    ├─ apply_inbound(...)
    ├─ restore(...)
    └─ history(...)
          │
          ▼
Clipboard 内部自行选择
    NormalCapture
    InteractiveInbound
    StoreOnlyPull
```

`EntryIdentityCoordinator`、active reconcile、spool 和 worker 生命周期均为 Clipboard 私有实现，不再
暴露给 Engine。active reconcile 的启动顺序固定为：

```text
加载持久 register
        ↓
执行 active reconcile
        ↓
允许 inbound / peer-online worker 启动
```

该顺序由 `clipboard/runtime.rs` 保证，不依赖 Engine 调用者记忆；reconcile 失败时相关 worker 不得启动。

## 明确保留在 Engine 的责任

- 创建具体 Infra adapter。
- 选择 MasterKey、数据库、Iroh 与平台能力的具体实现。
- 装配跨层 observability decorator。
- 执行 Iroh router 的宿主级注册。
- 保持 `uc-engine` 稳定公开入口和结果映射。
- 创建并注入 033 的 V3 一次性升级 adapter。
- 保证 Space 切换仍是 control-only 切换，不触发内容 rewrap。

最终深度来自调用者只表达“组装、启动、关闭、执行领域意图”。即使注入的 port 数量仍然较多，它们也
只进入唯一 assembly，不再被 Engine 的多个 builder 反复拆分和投影。

## Data Model

不新增持久化数据，只调整内存对象图：

`ApplicationAssembly` 保存尚未绑定网络的进程级领域 assembly；`ApplicationNetworkBinding` 是一次性的
Router 注册能力，Engine 只能从中取得必须注册的窄 endpoint，完成注册后消费该 binding 并交回
Application。`ApplicationRuntime` 保存最终 `AppFacade`、入站入口与全部领域 owner。三者都只存在于内存，
不序列化、不跨进程、不进入宿主合同，也不新增数据库字段。

## API / Interface

```rust
impl ApplicationAssembly {
    pub fn build(deps: ApplicationDeps) -> Self;
    pub fn assemble_network(&self, adapters: ApplicationNetworkAdapters)
        -> ApplicationNetworkBinding;
}

impl ApplicationRuntime {
    pub async fn start(
        assembly: &ApplicationAssembly,
        adapters: ApplicationAdapters,
    ) -> Result<Self, ApplicationStartError>;
    pub async fn shutdown(&self) -> ApplicationShutdownReport;
}
```

- `build` 只构造，不启动后台任务；失败时无残留任务。
- `start` 任一阶段失败时，已启动领域逆序关闭，再返回原始失败。
- 唯一所有权防止重复 shutdown；Engine 不让领域重复关闭。
- Application 错误使用 `#[source]` 或等价 typed source；禁止字符串化后丢失 source。
- 领域构造器降为 `pub(crate)`，不进入 `facade/mod.rs` 白名单。

## Workflow

### 组装与启动

1. Engine 构造平台、网络、持久化和安全 adapter。
2. Engine 在持续观测 port 外安装领域 decorator。
3. Engine 一次调用 `ApplicationAssembly::build`，再一次提交网络 adapters。
4. Application 返回一次性 `ApplicationNetworkBinding`；Engine 只读取并注册窄 endpoint，随后消费 binding
   形成最终 adapters，并调用一次 `ApplicationRuntime::start`。
5. Clipboard 先完成 active-register reconcile，再启动读取或广播该 register 的 worker。
6. 任一启动失败时反向关闭已启动 owner；回滚失败作为 typed 附加信息，不覆盖原 source。

### 正常关闭

1. Engine 停止新宿主请求，并先停止会继续触发 Application 恢复或事件工作的网络观测与 presence 转发任务。
2. Engine 调用一次 `ApplicationRuntime::shutdown`。
3. ApplicationRuntime 停止生产新工作，等待 Clipboard/Transfer 排空，再关闭 Search、Space 等领域 owner。
4. Engine 最后关闭 Iroh/Infra 网络资源。

# 6. Implementation Plan

严格按以下顺序实施。每个切片 clean cutover，并在同一提交删除旧入口。

## Slice 1：计划权威与绿色基线

**位置**：[规格 018](018-domain-oriented-application-layout.md)、计划索引、
`crates/uc-application/src/space/membership/remove_space_member.rs`、
`crates/uc-engine/src/testing/host_adapter_contract.rs`。

1. 将 018 标记为被 031 取代并移入 completed；ADR-018 保持已采纳。
2. 查明并修复两条稳定红测，不得盲改期望：
   - `removal_commits_all_local_facts_once_before_returning_success` 实际
     `Paused(LocalMemberInactive)`，期望 `Paused(PendingLocalDecision)`；
   - `membership_convergence_is_queryable_through_the_public_engine` 发出
     `QueryDeviceGroupChoices` 后实际为 `device_group_choices`，旧断言期待 `DeviceTrust`。
3. `cargo test -p uc-application --locked` 与 `cargo test -p uc-engine --all-targets --locked` 绿色后
   才开始 Slice 2。

**风险**：红测可能属于 029/030。若需改变稳定产品语义，暂停 031 并先由所属规格决策。

## Slice 2：删除 wiring inventory

**位置**：`crates/uc-application/src/deps.rs`、Engine wiring、架构检查。

删除 `SecurityPorts.current_profile`、`SecurityPorts.blob_cipher`、
`AppDeps.portable_current_space_identity`；保留真实 adapter 的 wiring 局部消费，并增加禁止回流的负向检查。

**风险**：无 Core、无行为、无持久化修改；若发现生产读取，停止并更新审计。

## Slice 3：删除死亡 capability

拆成三个原子提交：

1. admission：删除 `prepare_admission_offer` 与 `derive_admission_proof_key`；
2. pairing crypto：删除 `pin_hasher` 与 `short_code`；
3. receive state：先增加 entry delete/settlement 同时清理 receive state 的真实 SQLite contract test，再删除
   `get_artifacts`、`delete_state` 与 `purge_orphans`。

每组同步删除 Core trait、Infra adapter、Application deps、Engine wiring、exports 和专属死能力测试。

**风险**：会修改 Core；零调用不等于零隐私职责，`delete_state` 必须最后删除。

## Slice 4：深化 File Transfer assembly

**位置**：`crates/uc-application/src/transfer/`、Engine file transfer/sync assembly。

1. 复用现有 `FileTransferFacade`/`FileTransferLifecycle`，不增加第二个 facade。
2. 把 `FileTransferLifecycleDeps`、`InboundReceiveAttemptDeps`、readiness、reconciliation、session registry
   与 materializer 组合收回 Application。
3. 以 interactive receive、store-only pull、cancel 完整 intent 隐藏 begin/claim/fail/commit 顺序。
4. 删除 Engine 的步骤级 deps 和 `with_directory_receive_attempt_ports` 构造。
5. 为 store、publish、readiness、recovery 建立 typed error 映射和 `source()` 测试，删除字符串化转换。

**风险**：稳定 Engine 错误码不变，source chain 是必须保留的 interface。

## Slice 5：独立收口 Search assembly

**位置**：Application Search、Engine `assembly/search.rs` 与 runtime。

Application 内部构造 query/index/projection/maintenance runtime；删除 Engine 的
`build_search_runtime(&AppDeps)`。保留四个真实 Search capability ports。测试通过 Search interface
覆盖 query、rebuild、repair、lock/unlock 和 shutdown。

**风险**：Search 不等待 Space，不重新合并两者。

## Slice 6：补齐 Settings assembly

**位置**：Application Settings、Engine facade/wiring assembly。

1. Settings 内部构造 Settings、Diagnostics、Storage、ConfigMigration、Upgrade。
2. Engine 只注入 secure storage、config repository、relay settings 等 adapter，不再预构造
   `ConfigMigrationFacade` 或逐项保存依赖。
3. relay 具体 host/network adapter 仍由 Engine 选择；恢复、重试和一致性顺序归 Settings。
4. 收紧公开白名单，补迁移、诊断、升级与 relay recovery interface 测试。

**风险**：不把宿主网络创建移入 Application。

## Slice 7：Clipboard 垂直深化

### 7A：Coordinator、capture 与 facade 依赖

- Application 单例创建 `EntryIdentityCoordinator`，删除内部 re-export；
- 收回 normal capture 对象图；
- 同时把 Restore/History 宽 bundle 改为真实依赖，不创建 fixture 固定过渡 bundle。

### 7B：Inbound modes 与 receive intent

- 建立 normal inbound、interactive inbound、store-only pull 三个命名模式；
- 模式只表达尾部效果，公共物化/身份流程由 Clipboard 内部复用；
- 删除 Engine 的 Capture/ApplyInbound/BlobProcessing 步骤拼装。

### 7C：Clipboard runtime

- 先执行 active-register reconcile，再启动 spool、inbound、peer-online 与 sync workers；
- 失败和 shutdown timeout 保留 typed source；
- 删除孤立 wiring 字段和 Engine 对 Clipboard task 顺序的持有。

**风险**：reconcile 是安全/一致性门禁，不再保留“接入或删除”的开放选择。

## Slice 8：Space assembly（有门禁）

**前置条件**： [029](029-durable-membership-history-anti-entropy.md) 与
[030](030-membership-conflict-resolution-and-chaos-validation.md) 已完成并记录验证矩阵；030 已删除旧
payload rewrap/content-key-directory 假设，并按
[033](033-immutable-content-protection-context.md) 的稳定 profile data generation 与 control-only
Space generation 完成重基线。

1. Application factory 内部构造 `SpaceApplicationDeps` 与 `SpaceSessionActivityDeps`。
2. `SpaceApplication` 继续唯一负责 admission、membership、anti-entropy 和 recovery。
3. Engine 保留 Iroh、具体 Infra transition/recovery adapter 与 host capability 的选择。
4. Engine 先安装 032/033 observability decorator 再注入 Application；Application 不手工计时。
5. 删除 Engine 对 Space 内部 use case/runtime deps/session activity 的持有和 re-export。

**风险**：开始前重新读取热点 `sync_engine.rs`；门禁不满足时不得实施 Slice 8 或 Slice 9。

## Slice 9：最终 Application assembly/runtime 收口

1. 建立唯一 `ApplicationAssembly::build(ApplicationDeps)`；删除所有 Engine `fn ...(&AppDeps)`。
2. 建立具体 `ApplicationRuntime` 统一拥有 start、失败回滚和 shutdown。
3. Engine 只持有 `AppFacade` 与 `ApplicationRuntime`，不持有领域 handles。
4. clean cutover 删除旧 `AppDeps` 或直接收缩/重命名为唯一 `ApplicationDeps`，不保留 alias。
5. crate root 只公开 `facade` 与 `deps`；白名单不导出 use case、coordinator、runtime deps 或 session。
6. 按“Application 内部目录预览”落位 `application.rs` 和五个领域的 assembly/runtime；删除同义旧路径，
   不把领域规则上提到 `application.rs`。
7. 架构检查禁止 Engine 导入内部类型、构造领域对象图或新增 `&AppDeps` 分发函数。

**删除检查**：若删除 assembly/runtime 后，capture、receive、recovery、retry 和 lifecycle 顺序不会重新
散落到 Engine，新模块仍是浅转发层，必须继续合并或删除。

# 7. Edge Cases

```text
Scenario: Application build 中途失败。
Expected behavior: 返回保留 source 的错误，无后台任务或半初始化全局状态。
Implementation: build 阶段禁止 spawn，资源由局部所有权自然释放。
```

```text
Scenario: 第 N 个领域启动失败。
Expected behavior: 逆序关闭前 N-1 个领域，原失败保持主错误。
Implementation: Runtime 记录已启动阶段；回滚失败作为 typed 附加错误。
```

```text
Scenario: Active register 为空、损坏或 stale。
Expected behavior: 清除/修复成功后才启动 worker；存储或 OS clipboard 失败则整体启动失败。
Implementation: reconcile 是 Clipboard runtime 首个 gate，不与 worker 并发。
```

```text
Scenario: receive 取消与 commit 竞争。
Expected behavior: 只有一个终态，artifact/receive state 按事务 contract 清理。
Implementation: intent owner 内部持有 begin/claim/cancel/fail/commit 顺序。
```

```text
Scenario: shutdown 时仍有 spool、transfer 或 search maintenance。
Expected behavior: 停止新工作后有界等待；超时返回稳定且脱敏的错误。
Implementation: ApplicationRuntime 表达领域依赖，领域 owner 自己 drain/cancel。
```

```text
Scenario: 旧 profile 或 V2 密文需要升级。
Expected behavior: 仍只由 033 一次性 upgrade owner 处理，031 不增加 rewrap。
Implementation: Space 负向检查继续禁止 payload rewrap 回流。
```

# 8. Testing Strategy

## Unit Test

- Domain assembly 使用 in-memory/test adapters，只通过领域 interface 测试。
- ApplicationRuntime 覆盖完整启动、逐阶段失败回滚、正常关闭和关闭失败 source。
- Active Clipboard 覆盖 empty/live/stale/corrupt register，并断言 worker 晚于 reconcile。
- File Transfer 覆盖完成、部分失败、取消竞争、retry、timeout、清理和重启恢复。
- Search 覆盖 query、rebuild、repair、lock/unlock、shutdown；Settings 覆盖迁移、诊断、升级和 relay recovery。

## Integration Test

- 真实 SQLite 验证 entry delete/settlement 同时清理 receive state。
- Engine 稳定入口验证 capture、inbound、store-only pull、active LWW、spool 重启和 shutdown。
- Slice 8 门禁满足后，以真实 SQLite/Iroh loopback 验证 Space 流程。
- 验证 Engine decorator 仍报告阶段/结果，Application 没有手工计时。
- 验证下层失败经 Application/Engine 后仍有 `source()` chain，公开文本脱敏。

## Regression Test

每个代码切片至少运行：

```bash
cargo metadata --locked --format-version 1
cargo test -p uc-application --locked
cargo test -p uc-engine --all-targets --locked
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

receive/clipboard/space 行为追加真实 SQLite、Iroh loopback 和 Engine 双实例测试。设备矩阵未执行项记为
“跳过”。意外新增持久字段时停止本计划，单独做安全和迁移评审。

# 9. Acceptance Criteria

* [x] 018 已作为被 031 取代的实施历史关闭，ADR-018 与 031 分别是稳定决策和完成记录入口。
* [x] 两条稳定红测先恢复绿色，后续切片从绿色 Application/Engine 基线开始。
* [x] 三个 wiring-only 字段与七个 capability 已删除，receive state 清理有真实 SQLite contract。
* [x] Engine 不再构造五个领域的内部 use case/runtime deps。
* [x] Active Clipboard reconcile 先于相关 worker，失败阻止 Application 启动。
* [x] Search 与 Space 分开迁移，Space 满足 029/030 门禁。
* [x] Engine 继续拥有 Iroh/Infra adapter 和 observability decorator 的选择。
* [x] 唯一 assembly 隐藏对象图，唯一具体 runtime 隐藏应用启动、回滚和关闭。
* [x] Application 目录达到目标预览；`application.rs` 只协调领域 assembly/lifecycle，不含领域业务规则。
* [x] Clipboard 三种模式由领域内部选择，EntryIdentityCoordinator、reconcile、spool 和 workers 不向 Engine 暴露。
* [x] Application 白名单不包含内部 use case、coordinator、runtime deps 或 session。
* [x] 下层错误不再字符串化丢失 source，公开错误仍脱敏。
* [x] 旧路径、过渡 bundle、alias 和只锁定内部顺序的测试同轮删除。
* [x] 架构检查阻止 `&AppDeps` 分发、内部导入和 Engine 领域对象图构造。
* [x] 031 规定门禁通过；设备、Desktop 与发布矩阵未执行项明确为“跳过”。

## 完成验证记录（2026-09-02）

- `cargo test --workspace --all-targets --locked` 在最终树通过：Application 733、Engine 131、Infra 771
  项通过且 4 项按既有标记 ignored，三端绑定及其余 workspace suites 同步通过；依赖防火墙 34 项通过，
  覆盖顶层 owner、composition root、一次性网络 binding 与公开白名单。LAN compatibility 179 项通过，Engine
  `lan-compat` all-targets feature check 通过。
- 029/030 的 13 个非忽略 Engine SQLite/Iroh 场景均取得本轮通过证据：029 admission 重启传输、既有设备
  切换、新设备稳定加入、F0-F7 及两项 topology 场景。F6 首次暴露关闭时网络观测任务晚于 Application
  停止，以及刷新调用成功不等于目标 peer 已可用；修正关闭顺序并在发送前等待成对可达后独立通过。
  F7 两次在十节点高负载下并发签发三张邀请时因 admission gate 尚在维护而返回可重试
  `invalid_state`；按场景语义改为带节点诊断的顺序签发后独立通过（413.27 秒），没有延长等待时间或
  放宽断言。F4 的一次受污染组合结果同样未作为通过证据，干净独立进程通过。
- 回归发现 V3 runtime manifest 被 V2-only loader 误判为损坏，导致既有设备第二次加入返回
  `RecoveryRequired`。Joiner source snapshot 改为识别 V2/V3 runtime；V2/无 manifest 继续使用原 V1
  字节编码，V3 使用包含 profile data/control generation 的 V2 snapshot，并新增两项真实 manifest 回归。
- 显式 ignored 的两设备 1 秒热路径性能门禁已按 `--ignored` 在最终树执行，实测 8.016 秒，未达到
  1 秒目标，明确记为**未通过**；本计划没有放宽阈值或把该结果写成绿色。
- 当前相邻 Desktop 仓库位于 `v0` checkout，缺少 029/030 的
  `tests/e2e/tests/membership_convergence.rs` harness，因此本轮 C0-C5 记为**跳过**。029/030 完成计划保存的
  最终源码历史证据仍为 8/8 通过，但不冒充本轮复验。
- 实体设备矩阵与 Release bundle 本阶段未提供，均记为**跳过**，未记为“通过”。

# 10. Risks and Trade-offs

- `sync_engine.rs`/`runtime/mod.rs` 冲突高：按领域顺序、Search/Space 拆分和 clean cutover 降低冲突。
- assembly 可能退化成 wrapper：以删除检查和 Engine import 检查判断，不用实现行数判断 depth。
- interface 测试会替代步骤 mock：新覆盖建立后删除旧浅测试，不永久叠加两套。
- 集中 runtime 可能改变生命周期：先固定可观察结果与资源依赖，逐阶段测试失败回滚。
- typed source 可能扩大内部类型传播：稳定 Engine 分类不变，内部用 `#[source]` 连接，公开消息脱敏。
- 只缩小 `AppDeps` 仍让 Engine 理解对象图；万能 Application port 会合并真实 seams；多个 lifecycle handle
  仍让 Engine 排序。三种替代方案均拒绝。

# 11. Open Questions

没有未决架构选择。两条红测根因需要在 Slice 1 调查，但它们是实施门禁。若需改变稳定产品语义，必须
暂停 031，并在对应成员规格中先取得明确决策。

## 相关文档

- [ADR-018](../../design-docs/decisions/018-domain-oriented-application-layout.md)
- [已被取代的规格 018](018-domain-oriented-application-layout.md)
- [端口设计](../../design-docs/ports.md)
- [Application 分层规则](../../design-docs/layers/application.md)
- [错误处理与转换](../../design-docs/error-handling.md)
- [文件传输 port 拆分 ADR](../../design-docs/decisions/009-file-transfer-port-split.md)
- [029 持久化成员历史反熵](029-durable-membership-history-anti-entropy.md)
- [030 成员分叉选择与复杂拓扑验证](030-membership-conflict-resolution-and-chaos-validation.md)
- [033 不可变内容保护上下文](033-immutable-content-protection-context.md)
