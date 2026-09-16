# 规格 034：确定性虚拟 Peer Network 测试套件

## 状态

- **状态**：设计完成，待实施
- **日期**：2026-09-03
- **前置规格**：[029 持久化成员历史反熵](../completed/029-durable-membership-history-anti-entropy.md)、[030 成员分叉选择与复杂拓扑验证](../completed/030-membership-conflict-resolution-and-chaos-validation.md)、[031 Application 依赖表面深化](../completed/031-application-dependency-surface-deepening.md)
- **完整负责人**：`uc-application` 的 test-only `VirtualMembershipTopology`
- **调用方唯一动作**：测试场景只提交拓扑动作并调用一次有界收敛驱动；不得编排单条协议消息、ACK、水位、恢复阶段或后台任务
- **成功结果**：在给定 round/frame 预算内得到满足断言的 `VirtualTopologySnapshot` 和可复现脱敏 trace
- **失败结果**：返回稳定的测试失败分类，并附最后一段脱敏 trace；不得依赖 wall-clock 超时推断原因
- **重试与重启责任**：Application 生产负责人继续拥有持久欠账和恢复；virtual topology 只驱动逻辑时间、maintenance round 与节点重建，不复制重试规则

# 1. Overview

规格 030 已用真实 Engine operation、SQLite、Iroh endpoint、网络分区和正文传输完成 F0-F7 验收。其中 F7 单项
耗时 430.46 秒，规格 029 的 Desktop C0-C5 串行验收也超过十分钟。真实验证证明了交付链路，但把成员协议、
持久化、安全状态、Engine 生命周期、rendezvous 和 Iroh 连接同时放入每个复杂拓扑，造成三个问题：

1. 多数协议回归只有运行到分钟级 E2E 才能发现，反馈过慢。
2. 失败同时跨越 Application、Infra 和 Engine，难以判断是拓扑规则、持久恢复还是 Iroh adapter 问题。
3. 环、深链和不平衡树只能通过最终状态间接判断；缺少逐协议、逐链路的确定性消息预算，难以直接证明无循环、
   无重复 effects 和无公平性饥饿。

本规格在现有领域 port seam 上增加 test-only virtual provider。它把真实 `SpaceApplication`、成员账本、反熵、
冲突恢复和维护顺序连接成内存多节点拓扑，以确定性节点顺序、逻辑时钟和有界 frame trace 执行 F0-F7 的协议矩阵。
Iroh 继续是生产 adapter；ALPN、认证 remote identity、codec、frame bounds、QUIC timeout、连接关闭与重连由独立
Iroh provider contract 和小型 Engine smoke 验证。原 F0-F7 真实 Iroh 测试不删除、不改写历史结果，转入明确的
nightly/release slow lane。

本规格不增加一个覆盖全部网络能力的生产 `TransportProvider`。现有窄 port 已经是实际 seam；再包装成总 provider
只会复制 Engine 组装清单、泄露 Iroh 生命周期，并形成浅模块。

# 2. Goals

- 在 `uc-application` 内建立不进入生产构建和公开白名单的 deterministic virtual membership network。
- 复用真实 `MembershipHistoryAntiEntropy`、冲突选择/恢复、成员维护和 ledger CAS 流程，不实现第二套成员状态机。
- 通过现有 `MembershipHistoryExchangePort`、`RestrictedMembershipDeliveryPort`、`GroupUpdateDispatchPort` 和
  `MembershipBranchRecoveryChannelPort` 注入 virtual adapter，不改变生产 port 接口或所有权。
- 以 typed domain message 传输，保证测试覆盖 port 以上的真实业务语义；Iroh wire 由真实 provider contract 覆盖。
- 用稳定节点顺序、逻辑时钟、round/frame 双预算和脱敏 trace 确定性执行 F0-F7。
- 将 F0-F7 中的 branch/head、成员资格、冲突、effects、group epoch、公平性和授权矩阵放入常规 Application 测试。
- 保留真实 SQLite、control-generation、MLS、Iroh partition、Engine 生命周期和 exact content 的独立验证证据。
- 同一场景重复运行时产生相同的最终 snapshot 和 trace signature；失败可用场景名和固定 seed 单独复现。
- 常规 virtual F0-F7 总耗时在当前 macOS-14 PR runner 上不超过 30 秒，且场景内不使用真实 `sleep` 等待收敛。

# 3. Non-Goals

- 不修改规格 029/030 已完成的历史证据，也不把 virtual 结果登记为真实 Iroh、SQLite、MLS 或设备通过。
- 不创建 Engine 级通用 `TransportProvider`、万能字节总线或可由产品选择的网络 provider。
- 不公开 `SpaceApplication`、内部 use case、maintenance runtime 或测试 fixture。
- 不把 virtual network 加入 `uc-application` 的 `test-support` feature；该 feature 继续只服务已有外部测试需求。
- V1 不虚拟化邀请 discovery、完整 Space admission、clipboard/blob/file transfer 或 LAN compatibility。
- V1 不实现 `PeerReachabilityPort` 或模拟 Iroh presence；topology 在动作边界直接选择真实 maintenance trigger，
  presence cache、probe 和连接在线状态继续由现有 Iroh tests 验证。
