# Findings & Decisions

## Requirements

- 重新设计 tracing 与日志架构，符合 Core/Application/Infra/Engine 分层。
- 产出实现级 Spec 和执行计划，不直接实施。
- 分片必须最小可验证；可并行分片要声明多 Agent 文件所有权、依赖和汇合门。
- 不为观测向 Engine 暴露 Application/Core 内部步骤、状态或查询接口。
- 保持隐私、失败隔离、P2P 默认和 Engine 稳定入口约束。
- 用户确认本地 trace 平台使用 Jaeger，生产优先使用 PostHog。
- 用户确认远程诊断许可只归宿主，不保留 Engine 同名开关。
- 用户确认本地日志保留 7 天，总量上限为十进制 100,000,000 bytes。

## Research Findings

- 当前 `uc-observability-contract::otlp` 只发出 target=`uc_otlp` 的 tracing event，不安装 exporter，也不发送 OTLP。
- Apple/Android binding 当前各自安装全局 subscriber，输出系统日志并可附加 info+ 按天滚动文本文件；安装失败或全局 subscriber 已存在时未公开结果。
- 产品 analytics 通过 `AnalyticsPort` 是独立链路，不应与运行诊断合并。
- 当前 Space 稳定耗时集中在 Engine port decorator；Application 禁止散布 Instant/tracing。
- 官方 OpenTelemetry 将 traces、metrics、logs 视为独立 signals；日志可由 TraceId/SpanId 自动关联到当前 span。
- 官方默认跨进程传播使用 W3C Trace Context；外来 context 必须视为不可信输入，baggage 不适合承载敏感或授权信息。
- 当前工作区已有未提交 observability 原型和一组无关 profile upgrade 修改；正式规格必须明确原型是吸收还是删除，不能默认它们属于基线。
- 规格 035 已完成 Space admission/membership 的 Application-owned bundle 与 Engine decorator 收口；新规格不得重建 bundle、第二入口或 Application recorder port。
- `docs/exec-plans/active/mobile-log-file-layer.md` 仍标为“待实现”，但当前 binding 已有 OSLog/Logcat + info 文件层，最近提交也包含观测补全；037 必须先做事实校准，已实现部分更新稳定文档并移出 active，未实现的保留期限/上传进入 037。
- 规格编号 036 已完成，037 当前未占用，可作为新计划编号。
- 当前未提交观测原型修改 Engine admission/membership decorator、sync_engine、session supervisor 和观测文档；037 必须把它列为基线外原型，实施首片先选择吸收或删除，不能在其上假设 green baseline。
- 官方 Rust 实现建议继续用 `tracing` 作为应用日志入口：`tracing-opentelemetry` 负责 span -> OTel trace，`opentelemetry-appender-tracing` 负责 tracing event -> OTel log，两者不能混称。
- 官方 OTLP exporter 支持 traces/logs/metrics；生产示例推荐 batch exporter。Rust 0.28+ 的 batch span/log processor 使用专用后台线程，不再要求调用方传 Tokio runtime，但 grpc-tonic exporter 构造仍需 Tokio runtime；规格应优先选择 OTLP/HTTP + blocking client 以简化 Apple/Android 生命周期，最终版本实施前锁定兼容矩阵。
- Rust provider 必须由应用 owner 保留并显式 `shutdown`/`force_flush`；不能只设 global provider 后丢失 owner，也不能依赖旧的 global shutdown。
- 官方 batch log processor 默认有界队列，满时丢新日志而不反压业务线程；span batch processor同样有 max queue/batch/delay/timeout。规格必须把丢弃计数、限频本地告警和关闭预算列为验收项。
- OpenTelemetry 推荐由 tracing appender 桥接现有 tracing event；避免 exporter 自身日志再次进入同一 exporter 形成递归观测。
- Trace sampling 必须使用 parent-based 语义保持跨设备一致；早期 SDK sampling 与 Collector 后期 sampling职责分开，业务代码不参与采样判断。
- 仓库固定 Rust 1.95，满足当前官方 OpenTelemetry Rust 的最低编译版本要求；实施仍需把 opentelemetry/tracing-opentelemetry/appender 的兼容版本矩阵锁在 Cargo.lock 并做移动交叉编译。
- UniFFI 当前在 `MobileEngine::start_inner` 前安装进程级 global subscriber；`MobileEngine` 可 suspend/resume/shutdown/recreate，因此 provider 生命周期不能绑定单个 Engine 实例。
- iOS 主应用、分享扩展、键盘扩展是独立进程，各自可以拥有一个 process observability runtime；同一进程内重复创建 Engine 必须复用同一 runtime。
- Mobile suspend 应先完成 Engine suspend，再对 process provider 做有界 `force_flush`；Engine shutdown 只 flush，不 shutdown process provider。桌面进程最终退出可显式 shutdown provider。
- 当前 `telemetry_enabled` 存在于 Engine settings，但 binding 在 Engine 启动前安装 subscriber，尚无单一 owner 能一致控制远程 layer；037 必须明确设置迁移或可重载控制，不能形成 host/Engine 两份开关。
- 当前 HarmonyOS binding 未见与 Apple/Android 对等的 tracing subscriber 安装；037 的平台矩阵必须单列 HarmonyOS，而不是把 UniFFI 结果外推。
- `mobile-log-file-layer.md` 的文件层、中继日志实际已存在，但 active plan 状态陈旧；037 Slice 0 应先校准并关闭该计划，避免重复实现。
- `telemetry_enabled` 与 `usage_analytics_enabled` 当前只在 settings model/DTO 中读写，未找到对 tracing layer 或 analytics sink 的生产门控；现有注释描述的开关责任与实际接线不一致。037 必须把 remote diagnostics 与 product analytics 分别接到各自唯一开关。
- 仓库 production tracing 分布约为 Application 76 个文件、Infra 81 个文件、Engine 42 个文件；Core 唯一 tracing 位于 `TaskRegistry`。不能一次性删除所有 raw tracing，必须先按语义事实、稳定依赖耗时、内部 debug、宿主输出四类清单化。
- `uc-observability-contract::analytics::Event` 是 PostHog 风格、长期不可重命名的产品事件 schema，包含 `$os` 等供应商字段；它不应成为 OTel Resource/Logs schema 的基础。037 保留 analytics 行为并把产品字段与运行诊断资源字段物理分开。
- `BindingConfig` 当前只有 app version/profile id；远程诊断开关、endpoint、认证和环境没有 binding bootstrap 输入。HostCapabilities 只有 analytics sink/identity，没有 diagnostics sink。
- `uc-ohos-napi` 没有 tracing/subscriber 依赖或安装入口，只构造 logs 目录；HarmonyOS 当前不会建立与 UniFFI 对等的系统/文件日志管道。
- 当前 Core `TaskRegistry` 直接依赖 tracing，是 Core“纯规则”边界的例外。037 可将 task supervision 观测委托给调用层或 contract helper，但不应把整套 runtime 迁出作为本规格前置。
- 官方当前兼容版本组是 OpenTelemetry crates 0.32 与 `tracing-opentelemetry` 0.33；其版本号不严格同步，必须作为一组锁定和升级。
- 远程 logs 必须使用 `OpenTelemetryTracingBridge`，remote traces 使用 `OpenTelemetryLayer`；后者新版会在 span enter 时激活 OTel Context，这是日志自动获得 TraceId/SpanId 的必要条件，必须通过真实导出验证而非只编译。
- 同一个 tracing event 可能同时进入 OTel LogRecord 和 trace span event。037 选择 LogRecord 作为完整诊断事件唯一副本，trace 只保留 span、状态、错误与少量关键 event，并使用官方 event filter 统计被过滤数量。
- 设备应发送到受认证 HTTPS Collector Gateway，由 Collector 负责后端路由、二次字段白名单、限流和 tail sampling；设备侧仍是第一道隐私边界。
- 第一条远程切片只覆盖低频 admission/membership 并 AlwaysOn；高频 clipboard 后续采用 ParentBased root sampling。若要求错误 trace 全保留，只能全发后 Collector tail sample，或依靠独立错误日志。
- 官方 Rust 没有移动端可靠离线队列；第一版明确尽力发送并保留系统/本地文件，不新增磁盘遥测队列。若未来新增，因持久化默认敏感必须另立安全规格。
- 远程层必须屏蔽 exporter 自身依赖（如 HTTP/gRPC 客户端）的日志，防止 telemetry-induced telemetry；本地系统层可以保留限频 exporter 健康告警。
- 仓库审查发现 Application/Infra 现有 info span/event 会记录 Space、设备、条目摘要、真实路径等禁止字段；这些已进入系统日志和明文本地文件。037 必须在任何远程 exporter 前先建立全仓字段白名单与真实输出扫描，并清理已知违规点。
- 当前架构门禁只覆盖少量 Application 文本模式，没有系统检查 Infra、错误正文、Space/entry id、摘要和真实文件输出；新门禁需同时做静态负向 fixture 与运行时 sink 捕获。
- `uc-observability-contract` 当前同时拥有 analytics schema、identity facade、task supervision、FlowId 和伪 OTLP recorder，职责过宽。037 将 schema/propagation 与 runtime/exporter 分开；`spawn_supervised` 移到实际运行层，analytics 保持独立模块。
- 当前日志导出只识别旧桌面文件命名，不能覆盖移动 `engine.*.txt`；新本地 JSONL sink、保留清理和诊断导出必须形成一个完整 owner。
- 当前未提交原型从 `SpaceAdmissionId` 派生 flow，并解析 `PendingGroupUpdate.update_id` 字符串恢复关联，后者把业务持久格式泄露给 Engine。Slice 0 必须删除 `admission_flow_id*`、`admission_update_flow_id`、对应 `sha2`/`hex` 依赖和伪跨设备 span；四个新增 port 调用计时可在字段契约冻结后选择重写保留。
- session transition 原型失败事件缺少稳定 `error_kind`，若在基线清理后重新实现，必须覆盖全部提前返回和 total outcome。
- 未使用的 Core `TraceMetadata`/`extract_trace` 是过时方向；037 应删除而不是复用。现有 ClipboardHeader.flow_id 需在新 trace context 传播完成后 clean cutover，不能提前硬删。

