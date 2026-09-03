# Findings & Decisions

## Requirements
- 用户希望检查 `crates/uc-engine/src/assembly/observability` 的做法，并通过一个 Spec 推广到其他适用位置。
- 用户已批准规格 035，当前任务改为完整实现并提交。
- 文档与注释使用中文，仓库路径使用相对路径。
- 任何仓库修改都要同步 `docs/architecture/architecture-bible.md` 的文档维护记录。

## Research Findings
- `SpaceRuntimeAdapters` 当前有 39 个扁平字段；`SpaceApplication::build_from_deps` 可在入口一次解构为 admission/membership 两个 bundle，不需要改变 use case interface。
- 准入只有 7 个已装饰 port；邀请解析、start state、cancellation、Sponsor preparation 等其余 admission port 需要原样透传。现有 transition decorator 只由 `DefaultJoinerActivationPreparation` 和 `DefaultJoinerActivationExecutor` 的内部依赖使用，规格要求删除其四个嵌套事件。
- `sync_engine.rs` 当前先单独包装 transition，再构造 `AdmissionPortImplementations`，最后把 `ObservedAdmissionPorts` 逐字段 clone 回扁平 `SpaceRuntimeAdapters`；这是 clean-cutover 的唯一生产调用点。
- `SpaceApplication::build_from_deps` 对 membership 的 `current_join_status` 有跨领域读取，但该能力按规格归 admission bundle；构造 `QueryDeviceTrustUseCase` 时从 admission 解构后移动即可，无需复制到 membership bundle。
- Membership 七个观测方法的错误均可由稳定 enum variant 穷举分类：ledger 五类、history/group update 三类、restricted 两类、branch recovery 三类；无需格式化 source。
- `MembershipHistoryMessage` 当前恰有规格列出的八个 V3 variant，可用无 wildcard 的匹配固定 request/response kind，新 variant 会触发编译失败。
- `ClipboardReceiverPort::subscribe` 的同步订阅不构成长耗时阶段；确认后继 rollout 表把 transport 首切片聚焦 `ClipboardDispatchPort::dispatch` 是合理的，本次不实现。
- Engine 已有多处 scoped `tracing::Dispatch` + capturing writer 测试，可在 observability 私有模块内复用同样形状验证字段白名单与敏感 source 不泄漏，无需新增生产 test-support interface。
- 架构脚本的 `repositorySources()`/`collectProblems()`/`runNegativeFixtures()` 已提供集中 source snapshot 与可变负向 fixture seam；035 门禁可作为一个纯文本形状检查加入现有框架。
- Branch recovery request 可用公开 `from_bytes` constructors 构造纯测试值，因此可直接通过 decorator 验证带 `anyhow::Error` 的 source 对象未被字符串化或替换。
- `docs/design-docs/observability.md` 已固定唯一范式：跨层持续计时、结果分类和阶段诊断由 Engine 组装层的领域 port decorator 实现；Application 不得散布时钟、tracing 或阶段记录。
- `admission.rs` 不是单个 wrapper，而是一个领域级 assembly：`AdmissionPortImplementations` 接受 7 个真实 port，`ObservedAdmissionPorts::assemble` 一次返回 7 个同 interface decorator；Space session transition 通过同一类型的专门入口装饰。
- decorator 保持原 port interface，调用真实能力后原样返回 `Result`，不改变 source、重试、持久化或通信顺序。
- transport 是关键深度案例：建立连接返回 `Box<dyn AuthenticatedAdmissionExchangePort>`，decorator 会继续包装这个返回 port，因此 observation 不在下一阶段断裂。
- 每类能力拥有私有 operation enum、显式 observation policy、固定 tracing target/schema；成功空 load 可被 policy 降噪，错误仍记录。
- 现有单元测试主要验证 policy 决策；生产 Engine 在 `sync_engine.rs` 组装真实 port 后统一替换并注入 Application。
- `docs/architecture/architecture-bible.md` 和设计文档已声明该范式可扩展到剪贴板、成员等领域，但当前 `assembly/observability/` 只有 `admission.rs`。
- `docs/exec-plans/completed/031-application-dependency-surface-deepening.md` 要求 Engine 继续选择 observability decorator，并在把 Application 对象图收回 Application 时保留这一 composition-root 职责。
- Application 仍有多处真实的手工性能计时：blob publish、clipboard inbound、dispatch fanout/per-peer/delivery、outbound pipeline、initialize Space 等；调度 deadline/cooldown 使用的 `Instant` 不属于观测迁移范围。
- Application 还直接依赖 analytics/OTLP/FlowId。这些包含产品分析与跨步骤关联语义，不能仅凭出现观测类型就一律搬到 port decorator；需按“持续依赖调用观测”与“业务结果/产品事件”分类。
- 031 完成后的架构已经把 Application 内部对象图收回 Application；Engine 只选择 adapter 并提交 `ApplicationNetworkAdapters` 等能力 bundle。因此推广必须装饰 Engine 本来就拥有的 adapter，不能为了埋点重新让 Engine 看见 Application use case。
- `ClipboardDispatchPort` 是高价值真实 seam：返回 `DispatchReport { transport, timing, outcome }`，现有 Application 根据该报告记录地址解析、连接、开流、写帧、等待确认和总耗时；这些可由 Engine decorator 从原调用输入/输出安全派生。
- `BlobTransferPort`、`BlobReferenceRepositoryPort`、`TransferCipherPort` 也是 Engine 已选择的真实 seam，可承接 publish/fetch/ticket/reference/cipher 的依赖调用计时；但它们属于 blob/file-transfer 领域，不应塞进 clipboard schema。
- Clipboard outbound 目前还记录 `entry_id`、MIME、文件/representation 细节，deferred drain 记录 `snapshot_hash`；这些字段与根级隐私规则冲突或至少没有出现在批准的稳定分类清单，推广时应 clean cutover 到计数、大小 bucket、结果分类和耗时。
- 并非每个 `Instant` 都是迁移目标：fan-out deadline、重试 deadline、网络切换窗口、cooldown 和测试等待属于行为语义，必须保留在 owner 内。
- Application 内部纯计算阶段（planner、decode、summarize）没有 Engine 持有的真实 adapter seam。不得为了保留每个旧阶段指标而新增“记录阶段”的浅 port 或重新泄漏 use case；无真实 seam 的手工阶段指标应删除，或由后续独立设计把该行为深化成真正可替换的 module。

