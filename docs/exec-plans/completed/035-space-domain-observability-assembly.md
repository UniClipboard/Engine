# 规格 035：Space 观测装配 interface 收敛与仓库推广准则

## 状态

- **状态**：已完成
- **日期**：2026-09-03
- **前置规格**：[029 持久化成员历史反熵](../completed/029-durable-membership-history-anti-entropy.md)、[030 成员分叉选择与复杂拓扑验证](../completed/030-membership-conflict-resolution-and-chaos-validation.md)、[031 Application 依赖表面深化](../completed/031-application-dependency-surface-deepening.md)
- **完整负责人**：`uc-engine::assembly::observability` 唯一负责跨层依赖调用的持续计时、结果分类、降噪 policy 与安全事件 schema；`uc-application` 只拥有并消费领域 adapter bundle
- **调用方唯一动作**：Engine 在每个真实装配 seam 构造一个 Application 定义的 adapter bundle，并调用一次该 seam 的主要观测入口；不得逐 port 取回观测结果或自行选择 policy，也不得为了凑成宽泛业务领域的单次调用而改变启动顺序
- **成功结果**：返回与输入相同 Application interface 的已装饰 bundle；每个被批准的依赖调用产生符合 policy 的固定结构化事件，业务结果保持不变
- **失败结果**：观测本身不返回失败且不得改变业务失败；底层 port 的原始结果、错误类型和 source chain 原样返回
- **重试与重启责任**：继续由原 Application 完整负责人和持久状态承担；decorator 不保存业务状态、不重试、不恢复、不改变调用顺序

# 1. Overview

`crates/uc-engine/src/assembly/observability/admission.rs` 已证明 port decorator 能把准入依赖调用的计时和
结果分类从 Application 移到 Engine composition root。它还覆盖了 transport 返回 authenticated exchange 后的继续
包装，并用领域 operation、显式 policy 和固定 schema 避免记录邀请、身份、地址、凭据或密钥。

当前实现不适合作为原样复制的模板。`AdmissionPortImplementations` 与 `ObservedAdmissionPorts` 镜像同一组七个
port，另有独立 `observe_session_transition` 入口；`sync_engine.rs` 构造输入后还要逐字段取回输出。模块集中隐藏了
decorator 构造，却没有收窄调用者必须理解的 wiring interface。每增加一个被观测 port，都要同时修改输入 struct、
输出 struct、assemble 函数和调用方字段投影。

同时，Application 已定义一个包含准入、成员账本、反熵、冲突恢复和 group update 的扁平
`SpaceRuntimeAdapters`。它是真实消费者依赖清单，却没有按 admission 与 membership 责任分组。若继续在 Engine
为每个领域复制一对 raw/observed bundle，会产生第三套依赖清单，并把浅 wiring pattern 推广到其他领域。

本规格先修正参考实现，再推广到 Space membership。Application 提供消费者拥有的
`SpaceAdmissionAdapters` 和 `SpaceMembershipAdapters`；这两个 bundle 恰好都在 `sync_engine.rs` 的同一网络装配阶段完整，
因此 Engine 对每个 bundle 只公开一个主要观测入口，入口按值接收并返回同一 bundle。具体 decorator、operation enum、
policy 和 schema 映射全部私有。V1 保留调用级阶段耗时，但只观测 Application 直接调用的 port，不观测 Infra adapter
内部的嵌套调用。

该形状不能机械推广为“一个宽泛业务领域只能调用一次 `observe_<domain>`”。Clipboard 与 Blob 的真实依赖分别在
`wire_dependencies` 的进程装配阶段和 Iroh network binding 阶段出现；Application 已在
`ApplicationAssembly::assemble_network` 内部汇合两批依赖。该汇合点存在，但不属于 Engine decorator seam。为了观测而
延迟 Application 构造、让 Engine 保留可绕过 decorator 的 raw `Arc`，或向 Application 注入 recorder，都会扩大 interface
并破坏 locality。本规格因此把可推广规则定义为“每个真实装配 seam 一个主要入口”，并记录其他领域的后继路线，
不把 Clipboard/Blob 的两阶段对象图重整纳入本次实施。

# 2. Goals

- 用 Application 拥有的 `SpaceAdmissionAdapters` 取代 Engine 私有的两份镜像准入 bundle。
- 用 Application 拥有的 `SpaceMembershipAdapters` 收拢成员账本与网络能力，并作为 membership 观测的唯一输入输出
  interface。
- 让 `assembly::observability` 对 Space admission 与 membership 各只暴露一个主要观测入口；将仓库级规则固定为每个真实
  装配 seam 一个入口，而不是每个宽泛业务领域强制一个入口。
- 保留准入 recovery state、transport、Sponsor state、Joiner candidate 和 activation 的调用级耗时与结果分类。
- 为成员账本 load/commit、历史交换、受限投递、group update 投递和两阶段分支恢复增加调用级耗时与稳定结果分类。
- decorator 对成功值、错误值、错误 source、调用次数、调用顺序和返回 port 行为完全透明。
- 所有事件只包含固定 operation/outcome/error kind、耗时和批准的计数；禁止读取或输出敏感参数及错误文本。
- 删除 `AdmissionPortImplementations`、`ObservedAdmissionPorts`、公开 decorator/policy 构造器及独立 session
  transition 观测入口，不保留兼容 alias。
