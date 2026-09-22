# Engine 测试架构

## 状态

- 状态：已实施，后续按场景渐进采用
- 日期：2026-09-21
- 实施记录：[044 Engine testkit 基础](../exec-plans/completed/044-engine-testkit-foundation.md)
- 后续领域场景计划：[034 确定性虚拟 Peer Network](../exec-plans/active/034-deterministic-virtual-peer-network-test-suite.md)
- 已完成领域采用：[045 确定性成员恢复高价值场景](../exec-plans/completed/045-deterministic-membership-recovery-scenarios.md)
- 已完成真实依赖采用：[046 真实依赖与独立进程 testkit 采用](../exec-plans/completed/046-real-dependency-testkit-adoption.md)
- 使用指南：[Engine 测试使用指南](testing-guide.md)
- 当前采用清单：[Engine 测试采用清单](../references/test-adoption-inventory.md)
- 当前采用与进程韧性计划：[048 testkit 采用指南与进程韧性](../exec-plans/active/048-testkit-adoption-and-process-resilience.md)

# 1. Overview

Engine 已有 Core、Application、Infra、Engine、独立进程、真实网络和设备宿主等多层测试，但缺少统一的测试控制与证据格式。测试通常各自实现等待、临时目录、端口、计时、失败文本和清理，运行入口主要依赖 `cargo test` 名称过滤与全局串行设置。复杂场景失败时，难以稳定区分产品失败、环境失败和测试框架失败，也缺少跨 CI 运行可聚合的阶段耗时与复现信息。

本设计建立两个互补边界：

1. `cargo-nextest` 负责测试进程的发现、过滤、分组、并发、超时、重试和 JUnit 输出。
2. 仓库内 `uc-testkit` 负责单个场景内部的身份、预算、阶段计时、事件等待、资源租约、失败分类、结构化诊断和清理结果。

两者都不拥有产品业务流程，不解释 Engine 内部状态机，不替代真实 Iroh、SQLite 或设备验收。领域级确定性网络由规格 034 在后续单独实施。

# 2. Goals

- 提供不依赖产品 crate 的可复用测试 kit，并保持生产构建不依赖该 crate。
- 每个采用 testkit 的场景都能输出稳定 JSON 工件和简短人类摘要。
- 失败结果必须包含稳定分类、未满足条件、最后事件、阶段耗时、固定 seed、复现命令和工件位置。
- 提供无端口竞态的本地端口租约、权限受控的临时目录和明确清理结果。
- 提供事件驱动等待；wall-clock 只作为测试框架保护预算，不表达产品业务 deadline。
- 用 nextest 配置表达 `fast`、`persistence/provider`、`engine-smoke`、`process`、`real-network` 和 `device` 边界。
- 保留所有现有 `cargo test` 入口，CI 先增加并行证据入口，不替换现有门禁。

# 3. Non-Goals

- 本轮不迁移、修改、重写或复制 t-0010 工作区及其测试场景。
- 本轮不实现规格 034 的虚拟成员网络、虚拟业务时钟或 Application 手动维护入口。
- 不修改生产接口、业务行为、持久格式、设备协议或公开 Engine 契约。
- 不一次性移动现有测试文件，不删除旧测试，不把全部测试改写为新 DSL。
- 不自研 nextest 已提供的测试发现、进程调度、重试、分片、超时或 JUnit。
- 不把测试工件上传到产品观测系统，不记录设备名、地址、邀请、令牌、正文或真实用户路径。

# 4. Current Architecture Context

```text
Component: Cargo/libtest
Path: Cargo workspace, .cargo/config.toml
Responsibility: 构建和运行 Rust 测试；当前默认 RUST_TEST_THREADS=1。
Relationship: 保持兼容；nextest 是新增执行入口，不删除 cargo test。
```