## Technical Decisions

| Decision | Rationale |
| --- | --- |
| 区分在线 trace 与 durable flow | trace 表达一次因果执行；flow 跨重试、断网和重启聚合，生命周期不同 |
| Host/Binding 拥有 sink/exporter | Engine 保持供应商无关，网络上传失败不影响业务 |
| Core 完全不感知观测 | 业务规则不能依赖日志、采样或 exporter 状态 |
| Application 只拥有语义，禁止计时 | 业务负责人最清楚结果含义，Engine decorator 继续测量依赖耗时 |
| 不使用 baggage 传播 flow | baggage 会跨边界传播且不自带完整性保护 |
| 保留 035 的 decorator 架构 | 它已经验证并由架构门禁固定，新计划只补真实 signal pipeline、schema 与跨设备传播 |
| 037 采用 clean cutover | `uc_otlp` 伪上传命名、旧文本 sink 与新真实 OTLP 不能长期并存 |
| 同时安装 trace layer 与 log appender | 官方 Rust 生态将 tracing span 和 tracing event 分别桥接到 OTel traces/logs |
| HostObservabilityRuntime 持有 provider 生命周期 | 官方 Rust 已移除 global shutdown，应用 owner 必须显式 flush/shutdown |
| 使用 batch processor 且有界丢弃 | 网络发送不能反压配对/同步热路径，官方 batch processor提供该语义 |
| 跨设备采用 W3C Trace Context，flow id 只作跨重启聚合 | trace表达在线因果；flow表达持久业务尝试，二者不能互相冒充 |
| ProcessObservabilityRuntime 与 Engine 实例解耦 | global subscriber/provider 是进程资源，移动 Engine 实例可重建 |
| 远程观测开关必须单一 owner | 当前启动顺序不允许 host 和 Engine 各持一份真假状态 |
| 产品 analytics 与运行诊断彻底分离 | 前者有供应商语义和身份，后者使用 OTel resource/trace/log 且禁止身份关联 |
| 先做全仓 tracing inventory 再迁移 | 200 个以上文件/调用点语义不同，机械替换会破坏诊断或分层 |
| OTel LogRecord 是诊断事件唯一完整远程副本 | 避免同一 tracing event 同时作为 log 与 span event 双重计费和展示 |
| 首片 admission/membership 远程 trace 全采样 | 低频且是跨设备正确性验收入口，先保证完整因果链 |
| 第一版不做遥测磁盘队列 | 官方无移动端可靠方案，仓库密文持久化约束使其必须单独设计 |
| 设备统一发送 Collector Gateway | 后端供应商、二次脱敏、限流和 tail sampling 留在服务端 |
| 本地 Collector + Jaeger，生产 Collector 优先 PostHog | Jaeger 负责本地 trace 可视化；Collector 保持客户端与最终平台解耦 |
| 本地日志 7 天/100 MB | 用户确认；按十进制 100,000,000 bytes 固定，避免 MB/MiB 歧义 |
| 隐私白名单先于远程上传 | 当前本地日志已存在违规字段，先接 exporter 会扩大泄露面 |
| 删除当前未提交 flow 原型后再建 tracer bullet | 原型没有标准 context propagation，且解析业务 update id 泄露 Infra 格式 |
| 本地日志、保留清理、诊断导出由一个 owner 管理 | 当前写入与导出命名不一致，分散修补会继续漏文件 |