- 用单元测试、port contract test 和架构检查阻止 Space 镜像 bundle、Space admission/membership 多入口、Application
  新增手工持续计时及敏感字段回流。
- 明确 Clipboard、Blob Transfer、File Transfer、Search、Settings 与运行期连接观测的后继分类，避免实施期间把不同
  seam、业务阶段计时和行为时钟混为同一种 decorator。

# 3. Non-Goals

- 不把多个领域合并成通用 `Observed<T>`、万能 middleware、字符串 phase registry 或通用事件 builder。
- 不为观测新增 Application recorder/callback port，也不要求业务调用点提交开始时间、成功布尔值或日志字段。
- 不修改 admission、membership、反熵、冲突恢复、group update 的业务状态机、重试、退避、持久化或网络顺序。
- 不修改 SQLite schema、MasterKey AEAD 载荷、wire message、ALPN、Engine 稳定公开操作或绑定合同。
- 不把 Infra adapter 的内部步骤伪装成 Application 调用级阶段；Iroh/SQLite/MLS 内部诊断继续由各 adapter 自己负责。
- 不在本规格迁移 Clipboard、Blob Transfer、File Transfer、Search、Settings 或 LAN compatibility；本规格只记录它们
  应从哪个真实 seam 起步及进入后继规格前必须满足的证据。
- 不为实现单个 `observe_clipboard` 或 `observe_blob_transfer` 而延迟 `ApplicationAssembly::build`、搬迁
  `ApplicationAssembly::assemble_network`、改变进程后台启动顺序，或在 Engine 长期保留 raw adapter clone。
- 不移除产品 analytics、一次性故障诊断日志或用于 deadline、timeout、cooldown、退避的行为时钟。
- 不改变 `uc-observability-contract` 的宿主 sink、过滤、文件层或 OTLP exporter 行为。

# 4. Current Architecture Context

```text
Module: Admission observability assembly
Path: crates/uc-engine/src/assembly/observability/admission.rs
Responsibility: 装饰七个准入 port，并单独装饰 AdmissionSpaceTransitionPort。
Relationship: 具体 decorator 行为正确，但 raw/observed 镜像 bundle 与多个入口形成偏宽的 Engine 内部 interface。
```

```text
Module: Engine Space composition root
Path: crates/uc-engine/src/assembly/sync_engine.rs
Responsibility: 选择 SQLite、Iroh、密码与 Application port 的具体 adapter，并组装 Space runtime。
Relationship: 当前逐项构造 AdmissionPortImplementations，再逐字段投影 ObservedAdmissionPorts 到 SpaceRuntimeAdapters。
```

```text
Module: Space runtime adapter interface
Path: crates/uc-application/src/space/application.rs
Responsibility: 定义 Engine 提交给 SpaceApplication 的网络、安全和持久化能力。
Relationship: 当前 SpaceRuntimeAdapters 扁平包含 admission 与 membership 数十个字段，是消费者依赖的唯一事实来源，
但领域分组不足。
```

```text
Module: Space admission owner
Path: crates/uc-application/src/space/admission/
Responsibility: Joiner、Sponsor、recovery 三条流程及其稳定 port interface。
Relationship: Application 直接调用 recovery/state/transport/preparation/executor port；这些调用是准入持续观测的合法 seam。
```

```text
Module: Space membership owner
Path: crates/uc-application/src/space/membership/
Responsibility: 加密 ledger、历史反熵、受限投递、group update、effects、冲突选择与恢复。
Relationship: Application 直接调用 ledger 与 membership transport ports；当前没有统一 Engine decorator。
```

```text
Module: Runtime observability design
Path: docs/design-docs/observability.md
Responsibility: 固定 Engine 领域 port decorator、显式 policy、固定 schema 和隐私规则。
Relationship: 已禁止 Application 手工持续计时和跨领域通用 observer，但尚未规定 Application-owned bundle 与单入口约束。
```

```text
Module: Application process assembly
Path: crates/uc-engine/src/assembly/wire/mod.rs、crates/uc-application/src/application.rs、crates/uc-application/src/clipboard/assembly.rs
Responsibility: Engine 先创建 transfer cipher、存储、系统剪贴板等进程期 adapter，再以 ApplicationDeps 构造 ApplicationAssembly。
Relationship: ClipboardAssembly 在 Iroh adapter 出现前已持有并使用这些能力；这是早期进程 adapter seam。
```

```text
Module: Application network binding
Path: crates/uc-engine/src/assembly/sync_engine.rs、crates/uc-application/src/application.rs
Responsibility: Engine 在 Iroh builder 上创建 clipboard dispatch/receiver、active clipboard 与 blob transfer adapter，再一次提交 ApplicationNetworkAdapters。
Relationship: ApplicationAssembly::assemble_network 在 Application 内部把晚期网络 adapter 与先前 ApplicationDeps 汇合；
该方法是 Application 对象图的组装 interface，不是 Engine 可以取得完整 raw Clipboard/Blob bundle 的 decorator seam。
```