- V1 不模拟 QUIC 握手、stream、拥塞、MTU、ALPN 协商、已有连接被关闭或 relay 行为。
- V1 不实现随机丢包、任意乱序、带宽、延迟分布或概率故障；F12 的 codec/分页/提交故障继续在责任层测试。
- 不用 in-memory repository 替代真实 SQLite 原子性、密文持久化或 control-generation 崩溃恢复证据。
- 不用授权矩阵替代 exact text、密文和错误密钥的真实数据面验证。
- 不顺便迁移现有 port 所有权，不清理与本规格无关的单元测试 fake。

# 4. Current Architecture Context

```text
Component: Engine F0-F7 membership topology E2E
Path: crates/uc-engine/tests/space_membership_auto_pairing_e2e.rs
Responsibility: 通过公开 Engine operation、真实 profile 目录、SQLite、rendezvous 和 Iroh endpoint 执行复杂拓扑。
Relationship: 当前 `MembershipTopology` 同时拥有节点生命周期、邀请、endpoint id、分区、轮询和业务断言；保留为真实 slow lane，协议矩阵迁入 Application virtual suite。
```

```text
Component: SpaceApplication
Path: crates/uc-application/src/space/application.rs
Responsibility: 组装成员 ledger、历史 endpoint、冲突恢复、维护负责人及 Space 生命周期出口。
Relationship: 已有 crate-private `build_for_test` 和 endpoint accessor；virtual fixture 应在同 crate 的 `cfg(test)` 模块使用，不扩大公开 interface。
```

```text
Component: MembershipHistoryAntiEntropy
Path: crates/uc-application/src/space/membership/anti_entropy.rs
Responsibility: 统一承担历史入站、出站、ACK、水位、重试欠账与同步结果。
Relationship: virtual history adapter 必须把消息交给远端真实 endpoint；测试不得自行解释 summary、suffix 或 ACK。
```

```text
Component: Membership transport ports
Path: crates/uc-core/src/membership/ports.rs, crates/uc-application/src/space/membership/recover_conflict/ports.rs
Responsibility: 表达历史交换、受限投递、group update 和两阶段 branch recovery 的领域能力。
Relationship: Iroh 与 virtual 是这些既有 seam 上的两个 adapter；034 不增加上层总 provider。
```

```text
Component: Iroh membership adapters
Path: crates/uc-infra/src/network/iroh/membership_history_exchange_adapter.rs, crates/uc-infra/src/network/iroh/group_update_adapter.rs, crates/uc-infra/src/network/iroh/membership_branch_recovery_adapter.rs
Responsibility: 地址解析、ALPN、认证来源、codec、frame bounds、timeout、ACK 与 endpoint dispatch。
Relationship: 继续作为生产 adapter；独立 contract 验证 port 到 Iroh 的映射，virtual suite 不复制 wire 实现。
```

```text
Component: IrohNetworkPartitionGate
Path: crates/uc-infra/src/network/iroh/network_partition.rs
Responsibility: 在连接前和握手后拒绝 blocked endpoint，并关闭已建立连接。
Relationship: `VirtualPeerNetwork::partition` 只阻断动作边界后的新领域调用，不能替代真实 gate contract。
```

```text
Component: Membership persistence and branch transition integration tests
Path: crates/uc-infra/tests/membership_ledger.rs, crates/uc-infra/src/security/v3_membership_branch_transition/tests.rs
Responsibility: 验证真实 SQLite、MasterKey AEAD、CAS、nonce、control-generation 阶段与崩溃恢复。
Relationship: virtual node restart 只验证 Application 重新组装和恢复决策；介质与安全原子性继续由这些测试证明。
```

当前数据流为：Application 调用领域 port，Iroh adapter 编码并建立连接，远端 handler 从连接身份解析
`DeviceId` 后调用 Application endpoint。复杂拓扑通过 Engine dev operation 把 endpoint id 加入分区 gate。virtual
suite 只替换“领域 port 到远端 endpoint”这一段，前后的 Application 业务逻辑保持不变。

# 5. Proposed Design

## Components

### `VirtualPeerNetwork`

- **位置**：`crates/uc-application/src/space/testing/virtual_network.rs`
- **职责**：保存 test node 注册表、有向 link policy、单调 frame sequence、调用计数和脱敏 trace；按领域协议把一次
  调用路由到目标 endpoint。
- **输入**：已注册 source、目标 `DeviceId`、`VirtualProtocol` 和 typed request。
- **输出**：typed response 或 `VirtualDeliveryError`。
- **关系**：它不读取 ledger、不运行 membership policy、不生成 ACK、不保存重试欠账。

删除检查：若删除该模块，身份绑定、分区、路由、预算和 trace 会重新散落到每个 fake port 和 F0-F7 场景，因此
该模块应隐藏这些共同知识。