```text
Component: Existing test hosts and scripts
Path: tests/hosts/, scripts/testing/
Responsibility: 平台宿主、独立进程和真实网络验收。
Relationship: 后续可采用 uc-testkit 工件，但本轮不迁移其场景。
```

```text
Component: Deterministic virtual peer network plan
Path: docs/exec-plans/active/034-deterministic-virtual-peer-network-test-suite.md
Responsibility: 未来在 Application port seam 上验证成员业务拓扑。
Relationship: 使用本设计的身份、预算和报告基础；不由 uc-testkit 复制业务协议。
```

```text
Component: CI
Path: .github/workflows/pr-check.yml
Responsibility: 现有 checks、coverage 和 Linux connection recovery 门禁。
Relationship: 本轮只新增 testkit/nextest 非破坏 job 或 step；原 job 继续存在。
```

# 5. Proposed Design

## Components

### `uc-testkit::Scenario`

- 职责：场景生命周期唯一负责人，汇总身份、预算、阶段、事件、失败与清理并写入最终工件。
- 输入：稳定场景名、固定 seed、`ScenarioBudget`、静态复现命令和可选工件根目录。
- 输出：`ScenarioReport`、JSON 文件、人类摘要。
- 关系：辅助模块只提供能力；调用方不自行拼装 JSON 或重复决定终态。

### `ScenarioIdentity`

- 职责：验证并规范化场景名，生成只含安全字符的 `artifact_id`。
- 输入：仓库内静态场景名和 seed。
- 输出：稳定场景名、seed、artifact id。
- 约束：不接收设备名、路径、地址或凭据。

### `ScenarioBudget` 与 `StageTimer`

- 职责：保存 wall-clock 保护预算和阶段耗时。
- 输入：显式总预算；阶段名只能使用稳定、脱敏标签。
- 输出：毫秒耗时、是否超出预算。
- 约束：不表达产品 deadline；业务虚拟时间留给领域专属 test seam。

### `EventLog` 与 `wait_for_event`

- 职责：有界保存脱敏事件，使用 revision/watch 通知等待者，返回最后事件和未满足条件。
- 输入：稳定事件类别；等待谓词和预算。
- 输出：匹配事件或 `ScenarioFailure::ConditionTimeout`。
- 约束：禁止保存自由格式产品 payload；默认只保留最后 128 条。

### `TempDirLease` 与 `TcpPortLease`

- 职责：提供 0700 临时目录和由已绑定 listener 持有的 loopback 端口。
- 输入：场景资源标签。
- 输出：资源句柄和脱敏资源记录。
- 约束：端口不能通过“探测后释放”分配；目录真实路径不进入 JSON。

### `ArtifactWriter`

- 职责：以拒绝覆盖方式创建场景目录，原子写入 `result.json` 和 `summary.txt`。
- 输入：完整 `ScenarioReport`。
- 输出：相对工件位置或 `FrameworkArtifact` 失败。
- 约束：工件根由调用方显式提供；统一脚本读取 `UC_TEST_ARTIFACTS_DIR`，未设置时在 workspace `target/test-artifacts` 下创建独立运行目录。

### nextest profiles

- 职责：通过现有 binary/package/path 模式做最小可维护分组。
- 输入：`.config/nextest.toml` 和统一 shell 脚本。
- 输出：JUnit、slow/timeout/retry 结果和退出码。
- 约束：当前没有源码标签时不移动大量文件；按 package、binary 和测试名映射，后续随自然拆分改进。

## Data Model

`ScenarioReport` 使用版本化 JSON：

```json
{
  "schema_version": 2,
  "scenario": "testkit-success-demo",
  "artifact_id": "testkit-success-demo-seed-000000000000002a",
  "artifact_directory": "testkit-success-demo-seed-000000000000002a-run-1234-0001",
  "seed": 42,
  "outcome": "passed",
  "failure": null,
  "last_event": { "sequence": 2, "kind": "condition-ready" },
  "stages": [{ "name": "wait", "elapsed_ms": 3 }],
  "resources": [{ "kind": "temp-dir", "label": "profile", "cleanup": "completed" }],
  "cleanup": "completed",
  "total_elapsed_ms": 8,
  "reproduce": "cargo nextest run -p uc-testkit -E 'test(...)'"
}
```