## Issues Encountered

| Issue | Resolution |
| --- | --- |
| 现有文档把“OTLP-compatible 日志”与真实 OTLP 发送混称 | 新规格要求重命名并建立真实 exporter 验收 |
| 完整配对与 Engine session transition 的关联容易诱发步骤泄露 | 规格必须使用完整意图/结果或 span link，不允许 Engine 查询内部阶段 |
| 当前日志存在禁止字段且门禁未发现 | Slice 0 建立静态+运行时隐私基线，远程 exporter 以零违规为前置门 |

## Resources

- https://opentelemetry.io/docs/concepts/signals/
- https://opentelemetry.io/docs/concepts/context-propagation/
- https://opentelemetry.io/docs/concepts/signals/baggage/
- https://opentelemetry.io/docs/specs/otel/logs/data-model/
- https://opentelemetry.io/docs/specs/semconv/resource/
- https://github.com/open-telemetry/opentelemetry-rust
- https://github.com/open-telemetry/opentelemetry-rust/blob/main/docs/migration_0.28.md
- https://github.com/open-telemetry/opentelemetry-rust/blob/main/docs/design/logs.md
- https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-otlp/examples/basic-otlp/README.md
- https://github.com/tokio-rs/tracing-opentelemetry
- https://opentelemetry.io/docs/specs/otel/trace/sdk/
- https://opentelemetry.io/docs/languages/rust/exporters/
- https://opentelemetry.io/docs/collector/deploy/gateway/
- https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-appender-tracing/README.md
- https://opentelemetry.io/docs/security/handling-sensitive-data/
- https://opentelemetry.io/docs/specs/otlp/
- `docs/design-docs/observability.md`
- `docs/exec-plans/completed/035-space-domain-observability-assembly.md`
- `docs/exec-plans/active/mobile-log-file-layer.md`
- `docs/exec-plans/completed/036-architecture-deepening-clean-cutovers.md`
- `crates/uc-core/src/ports/observability.rs`
- `crates/uc-application/src/settings/diagnostics.rs`
- `scripts/architecture/check-engine-repository.mjs`

## Visual/Browser Findings

- 官方说明跨进程 trace 依赖 context propagation；默认 propagator 使用 W3C Trace Context。
- 官方日志模型的 TraceId、SpanId、TraceFlags 是顶层字段，SpanId 存在时 TraceId 也应存在。
- 官方警告 trace headers 可被伪造；baggage 会随请求传播且可能泄露给非预期服务。
- 官方 Rust 将 tracing event 的 OTel logs bridge 与 tracing span 的 OTel traces bridge拆成两个 crate；必须同时装配才能获得日志和调用轨迹。
- 官方生产示例使用 batch exporter；provider 需要显式生命周期管理，不能依赖全局清理。
- 官方推荐 tracing event 通过 logs bridge 进入 OTel Logs，span 通过 tracing-opentelemetry 进入 OTel Traces；二者需要去重策略。
- Collector Gateway 适合集中供应商路由、字段处理和 tail sampling，但设备侧仍须先阻止敏感字段生成。