### `VirtualMembershipTransport`

- **位置**：`crates/uc-application/src/space/testing/virtual_transport.rs`
- **职责**：作为每个节点的 test-only adapter bundle，实现现有领域 ports，并把 `VirtualDeliveryError` 映射为各
  port 的稳定错误分类。
- **输入**：本机注册身份、共享 `VirtualPeerNetwork`。
- **输出**：可注入 `SpaceRuntimeAdapters` 的 trait objects。
- **关系**：source identity 只能从 adapter 注册信息取得，调用参数不能伪造；远端 endpoint 仍执行真实业务验证。

### `VirtualMembershipNode`

- **位置**：`crates/uc-application/src/space/testing/virtual_node.rs`
- **职责**：持有可跨重建保留的 test repositories、确定性 clock/signature/security adapters，以及当前
  `SpaceApplication`；提供生产负责人级动作，不暴露 ledger 逐字段修改。
- **输入**：`VirtualNodeSeed` 和 transport bundle。
- **输出**：成员动作结果、诊断 snapshot、授权 scope 和 endpoint registration。
- **关系**：节点启动后禁止测试直接写 repository。`restart` 销毁并重建 `SpaceApplication`，复用同一 durable
  test repository 和逻辑身份。

### `VirtualMembershipTopology`

- **位置**：`crates/uc-application/src/space/testing/topology.rs`
- **职责**：F0-F7 的唯一测试入口；按稳定顺序执行节点动作、网络控制、逻辑时间推进、maintenance round、预算和
  最终断言。
- **输入**：声明式 `VirtualTopologyAction`、`ConvergenceExpectation`、`VirtualExecutionBudget`。
- **输出**：`VirtualTopologySnapshot`、trace signature 或 `VirtualTopologyFailure`。
- **关系**：它驱动完整 Application owner，不解释 membership message 或 transition phase。

### Application manual maintenance test seam

- **位置**：`crates/uc-application/src/space/application.rs`
- **职责**：仅在 `cfg(test)` 下允许 topology 对当前 `MaintainSpaceMembershipUseCase` 执行一个明确 trigger。
- **输入**：`MembershipMaintenanceTrigger`。
- **输出**：真实 `MembershipMaintenanceReport`。
- **关系**：virtual scenario 不启动后台 interval runtime；生产构建、公开 interface 和 Engine 组装不变。

### Iroh membership provider contract

- **位置**：`crates/uc-infra/src/network/iroh/membership_provider_contract_tests.rs`
- **职责**：用真实 loopback endpoint 验证 Iroh adapters 对既有领域 ports 的实现，包括认证 source、codec、frame
  bounds、ACK、拒绝、timeout 分类、两阶段 recovery 和 partition gate。
- **输入**：真实 Iroh endpoints 与最小 endpoint fakes。
- **输出**：port 级成功或稳定错误分类。
- **关系**：只测试 adapter，不重跑完整 F0-F7 业务拓扑。

### Real-Iroh slow-lane runner

- **位置**：`scripts/testing/run-real-iroh-membership-topologies.sh`、`.github/workflows/membership-topology.yml`
- **职责**：串行执行被标记为 slow lane 的 F0-F7，并保存每项结果；支持 schedule 和手工触发。
- **输入**：当前提交与固定测试列表。
- **输出**：逐场景通过/失败；未执行时只能标为“跳过”。
- **关系**：不改变 release bundle 规则；正式发布前必须有同一提交的 slow-lane 结果或明确记录为跳过。

workflow 默认每日 UTC 02:00 执行，保留 artifact 14 天；同时提供 `workflow_dispatch`。

## Data Model

### `VirtualNodeKey`

test-only 稳定节点键。场景可显示 `A`～`J` 等固定标签，但不得包含真实设备名、`DeviceId`、endpoint id、地址或
路径。`DeviceId` 仅保存在注册表内部用于 port 路由和生产业务校验。

### `RegisteredMembershipEndpoints`

```rust
struct RegisteredMembershipEndpoints {
    device_id: DeviceId,
    history: Arc<dyn MembershipHistoryExchangeEndpointPort>,
    branch_recovery: Arc<dyn IssueMembershipBranchRecoveryPort>,
    group_updates: Arc<dyn GroupRevocationPort>,
}
```

注册生命周期与 `VirtualMembershipNode` 的运行实例一致。`stop` 移除 endpoints 但保留节点 repository；`restart`
以相同业务身份和新 Application 实例重新注册。

### `VirtualProtocol`

固定枚举：

- `MembershipHistory`
- `RestrictedMembership`
- `GroupUpdate`
- `BranchRecoveryGroupInfo`
- `BranchRecoveryExternalCommit`

不得使用自由字符串协议名。Admission、clipboard、blob 和 LAN 不进入 V1。

### `VirtualLinkState`

有向 link 只有 `Open` 和 `Blocked`。未注册目标等价 `Unavailable`。`partition(groups)` 阻断所有跨组双向 link，
`bridge(left, right)` 只打开指定双向 link，`heal(nodes)` 恢复相关 link。link mutation 前 topology 必须完成当前
动作；V1 不定义对 in-flight delivery 的取消语义。