`FailureKind` 固定为：

- `product_invariant`
- `product_timeout`
- `environment_unavailable`
- `resource_collision`
- `driver_protocol`
- `fixture_invalid`
- `framework_artifact`
- `cleanup_failed`

testkit 自身通常只产生后六类。产品测试调用方可以显式提交前两类，但 testkit 不推断产品语义。

## API / Interface

```rust
pub struct ScenarioConfig { /* stable name, seed, budget, reproduction, artifact root */ }

pub struct Scenario { /* owns recorder, resources and final report */ }

impl Scenario {
    pub fn start(config: ScenarioConfig) -> Result<Self, ScenarioFailure>;
    pub fn event(&self, kind: &'static str);
    pub fn stage(&self, name: &'static str) -> StageGuard;
    pub async fn wait_for_event(
        &self,
        condition: &'static str,
        predicate: impl Fn(&ScenarioEvent) -> bool,
    ) -> Result<ScenarioEvent, ScenarioFailure>;
    pub fn temp_dir(&mut self, label: &'static str) -> Result<TempDirLease, ScenarioFailure>;
    pub fn tcp_port(&mut self, label: &'static str) -> Result<TcpPortLease, ScenarioFailure>;
    pub fn finish(self, result: Result<(), ScenarioFailure>) -> Result<ScenarioReport, ScenarioFailure>;
}
```

`finish` 必须尝试写工件。若原始结果失败且工件写入也失败，保留原始失败为 primary，并把工件失败作为 secondary failure；不得覆盖产品失败。

## Workflow

1. 测试用静态名称、固定 seed、预算和复现命令创建 `Scenario`。
2. `Scenario` 创建安全工件目录并开始总计时。
3. 测试按需取得临时目录或绑定端口，资源由 lease 持有。
4. 测试通过 `StageGuard` 记录阶段，通过 `EventLog` 记录稳定事件。
5. `wait_for_event` 只在新事件到达时重新检查条件，wall-clock 预算到期时返回包含最后事件的稳定失败。
6. 测试显式释放或交还资源；`finish` 记录清理状态和最终结果。
7. `ArtifactWriter` 写 JSON 和人类摘要；测试可以断言报告，也可由 CI 上传目录。
8. nextest 收集进程结果和 JUnit；两类报告通过场景名关联，不互相替代。

同一 `artifact_id` 的并行或重复运行使用不同 `artifact_directory`。稳定身份用于关联，实例目录用于定位实际文件；调用方不再自行拼接 PID。独立子进程通过 Scenario 的有界运行入口完成 spawn、wait、超时 kill 和再次 wait，退出码的业务含义仍由调用场景决定。

## Test Layers and Groups

| 组 | 边界 | 当前映射原则 | 默认执行 |
| --- | --- | --- | --- |
| `fast` | Core/Application 纯规则与 uc-testkit 自测 | package/binary 默认集合，排除下面慢组 | 本地与 PR |
| `persistence-provider` | SQLite、AEAD、OpenMLS、Iroh provider contract | Infra 指定 binary/name pattern | PR 按影响，主线完整 |
| `engine-smoke` | Engine 公开装配和短链路 | `uc-engine` 指定 integration binary | PR |
| `process` | 独立进程、崩溃、重启 | host package/binary | 相关 PR 与主线 |
| `real-network` | namespace、relay、真实丢包、兼容版本 | 独立脚本/workflow，不伪装普通 nextest 单测 | 相关 PR smoke、nightly 完整 |
| `device` | 模拟器与实体设备 | 平台脚本和 device matrix | RC/release |