## Technical Decisions
| Decision | Rationale |
|----------|-----------|
| 推广领域具体 decorator assembly，不创建通用 `Observed<T>` | 现有设计明确要求业务 operation、policy、schema 保持领域所有权；通用框架会把字符串阶段和字段知识泄漏给调用者。 |
| 只迁移持续跨层依赖调用的计时/结果分类 | deadline、退避、冷却属于业务/运行时语义；产品 analytics 可能是业务结果事件，不能与性能 decorator 混为一谈。 |
| 不为观测新增一次性 callback/recorder port | 它只转移日志语句，不隐藏复杂性；删除后不会把业务能力复杂度散回调用者，属于浅 module。 |
| 保持 Engine 只装饰其本来选择的 adapter | 避免回退 031 已完成的 Application 对象图深化。 |
| 规格文件为 `docs/exec-plans/completed/035-space-domain-observability-assembly.md` | 与现有编号连续，并明确本轮只在真实 Space seam 实施，而非建立通用 telemetry framework。 |
| 035 只在同一 Space 装配时点从 admission 推广到 membership | Clipboard/Blob 的 adapter 在不同生命周期时点出现；把它们纳入会迫使多入口或重排 031 的两阶段 Application assembly，应先另行设计。 |
| 035 的领域 schema 只记录固定 operation/outcome/error kind、耗时和已批准计数 | 禁止直接记录参数、错误文本、身份、业务 id、digest、凭据或 payload。 |
| 只观测 Application 直接调用的 port | 当前 `AdmissionSpaceTransitionPort` 是 Infra preparation/executor 的内部依赖；外层和内层同时计时会重复计算，并迫使 admission 暴露第二入口。 |
| 后继推广以真实 adapter seam 为单位 | 不为 Clipboard/Blob 的单个宽泛入口重排 Application 两阶段生命周期；本实现范围仍只有 Space。 |

## Issues Encountered
| Issue | Resolution |
|-------|------------|
| `to-spec` 初次读取路径错误 | 从仓库 skill 根读取完整说明。 |

## Resources
- `.agents/skills/to-spec/SKILL.md`
- `/home/mark/.agents/skills/codebase-design/SKILL.md`
- `/home/mark/.agents/skills/planning-with-files/SKILL.md`
- `docs/design-docs/observability.md`
- `docs/design-docs/engineering-principles.md`
- `docs/architecture/architecture-bible.md`
- `crates/uc-engine/src/assembly/observability/admission.rs`
- `crates/uc-engine/src/assembly/sync_engine.rs`
- `docs/exec-plans/completed/031-application-dependency-surface-deepening.md`
- `docs/exec-plans/completed/035-space-domain-observability-assembly.md`