### `VirtualFrameRecord`

```rust
struct VirtualFrameRecord {
    sequence: u64,
    protocol: VirtualProtocol,
    source: VirtualNodeKey,
    target: VirtualNodeKey,
    outcome: VirtualFrameOutcome,
}
```

`VirtualFrameOutcome` 只含 `Accepted`、`Rejected`、`Unavailable`、`Invalid`。记录中禁止保存 payload、错误文本、
业务 id、branch/head、成员身份、凭据或地址。失败只保留最后 128 条记录；完整计数按 `(protocol, source, target,
outcome)` 聚合。

### `VirtualExecutionBudget`

默认值：`max_rounds = 128`、`max_frames = 10_000`、`max_trace_records = 128`。每个场景可以向下收紧，不能在测试
内部静默提高。预算耗尽返回 `BudgetExceeded`，并报告已执行 round、frame 聚合和脱敏 trace。

### `VirtualTopologySnapshot`

每个节点只保存断言所需的稳定测试投影：branch 等价类标签、effective member count、membership 状态、group
epoch、pending conflict/effect 数量、ledger revision、可恢复/需重新配对状态和授权 peer 集合。snapshot 不包含
原始 branch/head digest、签名、恢复包或密钥。

## API / Interface

所有接口均为 `pub(super)` 或更窄，并受 `cfg(test)` 限制：

```rust
impl VirtualMembershipTopology {
    async fn from_seeds(
        seeds: impl IntoIterator<Item = VirtualNodeSeed>,
    ) -> Result<Self, VirtualTopologyFailure>;

    async fn execute(
        &mut self,
        action: VirtualTopologyAction,
    ) -> Result<(), VirtualTopologyFailure>;

    async fn converge_until(
        &mut self,
        expectation: &ConvergenceExpectation,
        budget: VirtualExecutionBudget,
    ) -> Result<VirtualTopologySnapshot, VirtualTopologyFailure>;

    async fn run_rounds(
        &mut self,
        rounds: usize,
        trigger: VirtualMaintenanceTrigger,
    ) -> Result<(), VirtualTopologyFailure>;

    fn partition(
        &mut self,
        groups: &[&[VirtualNodeKey]],
    ) -> Result<(), VirtualTopologyFailure>;
    fn bridge(
        &mut self,
        left: VirtualNodeKey,
        right: VirtualNodeKey,
    ) -> Result<(), VirtualTopologyFailure>;
    fn heal(&mut self, nodes: &[VirtualNodeKey]) -> Result<(), VirtualTopologyFailure>;
    async fn stop(&mut self, node: VirtualNodeKey) -> Result<(), VirtualTopologyFailure>;
    async fn restart(&mut self, node: VirtualNodeKey) -> Result<(), VirtualTopologyFailure>;
    fn trace_signature(&self) -> [u8; 32];
}
```

`VirtualTopologyAction` V1 包含 `Partition`、`PartitionGroups`、`Bridge`、`Ring`、`Chain`、`Heal`、`Stop`、
`Restart`、`Remove`、`Decide`、`ResolveConflict`、`AdvanceClock`、`RunRounds` 和 `AssertSnapshot`。Admission
相关 `Create/Join` 不进入 V1；场景通过 `VirtualNodeSeed` 建立已验证前置历史。

`VirtualNodeSeed` 只能在节点构造前使用。它必须调用生产 `VersionedMembershipHistory` constructor、编码/解码和
签名验证生成合法 ledger，不能手写跳过验证的内部 record。节点启动后，成员改变只能调用真实 Application owner。

`VirtualPeerNetwork` 内部 route 顺序为：

1. 在短锁内解析 source 注册、target 注册、link 状态并分配 sequence。
2. 释放网络锁。
3. 调用远端 typed endpoint；任何网络锁不得跨 `await`。
4. 在短锁内记录稳定 outcome 和计数。
5. 返回 typed response 或映射后的 port error。

身份绑定规则：source `DeviceId` 永远来自 `VirtualMembershipTransport` 的注册信息。普通 scenario API 不接受
source `DeviceId` 参数；需要验证恶意来源的单元测试使用独立 `inject_unauthenticated_for_test`，不得进入拓扑 DSL。

错误映射固定如下：

| Virtual outcome | History | Restricted delivery | Group update | Branch recovery |
| --- | --- | --- | --- | --- |
| target missing / link blocked | `Offline` | `Deferred` | `Offline` | `Unavailable { source }` |
| endpoint business reject | `Rejected` | `Rejected` | `Rejected` | `Rejected { source }` |
| endpoint response/type invalid | `Transport` | `Rejected` | `Transport` | `Invalid { source }` |
| frame budget exhausted | `Transport` | `Deferred` | `Transport` | `Unavailable { source }` |