## Configuration Format

- `.config/nextest.toml` 保存 profile、test group、override、JUnit、slow timeout 和 retry。
- `scripts/testing/run-test-group.sh <group>` 是统一本地入口，只翻译稳定组名为 nextest expression 或现有脚本。
- nextest 缺失时脚本明确失败并给出固定安装命令，不静默退回不同语义。
- `cargo test` 继续可直接运行；本轮不修改其测试集合。
- `UC_TEST_ARTIFACTS_DIR` 只控制工件根目录，不改变测试行为。

## CI Integration

- PR workflow 新增独立 `testkit` job：安装固定 cargo-nextest 版本，运行 `fast` 中的 `uc-testkit` 自测与示范，上传 JUnit 和结构化工件。
- 现有 checks、coverage 和 connection-recovery job 保持不变。
- 后续组迁移必须先双轨运行并核对测试数量、耗时和失败差异，再决定是否替换旧入口。
- CI retry 仅由 nextest profile 配置；默认 profile 不重试。环境型 nightly 未来最多重试一次，retry-pass 单独统计。

# 6. Implementation Plan

## 长期路线

本路线只服务 Engine。Desktop、Mobile 的界面自动化、跨端操作编排和产品宿主平台不进入 testkit；Engine
自身的多节点、网络、存储、独立进程与重启恢复属于范围。测试作者只描述节点准备、业务动作和最终期望，kit
统一承担节点生命周期、故障施加、事件等待、资源隔离、清理和诊断。kit 不解释领域消息，也不复制业务状态机。
简单、快速且没有资源生命周期的单元测试继续保持普通 `cargo test`。

### 两条运行线

| 运行线 | 环境与用途 | 完成边界 |
| --- | --- | --- |
| 快速确定性线 | 日常与 PR；使用真实 Application 负责人、可控时间和消息传递、固定 seed、独立临时资源 | 单场景尽量不超过 1 秒，整组不超过 1 分钟，均不含编译 |
| 真实环境线 | nightly 与手工单场景；实际 Engine 多进程、独立身份/资料/端口、真实存储和真实网络 | 环境准备、场景和清理合计不超过 30 分钟；编译时间与整次总耗时另列 |

真实环境按“本机真实多进程 -> 隔离网络 -> 必要的真实远端环境”推进，优先复用 network namespace、现有
Iroh host 和成熟系统工具。网络故障必须在对应真实环境中实际生效；mock 返回错误只能证明 provider 或业务
分类，不能登记为断网通过。并非所有场景都要求快速线与真实线使用同一 fixture，但两者必须通过覆盖映射说明
各自证明的边界。发布前检查近期 nightly 结果，并手工补跑受本次变更影响的关键场景。

### 首批业务覆盖

首批按以下顺序建立快速确定性证据和最小真实环境代表场景：配对；文字和文件传输；断线重连；重启恢复；旧资料升级。

以下矩阵是当前事实，不是目标声明。“部分”表示已有相关责任层证据，但尚无开发者只描述准备/操作/预期的完整
快速多节点场景；真实线的“已接入”只有在对应提交工件读回后才升级为“已验证”。