```text
Module: Existing Clipboard and Blob timing
Path: crates/uc-application/src/clipboard/inbound/runtime.rs、crates/uc-application/src/clipboard/sync/dispatch_entry/per_peer.rs、crates/uc-application/src/transfer/blob/publish_blob.rs
Responsibility: 当前分别记录队列/策略/解密/解码/应用、单 peer dispatch，以及 hash/publish/save reference/ticket 等阶段。
Relationship: 其中 adapter 调用耗时可在对应 Engine seam 的后继规格中 clean cutover；纯 Application 计划、策略、解码和
端到端业务耗时没有等价 Engine port，不得通过虚构 recorder port 迁移。
```

```text
Module: Architecture preflight
Path: scripts/architecture/check-engine-repository.mjs
Responsibility: 检查依赖方向、Application/Space interface、敏感 tracing 字段和 retired implementation 回流。
Relationship: 需要新增观测 interface 形状和旧镜像类型的负向门禁。
```

当前准入数据流：

```text
Engine concrete adapters
    → AdmissionPortImplementations
    → ObservedAdmissionPorts::assemble
    → 逐字段取回
    → SpaceRuntimeAdapters
    → SpaceApplication
    → admission services
```

目标数据流：

```text
Engine concrete adapters
    → Application-owned SpaceAdmissionAdapters
    → observe_admission（一次）
    → SpaceRuntimeAdapters.admission
    → SpaceApplication
```

当前 Clipboard/Blob 两阶段数据流：

```text
Engine wire phase
    → transfer_cipher / storage / system clipboard 等早期 adapter
    → ApplicationDeps
    → ApplicationAssembly::build
    → ClipboardAssembly 已持有早期能力

Engine network phase
    → clipboard_dispatch / receiver / active clipboard / blob_transfer 等晚期 adapter
    → ApplicationNetworkAdapters
    → ApplicationAssembly::assemble_network
    → Application 内部与早期能力汇合为 ClipboardSyncFacade / BlobTransferFacade
```

后继观测不得把该图伪装为一个 Engine 侧完整 `ClipboardAdapters`。应在早期和晚期各自真实 seam 上使用独立的
Application-owned bundle；同一底层调用只允许由一个 decorator 或一个既有业务计时 owner 记录，禁止重复计时。

# 5. Proposed Design

## Components

### Application-owned Space adapter bundles

- **位置**：新增 `crates/uc-application/src/space/adapters.rs`，由 `crates/uc-application/src/space/mod.rs` 精确导出类型。
- **职责**：作为 Engine 到 `SpaceApplication` 的被动配置 interface，保存真实消费者所需的 port；不构造 use case、
  不选择 adapter、不记录事件。
- **输入**：Engine 选择的具体 adapter 或已擦除为 `Arc<dyn Port>` 的能力。
- **输出**：`SpaceRuntimeAdapters { admission, membership }`。
- **关系**：Application 是 bundle 的唯一 owner；Engine observability 只转换 bundle，不定义镜像类型。

目标类型：

```rust
pub struct SpaceRuntimeAdapters {
    pub admission: SpaceAdmissionAdapters,
    pub membership: SpaceMembershipAdapters,
}

pub struct SpaceAdmissionAdapters {
    // 当前 SpaceRuntimeAdapters 中只被 admission/query-current-join 消费的字段
}

pub struct SpaceMembershipAdapters {
    // 当前 SpaceRuntimeAdapters 中只被 membership/maintenance 消费的字段
}
```

`SpaceAdmissionAdapters` 包含当前 joiner、sponsor、recovery 和 current-join projection ports；
`SpaceMembershipAdapters` 包含 ledger、签名、成员初始化、历史交换、受限投递、group update、effects、冲突恢复、
cleanup 和 membership network activity ports。字段只移动，不改 trait interface。

跨两个流程使用的 capability 按实际 owner 只出现一次：例如 `current_join_status` 归 admission bundle，
`SpaceApplication` 在构造 membership query 时从该 bundle 取得；不得在 membership bundle 再复制一个 Arc 字段。

### Admission observation module

- **位置**：`crates/uc-engine/src/assembly/observability/admission.rs`。
- **职责**：装饰 `SpaceAdmissionAdapters` 中已批准的 Application 直接依赖调用。
- **输入**：完整 `SpaceAdmissionAdapters`。
- **输出**：字段和类型完全相同的 `SpaceAdmissionAdapters`。
- **关系**：所有 `ObservedX`、operation、policy、record 函数均为模块私有。

模块向父模块只提供：

```rust
pub(super) fn observe_admission(
    adapters: SpaceAdmissionAdapters,
) -> SpaceAdmissionAdapters;
```

`observability/mod.rs` 精确重导出：

```rust
mod admission;
pub(crate) use admission::observe_admission;
```

不得再导出 `ObservedAdmissionPorts`、`AdmissionPortImplementations`、具体 decorator 或 policy。

### Membership observation module

- **位置**：新增 `crates/uc-engine/src/assembly/observability/membership.rs`。
- **职责**：为成员关键持久化与认证网络调用提供调用级耗时、稳定结果分类和降噪。
- **输入**：完整 `SpaceMembershipAdapters`。
- **输出**：字段和类型完全相同的 `SpaceMembershipAdapters`。
- **关系**：与 admission 相同，仅通过 `observability/mod.rs` 暴露一个 `observe_membership`。

V1 装饰以下 port：