带 source 的 Application 错误必须保留 `VirtualDeliveryError` source chain；错误 `Debug` 和 Display 不包含身份或
payload。现有不携带 source 的 Core transport error 不在本规格顺带改型。

## Workflow

### 场景准备

1. fixture builder 使用生产 Core constructor 和 deterministic signer 创建共同 baseline 与合法 sibling histories。
2. 每个 `VirtualNodeSeed` 只包含该节点起始时应持有的完整已验证状态、逻辑身份和持久 test repositories。
3. topology 创建所有节点的 test adapters 和 dormant `SpaceApplication`，获取真实 endpoints 后注册到 network。
4. 不启动 `SpaceMembershipMaintenanceRuntime`；所有推进由 topology 的 manual maintenance driver 完成。

### 确定性收敛

1. topology 按 `VirtualNodeKey` 排序选择本 round 的在线节点。
2. 每个节点调用一次真实 `MaintainSpaceMembershipUseCase`，使用场景指定的 Startup、StateChanged、PeerOnline 或
   Periodic trigger。
3. transport 直接把 typed 请求送入目标 Application endpoint，并记录 frame outcome。
4. round 结束后读取只读 snapshot；若满足 expectation 立即成功。
5. 若仍可推进，场景显式推进 logical clock 后进入下一 round。
6. 达到 round/frame 预算仍未满足时返回 `BudgetExceeded`，输出 snapshot 差异、计数与最后 128 条脱敏 trace。

### 分区、停止与恢复

1. link mutation 只发生在两个已完成 action 之间。
2. `partition` 后的新调用立即得到对应 port 的 unavailable/offline 结果；Application 自己保存欠账和退避。
3. `stop` 注销 endpoints 并销毁当前 Application，不清除 repository。
4. `restart` 用原 repository、身份和 clock 重建 Application，重新注册 endpoints。
5. `heal` 只恢复 link；后续收敛仍必须由真实 maintenance round 发现和偿还欠账。

### Slow lane

1. F0-F7 原 Engine tests 保持源码和真实断言，增加带原因的 `#[ignore]` slow-lane 标记。
2. runner 显式逐项执行 F0-F7，固定 `--test-threads=1`，不得用名称模糊过滤遗漏场景。
3. nightly workflow 保存逐场景结果；发布前引用同一提交结果。未执行时记录“跳过”，不得沿用旧提交结果。

# 6. Implementation Plan

```text
Step 1
Files: crates/uc-application/src/space/application.rs, crates/uc-application/src/space/mod.rs
Change: 增加 cfg(test) manual maintenance driver，保留同一个生产 MaintainSpaceMembershipUseCase；注册 testing 子模块。
Risk: 若复制 assembly 或启动第二个 runtime，会产生双 owner；测试模式必须只有手动 driver 推进。
```

```text
Step 2
Files: crates/uc-application/src/space/testing/virtual_network.rs, virtual_transport.rs
Change: 先写 link、identity、routing、error mapping、frame budget 和脱敏 trace 红测，再实现 virtual network 与四类 port adapter。
Risk: 网络锁跨 endpoint await 会死锁；source 从调用参数取得会允许伪造认证身份。
```

```text
Step 3
Files: crates/uc-application/src/space/testing/fixtures.rs, virtual_node.rs, topology.rs
Change: 提取最小合法 history/signature/repository fixture，组装真实 SpaceApplication，增加 restart、logical clock、manual rounds、snapshot 与 convergence budget。
Risk: fixture 若直接构造不可达内部状态，会形成第二套协议；所有 seed 必须经生产 constructor 和验证器。
```

```text
Step 4
Files: crates/uc-application/src/space/testing/scenarios.rs
Change: 按 F0-F7 建立协议等价矩阵；从各场景第一个需要网络传播的合法状态开始，不虚拟完整 admission。
Risk: 逐字复制 Engine 轮询会保留慢测试；断言必须改为稳定业务结果、授权矩阵和 frame/round 上限。
```

```text
Step 5
Files: crates/uc-infra/src/network/iroh/mod.rs, crates/uc-infra/src/network/iroh/membership_provider_contract_tests.rs
Change: 汇总或补齐真实 loopback contract：history codec/source、group ACK、branch recovery 两阶段和 partition close/reject/heal。
Risk: 只测成功 round trip 会漏掉认证来源和已有连接关闭，这些正是 virtual 无法覆盖的差异。
```

```text
Step 6
Files: crates/uc-engine/tests/space_membership_auto_pairing_e2e.rs, scripts/testing/run-real-iroh-membership-topologies.sh, .github/workflows/membership-topology.yml
Change: F0-F7 标为明确 slow lane；脚本逐项串行运行；新增 scheduled/workflow_dispatch job。保留现有快速 admission/restart/content smoke 非 ignored。
Risk: `cargo test` 的 ignored 计数不能记为通过；release/nightly 记录必须绑定当前 commit。
```