| 类别 | 快速确定性线 | 真实 nightly / 手工线 |
| --- | --- | --- |
| 配对 | **部分**：五个成员恢复场景覆盖最终确认重试、三设备可见性和旧候选收敛；034 只覆盖已准入两节点的成员历史分区/恢复。尚无完整 invitation -> settled 的快速多节点入口 | **当前提交已验证**：`E01-complete-pairing` 使用真实 Engine 多进程、独立资料和公开 setup/eligibility/peer 终态；direct 三节点 9.891 秒，relay 两节点 3.543 秒 |
| 文字与文件传输 | **未形成统一快速多节点场景**：保留既有 Application/Engine 组件测试，不能把真实 runner 的 E02 名称算作快速覆盖 | **当前提交已验证**：direct exact text 0.298 秒、双向 exact bytes 0.226 秒；relay 分别 0.134 秒和 0.242 秒 |
| 断线重连 | **未形成首批快速多节点入口**：034 的 membership message 分区不等于真实连接重建 | **当前提交已验证回归**：E03/E04/E06/E10/E13 实际施加 namespace/relay/known-peer 故障；四种模式工件均通过 |
| 重启恢复 | **部分**：`restart_continues_from_persisted_admission` 使用真实 Application 负责人和固定 seed；不是完整 Engine 进程重启 | **当前提交已验证回归**：E11/E12 停止/重建真实 Engine 并继续 exact text；direct 工件通过 |
| 旧资料升级 | **已有 focused 证据但不属于多节点模拟**：真实 synthetic storage migration 与 process crash recovery 18 项，本地测试累计 5.546 秒 | **workflow 已接入、默认分支未生效**：scheduled/手工 `profile-upgrade` 复用同两项 binary；alpha.5 外部完整 fixture 未验证 |

### 当前可复用入口

| 入口 | 测试作者描述 | 框架承担 | 当前限制 |
| --- | --- | --- | --- |
| `Scenario` + Application fixtures | 固定 seed、调用一个真实负责人、最终公开状态 | 预算、阶段、事件等待、临时资源、清理、JSON/文本/JUnit 和复现命令 | 节点准备仍由各领域 fixture 提供，尚无覆盖五类的统一 topology API |
| `VirtualMembershipNetwork` | 注册两个已准入节点、send/partition/heal、预期 ACK/Offline | typed message 路由、frame 预算、脱敏 trace | 只覆盖成员历史，不负责 invitation、内容或连接生命周期 |
| `run-connection-recovery-e2e.sh --mode ... --case ...` | mode、场景前缀、repeat | Engine 进程、profile、身份、端口、namespace、relay、等待、清理和 JSON 工件 | Linux/root 环境；PR 全矩阵仍约 67 分钟，不属于快速线 |
| `engine-real-environment.yml` 的 `profile-upgrade` | 选择升级模式 | 固定 nextest、编译/场景/总耗时、JUnit 与 testkit 工件 | workflow 尚未进入默认分支，当前不能 workflow_dispatch |

选择迁移对象时，先处理这五类中最慢、最不稳定且诊断收益最高的测试；已有简单快速测试保持原样。t-0010
等活跃修复测试在其工作结束、覆盖映射和对照充分后再评估，不读取、修改或复制其工作区。新旧入口双轨期间，
旧测试和原门禁继续保持权威，不做全仓一次性迁移。

### 测量与失败原则

- 固定记录工具链、runner、操作系统/runner 类型、CPU 并发、样本数和是否为 warm build。
- 分别报告编译、环境准备、测试、清理和整次墙钟；不得只给测试本体耗时掩盖实际等待。
- 超出预算即失败；不得通过放宽断言、删除覆盖、增加固定 sleep 或自动重试把超时改成通过。
- 默认不自动重试。真实环境如按明确策略重跑，首次失败工件必须保留，retry-pass 单独登记。
- 无法稳定重现的真实网络失败应明确记录环境、首次证据和复现限制，不改写为普通通过。

### 报告与 CI

不建设独立报告站点。GitHub Checks 摘要提供结果、耗时和工件链接，下载工件保存 JUnit、结构化 JSON、人类
摘要与脱敏日志。失败证据至少包含场景、当前步骤、预期/实际、各节点稳定状态投影、阶段耗时、复现命令、
资源清理结果和相关日志位置。真实环境支持 workflow 手工选择单个场景；nightly 运行完整受控矩阵。

### 计划关系与实施顺序

