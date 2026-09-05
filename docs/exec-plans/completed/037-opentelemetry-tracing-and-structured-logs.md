# 规格 037：OpenTelemetry tracing 与结构化日志 clean cutover

## 状态

- **状态**：已完成
- **日期**：2026-09-05
- **前置规格**：[035 Space 观测装配 interface 收敛](../completed/035-space-domain-observability-assembly.md)、[036 关键模块深化与退役路径 clean cutover](../completed/036-architecture-deepening-clean-cutovers.md)
- **取代计划**：[移动端日志文件层](../completed/mobile-log-file-layer.md)中已经实施的文件日志部分已在 Slice 0 校准归档；未完成的保留、导出和远程发送由本规格取代
- **完整负责人**：宿主进程拥有唯一 `ProcessObservabilityRuntime`，负责 subscriber、OpenTelemetry providers、系统/文件/远程 sinks、过滤、批处理、刷新与关闭；`uc-engine::assembly::observability` 继续只负责跨层完整 capability 的稳定耗时与结果分类；Application 只负责业务流程，Infra 只负责具体能力与受认证网络上下文传播，Core 不感知观测
- **调用方唯一动作**：宿主在创建任意 Engine 前用一份宿主配置安装进程级观测运行时，并在平台生命周期节点调用其完整 `flush` 或最终 `shutdown`；业务调用方继续只调用现有 Engine operation，不提交阶段、计时或日志字段
- **成功结果**：系统日志与本地有界 JSONL 使用同一安全记录合同，OTLP traces 与 OTLP logs 另共用同一 Resource；本地以 Jaeger 验证 trace、以 Collector 可解码 sink 验证 logs，生产 Collector 优先输出到 PostHog；在线跨设备调用通过 W3C Trace Context 形成父子 trace，Space 准入重试/重启通过匿名 `uc.flow.id` 聚合；产品 analytics 保持独立
- **失败结果**：配置冲突或进程已有其他 subscriber 时安装明确失败；运行时安装完成后，编码、排队、上传、刷新或关闭失败只产生限频本地健康结果，不改变配对、同步、持久化或生命周期结果；队列满时丢观测数据，不反压业务
- **重试与重启责任**：OTLP batch processor 只负责进程内有界批量；HTTP client 提供单次有界发送，移动进后台执行有时限的 `force_flush`，恢复后复用同一 provider；第一版不持久化或重放远程队列，进程终止时未发送数据允许丢失；业务恢复仍由原 Application/Infra owner 负责

## 研究依据