```text
Step 7
Files: scripts/architecture/check-engine-repository.mjs, docs/architecture/architecture-bible.md, docs/exec-plans/active/034-deterministic-virtual-peer-network-test-suite.md
Change: 增加负向检查，禁止生产 `TransportProvider`、公开 Space test assembly 和 virtual provider 进入非 cfg(test)；同步稳定设计与实际验收证据。
Risk: 文本检查不能替代编译依赖检查；负向 fixture 必须证明规则可执行。
```

# 7. Edge Cases

```text
Scenario: 目标节点未注册或已停止。
Expected behavior: 调用映射为 Offline/Deferred/Unavailable，Application 保留欠账；不得 panic 或删除水位。
Implementation: route 在短锁内解析注册表并记录 Unavailable，不调用 endpoint。
```

```text
Scenario: 有向 link 只阻断一个方向。
Expected behavior: A→B 失败不代表 B→A 失败；ACK 和水位只按实际认证方向推进。
Implementation: link policy 以有序 `(source, target)` 为键，partition helper 显式写入双向规则。
```

```text
Scenario: 分区时存在已开始的 delivery。
Expected behavior: V1 只允许在 action 边界修改 link，已开始调用按开始时 snapshot 完成；不得宣称等价于 Iroh 关闭已有连接。
Implementation: topology 在 link mutation 前确认无 scenario action 正在执行；真实取消语义由 Iroh contract 验证。
```

```text
Scenario: endpoint 在处理请求时触发 maintenance wake。
Expected behavior: 当前 endpoint 完成后由后续 manual round 处理；不得递归启动第二个 maintenance owner。
Implementation: virtual tests 不启动 background runtime；topology 在 round 边界重新读取状态。
```

```text
Scenario: endpoint 又发起嵌套 transport 调用。
Expected behavior: 不死锁；嵌套调用也消耗 frame budget并记录因果顺序。
Implementation: network state lock 在 endpoint await 前释放，sequence 在每次 route 开始时分配。
```

```text
Scenario: 环拓扑形成无限协议扩散。
Expected behavior: 在 `max_frames` 内收敛；否则以 BudgetExceeded 失败并显示最后 trace，而不是等待 wall-clock timeout。
Implementation: 每次 route 原子消耗 frame budget，F5 另在稳定后执行额外 rounds 并断言无新增 conflict/effect 和有界 frame 增量。
```

```text
Scenario: 合法 peer 与 Diverged peer 同时存在。
Expected behavior: 公平游标最终服务合法 peer；冲突 peer 不消耗全部 round 预算。
Implementation: F7 按每个 peer 的首次成功 round 和 frame count 断言上界，不只检查最终成员数。
```

```text
Scenario: restart 时有持久 retry debt、conflict choice 或 branch recovery session。
Expected behavior: 新 Application 从同一 repository 继续，只向前推进；network 不替它记忆业务阶段。
Implementation: node stop/rebuild 保留 repositories，清空易失 endpoints 和 Application 实例。
```

```text
Scenario: seed 历史损坏、错 lineage 或签名无效。
Expected behavior: topology 构造失败，零节点注册、零 frame。
Implementation: `VirtualNodeSeed::validated` 强制生产 decode/verify；不提供 unchecked constructor。
```

```text
Scenario: 空 topology、重复节点键、重复 DeviceId 或未知节点 action。
Expected behavior: 构造或 action 立即返回稳定 fixture error，不进入 maintenance。
Implementation: 注册阶段验证唯一键和唯一业务身份；禁止 `unwrap`/`expect` 进入非测试生产代码。
```

```text
Scenario: frame 或 round 计数接近极限。
Expected behavior: checked arithmetic；溢出视为 BudgetExceeded，不回绕成成功。
Implementation: 使用 `checked_add`，默认预算远低于 `u64::MAX`。
```

```text
Scenario: trace 可能泄漏 payload 或身份。
Expected behavior: trace 只包含测试标签、协议枚举、序号和结果；Debug 快照无敏感字段。
Implementation: `VirtualFrameRecord` 不持有 payload，增加格式化和敏感 canary 负向测试。
```

```text
Scenario: 旧版本或 LAN compatibility。
Expected behavior: 034 不建立 fallback 或兼容 adapter；生产 Iroh 失败仍不自动切换 LAN。
Implementation: architecture check 保持既有 P2P/LAN 门禁，virtual provider 仅在 cfg(test) 可达。
```

# 8. Testing Strategy

## Unit Test

### Virtual network

- 输入：A/B 注册、双向 open link；操作：history request/response；预期：B endpoint 看到的 source 是 A 的注册
  `DeviceId`，trace 只有一条 Accepted 且无 payload。
- 输入：A→B blocked、B→A open；操作：双向调用；预期：仅 A→B 映射 Offline，方向不被合并。
- 输入：未知、停止和重复注册节点；操作：route/register；预期：稳定 fixture/delivery error，无 endpoint 调用。
- 输入：远端 endpoint 返回 reject/invalid；操作：分别经过四种 adapter；预期：严格符合错误映射表，带 source 的
  错误 `source()` 非空。