| 计划 | 角色 | 当前状态 |
| --- | --- | --- |
| 044 | testkit、nextest 分组、结构化报告和 CI 旁路基础 | 已完成 |
| 045 | 五个成员恢复确定性场景，证明 Application 真实负责人接入 | 已完成 |
| 046 | provider、独立进程和持久恢复的真实依赖代表场景 | 已完成 |
| 047 | draft PR、远程工件核验和至少 10 个工作日的自然趋势观察 | 首轮完成；长期观察未完成 |
| 048 | 首次使用指南、采用清单、并行工件隔离和有界子进程 | 已完成；远程工件已复验 |
| 034 | 快速线的确定性成员网络与真实环境五类映射 | 最小成员网络已完成；完整快速五类仍未完成；真实 E01/E02 与 nightly 基础已接入 |

后续顺序为：读回当前 E01/E02 文件工件 -> 只在实际收益明确时补完整配对或传输的快速 Application 场景 ->
默认分支生效后取得分 mode nightly 与 profile-upgrade 的准备/场景/清理实测 -> 在跨工作日数据充分后再评估旧入口。
每一步都先保留可回退的旧门禁。

### “kit 首版完成”与“全仓迁移完成”

kit 首版完成只表示以下交付可用：贡献者指南和可运行示例；稳定分组和 GitHub 实跑；成功、业务失败、环境失败、
driver/framework/cleanup 失败诊断；并行资源隔离；有界等待与子进程回收；旧入口兼容。044-048 已完成这些能力及
远程工件复验；这不包含完整快速五类、默认分支 nightly 长期样本或全仓迁移。

全仓迁移完成是独立的长期结果：首批五类及后续入选场景均有覆盖映射、双轨对照和真实环境责任；旧入口只有在
自然运行样本、诊断收益和回退条件全部满足后才可删除；真实网络和设备未执行时仍必须标为未验证。首版完成不
代表全仓迁移完成，也不代表 nightly 长期稳定。

### 可验证完成标准

- 新贡献者按指南能运行最小成功与受控失败示例，并定位 JUnit、JSON、摘要和复现命令。
- 同一身份场景并行运行不覆盖工件，临时目录、端口和子进程均有明确清理结果。
- draft PR 的远程 evidence job 实际上传并可下载上述工件；不能只以 workflow 绿色验收。
- 快速单场景和整组、nightly、编译与整次总耗时均按本节口径报告。
- 五类业务覆盖有新旧映射；真实网络故障只由实际网络环境证明。
- 10 个工作日趋势使用自然运行样本，不用同日重复运行凑数。

044-048 维护已交付切片和实际证据；034 维护下一阶段确定性多节点实现。本设计是长期路线的唯一事实来源，不再
为同一范围创建另一份并行路线图。

# 7. Edge Cases

```text
Scenario: 场景名包含路径分隔符、空白或敏感文本。
Expected behavior: 创建阶段以 FixtureInvalid 拒绝，不创建工件。
Implementation: 只允许小写 ASCII、数字和单连字符，长度有界。
```

```text
Scenario: 条件等待前事件已经发生。
Expected behavior: 先检查有界历史并立即成功，不要求再次发送事件。
Implementation: wait_for_event 在订阅 revision 后先读取快照，再等待变化，避免丢失唤醒。
```

```text
Scenario: 等待超时且尚无事件。
Expected behavior: ProductTimeout 或 DriverProtocol 由调用方选择；报告明确 last_event=null。
Implementation: testkit 默认产生 DriverProtocol/ConditionTimeout，不猜测产品状态。
```

```text
Scenario: 端口 lease 仍存活时另一个场景请求端口。
Expected behavior: 系统分配不同端口；不得发生探测后竞争。
Implementation: listener 从 bind 到交给调用方一直保持打开。
```

```text
Scenario: 原始断言失败后工件写入失败。
Expected behavior: 原始失败保持 primary，报告 secondary framework_artifact；退出仍失败。
Implementation: ScenarioFailure 支持一个有界 secondary 列表，不拼接任意错误文本。
```