- [OpenTelemetry signals](https://opentelemetry.io/docs/concepts/signals/)
- [Context propagation](https://opentelemetry.io/docs/concepts/context-propagation/)
- [Logs data model](https://opentelemetry.io/docs/specs/otel/logs/data-model/)
- [Rust OpenTelemetry implementation](https://github.com/open-telemetry/opentelemetry-rust)
- [Rust OTLP production example](https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-otlp/examples/basic-otlp/README.md)
- [tracing-opentelemetry](https://github.com/tokio-rs/tracing-opentelemetry)
- [Collector Gateway deployment](https://opentelemetry.io/docs/collector/deploy/gateway/)
- [Sensitive data handling](https://opentelemetry.io/docs/security/handling-sensitive-data/)
- [PostHog 官方 Collector 路由](https://github.com/PostHog/posthog/blob/master/docker-compose.base.yml)
- [PostHog 官方认证说明](https://github.com/PostHog/posthog.com/blob/master/contents/docs/metrics/index.mdx)

# 1. Overview

当前仓库有 `tracing` span/event、产品 analytics、OSLog/Logcat、本地文本文件和名为 `uc_otlp` 的阶段日志，但没有
OpenTelemetry SDK、logs bridge、trace layer 或 OTLP exporter。`crates/uc-observability-contract/src/otlp.rs` 只是调用
`tracing::info!`，不能把数据发送到 Collector，也不能自动产生 TraceId/SpanId。Apple/Android binding 会安装全局
subscriber，但安装失败被忽略；HarmonyOS 和直接 Rust 宿主没有对等安装。文件层按天滚动但没有保留上限，诊断导出又不识别
移动文件名。

当前职责也不一致。规格 035 曾把 Space 的调用级依赖计时收口到 Engine decorator，但这仍让 Engine 理解准入状态、准备和激活
子步骤；本规格最终只保留完整能力和认证 endpoint，跨步骤关联改由 Application 完整 owner 提供不透明作用域。Application 与
Infra 仍有大量直接 span、手工 `Instant` 和日志，部分 info 记录包含 Space、设备、摘要、地址或真实路径，会进入系统日志和明文
本地文件。现有架构检查没有覆盖全部 Infra、错误正文和真实 sink 输出。直接接远程 exporter 会扩大泄露面。

本规格建立一个最小而完整的长期架构：先固定安全 schema 和真实进程级输出运行时，再用 Clipboard 建立第一个跨设备
tracer bullet；之后可并行迁移 Space 配对和补齐移动平台输出，最后一次性删除伪 OTLP、旧阶段计时和临时关联代码。
当前 Engine 尚未发布，wire 与公开观测 bootstrap 可以 clean cutover，不增加兼容版本号，也不保留第二发送路径。

# 2. Goals

- 建立真实 OTLP traces 与 OTLP logs 管道，并用本地 Collector 或可解码 OTLP 接收端证明数据实际到达。
- 保留 `tracing` 作为 Rust 统一埋点入口，分别用官方 trace layer 和 logs bridge 输出两个 signal。
- 让一次在线跨设备请求使用 W3C Trace Context 形成真实父子 trace；让 Space 准入的断网、重试和重启使用独立匿名 `uc.flow.id` 聚合。
- 固定 Core、Application、Infra、Engine、Binding/Host 与 Collector 的唯一责任，不为观测新增业务步骤查询或 Engine 编排。
- traces 与 logs 共用同一 Resource，日志在 span 内自动获得 TraceId/SpanId；字段名称、类型和失败分类单一。
- 产品 analytics 保持独立身份、事件和供应商语义，不进入运行诊断 trace/log，也不共享 flow id。
- 所有远程记录只来自明确 allowlist；本地系统/文件日志也不得包含仓库禁止的内容、身份、地址、路径或原始错误正文。
- 远程发送使用官方有界 batch processor；Collector 不可达、队列满和 shutdown 超时均不阻塞业务。
- 本地文件改为有界 JSONL，能够被现有诊断导出完整发现；旧文本命名和过期 active plan 不长期保留。
- 用架构门禁和真实 sink 测试阻止伪 OTLP、双 subscriber、Application 手工持续计时、业务步骤泄露和敏感字段回流。

# 3. Non-Goals

- 不在本规格实现 metrics、profiles、移动端可靠离线遥测队列或遥测磁盘重放。
- 不把 OpenTelemetry、W3C context、exporter 或 Collector 类型引入 Core 或 Application 公共接口。
- 不为观测新增 Application recorder/callback port，不让 Engine 查询 admission、membership、transition 或其他内部阶段。
- 不把约 200 个历史 tracing 文件一次性机械重写；本规格先完成新架构、Clipboard 和 Space 两条关键流程，并清理所有已知
  会进入持久/远程 sink 的敏感记录。其余低风险本地 debug 按 inventory 进入后继计划。
- 不改变产品 analytics 事件名、属性、身份合并或供应商 sink；`usage_analytics_enabled` 的现有行为不在本规格修复。
- 不让 Collector 成为设备侧隐私检查的替代品；Collector 只提供第二道过滤、限流和后端路由。
- 不保存旧 `traceparent` 以跨重启延续已经结束的 trace，也不使用 baggage 传播 flow、设备、Space 或成员信息。
- 不让观测失败改变配对、同步、存储升级、session transition、suspend/resume/shutdown 的稳定结果。
- 不在未完成真实移动构建前声明 iOS、Android 或 HarmonyOS 通过；未运行的设备项目只能标为“跳过”。

# 4. Current Architecture Context

```text
Module: Product analytics contract
Path: crates/uc-observability-contract/src/analytics/
Responsibility: 类型化产品事件、身份与宿主 sink 合同。
Relationship: 保留现状；不得与运行诊断资源、TraceId、SpanId 或 flow 合并。
```

```text
Module: Pseudo-OTLP timing
Path: crates/uc-observability-contract/src/otlp.rs、src/stages.rs
Responsibility: 当前把 Clipboard 阶段写成 target=uc_otlp 的普通 tracing event。
Relationship: 没有 SDK/exporter；由真实 trace/log pipeline 和 Engine decorator clean cutover 后删除。
```

```text
Module: Space observability assembly
Path: crates/uc-engine/src/assembly/observability/
Responsibility: 按 Application-owned bundle 装饰真实依赖，记录调用耗时、结果、失败分类与降噪。
Relationship: 规格 035 已完成并由架构门禁固定；本规格保留其 module/interface 形状，只统一 schema、span 和 exporter。
```

```text
Module: Clipboard tracing
Path: crates/uc-application/src/clipboard/、crates/uc-core/src/ports/clipboard/sync_dispatch.rs
Responsibility: 当前由 Application 生成 flow id、创建 span、手工计时并把 DispatchTiming 暴露到调用结果。
Relationship: 观测复杂度已泄露到 Core/Application 公共业务接口；作为第一条跨设备 clean-cutover 路径。
```

```text
Module: Iroh wire adapters
Path: crates/uc-infra/src/network/iroh/
Responsibility: 具体认证网络、frame codec、P2P handler 和地址解析。
Relationship: 标准 trace context 的注入、边界、认证后提取属于 Infra；Application/Core 不理解 traceparent/tracestate。
```

```text
Module: Mobile tracing bootstrap
Path: bindings/uc-engine-uniffi/src/apple.rs、android.rs、file_log.rs、runtime.rs
Responsibility: 当前安装 OSLog/Logcat 与 info+ 日滚动文本层。
Relationship: subscriber 是进程资源但 provider owner 缺失；无 OTLP、无容量上限、重复安装失败静默。
```

```text
Module: HarmonyOS binding
Path: bindings/uc-ohos-napi/
Responsibility: N-API Engine/host 转换。
Relationship: 当前没有 subscriber、文件层或 OTLP 生命周期，必须独立验收。
```

```text
Module: Diagnostics export
Path: crates/uc-application/src/settings/diagnostics.rs
Responsibility: 导出受管诊断日志。
Relationship: 当前文件名 allowlist 不包含移动 engine.*.txt；新 JSONL sink 与导出必须由一个完整 owner 解释。
```

```text
Module: Repository architecture preflight
Path: scripts/architecture/check-engine-repository.mjs
Responsibility: 依赖、装配、隐私和 retired 路径检查。
Relationship: 当前只覆盖有限文本模式；未阻止 Infra 敏感日志、伪 OTLP、双 exporter 或业务步骤查询。
```

当前信号流：

```text
tracing span/event
  -> Apple OSLog 或 Android Logcat
  -> 可选 info+ 文本文件

target=uc_otlp event
  -> 仍走上面的普通日志

AnalyticsPort event
  -> 宿主产品分析 callback
```

目标信号流：

```text
tracing span  -> tracing-opentelemetry       -> Trace SDK -> BatchSpanProcessor -> OTLP
tracing event -> opentelemetry-appender      -> Logs SDK  -> BatchLogProcessor  -> OTLP
             \-> 系统日志
             \-> 有界本地 JSONL

OTLP/HTTP protobuf -> 受认证 Collector Gateway -> 后端 traces/logs
AnalyticsPort      -> 原产品分析供应商（完全独立）
```

# 5. Proposed Design

## Components

### Observability schema contract

- **位置**：`crates/uc-observability-contract/src/diagnostics/`；现有 `analytics/` 保持独立。
- **职责**：固定 `TelemetrySchemaVersion`、`FlowId`、资源字段、领域/操作/结果/失败枚举、字段 allowlist 与隐私分类。
- **输入**：Engine/Infra 已持有的稳定枚举、计数和时长；Application 完整准入 owner 已有的固定长度随机 attempt 材料。
- **输出**：只含批准字段的 trace/log attribute 值；不发送、不安装 subscriber、不持有 provider。
- **关系**：Core 不依赖 diagnostics；Application 不调用 raw tracing/OTel，只开启不可读取的不透明准入作用域；Engine decorator
  和 Infra wire adapter 消费固定记录合同。

`FlowId` 的长期语义是“跨重试/重启聚合同一业务尝试”，不是 TraceId。schema v1 只有 Application 完整准入恢复 owner 能用已有
32-byte 随机 attempt 材料开启异步作用域；合同内部以 domain-separated SHA-256 截取 128 bit。作用域不返回 FlowId，Engine、Infra、
Core 和公开接口均没有构造或读取入口。

### ProcessObservabilityRuntime

- **位置**：新增内部 crate `crates/uc-observability-runtime/`；`uc-engine` 只重导出宿主需要的稳定 bootstrap facade，绑定仍只
  依赖 `uc-engine`。
- **职责**：构造 Trace/Logs providers、OTLP/HTTP exporters、batch processors、容量与失败计数、filters、Resource、本地 JSONL
  与 provider 生命周期；安装进程级唯一 subscriber。
- **输入**：宿主拥有的 `ObservabilityConfig`：是否远程发送、Collector endpoint、脱敏认证、环境、发布渠道、本地日志目录。
- **输出**：`ProcessObservabilityHandle`：`force_flush(deadline)`、`shutdown(deadline)`、只读 health summary；health 分别给出远程
  trace/log 发送前丢弃总数、发送失败批次和本地文件丢弃数。发送前丢弃的首个原因区分格式拒绝、锁争用、队列满和已关闭。
- **关系**：Engine 实例不持有 provider；一个进程内多个 Engine 复用同一 handle。安装已存在且配置一致返回 `Reused`，配置
  不一致返回稳定 `AlreadyInstalled`，不得静默成功。

版本组一次锁定：OpenTelemetry crates `0.32.x` 与 `tracing-opentelemetry 0.33.x`。实施前用 Rust 1.95 和全部目标平台编译
确认；版本组不拆开升级。远程使用 HTTPS OTLP/HTTP protobuf。认证值全程 secret wrapper，Debug、错误和日志不得出现
endpoint、header 或 token。

### Platform output adapters

- **Apple**：保留 OSLog；使用共同 JSONL 与远程 layers。
- **Android**：保留 Logcat；使用共同 JSONL 与远程 layers。
- **HarmonyOS**：新增进程级安装入口；共同 JSONL 与远程 layers 不复制。当前系统输出使用通用 JSON fallback，HiLog/实体设备输出
  未实现，不把 N-API host adapter 写成既有能力。
- **Direct Rust host**：通过 `uc-engine` 稳定 bootstrap facade 安装 generic/system layer；同一进程不得再安装第二 subscriber。

本地文件固定 `engine.YYYY-MM-DD.jsonl`，只记录 info+，保留 7 天且总量上限 100 MB；启动与每日切换时按最旧优先清理。
目录或 writer 失败降级为系统+远程，不影响 Engine。诊断导出只通过同一 owner 的文件枚举方法读取，删除旧前缀猜测。

### Engine observability assembly

- **位置**：保留 `crates/uc-engine/src/assembly/observability/<domain>.rs`。
- **职责**：完整 capability 的 client/internal span、调用耗时、稳定 outcome/error type、slow-success policy；只有既有完整 port
  调用才在此装饰。
- **接口**：Clipboard 和成员网络按真实完整 port 装饰；Space 准入只装饰现有完整认证 endpoint，不包装 adapter bundle 内部步骤。
- **限制**：不观测状态加载、提交、材料准备或恢复子步骤，不解析业务标识，不添加步骤接口，不保留 raw adapter 绕过包装。

远程可用 span/event 的 target 统一为 `uc.telemetry`。原 `admission.performance`、`membership.performance`、
`storage.performance`、`uc_otlp` 在迁移结束后删除。普通模块 target 默认不进入系统、JSONL 或远程受管输出。

### Core task lifecycle result

`uc-core::TaskRegistry` 删除 tracing 依赖和直接日志，`shutdown` 改为返回不含名称、路径或错误正文的纯
`TaskShutdownReport { completed_count, timed_out_count, join_error_count }`。Engine/Application 中实际拥有对应 runtime 的调用者
决定是否记录完整关闭结果。`uc-observability-contract::spawn_supervised` 不迁入 Core 或新 exporter runtime；Slice 4 按现有调用者
归属收回 Application 私有 support，Infra/compatibility 调用改由各自已有生命周期 owner 监督，不建立新的跨层通用 helper crate。

### Infra trace propagation

- **位置**：`crates/uc-infra/src/network/iroh/trace_context.rs` 与各协议私有 wire codec。
- **职责**：用官方 W3C propagator 注入/提取唯一 `traceparent`；限制长度；创建 remote parent。当前没有厂商状态需求，不传播
  `tracestate` 或 baggage。
- **认证**：接收方完成现有对端/消息认证后才使用 context。Clipboard context 由既有端到端认证 QUIC request 完整性保护；已有
  独立消息 MAC 的协议把 context 纳入该 MAC。不得为了观测修改 Application/Core 的内容加密 AAD。context 永不参与授权、
  消息摘要、幂等或业务结果。
- **失败**：缺失、损坏、超限、未知或未认证 context 全部忽略并创建本地 root span，业务消息按原结果继续。
- **生命周期**：trace context 只随一次在线请求传播，不持久化；重试/重启创建新 trace。只有 Application 完整准入 owner 的不透明
  作用域会让 client span 自动携带 `uc.flow.id`；Infra 与 Engine 不自行派生。没有合法作用域时省略。

不使用 baggage。Infra 不向 Application/Core 暴露 OpenTelemetry 类型；Application 看到的业务 message、result 和 error 不增加
trace 字段。

### Collector Gateway

- **位置**：`tests/observability/collector/` 保存固定测试配置与接收断言；生产 Collector 在部署仓维护。
- **职责**：后端路由、二次 allowlist、限流、tail sampling 和多目标输出。本地开发把 trace 输出到 Jaeger，并把 logs 输出到
  可解码测试 sink；生产优先输出到 PostHog Logs/Traces，客户端始终只连接 Collector。
- **限制**：设备侧必须先脱敏；Collector 不修复设备已经发送的敏感数据。
- **兼容策略**：生产部署在取得真实 PostHog 项目凭据后验证当时可用的 OTLP signal、endpoint 与认证；若暂不接收 trace 或 log
  中任一种，Collector 将该 signal 输出到第二个标准后端。037 所在环境无项目凭据，只验证无秘密模板和 Collector 合同并明确跳过
  真实投递；不得修改客户端 schema、wire context 或增加 PostHog 专用发送器。

## Data Model

### Resource

traces 与 logs 共用：

| Attribute | Value | Cardinality / privacy |
| --- | --- | --- |
| `service.namespace` | `uniclipboard` | 固定 |
| `service.name` | `uc-engine` | 固定 |
| `service.version` | 去除 build metadata 的受限 SemVer | 只允许 stable/alpha/beta/rc |
| `service.instance.id` | 每进程随机 UUID | 不跨进程稳定 |
| `deployment.environment.name` | development/test/staging/production | 固定枚举 |
| `os.type` | ios/android/macos/windows/linux/ohos/other | 固定枚举 |
| `host.arch` | 运行时从固定架构集合取得 | 不接受宿主自由字符串 |
| `uc.app.channel` | development/test/alpha/beta/stable/production | 固定枚举 |
| `uc.telemetry.schema.version` | 初始 `1` | 固定 |

禁止 `device.id`、设备名、profile、Space、用户身份或稳定硬件标识。

### Span and log attributes

| Signal | Attribute | Meaning |
| --- | --- | --- |
| span/log | `uc.domain` | 固定领域枚举 |
| span/log | `uc.operation` | 固定完整能力枚举 |
| span/log | `uc.role` | local/client/server/joiner/sponsor 等固定枚举 |
| span | `uc.flow.id` | 仅 Space 准入 Joiner client 可用的匿名 durable flow；可选，永不作为授权 |
| log | `event.name` | 固定事件名 |
| log | `uc.outcome` | ok/error/deferred/rejected/cancelled |
| log | `error.type` | 稳定错误类别，不是错误正文 |
| log | `duration_ms` | 完成事件的整数耗时；trace 自身保存开始/结束时间 |

字段一律使用点分层命名，不再同时维护 `flow_id` 与 `flow.id`。缺失字段直接省略，不写 `none`、`unknown-id` 或空字符串。

### Signals

- **Trace**：表示一次在线因果执行；client/server/internal span 使用真实父子关系。
- **Log**：表示离散完成、失败和恢复事实；当前 span 存在时自动附加 TraceId/SpanId。
- **Flow**：只作为 Joiner client span attribute 聚合跨重试、断网和重启，不冒充 TraceId；log 通过 TraceId/SpanId 关联当前 span。
- **Analytics**：产品使用与漏斗；拥有独立身份和事件 schema，不携带运行诊断 TraceId/FlowId。

同一个 tracing event 的完整远程副本只进入 OTel LogRecord。Trace layer 只接收通过元数据检查的 span，不记录任何 span event；
log layer 独立检查并接收固定事件，避免日志与 trace 双份存储。

## API / Interface

### Host-facing bootstrap

```text
ProcessObservabilityRuntime::install(config) -> Installed(handle) | Reused(handle) | InstallError
handle.force_flush(deadline) -> FlushSummary
handle.shutdown(deadline) -> ShutdownSummary
```

该 facade 是进程基础设施，不属于 Engine operation。移动 `suspend` 成功后 binding 在非 UI 线程调用有界 flush；`resume`
复用 provider；单个 `MobileEngine::shutdown` 只 flush。只有宿主确定进程退出时才 shutdown process provider，shutdown 后不得
在同一进程复活。

当前 `telemetry_enabled` 未接任何生产 gate。用户已确认由宿主拥有唯一远程诊断许可；本规格将远程诊断许可迁移到宿主启动配置，并从
Core/Application/Engine settings DTO 一次性删除 `telemetry_enabled`；不建立 host/Engine 双开关。产品仓须在升级同一
Engine revision 时把原 UI 偏好迁移到宿主配置。`usage_analytics_enabled` 保持独立，不复用此字段。

### Existing business interfaces

- Application facade、port、result 不增加 trace/step/timing 字段。
- Engine decorator 只使用接口已有完整输入/输出建立 span/event。
- Infra wire context 是私有 transport metadata，不进入 Core/Application message。
- 若一个完整生命周期无法从现有 seam 关联，先把其完整 intent owner 设计正确；禁止新增步骤查询只为取得 flow id。

## Workflow

### Process startup

1. Host 读取自己的远程诊断许可与 Collector 配置。
2. Host 安装一次 ProcessObservabilityRuntime；本地 sinks 先可用，远程 layer 失败只降级。
3. Host 创建 Engine；Engine/Infra 发出的批准 span/event 进入同一 subscriber。
4. 每个 Engine 实例继续独立启动和关闭，不拥有 process provider。

### Cross-device request

1. Application 完整准入恢复 owner 用既有随机 attempt 材料开启不可读取的异步关联作用域。
2. Infra 建立 client transport span，作用域自动附加匿名 `uc.flow.id`，再把 W3C context 注入受认证 wire metadata。
3. 对端 Infra 完成业务认证后提取 remote context并建立 server transport span。
4. Engine 在既有完整认证 endpoint 上建立 `space_admission` internal 子节点，原样转发消息与结果，不读取内容、编号或步骤。
5. 调用树形成 `network_transport client -> network_transport server -> space_admission sponsor endpoint`；完成日志自动关联当前 span。
6. 下一次后台重试建立新 TraceId，但同一持久 attempt 的 client root 保留同一 flow；server 与 endpoint 不重复携带 flow。

### Mobile lifecycle

1. `suspend` 先完成 Engine 暂停，再在 binding worker 执行有界 flush。
2. 超时或失败只写本地限频 exporter health，不改变 suspend 结果。
3. `resume` 复用同一 provider，不重复安装 subscriber。
4. Engine shutdown flush 但不关闭进程 provider；宿主进程最终退出才 shutdown。

## Privacy and filtering

远程 layer 只允许 target=`uc.telemetry` 和固定 schema。`hyper`、`h2`、`tonic`、`reqwest`、OpenTelemetry exporter 自身
target 从远程层排除，避免递归观测；本地只保留固定 `exporter_error_kind` 与丢弃计数，不记录 endpoint 或错误正文。

设备侧运行时测试必须扫描系统 writer、JSONL 与原始 OTLP protobuf，拒绝：剪贴板内容、密码、密钥、令牌、邀请、设备名、
地址、文件名、路径、profile/Space/member/device/entry/transfer 原始 ID、payload/digest、原始错误正文及其可恢复派生值。

### Sampling

- 设备使用 `ParentBased(root=AlwaysOn)`，保证跨设备父子关系完整且不在设备实现结果感知 sampler。
- 本地 Collector 保留全量 trace，便于开发和端到端验收。
- 生产 Collector 使用 tail sampling：错误 trace 全部保留，其他 trace 固定保留 10%；日志不依赖 trace sampling。
- 本地系统/JSONL与远程采样独立，远程关闭不删除本地诊断。

### Performance and overload

- 使用官方 BatchSpanProcessor 与 BatchLogProcessor，不使用生产 SimpleProcessor。
- 官方 processor 使用固定 2,048 条队列和 512 条批次；容量门使用相同上限并通过压力测试确认。
- 队列满只丢观测数据；业务线程不得等待 exporter 网络。
- 本轮以双设备配对作为量化门禁：disabled 远程层的 p95 增量不超过 2%；启用且 Collector 健康或不可达时增量不超过 5%，以同机 A/B 测量。同步只做功能回归，不用未采集的性能样本扩大量化结论。
- 每次交付记录二进制体积和常驻内存差值；没有数据前不设伪精确上限。

## Clean-cutover deletion inventory

- 删除 `uc-observability-contract/src/otlp.rs`、`stages.rs` 及其测试和 `target=uc_otlp`。
- 删除 Core 未使用的 `TraceMetadata` / `extract_trace`；Core 默认生产路径不再依赖 tracing。
- 删除 Core `TaskRegistry` 的 tracing feature/default dependency；以纯 `TaskShutdownReport` 保留关闭结果，由 Engine/Application
  runtime owner 记录。按调用者归属移除 contract crate 中的 `spawn_supervised`，不建立同义跨层 helper。
- 删除 Clipboard 公共 `DispatchTiming`、只为观测存在的 timing fields 和 Application 手工 `Instant`/阶段 recorder。
- 新 context wire 生效后删除旧 `ClipboardHeader.flow_id` 兼容写入与 synthetic fallback；当前未发布协议不保留双 writer。
- 删除当前未提交原型中的 `admission_flow_id*`、`admission_update_flow_id`、对 `PendingGroupUpdate.update_id()` 的字符串解析、
  对应 Engine `sha2`/`hex` 直接依赖和伪跨设备同名 span；不得把工作树原型写成既成架构。
- 重新实现配对时只保留合法 port decorator；session lifecycle 失败必须有稳定 error type，所有提前返回必须有 total outcome。
- 更新或关闭 `mobile-log-file-layer.md`，不保留 active/completed 双副本；删除旧 `engine.*.txt` writer 和诊断导出前缀猜测。
- 清理已知包含路径、地址、原始 ID、摘要或 `%error` 的持久级日志；剩余 local debug inventory进入明确后继项。

# 6. Implementation Plan

## Dependency graph

```text
Slice 0 真实基线与隐私红线（串行）
  -> Slice 1 单机真实 OTLP tracer bullet（串行，冻结契约/依赖）
    -> Slice 2 Clipboard 跨设备 context tracer bullet（串行，冻结传播）
      -> Slice 3A Space 配对迁移（Agent A） ─┐
      -> Slice 3B 平台输出补齐（Agent B）   ─┤ 可并行
                                             -> Slice 4 clean cutover（串行）
                                               -> Slice 5 总验收与文档收口（串行）
```

## Slice 0: Baseline truth and privacy gate（必须单 Agent 串行）

**File**：当前 dirty observability 原型文件、`docs/exec-plans/completed/mobile-log-file-layer.md`、
`scripts/architecture/check-engine-repository.mjs`、新增测试 sink/fixture；不得修改或回退无关
`crates/uc-infra/src/security/profile_storage_upgrade/target.rs`。

**Change**：

1. 逐文件标记当前未提交原型为删除、重写或保留；先删除本规格 deletion inventory 指定的 flow/update-id 解析和伪 span。
2. 保留已验证且不依赖关联原型的 035 decorator 基线；取得 clean、可重复的 focused/workspace 基线。
3. 建立远程字段 allowlist 和系统/文件/OTLP 捕获测试，先用已知路径/地址/ID/error 哨兵证明红测有效，再清理已知违规日志。
4. 校准移动文件日志计划：已实现部分写入完成证据；与新 JSONL/retention/export 冲突部分标为由 037 取代并移出 active。
5. 生成 `docs/generated/observability-inventory.md`，按 stable remote、local operational、local debug、product analytics、delete
   五类列出当前 target/callsite；生成物不是手工事实来源。

**Exit gate**：远程 sink 隐私红测能抓到每类哨兵；清理后绿；架构门禁拒绝 Engine 步骤查询、Application timing 和 Infra
敏感 info；基线命令及非本任务 dirty 边界记录完整。

**Risk**：误把当前原型或无关 profile upgrade 变更当作 HEAD。实施者必须先记录 `git diff --name-only` 和逐文件归属，禁止
reset/checkout 整个工作树。

## Slice 1: Single-process real OTLP tracer bullet（必须单 Agent 串行）

**File**：新增 `crates/uc-observability-runtime/`、`crates/uc-observability-contract/src/diagnostics/`、workspace/Cargo lock、
`crates/uc-engine/src/contract/observability.rs`、storage upgrade observability、`tests/observability/`。

**Change**：锁定官方兼容版本组；实现 ProcessObservabilityRuntime、共享 Resource、trace/log 双 bridge、remote allowlist、batch
processors、health、flush/shutdown；用 `profile_storage_upgrade` 作为单机完整 span+log 样例。CI 使用 in-memory exporter 和本地
可解码 OTLP/HTTP fixture；人工验收使用 Collector + Jaeger 查看 trace，并用 Collector 可解码 sink 验证 logs。生产路由以
无秘密 PostHog 模板固定，真实项目投递只在具备项目凭据的部署环境验收。

**Exit gate**：接收端实际收到一条 trace 和一条 LogRecord；日志 TraceId/SpanId 与 span 一致；provider 可重复 flush、一次
shutdown；Collector 不可达和队列满不改变 upgrade 结果；远程 payload 隐私扫描通过；Rust 1.95 与各 target 依赖编译通过。

**Risk**：global subscriber 测试互相影响。provider/layer contract 用 scoped subscriber；global install 用独立测试进程，不靠
并发单测共享全局状态。

## Slice 2: Clipboard cross-device tracer bullet（必须单 Agent 串行）

**File**：`crates/uc-core/src/ports/clipboard/sync_dispatch.rs`、`crates/uc-application/src/clipboard/`、
`crates/uc-infra/src/network/iroh/clipboard_*`、新增 Infra trace context module、Engine Clipboard decorator、相邻测试。

**Change**：Engine 装饰完整发送 capability 并创建稳定 client span；Infra 使用官方 W3C propagator 在已认证 wire metadata 注入/提取，
接收 endpoint 在认证后创建 server span，地址解析和连接只作为 Infra 私有 internal span；
Application 删除手工 timing/span，Core 删除 DispatchTiming；同一 event 只作为 OTel log，不复制为普通 span event。协议按用户批准
原地升级，不保留第二 writer 或增加业务版本号；用前置格式标记让新旧布局双向明确不兼容，所有绑定与测试 fixture 同提交更新。

**Exit gate**：两个真实本机 Iroh endpoint 的发送/接收 span 使用同一 TraceId 且 parent 正确；日志自动关联；缺失/损坏/超限/
未认证 context 均不影响业务且不被信任；网络时长与接收提交可在平台分辨；Application/Core 不出现 OTel 类型和手工计时。

**实施结论**：旧 flow id 每次发送临时生成，并不跨重试/重启，已连同 Core header 字段和 Application 传播删除。Clipboard tracer
bullet 先省略 `uc.flow.id`，只使用真实 W3C 因果关系；不得为满足字段存在而重新引入观测专用业务接口。

## Slice 3A: Space admission and membership migration（可多 Agent 并行：Agent A）

**前置**：Slices 0-2 全绿，schema、OTel 版本组、Infra context codec 和 Engine decorator pattern 已冻结。

**独占文件**：`crates/uc-engine/src/assembly/observability/{admission,membership}.rs`、
`crates/uc-engine/src/assembly/sync_engine.rs` 的 Space 区段、`crates/uc-engine/src/runtime/session_supervisor.rs`、
`crates/uc-infra/src/network/iroh/{space_admission,space_admission_wire}.rs`、相邻测试。

**禁止修改**：bindings、根 Cargo/Cargo.lock、contract diagnostics schema、`observability/mod.rs` 非 Space 区段、架构脚本、计划与
architecture bible。发现共享接口缺口时停止该片并交回整合 owner，不得私自扩接口。

**Change**：准入和成员完整网络能力迁移到统一 target/schema；删除 Engine 对准入状态、准备、激活、成员账本和分支恢复子步骤的
观测。在线每轮使用真实 client/server/endpoint trace；重试/重启由 Application 不透明作用域提供同一匿名 flow；session transition
只记录 Engine 完整生命周期；group update 不解析持久 update-id 字符串。

**Exit gate**：双设备配对在线调用树、失败分类、跨重启 flow 聚合和旧成员 group update 可查询；原始 admission/member/device/
update id 不出现；session transition 作为独立 Engine 生命周期 trace 在同一时间窗口展示，不伪造 admission parent/flow；其所有
失败和提前返回有 total outcome；既有 1 秒性能门禁仍能如实判定。

## Slice 3B: Apple/Android/Harmony output completion（可多 Agent 并行：Agent B）

**前置**：同 Slice 3A；Slice 1 已预先锁定全部平台依赖，Agent B 不修改 Cargo.lock。

**独占文件**：`bindings/uc-engine-uniffi/src/**`、`bindings/uc-ohos-napi/src/**`、三个 host probe 与 binding tests。

**禁止修改**：Engine/Application/Infra 业务与装配、根 Cargo/Cargo.lock、contract schema、架构脚本、公共计划文档。

**Change**：Apple/Android 复用共同 ProcessObservabilityRuntime 并保留 OSLog/Logcat；Harmony 增加对等进程安装；三平台统一
JSONL、保留清理、远程配置、重复 Engine 复用、有界 flush。远程许可迁到 host bootstrap；删除 Engine `telemetry_enabled`
需要的 binding DTO 适配由本片完成，但 Core/Application 字段删除留给 Slice 4 整合 owner。

**Exit gate**：三平台 binding 构建；重复创建 Engine 不重复 subscriber；suspend flush 后 resume 继续记录；单 Engine shutdown
不关闭 process provider；文件目录失败和 Collector 不可达不影响 Engine；真机未执行项明确标“跳过”。

## Parallel execution protocol for Slice 3

1. 整合 owner 在开始前发布 schema hash、Cargo.lock hash 和允许文件清单。
2. Agent A/B 各自在独立 worktree 开始，不共享未提交文件。
3. 两片不得修改任何共同文件；需要共同改动只写入 handoff，不在分支实现。
4. 每片先提交 focused tests 和文件清单，再由整合 owner按 A 后 B 顺序合并。
5. 合并后必须重新运行双设备 trace、三个 binding 编译和隐私 payload 检查；单片通过不能代表整体通过。

## Slice 4: Old contract retirement and integration（必须单 Agent 串行）

**File**：`uc-observability-contract` 旧 modules、Core/Application 旧 timing/flow、settings model/DTO、Cargo manifests/lock、
diagnostics export、本地 file owner、`observability/mod.rs`、共享 wiring。

**Change**：执行完整 deletion inventory；删除 Engine `telemetry_enabled` 并记录宿主设置迁移；Core TaskRegistry 返回纯关闭报告，
由现有 runtime owner 记录；按调用者归属删除 contract 的 task supervision 实现；统一 JSONL 枚举/导出；修复并行片提交的共享
接线；确保 product analytics 事件和 `usage_analytics_enabled` 不变；生成最终 inventory，剩余 local debug 逐项进入后继计划而非
模糊 TODO。

**Exit gate**：仓库不存在 `uc_otlp`、旧 stages/DispatchTiming、双 flow 字段、第二 subscriber、观测专用业务接口或 prototype
update-id 解析；移动 active plan 已关闭；所有公开 binding/schema 生成物更新一致。

## Slice 5: Guardrails, visual acceptance, and documentation（必须单 Agent 串行）

**File**：架构检查、Collector fixtures、观测设计、接口文档、架构圣经、037、active/completed indexes、CI workflow。

**Change**：增加允许 fixture 与负向 fixture；运行真实 Collector 并检查调用树、日志关联、flow 聚合；执行性能/过载/生命周期/
隐私矩阵；更新稳定文档后把 037 移入 completed，不保留 active 双副本。

**Exit gate**：本规格 Acceptance Criteria 全部有命令、计数或截图证据；未执行设备明确跳过；工作区只包含本规格文件和开始时
已记录的其他 owner 变更。

# 7. Edge Cases

```text
Scenario: Collector 未配置、DNS/TLS/认证失败或请求超时。
Expected behavior: 系统/JSONL继续；远程队列有界；业务结果不变；健康日志只含固定失败类并限频。
Implementation: batch exporter 与业务线程隔离，remote layer 不导出 exporter 自身日志。
```

```text
Scenario: span/log 队列达到上限。
Expected behavior: 新观测记录被丢弃，业务不等待；丢弃数量可从本地 health summary 观察。
Implementation: 使用官方有界 batch processor，不增加无界 channel 或同步 fallback。
```

```text
Scenario: 移动应用进入后台或被系统直接终止。
Expected behavior: suspend 后在预算内尽力 flush；超时不阻塞；直接终止允许丢最后一批远程数据。
Implementation: provider 属于进程，后台不 shutdown；第一版无磁盘遥测队列。
```

```text
Scenario: 同一进程创建、关闭、再创建多个 Engine。
Expected behavior: 复用同一 subscriber/provider；单 Engine shutdown 不使后续实例失去观测。
Implementation: process runtime 与 Engine 实例生命周期分离，配置不一致明确失败。
```

```text
Scenario: 收到缺失、损坏、超长或伪造 trace context。
Expected behavior: 不接受 remote parent，创建本地 root；业务认证和结果保持原语义。
Implementation: Infra 限界解析，认证成功后才 set parent；context 不参与授权。
```

```text
Scenario: 对端来自当前未发布协议之前的本地开发构建。
Expected behavior: 明确协议不兼容，不双写或静默降级；提示升级同一 Engine revision。
Implementation: 当前 clean cutover 原地更新全部绑定与 fixtures，不增加长期版本分支。
```

```text
Scenario: 配对或同步在重试、断网或进程重启后继续。
Expected behavior: 新在线尝试使用新 TraceId；有合法持久 attempt 的流程保留 `uc.flow.id`，其余流程省略；不持久化旧 traceparent。
Implementation: flow 只从完整 owner 已有随机 attempt 经固定用途单向派生，trace context 只存在于在线 wire。
```

```text
Scenario: 日志字段含路径、地址、设备、Space、内容、摘要或原始错误。
Expected behavior: 测试和架构门禁失败，任何 sink 都不得输出。
Implementation: schema allowlist、Debug redaction、运行时三 sink 哨兵扫描和 Collector 二次过滤。
```

```text
Scenario: 本地日志目录不可写、超过 7 天或超过 100 MB。
Expected behavior: 不可写时降级；超限时按最旧优先清理；不会删除目录外文件。
Implementation: owner 只枚举固定 `engine.YYYY-MM-DD.jsonl`，路径必须位于 HostDirectories.logs；容量按十进制
100,000,000 bytes 计算。
```

```text
Scenario: trace layer 和 logs bridge 同时接收一个 tracing event。
Expected behavior: 完整事件只形成一条 LogRecord，不再作为 span event 重复存储。
Implementation: trace layer 禁止记录 event；允许的完成与健康事件只由 logs bridge 输出。
```

# 8. Testing Strategy

## Unit Test

- Contract：每个 event/span schema 的字段名、类型、枚举、缺失行为和隐私 allowlist；FlowId 同输入稳定、不同 purpose 隔离、
  不等于原 ID。
- Runtime：Resource 一致、target filter、batch queue、重复 install、配置冲突、flush/shutdown 幂等、provider 关闭后拒绝新记录。
- Logs bridge：span 内 event 的 TraceId/SpanId 与当前 span 一致；普通 event 不重复进入 span event。
- Infra carrier：官方 propagator round trip、长度上限、缺失/损坏/未认证忽略、MAC/AAD 篡改拒绝。
- Local file：JSONL、info filter、7 天/100 MB 清理、目录逃逸拒绝、writer 失败降级、诊断导出枚举一致。
- Error mapping：所有 exporter/codec/provider 失败只映射固定 `error.type`，source 文本不进入任意 sink。

## Integration Test

- Single-process：启动本地 OTLP/HTTP fixture，运行 storage upgrade，解码请求并断言 trace+log+Resource+关联字段。
- Cross-device Clipboard：两个真实 Iroh endpoint，断言 client/server TraceId 和 parent，接收提交日志自动关联。
- Space admission：双 Engine 完整配对、四轮消息、session transition 和旧成员 group update；跨重启新 trace + 同 flow。
- Failure：Collector down、慢响应、401、TLS failure、队列满、flush timeout、重复 shutdown，业务结果与基线逐项相同。
- Privacy：向内容、邀请、设备、路径、错误注入哨兵，扫描系统 writer、JSONL 和原始 OTLP protobuf均不存在。

## Regression Test

- Product analytics 事件名/properties/identity tests 全部不变。
- 035 admission/membership decorator transparency、source chain、调用次数和 policy tests 全部不变。
- Engine stable operation/result/event 与各 binding generated contract 不增加业务步骤或 OTel 类型。
- P2P 默认、LAN 不自动降级、持久密文、release source 与 architecture preflight 全部通过。
- 比较观测关闭、Collector 健康、Collector 不可达三组双设备性能，确认不阻塞和预算。
- 同步路径只做功能回归，本轮没有单独采集同步 A/B 性能样本。

## Platform matrix

以下是实施要求；本轮没有运行的系统和设备项目在“明确跳过”中逐项记录，不能用目标编译代替运行验收。

| Check | macOS direct/Apple | iOS | Android | HarmonyOS | Linux/Windows |
| --- | --- | --- | --- | --- | --- |
| build | required | required | required | required | required |
| system sink | required | required | required | required | required |
| JSONL/retention/export | required | required | required | required | required |
| OTLP fixture | required | simulator/device | emulator/device | emulator/device | required |
| background flush/resume | n/a | required | required | required | n/a |
| physical device | if available | explicit pass/skip | explicit pass/skip | explicit pass/skip | if available |

## Final commands

```bash
cargo test -p uc-observability-contract --locked
cargo test -p uc-observability-runtime --locked
cargo test -p uc-engine assembly::observability --locked
cargo test -p uc-application --locked
cargo test -p uc-infra --locked
cargo test -p uc-engine --features dev-tools --test space_membership_auto_pairing_e2e \
  in_flight_admission_restart_uses_new_traces_and_one_flow --locked -- --ignored --exact --nocapture
cargo metadata --locked --format-version 1
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

一秒性能项另以指定的 ignored 诊断测试
`two_device_hot_path_pairing_completes_within_one_second` 运行；结构断言先执行，耗时门槛预期如实失败，直到 038 完成。它不属于
037 的绿色自动检查。任何 filter 必须确认执行非零目标测试；设备构建使用仓库 host scripts，未执行只记“跳过”。

# 9. Acceptance Criteria

* [x] `uc_otlp` 不再存在；一条真实 trace 和一条真实 LogRecord 到达可解码 OTLP receiver。
* [x] traces/logs 共用 Resource，日志自动带正确 TraceId/SpanId。
* [x] 在线跨设备 client/server span 属于同一 trace，parent 关系正确。
* [x] 重试/重启使用新 trace；只有已有合法持久 attempt 的 Joiner client span 携带同一匿名 `uc.flow.id`，其余 span 与 log 不伪造。
* [x] Core/Application 公共接口不包含 OTel、trace context、timing 或观测步骤。
* [x] Core 默认依赖不含 tracing；TaskRegistry 只返回纯关闭报告，调用层保留可观察的完整结果。
* [x] Engine 没有新增 Application/Core 阶段查询，也不解析业务持久字符串取得关联号。
* [x] Infra 只在认证后接受 bounded W3C context；异常 context 不改变业务结果。
* [x] Apple、Android、HarmonyOS 和直接 Rust host 都有唯一进程级安装与明确生命周期。
* [x] remote disabled、Collector down 和队列满均不阻塞业务；本轮双设备配对性能增量满足预算，未把同步功能回归写成量化结果。
* [x] JSONL 文件有 7 天/100 MB 上限并能被诊断导出完整发现。
* [x] 本地 Jaeger 能显示 tracer bullet 调用树，Collector 可解码 sink 能读取对应日志。
* [ ] 生产 Collector 已用真实项目验证并优先输出到 PostHog；客户端没有 PostHog 专用依赖或发送路径。（无项目凭据，真实投递跳过；模板已验证）
* [x] 原始 OTLP、macOS 实时系统消息和 JSONL 隐私扫描均无禁止字段。
* [ ] iOS、Android 与 HarmonyOS 实体设备系统输出隐私扫描。（无实体设备，跳过）
* [ ] Android 与 HarmonyOS 模拟器或实体设备上的系统输出、JSONL、OTLP 和后台生命周期。（无可用运行设备，跳过）
* [x] exporter 自身日志不会递归进入远程 exporter。
* [x] 产品 analytics schema、身份和发送行为不变，且不携带 TraceId/FlowId。
* [x] 当前未提交 flow/update-id 原型、旧 DispatchTiming/stages/TraceMetadata 和伪 OTLP 已删除。
* [x] 架构门禁能拒绝双 subscriber、Application timing、业务步骤泄露、敏感字段和 retired 路径回流。
* [x] Slice 3 多 Agent 文件所有权、schema hash 和汇合验证有实际记录。
* [x] 所有自动检查通过；未执行设备明确标为“跳过”。

# 10. Risks and Trade-offs

- **官方 Rust traces/exporter 尚非全部 Stable**：采用官方兼容版本组并锁定 Cargo.lock；升级必须整组验证，不分包漂移。
- **二进制与内存增加**：OTel SDK、protobuf 和 HTTP client 会增加移动产物；最终交付已记录同设置动态库差值和三轮进程峰值
  中位数，不在数据之外预设上限。
- **远程尽力发送会丢数据**：这是不阻塞业务和移动生命周期的明确取舍；可靠磁盘队列涉及密文、配额和清理，另立规格。
- **采样成本**：设备全采样保证父子关系完整；生产 Collector 对错误全保留、其他 trace 保留 10%，后续只在部署侧按真实量调整比例。
- **本地日志仍是威胁面**：远程 allowlist 不能代替本地清理；系统和 JSONL 必须通过同一隐私测试。
- **移除 Engine telemetry setting 影响产品仓**：换来单一宿主许可 owner 和无跨层控制 port；所有产品必须随同一 Engine revision
  迁移，不能长期保留双开关。
- **global subscriber 排他**：进程只能安装一次；返回明确 Reused/AlreadyInstalled 比当前静默忽略更严格，测试需独立进程。
- **trace 与 flow 双标识增加理解成本**：它们生命周期不同；统一 `uc.flow.id` 命名、可视化模板和文档是必要成本。
- **Application 业务细节不会全部进入 trace**：这是深模块和隐私换来的边界。需要新观测时先找完整 capability seam，不向 Engine
  增加步骤接口。
- **替代方案：只采集文本日志**：实现少，但无法自动关联跨设备因果、日志与 span，也不能满足目标。
- **替代方案：Application observation recorder port**：能看到任意内部阶段，但会扩大接口并让 Engine 理解步骤，明确拒绝。
- **替代方案：把业务 flow 放 baggage**：传播方便但可能被自动转发且无完整性保证，明确拒绝。
- **替代方案：设备直连厂商后端**：减少 Collector，但把 PostHog 认证、过滤和采样耦合到客户端，明确拒绝。

# 11. 外部与后续状态

1. 本仓不保存 PostHog 秘密；生产项目的真实投递因无凭据明确跳过，部署方在上线前验证项目地址和 token。
2. 客户端只接受宿主提供的 HTTPS Collector；实际 endpoint、证书和部署归产品部署仓，不改变本仓合同。
3. HarmonyOS 本轮明确选择通用 JSON 系统输出 fallback；原生 HiLog 只有在宿主提出需求并完成 SDK、许可证和实体设备验证后另行设计。
4. 已安装其他全局 subscriber 的直接 Rust 宿主会得到明确冲突结果，必须由该宿主的 bootstrap owner 合并，不能竞相安装。
5. 生产采样由 Collector 负责：错误全保留，其他 trace 先保留 10%；后续比例调整不改变客户端。

已确认决策：本地使用 Jaeger，生产优先使用 PostHog；远程诊断许可只归宿主；本地日志保留 7 天且总量不超过
100,000,000 bytes。

# 12. 实施记录

## 已完成结果

- 进程运行时、trace/log 双信号、系统输出、有界 JSONL、远程健康计数、串行 flush/shutdown 和诊断导出已使用同一合同。
- Clipboard、Space 准入、成员历史、成员组更新和 session lifecycle 已迁移到类型化记录；Core/Application 公共接口没有 trace、timing
  或业务步骤字段。Application 准入 owner 只开启不透明关联作用域；Engine 不构造关联号，也不装饰准入状态、准备、激活、成员账本
  或分支恢复子步骤。
- Space 认证消息把 `traceparent` 纳入现有 MAC；Infra 在认证后建立 server parent。通用 `network_transport` 不暴露业务消息阶段。
- Sponsor 只有收到 Joiner 对 reply 的确认后才记录成功；整条入站交换共用一个绝对截止时间，缺少确认、错误确认或超时都只记录
  一次明确失败。认证前失败只产生一条带真实耗时的无关联日志；认证成功后每个三层节点恰有一条关联完成日志。
- Space 的认证握手保持原布局，认证后的新旧 Request/Reply 使用互斥 frame kind；真实旧 Sponsor 测试会在密码认证后得到明确升级
  结果，当前普通协议错误使用独立关闭码。Engine 尚未发布，因此未增加版本号或保留双 reader。
- 首次交换不兼容稳定拒绝；Prepared、Applied、Cancelling 和 ActivePendingSettlement 保留原待交换与正式结果，只增加持久升级阻塞。
  Pending/Active 对外携带固定升级提示；提示出现、清除或明确拒绝后只发一次通用刷新，重复错误不发。对端上线立即重放同一请求并
  自动清除，不把阻塞误记为资料损坏；未发布旧 V1 待交换记录可严格读取，尾随内容与其他截断状态仍失败关闭。
- Apple、Android、HarmonyOS 与直接 Rust host 使用同一进程安装入口。Android 使用系统证书校验，并在 AAR 中携带其 Java 组件和
  消费者混淆规则。
- 宿主资源字段只接受固定值，HarmonyOS probe 与 smoke 已使用合法发布渠道；SDK 明确返回的超时保持为 `timed_out`，可重试的准入
  传输延期保持为 `deferred`，不会混入普通失败或生产错误采样。
- 设备编码前和 Collector 各有一道精确字段收窄；Collector 同时拒绝 resource/scope schema URL。日志正文为空且不携带 flow，事件名
  和 span 名来自固定枚举，源码位置、线程与 busy/idle 不发送。
- 最终可再生清单包含 1,247 个生产调用点：8 个稳定远程、6 个本地健康、1,231 个默认拒绝的历史调试点和 2 个产品分析调用点。
- 删除 `uc_otlp`、旧 stages、`DispatchTiming`、`TraceMetadata`、旧 flow、Engine `telemetry_enabled` 和跨层 task supervision helper；
  `usage_analytics_enabled` 与产品分析合同保持不变。

## 本地真实证据

- 最终代码按匿名 flow 查询得到 8 条 trace：四条连接建立记录，以及四棵
  `network_transport client -> network_transport server -> space_admission sponsor endpoint` 三层消息交换树；只有 client root 携带
  flow，每棵树的 TraceId、parent 和三层成功结果均正确。完整页面共有 17 条 trace、27 个 span；Jaeger 搜索页和三层调用树已通过
  实际页面截图检查。
- Collector 解码日志显示稳定事件名、空正文、正确 TraceId/SpanId；三层树每个节点恰有一条完成日志，日志均无 flow。原始 OTLP 的
  Resource、span 与 log 字段集合逐项等于白名单。
- macOS Unified Logging 实时捕获只出现类型化完成/健康记录；伪造同 target 的设备名、路径、正文和错误字段组合均未出现。JSONL
  与原始 OTLP 使用同一组哨兵也通过。
- 三设备测试证明新成员加入后，旧成员最终看到三成员状态并可向新成员发送内容。
- 中途关闭 Joiner 并从同一持久资料重启的真实双设备测试通过：重启前后只有一个匿名 flow，重启后产生新的在线 TraceId 并完成配对。
- Collector 不可达、HTTP 401、TLS 握手失败、慢响应、队列满、flush 超时、并发 flush/shutdown 和重复 shutdown 均有独立测试；
  业务提交不等待远程网络。
- 无观测、本地记录、健康远程、不可达远程四组先各预热 1 次，再轮换起始顺序交错运行 5 个正式样本，四组均 5/5 完成业务。
  p50 分别为 5.330650、5.462975、5.516288、5.429277 秒；五样本 p95 即各组最大值，分别为 5.744097、5.833397、5.601388、
  5.599411 秒。后三组 p95 相对无观测基线分别为 +1.5546%、-2.4844%、-2.5189%，满足 2%/5% 预算；样本只证明观测没有造成
  当前五秒瓶颈。
- 相同 release 设置下，UniFFI 动态库由 16,952,560 bytes 增至 17,157,488 bytes，增加 204,928 bytes（1.2088%）。同一双设备
  测试二进制交错运行三轮，最大常驻内存中位数为：无观测 311,623,680 bytes、本地记录 314,769,408 bytes、健康远程
  313,671,680 bytes；相对基线分别增加 3,145,728 bytes（1.0095%）和 2,048,000 bytes（0.6572%）。三样本只作诊断，不设正式上限。
- 最新一秒诊断报告端到端 5.338917 秒、当前传输外壳粗估 0.405162 秒、相减值 4.933755 秒。该粗估既包含部分本机工作，也漏掉部分初始
  网络等待，既不是严格上界也不是下界，不能据此在一秒附近判定通过；精确门禁和性能改造已进入规格 038。
- 最终全量回归通过：诊断合同 49 项、运行时 38 项、Core 360 项、Application 750 项、Infra 896 项、Engine 225 项、UniFFI 51 项、
  HarmonyOS 44 项；Infra 另有 7 项明确跳过的测试或文档示例。

## 明确跳过

- 当前环境没有 PostHog 项目 token 或项目地址。生产 Collector 模板已由 Collector 0.160.0 校验，但真实 PostHog 项目投递记为
  “跳过”，不得写成“通过”。客户端合同不因此改变。
- macOS 直接运行、iOS device/simulator、Android、HarmonyOS、Linux 和 Windows 目标编译均通过。Linux 在 macOS 上使用临时
  Zig 交叉编译适配；iOS 模拟器应用已完成构建、安装、启动、暂停、恢复和两路关闭；Android AAR 已验证 arm64/x86_64 与系统证书
  组件；HarmonyOS HAP 已签名校验，HAR 已检查包内容，N-API 正常启动与依赖失败两条本机宿主 smoke 均实际运行通过。iOS 工程生成器
  也已验证不受外置构建目录路径影响。
- iOS、Android 与 HarmonyOS 实体设备上的系统日志和真实 HTTPS 投递因无设备记为“跳过”；HarmonyOS 原生 HiLog 尚未实现，
  使用通用 JSON fallback。Android 与 HarmonyOS 本轮没有可用模拟器或实体设备，因此系统输出、JSONL、OTLP 及暂停/恢复/关闭运行验收
  均记为“跳过”，AAR、HAP/HAR 通过不能代替这些运行结果。Windows/Linux 本轮完成目标编译，未在对应实体系统运行系统输出、JSONL
  和 OTLP，记为“跳过”。

## 后续性能边界

1 秒目标不是本规格通过删除持久检查或放宽门禁解决的事项。代码审计识别出准入加密状态的重复读取/提交、维护轮重新加载与
session 切换等优先候选，但当前证据不能证明它们的占比或排序。规格 038 先完成可信分项测量，再决定实施顺序；后续仍须保留逐提交
重启恢复、密文损坏关闭式失败、唯一成员和旧成员更新验收。