- 输入：`max_frames = 2`；操作：发送三次；预期：第三次 BudgetExceeded，计数不回绕，trace 长度受限。
- 输入：含内容、设备、地址和路径 canary 的 payload；操作：失败并格式化 trace；预期：任何 canary 均不存在。
- 输入：相同 seed/actions；操作：重复执行；预期：trace signature 和最终 snapshot 完全一致。

### Node and topology

- 输入：合法 seed；操作：构造、stop、restart；预期：业务身份和 repository 状态保留，Application/endpoint 实例更换。
- 输入：损坏 seed；操作：构造；预期：验证失败且 network 注册表为空。
- 输入：manual maintenance；操作：同一节点并发请求两轮；预期：沿生产 execution lock 串行，无双重 effects。
- 输入：逻辑 clock 未推进；操作：重复 periodic round；预期：未到期 retry 不被 wall-clock 唤醒。

## Integration Test

### Virtual F0-F7 matrix

| 编号 | Virtual 前置与动作 | 常规 CI 断言 | 仍由真实层证明 |
| --- | --- | --- | --- |
| F0 | 从共同 baseline seed 两个 Add sibling，heal | 两个 branch 等价类、无自动赢家、跨分支授权关闭 | admission、邀请、exact text |
| F1 | seed Remove/Add sibling，交换历史 | Removed/Active 精确、冲突唯一、无联合历史 | Sponsor/Joiner admission 与真实 group update wire |
| F2 | seed 两个 Remove sibling，明确选择目标 | 选择不可变、目标成员/epoch、恢复 session 只前进 | MLS external commit、control-generation 介质切换 |
| F3 | Accept/Reject sibling，restart chooser | conflict/choice/revision 跨 Application 重建保持、授权隔离 | SQLite AEAD、真实进程重启、exact text |
| F4 | 两个三节点分支只开放单 bridge | bridge 只传播冲突证据，不拼成六成员历史 | Iroh link/connection 行为 |
| F5 | 四节点环双向传播同一 conflict | 每节点一个 issue、effects 不重复、frame 数有界 | QUIC 多连接时序 |
| F6 | 深链中间节点 stop，叶子选择后 heal | 不依赖原 Sponsor，逐跳最终同 branch，retry debt 偿还 | 真实 endpoint 重启和 secure session 恢复 |
| F7 | 十节点三 sibling 不平衡树 | 合法 peer 在固定 round 上界内被服务，冲突 peer 不饥饿合法 peer | 实际 Iroh 调度和资源压力 |

每个场景至少断言：branch 等价类、effective members、pending conflict/effect、ledger revision 单调、授权 peer
矩阵、round/frame 上限。group epoch 只有在场景使用的 deterministic security adapter 实际应用 production group-update
port 后才可断言；fixture 直接赋值的 epoch 不得计为通过。

### Iroh provider contract

- History：summary/suffix/ACK round trip、wire version、最大/超限 frame、未知认证身份、source 绑定、remote reject。
- Group update：accepted/rejected ACK、零长/超限 payload、offline 和 timeout 分类。
- Branch recovery：GroupInfo 与 external commit 两阶段绑定、错误方向/recipient、畸形/超限 frame、认证 source。
- Partition：新连接在 before-connect 拒绝、已连接在更新 blocked 集合后关闭、握手竞态拒绝、heal 后可新建连接。
- Shutdown：Router/handler 关闭后调用有界结束，不遗留长期任务。

### Real persistence/security integration

- 继续运行真实 membership ledger CAS/AEAD、nonce、防重放和 branch transition phase fault replay。
- F3 的 Application restart 不替代 SQLite restart；两者必须分别登记。
- 测试输出执行明文 canary 探针，禁止业务 payload 和身份进入数据库、日志或 trace。

## Regression Test

- 常规门禁运行 virtual F0-F7、现有 Core/Application F8-F13、真实 Infra contract 和非 ignored Engine smoke。
- F0-F7 原真实 Engine/Iroh 测试保留，slow-lane runner 逐项串行执行；失败不得用 virtual 通过覆盖。
- 至少保留这些非 ignored 真实 smoke：fresh join、完成准入后重启并 exact text、history adapter round trip、branch
  recovery round trip、partition reject/close/heal、group update ACK。
- P2P 失败不自动回退 LAN；三端绑定和 `uc-engine` 稳定 operation 不因 test provider 改变。
- 完整实现至少运行：