```text
Scenario: 测试 panic，未调用 finish。
Expected behavior: RAII 资源仍释放；本轮不承诺完整 JSON，nextest/JUnit 记录 panic。
Implementation: 后续可增加 panic hook 集成；V1 不用全局 hook 改变其他测试。
```

# 8. Testing Strategy

## Unit Tests

- 场景名与 seed 生成稳定 artifact id。
- 固定 seed 和同一输入生成相同复现信息。
- 事件在等待前/后到达都能匹配；超时携带条件和最后事件。
- 阶段计时只记录稳定标签，结束顺序正确。
- 临时目录权限和 Drop 清理通过。
- 端口 lease 在持有期间不可被第二 listener 绑定，释放后可复用。
- failure kind JSON 格式稳定，未知敏感字段不进入报告。
- JSON 与摘要写入成功，拒绝覆盖已有场景工件。

## Integration Tests

- 成功示范：异步任务发布两个事件，条件等待完成，资源释放，报告为 passed。
- 故意失败示范：等待不存在的事件，在短预算内失败；命令本身验证失败报告后成功退出，工件包含条件、最后事件、阶段耗时、复现命令和路径。
- 同一示范分别通过 `cargo test` 与 nextest 运行，证明兼容。

## Regression Tests

- `cargo metadata --locked` 和 workspace all-target check 证明新增 crate 装配正确。
- 现有一个与框架无关的快速测试继续通过，证明未替换 cargo test。
- nextest 实际测试数量非零；JUnit 和 JSON 工件存在且可解析。

# 9. Acceptance Criteria

- [x] `uc-testkit` 不依赖任何产品 crate，生产 crate 不依赖 `uc-testkit`。
- [x] 成功和故意失败示范真实执行并生成 JSON 与摘要。
- [x] 失败示范在 1 秒内结束，并包含未满足条件、最后事件、阶段耗时、seed、复现命令和工件位置。
- [x] 临时目录、端口和清理结果自测试通过。
- [x] `.config/nextest.toml` 能区分六类测试边界，未迁移的组明确使用现有脚本或保留为空映射。
- [x] `run-test-group.sh fast` 实际运行非零测试并生成 JUnit。
- [x] 现有 `cargo test` 入口仍可运行。
- [x] CI 新入口不删除或放宽任何现有门禁。
- [x] 报告不包含设备名、地址、邀请、令牌、正文或真实临时路径。
- [x] 仓库静态检查与相关测试通过；设备和真实网络未执行时明确记为跳过。

# 10. Risks and Trade-offs

- 独立 crate 增加一个 workspace target，但换来跨层复用且不污染产品模块。
- JSON 和 JUnit 是两份证据：前者表达场景语义，后者表达 runner 结果。合并为一份会迫使 testkit 复制 nextest 能力，因此保持关联而不合并。
- V1 对 panic 只依赖 RAII 和 JUnit，不保证写完整场景 JSON；全局 panic hook 会影响所有测试，暂不引入。
- Scenario 在 `finish` 前 panic/abort 时仍只依赖 RAII 与 JUnit；本轮增加的子进程入口处理受控异常退出和超时，不安装全局 panic hook。
- 路径/名称模式分组不如显式标签精确，但当前无需大规模移动文件。后续自然拆分测试 binary 后再收紧映射。
- 本轮只提供 wall-clock 保护，不提供业务虚拟时间。需要生产接口时由规格 034 单独实施，避免扩大产品范围。

# 11. Open Questions

- 仓库未提供 `CONTEXT.md`；若后续新增，应核对其测试命名和 CI 约束是否需要回写本文。
- 真实网络和 device 目前由脚本/workflow 管理；是否在未来统一生成同一 `ScenarioReport` schema，留到首批迁移后按实际需要决定。
- nextest 已固定为 `0.9.145` 并在当前 Rust 1.95 工具链实跑；升级策略随工具链更新维护。