- `LoadMembershipLedgerPort::load`
- `CommitMembershipLedgerPort::compare_and_commit`
- `MembershipHistoryExchangePort::exchange_membership_history`
- `RestrictedMembershipDeliveryPort::deliver_restricted_membership`
- `GroupUpdateDispatchPort::dispatch_group_update`
- `MembershipBranchRecoveryChannelPort::request_membership_branch_group_info`
- `MembershipBranchRecoveryChannelPort::submit_membership_branch_external_commit`

其余 membership 字段原样透传。未来增加 decorator 时只修改 `membership.rs`，不增加第二个入口或第二个 bundle。

### Seam-oriented rollout map

本规格的生产改动只覆盖 Space，但后继规格必须按下表选择 seam。表中的“候选”不是已批准事件清单；只有明确诊断问题、
固定 operation/error kind、降噪 policy、隐私字段和旧观测 clean-cutover 后，才能进入实现。

| 范围 | 当前真实 seam | 后继方向 | 本规格决定 |
| --- | --- | --- | --- |
| Space admission | `sync_engine.rs` 构造完整 `SpaceAdmissionAdapters` | `observe_admission` | 本次实现 |
| Space membership | `sync_engine.rs` 构造完整 `SpaceMembershipAdapters` | `observe_membership` | 本次实现 |
| Clipboard process | `wire/mod.rs` 构造 `ApplicationDeps` 中的密码、存储和系统能力 | 建立只含被批准调用的 Application-owned process bundle，并在交付 Application 前装饰 | 后继规格；不等待网络 adapter |
| Clipboard transport | `sync_engine.rs` 构造 `ApplicationClipboardAdapters` | 优先从 `ClipboardDispatchPort::dispatch` 及其既有 `DispatchTiming` 建立 transport 观测；receiver 的 `subscribe` 不作为耗时阶段 | 后继规格；不与 process seam 强行合并 |
| Blob transport | `sync_engine.rs` 构造 `blob_transfer`、`blob_reference` 与 progress reporter | 建立 Application-owned network bundle，评估 publish/fetch/ticket/reference 调用级观测并删除等价手工计时 | 后继规格；hash/cipher 仍归早期 seam |
| File Transfer | 主要经 Application facade 复用 Blob 与持久化 owner | 先证明独有的 port-call 诊断缺口，避免重复记录底层 Blob 调用 | 暂不新增 decorator |
| Search / Settings | 进程期 repository 与配置 seam | 仅在出现明确慢调用或失败分类需求后建立领域 bundle | 当前无持续调用级需求 |
| Connectivity / runtime | deadline、timeout、cooldown、retry、backpressure 生命周期 | 保留为行为时钟和运行期状态事件 | 不归 port decorator 推广 |

后继命名表达 seam，而不是假装整个业务领域只有一个装配时点，例如可使用 `observe_clipboard_transport`；具体命名由后继
规格按最终 Application-owned bundle 决定。本规格不预先创建空模块、占位函数或只有一个 adapter 的浅 interface。

### Observation interface architecture check

- **位置**：`scripts/architecture/check-engine-repository.mjs`。
- **职责**：固定单入口与可见性，不解析业务事件内容。
- **检查**：
  - 禁止 `AdmissionPortImplementations` 和 `ObservedAdmissionPorts` 回流；
  - `observability/mod.rs` 对 Space admission 与 membership 分别只能精确重导出一个主要入口；
  - `admission.rs`、`membership.rs` 不得出现 `pub(crate) struct Observed`、公开 policy 或公开构造器；
  - `sync_engine.rs` 必须分别且仅调用一次 `observe_admission`、`observe_membership`；
  - 禁止 `SpaceRuntimeAdapters` 回到 admission/membership 字段混排；
  - 负向 fixture 证明为 Space admission/membership 新增第二入口、镜像 bundle 或公开 decorator 会失败。

架构检查不禁止未来同一宽泛业务领域在两个真实 seam 各有一个入口；它检查的是单个 bundle 是否有唯一转换点、raw adapter
是否还能绕过 decorator，以及具体 implementation 是否泄漏给调用方。

## Data Model

本规格不新增持久数据。新增或调整的均为进程内装配类型。

### Admission operation

保留固定操作值：

- `recovery_state_load`
- `recovery_state_commit`
- `admission_channel_establish`
- `admission_message_exchange`
- `sponsor_state_load`
- `sponsor_state_commit`
- `joiner_candidate_prepare`
- `joiner_activation_prepare`
- `joiner_activation_state_load`
- `joiner_activation_state_commit`
- `joiner_activation_execute`

当前 `space_session_transition_preflight`、`space_session_transition_prepare`、
`space_session_transition_advance`、`space_session_transition_discard` 不再作为跨层 stage。它们是 Infra preparation/executor
内部的嵌套调用，不是 Application 直接调用的 port：前两者计入 `joiner_activation_prepare`，后两者计入
`joiner_activation_execute`。这避免同一工作被外层和内层重复计时，也使 admission 保持单次装饰入口。

### Membership operation

固定操作值：

- `membership_ledger_load`
- `membership_ledger_commit`
- `membership_history_exchange`
- `restricted_membership_delivery`
- `group_update_dispatch`
- `branch_recovery_group_info`
- `branch_recovery_external_commit`

### Event schema

领域 target 固定为：

- admission：`admission.performance`；
- membership：`membership.performance`。

不得由配置或调用参数覆盖 target，也不得把两个领域写入同一个通用 target。

所有事件包含：