```bash
cargo metadata --locked --format-version 1
cargo test -p uc-application --locked
cargo test -p uc-infra --locked
cargo test -p uc-engine --all-targets --locked
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

- slow lane 使用仓库脚本运行。未连接实体设备或未生成 Release bundle 时明确记录为“跳过”。

# 9. Acceptance Criteria

* [ ] `VirtualMembershipTopology` 是 virtual F0-F7 节点、逻辑时间、网络控制、预算与 trace 的唯一完整负责人。
* [ ] 生产代码没有新增通用 `TransportProvider`，现有领域 ports 和 Engine 公开 contract 不变。
* [ ] virtual provider 仅在 `uc-application` 的 `cfg(test)` 构建可达，不进入 `test-support` 公开 feature。
* [ ] virtual history、restricted delivery、group update 和 branch recovery adapters 均调用远端真实 Application endpoint/capability。
* [ ] source identity 只能来自注册 adapter；普通 topology action 无法伪造 `DeviceId`。
* [ ] 网络锁不跨 endpoint `await`，嵌套调用和并发 maintenance 不死锁。
* [ ] 节点启动后，场景不能直接改写 ledger；所有 seed 经生产 constructor、codec 和 signature verification。
* [ ] virtual 场景不启动真实 periodic runtime，不使用 wall-clock `sleep` 等待收敛。
* [ ] F0-F7 virtual 矩阵全部通过，并具有明确 round/frame 上限、授权矩阵和脱敏失败 trace。
* [ ] 同一 F0-F7 场景重复执行产生相同 snapshot 与 trace signature。
* [ ] virtual F0-F7 在 macOS-14 PR runner 上总耗时不超过 30 秒，单场景无分钟级 timeout。
* [ ] F5 能以 frame/effect 上限证明无循环，F7 能以首次成功 round 上限证明合法 peer 不被冲突 peer 饿死。
* [ ] Application restart、真实 SQLite restart 和真实 control-generation fault replay 分别通过并分别登记。
* [ ] Iroh provider contract 覆盖认证 source、codec/frame、ACK、两阶段 recovery、partition close/reject/heal 和 shutdown。
* [ ] 非 ignored Engine smoke 覆盖真实 admission、重启、exact content、group update 和 branch recovery。
* [ ] F0-F7 原真实 Engine/Iroh 测试仍存在，并可由 slow-lane 脚本逐项串行执行。
* [ ] nightly/release slow-lane 结果绑定当前提交；未执行项明确记为“跳过”，不沿用旧证据。
* [ ] trace、错误、日志和 CI artifact 不含设备身份、地址、邀请、branch/head、凭据、密钥、路径或业务内容。
* [ ] 架构检查阻止 virtual provider 进入生产依赖、公开 Space 内部 assembly 或恢复 Engine 级万能 provider。
* [ ] workspace check、Application/Infra/Engine tests、fmt、architecture 和 diff gates 全部通过。
* [ ] 实施结论同步到 `docs/architecture/architecture-bible.md`；完成后本计划移入 `completed/`。

# 10. Risks and Trade-offs

- **virtual 不等于 Iroh**：typed 调用绕过 ALPN、codec、QUIC 和连接生命周期。通过 Iroh provider contract、Engine
  smoke 与保留的 slow lane 明确补齐，而不是提高 virtual 仿真复杂度。
- **seed 降低 setup fidelity**：F0/F1 不再每次经过完整 invitation/admission。收益是把拓扑协议测试从准入成本中
  分离；真实 admission 与 fork 形成仍由 Engine smoke/slow lane 证明。
- **test-only manual driver 接近内部 seam**：它能稳定执行生产 maintenance owner，但不得导出到外部 crate 或让
  scenario 调用内部步骤。interface 只允许“一轮指定 trigger”，不暴露子 use case。
- **in-memory restart 可能给出过强信心**：它只能证明 Application 从保存状态恢复，不能证明 SQLite 事务、AEAD、
  fsync 或 manifest promotion；验收矩阵必须把这些证据分开。
- **两套场景会增加维护成本**：virtual 与 real slow lane 关注不同证据，不能逐行复制。F0-F7 的业务期望以本规格
  矩阵为入口，real 测试只保留必须跨 Infra/Engine 的断言。
- **未模拟概率网络**：确定性 link failure 更适合回归和复现，但不能发现所有真实调度问题。固定 Iroh slow lane
  继续承担集成风险；随机压力只能作为附加证据，不能替代固定矩阵。
- **30 秒目标依赖 CI**：不得把机器时间作为唯一正确性断言；round/frame budget 才是稳定契约，wall-clock 只作
  工程反馈目标。

替代方案一是在 Engine 增加完整 network provider factory，使所有 E2E 可切换 Iroh/virtual。该方案需要虚拟
IrohNode builder、Router、admission、clipboard、blob 和生命周期，interface 几乎等于实现清单，并破坏 031 已收口的
Application 私有装配，因此不采用。

替代方案二是只保留各 use case 的独立 fake。它不能表达多节点、有向分区、逐跳传播、frame budget 和公平性，
F5/F7 的复杂度会继续散落在测试调用方，因此不采用。

替代方案三是编写纯模型模拟器。它运行最快，但会复制 membership 状态机并可能与生产规则共同错误，不能作为
实现回归证据，因此不采用。

# 11. Open Questions

唯一未决项不阻止 virtual suite、Iroh contract 和 nightly slow lane 实施：

1. Release 是否硬性依赖同一提交的 slow-lane success 尚未在现有 release workflow 中定义。未决定前，发布记录必须
   明确写“通过”或“跳过”，不得自动继承 nightly 状态。