- `operation`: 私有固定 enum 的稳定字符串；
- `elapsed_ms`: 本地单调时钟耗时，使用饱和/检查转换到 `u64`；
- `outcome`: `ok` 或 `error`。

适用时增加：

- `error_kind`: 从 typed error variant 穷举映射，禁止 `Debug`、`Display` 或 `to_string()`；
- `request_kind` / `response_kind`: membership history 的固定消息 variant 名；
- `loaded_count`: admission recovery 成功非空 load 的数量。

Membership 每个 operation 的字段合同如下。表中未列出的字段不得出现在对应事件：

| operation | 必填字段 | 成功时增加 | 失败时增加 |
| --- | --- | --- | --- |
| `membership_ledger_load` | `operation`, `elapsed_ms`, `outcome` | 无 | `error_kind` |
| `membership_ledger_commit` | `operation`, `elapsed_ms`, `outcome` | 无 | `error_kind` |
| `membership_history_exchange` | `operation`, `elapsed_ms`, `outcome`, `request_kind` | `response_kind` | `error_kind` |
| `restricted_membership_delivery` | `operation`, `elapsed_ms`, `outcome` | 无 | `error_kind` |
| `group_update_dispatch` | `operation`, `elapsed_ms`, `outcome` | 无 | `error_kind` |
| `branch_recovery_group_info` | `operation`, `elapsed_ms`, `outcome` | 无 | `error_kind` |
| `branch_recovery_external_commit` | `operation`, `elapsed_ms`, `outcome` | 无 | `error_kind` |

`request_kind` 与 `response_kind` 只允许以下固定值，并通过匹配
`MembershipHistoryMessage` variant 产生：

- `summary_v3`
- `request_suffix_v3`
- `suffix_page_v3`
- `ack_v3`
- `restricted_event_v3`
- `restricted_decision_v3`
- `request_conflict_evidence_v3`
- `conflict_evidence_v3`

失败的 history exchange 没有 `response_kind`；成功事件没有 `error_kind`。实现必须为每种 operation 使用明确的
record 函数或等价的具体字段表达，禁止用接收任意可选字段 map 的通用 event builder。

Membership error kind 固定为：

- ledger：`locked`、`conflict`、`corrupt`、`unavailable`、`recovery_required`；
- history/group update：`offline`、`rejected`、`transport`；
- restricted delivery：`deferred`、`rejected`；
- branch recovery：`unavailable`、`rejected`、`invalid`。

禁止事件包含 port 参数、错误文本、source 文本、DeviceId、MemberInstanceId、AdmissionId、branch/conflict/transition id、
endpoint、地址、邀请、凭据、ledger revision/digest、消息字节、密钥、文件名或路径。

### Observation policy

- admission recovery 和 activation state load：保留“成功空结果不记录，非空成功与错误记录”。
- membership ledger load：错误全部记录；成功只在 `elapsed_ms >= 50` 时记录，避免正常查询淹没日志。
- membership ledger commit：全部记录。
- membership 网络调用：全部记录，因为每次都对应实际待处理工作。
- policy 常量只存在于各领域模块内部；V1 不从配置文件、环境变量或调用参数动态改变。

50ms 是诊断降噪阈值，不是业务 timeout、SLO 或失败条件；调用无论是否记录都原样完成。

## API / Interface

本规格实施后 Engine 其余模块可见的 Space 观测 interface：

```rust
pub(crate) fn observe_admission(
    adapters: SpaceAdmissionAdapters,
) -> SpaceAdmissionAdapters;

pub(crate) fn observe_membership(
    adapters: SpaceMembershipAdapters,
) -> SpaceMembershipAdapters;
```

约束：

- 两个函数同步、无失败、无副作用任务、无全局注册。
- bundle 按值传入，避免 raw/observed Arc 同时留在调用方。
- 未被装饰的字段直接移动到输出，不 clone 出第二套长期引用。
- decorator 方法先取 `Instant::now()`，调用 inner 一次，按 policy 记录，再原样返回结果。
- 不捕获 panic，不把日志 sink 失败转换成业务错误。
- 返回另一个 port 的方法必须包装返回 port；准入 transport 的 authenticated exchange 继续遵守该规则。
- “一个入口”以一次形成并交付的 Application-owned bundle 为范围。若一个业务领域跨越多个装配阶段，不得把它扩大为
  跨阶段 raw adapter registry，也不得为了函数名统一改变 Application 生命周期。

## Workflow

### Engine 启动装配

1. Engine 构造 admission 和 membership 的真实 SQLite、Iroh、密码及平台 adapter。
2. Engine 分别构造一次 `SpaceAdmissionAdapters` 与 `SpaceMembershipAdapters`。
3. Engine 分别调用一次 `observe_admission` 与 `observe_membership`，立即移动 raw bundle。
4. Engine 构造 `SpaceRuntimeAdapters { admission, membership }` 并一次提交 Application。
5. Application 解构两个 bundle，组装 admission protocol、membership owner 与 maintenance runtime。

### 一次被观测调用

1. Application owner 按原业务流程调用领域 port。
2. decorator 记录单调开始时间并调用 inner 一次。
3. inner 返回原成功值或原错误。
4. decorator 只从稳定 variant/计数决定是否记录事件。
5. decorator 返回未经转换的原结果；Application 继续原重试、提交或失败路径。

# 6. Implementation Plan

## Step 1：建立 Application-owned 领域 bundle

**文件**：`crates/uc-application/src/space/adapters.rs`、`crates/uc-application/src/space/mod.rs`、
`crates/uc-application/src/space/application.rs`、相关测试 fixture。

**修改**：

1. 新增 `SpaceAdmissionAdapters`、`SpaceMembershipAdapters`。
2. 将 `SpaceRuntimeAdapters` 收敛为两个领域字段。
3. `SpaceApplication::build_from_deps` 按领域解构，保持每个 port 的唯一消费位置。
4. 更新 `build_for_test` fixture，只构造 Application 定义的 bundle。

**风险**：字段误归属可能产生重复 Arc 或跨领域 clone。以实际消费者为准，同一 capability 只在一个 bundle 出现。

## Step 2：收紧 admission 参考实现

**文件**：`crates/uc-engine/src/assembly/observability/admission.rs`、
`crates/uc-engine/src/assembly/observability/mod.rs`、`crates/uc-engine/src/assembly/sync_engine.rs`。

**修改**：

1. 先用 contract tests 固定现有成功、失败、空 load、authenticated exchange 继续包装和结果透明性。
2. 改为 `observe_admission(SpaceAdmissionAdapters) -> SpaceAdmissionAdapters`。
3. 将所有 decorator、policy、operation 和 helper 降为模块私有。
4. 删除两份镜像 struct 与 `observe_session_transition`。
5. 删除 transition 嵌套事件；保留外层 activation prepare/execute 完整耗时。
6. `sync_engine.rs` 只保留一次 admission observe 调用。

**风险**：transition 指标名会 clean cutover。仪表盘或日志查询必须在同一切片改用 activation 操作，不能长期兼容双写。

## Step 3：实现 membership decorator

**文件**：新增 `crates/uc-engine/src/assembly/observability/membership.rs`，修改
`crates/uc-engine/src/assembly/observability/mod.rs`、`crates/uc-engine/src/assembly/sync_engine.rs`。

**修改**：

1. 为七个批准方法实现具体 decorator。
2. 为 message/error variant 编写穷举固定映射，不格式化 payload 或 source。
3. 实现 slow-success ledger load policy 与其边界测试。
4. 一次转换完整 `SpaceMembershipAdapters`，其余字段按值透传。
5. 在 Engine 构造完真实 membership adapters 后、提交 Application 前调用一次。

**风险**：membership maintenance 调用频率高；必须先验证降噪 policy，禁止以删除错误事件来降低量级。

## Step 4：增加事件与透明性 contract tests

**文件**：两个 observability 领域模块的 `cfg(test)`，必要时新增
`crates/uc-engine/src/assembly/observability/test_support.rs`，但只共享捕获 tracing event 的测试工具，不共享生产
operation/schema/policy。

**修改**：

1. 用可识别成功对象验证返回值未重建或降级。
2. 用带 source 的测试错误验证 `source()` 链和指针/分类保持。
3. 验证 inner 每次只调用一次，调用顺序不变。
4. 验证成功空 load 降噪、50ms 阈值两侧、所有错误必记。
5. 验证 authenticated exchange 返回 port 继续被包装。
6. 捕获事件并断言允许字段全集；向 fake 输入注入敏感哨兵字符串，断言日志中不存在。

**风险**：全局 tracing subscriber 测试可能相互干扰；使用 scoped default subscriber，并串行化仅确有全局冲突的测试。

## Step 5：架构与文档 clean cutover

**文件**：`scripts/architecture/check-engine-repository.mjs`、`docs/design-docs/observability.md`、
`docs/architecture/architecture-bible.md`、本规格与计划索引。

**修改**：

1. 增加 Space 单入口、私有 implementation、Application-owned bundle 和 retired marker 检查及负向 fixture。
2. 设计文档补充“每个真实装配 seam 一个主要入口、同型 bundle 转换、只观测 Application 直接 port 调用”，并写明
   跨阶段领域不得为观测制造人工汇合点。
3. 更新架构圣经当前状态和文档维护记录。
4. 完成验收后将本规格移入 `docs/exec-plans/completed/`，更新两侧索引，不保留 active 副本。

**风险**：文本检查易误报测试代码；检查应基于明确文件与 production 可见性 marker，并为允许/拒绝 fixture 各建一例。

# 7. Edge Cases

```text
Scenario: admission recovery 或 activation state 成功返回空结果。
Expected behavior: 不记录成功事件；业务结果原样返回。
Implementation: 由 admission 私有 policy 判断，错误永远不被空结果规则抑制。
```

```text
Scenario: membership ledger load 成功且耗时恰好 50ms。
Expected behavior: 记录事件。
Implementation: 使用 `elapsed >= Duration::from_millis(50)`，边界测试固定等于阈值的行为。
```

```text
Scenario: monotonic duration 超出 u64 毫秒表达范围。
Expected behavior: 事件耗时饱和为 u64::MAX，业务结果不受影响。
Implementation: `u64::try_from(as_millis()).unwrap_or(u64::MAX)`；生产代码不得 `unwrap()`。
```

```text
Scenario: inner port 返回带敏感文本的错误 source。
Expected behavior: 原错误及 source chain 返回给 Application，但事件只记录固定 error_kind。
Implementation: 不使用 `%error`、`?error`、Display、Debug 或 `to_string()`。
```

```text
Scenario: membership history 请求成功但回复 variant 与请求不同。
Expected behavior: 同一事件分别记录固定 request_kind 和 response_kind，不记录消息内容。
Implementation: 对请求和成功回复各做一次只匹配 enum variant 的纯映射。
```

```text
Scenario: admission transport 建链失败。
Expected behavior: 记录 establish error；不构造 exchange wrapper；原错误返回。
Implementation: 只在 `Ok(inner)` 时包装返回 port。
```

```text
Scenario: admission transport 建链成功，后续 exchange 失败。
Expected behavior: establish 与 exchange 各记录一次；exchange 原错误返回。
Implementation: 返回具体的私有 observed exchange port，并继承同一私有 policy。
```

```text
Scenario: 多个调用并发执行同一 decorator。
Expected behavior: 每次调用独立计时，无共享可变业务状态、锁或顺序耦合。
Implementation: decorator 只保存 immutable inner Arc 和 Copy policy。
```

```text
Scenario: tracing subscriber 未安装、过滤事件或 sink 写入失败。
Expected behavior: port 调用结果不变，Engine 不因观测失败退出或重试。
Implementation: 只使用 tracing 宏，不等待 sink、不把 sink 状态加入返回值。
```

```text
Scenario: 旧版节点参与 membership wire 交换。
Expected behavior: wire 兼容和业务结果与改动前一致。
Implementation: decorator 不编码、解码或修改消息；兼容性继续由 Iroh adapter contract 测试。
```

```text
Scenario: 后继 Clipboard/Blob 观测需要的 adapter 分别在进程期和网络期产生。
Expected behavior: 两个真实 seam 独立装饰；不延迟 Application 构造，不保留可绕过装饰的 raw clone，也不重复记录同一调用。
Implementation: 后继规格分别定义 Application-owned process/network bundle，并为既有 Application 手工阶段计时列出保留、
迁移或删除的逐项清单。
```

# 8. Testing Strategy

## Unit Test

- **输入**：成功空/非空 admission load、49/50/51ms membership load、每个 typed error variant。
- **操作**：直接调用私有 policy 与固定 mapping。
- **预期**：记录决策和字符串分类完全符合本规格；映射对 enum variant 穷举，无 wildcard 吞掉新增 variant。

- **输入**：每种 `MembershipHistoryMessage` variant。
- **操作**：调用 message-kind mapper。
- **预期**：只返回固定 variant 名，不访问或格式化内部字段。

## Integration Test

- **输入**：实现目标 port 的 capturing fake，返回带唯一标记的成功值或带 source 的失败。
- **操作**：构造完整 Application-owned bundle，调用一次 `observe_<domain>`，再通过返回 port 执行。
- **预期**：inner 调用一次；输入未经修改；成功值、错误 variant/source 原样返回；事件 operation/outcome/elapsed 正确。

- **输入**：建链成功并返回 capturing authenticated exchange 的 admission transport fake。
- **操作**：通过 observed transport 建链后执行 exchange。
- **预期**：两层各记录一次，peer binding 与 continuation 行为保持，exchange 结果不变。

- **输入**：包含 `SECRET_DEVICE`、`SECRET_PATH`、`SECRET_TOKEN`、`SECRET_ERROR` 的 fake 参数和 source。
- **操作**：执行所有失败路径并捕获 tracing output。
- **预期**：任何事件字段和消息均不包含哨兵值，只包含批准 schema。

## Regression Test

- **输入**：现有 admission 双实例 E2E、membership history/branch/group-update 测试矩阵。
- **操作**：通过真实 Engine 组装运行，不直接构造 decorator。
- **预期**：业务结果、错误分类、重试次数、ledger revision、ACK 水位和恢复结果不变。

- **输入**：架构检查允许 fixture 与三个拒绝 fixture（Space 镜像 bundle、Space 第二入口、公开 decorator）。
- **操作**：运行 `node scripts/architecture/check-engine-repository.mjs`。
- **预期**：允许 fixture 通过，三个拒绝 fixture 分别被明确检查命中。

交付前执行：

```bash
cargo test -p uc-engine assembly::observability --locked
cargo test -p uc-application space:: --locked
cargo test -p uc-infra membership --locked
cargo test -p uc-engine --all-targets --locked
cargo metadata --locked --format-version 1
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

设备和 Release bundle 不属于本规格行为边界；未执行时记为“跳过”，不得记为“通过”。

## 完成证据

- `cargo test -p uc-engine assembly::observability --locked`：10 项通过，包含 authenticated exchange 继续包装合同。
- `cargo test -p uc-application space:: --locked`：178 项通过。
- `cargo test -p uc-infra membership --locked`：43 项及相关筛选回归通过。
- `cargo test -p uc-engine --all-targets --locked`：unit、dependency firewall、host/public contract 与 Space E2E 全部通过。
- `node scripts/architecture/check-engine-repository.mjs`：production shape 与三个新增负向 fixture 通过。
- 实体设备矩阵与 Release bundle：跳过，本规格不涉及设备行为或发布产物。

# 9. Acceptance Criteria

* [x] `SpaceRuntimeAdapters` 只包含 `admission` 与 `membership` 两个 Application-owned 领域 bundle。
* [x] Engine 不再定义 raw/observed 镜像 port bundle。
* [x] `assembly::observability` 对 admission 和 membership 各只暴露一个函数。
* [x] 具体 decorator、policy、operation 与 record helper 全部是领域模块私有实现。
* [x] `sync_engine.rs` 对 admission、membership 两个完整 bundle 各执行一次装饰，不逐字段取回 observed ports。
* [x] 准入现有调用级阶段仍可观察；四个 nested session transition 指标已 clean cutover 到 activation 外层阶段。
* [x] 七个 membership 方法按本规格产生调用级耗时和稳定结果分类。
* [x] Admission 与 Membership 分别只写入 `admission.performance` 和 `membership.performance`，每个 operation 的事件字段严格符合矩阵。
* [x] 成功空 admission load 与快速成功 ledger load 按 policy 降噪，任何错误均不被抑制。
* [x] decorator 不改变成功值、错误类型、source chain、调用次数和调用顺序。
* [x] authenticated exchange 继续包装并通过既有 Engine/Space admission contract 回归。
* [x] 观测事件不含任何禁止的身份、地址、凭据、业务 id、payload、digest、错误文本、文件名或路径。
* [x] 不存在通用 `Observed<T>`、phase registry、Application observation recorder port 或调用方计时参数。
* [x] 架构检查能拒绝旧镜像 bundle、Space admission/membership 第二入口和公开具体 decorator 回流，但不把“一个宽泛业务
  领域只能有一个装配时点”编码成仓库规则。
* [x] admission、membership、Engine 全目标回归及仓库交付门禁全部通过。
* [x] `docs/design-docs/observability.md`、架构圣经和计划索引与最终实现一致。
* [x] 文档明确 Clipboard process、Clipboard transport、Blob transport、File Transfer、Search/Settings 与 runtime timer 的
  后继分类；035 不为 Clipboard/Blob 改变两阶段 Application 装配。

# 10. Risks and Trade-offs

- **配置 interface 仍然较宽**：Admission 和 Membership 确实依赖多个独立能力。bundle 不假装把这些 port 合成一个
  业务 port；它的价值是单一事实来源和单次提交，而不是减少真实依赖数量。后续只有在 Application owner 本身深化时
  才能减少字段。
- **失去 transition 内部细分指标**：改为只观测 Application 直接调用后，四个 Infra 内部 transition 操作不再作为
  跨层 stage。换来的是无嵌套重复计时、单一入口和清晰 seam。若这些内部阶段仍需长期诊断，应在 transition adapter
  内部使用固定安全事件，而不是重新暴露 Engine 第二入口。
- **文件体积增加**：membership 具体 decorator 会增加实现行数。它们共享原则但不共享业务 schema，重复少量结构比
  引入万能 generic observer 更容易审查隐私和结果映射。
- **日志量**：membership ledger load 高频。50ms slow-success policy 保留错误和慢调用，牺牲正常快速调用的完整分布；
  若未来需要分位数，应引入明确 metrics backend 规格，而不是把所有成功日志打开。
- **文本架构门禁脆弱**：检查只固定关键 interface 形状和 retired marker，不尝试用正则证明完整 Rust 可见性或隐私；
  编译测试与事件 contract test 仍是主要证据。
- **替代方案——只装饰完整 SpaceApplication**：interface 更小，但只能得到端到端耗时，不能满足已确认的调用级阶段需求。
- **替代方案——Application 注入 observation port**：可保留任意内部阶段，但会让业务调用点理解计时和 schema，违反
  composition-root 所有权并形成浅 module，因此拒绝。
- **替代方案——保持当前镜像 bundle**：实施最少，但每个领域永久维护 raw/output 两份依赖清单，无法作为可推广模式。
- **替代方案——为 Clipboard/Blob 建立单个人工汇合点**：表面上得到一个 `observe_<domain>` 调用，却要求延迟
  Application 构造、扩大两阶段 builder，或让 Engine 保留 raw clone。观测收益不足以承担生命周期与绕过风险，因此拒绝。
- **每个 seam 一个入口会增加命名数量**：Clipboard process 与 transport 未来可能各有一个入口。这个 interface 数量反映
  真实装配阶段；相较跨阶段 registry，它让依赖、测试和 clean-cutover 保持 locality。

# 11. Open Questions

035 的 Space 实施没有阻塞问题。后继领域仍需分别回答，不能在实施期间默认扩展：

- Clipboard process：哪些早期 port 确有持续调用级诊断价值；`TransferCipherPort` 是否单独成 seam，还是与实际共同消费者的
  storage/system port 组成更深的 Application-owned bundle。
- Clipboard transport：`ClipboardDispatchPort::dispatch` 返回的 `DispatchTiming` 哪些字段成为稳定观测 schema；现有
  Application analytics 与端到端 timing 哪些保留，避免双写。
- Blob transport：`publish`、`publish_path`、`issue_ticket`、`fetch`、`fetch_to_path`、reference repository 中哪些属于持续
  观测；现有 `publish_blob.rs` 的 hash/publish/save-ref/ticket 手工计时如何 clean cutover，且不得记录路径或 ticket。
- File Transfer：是否存在 Blob facade 之外的独有慢阶段或失败分类；若没有，不建立新 seam。
- Search、Settings：当前没有同等明确的持续调用级需求；出现真实诊断问题后再定义 schema 与 policy。
