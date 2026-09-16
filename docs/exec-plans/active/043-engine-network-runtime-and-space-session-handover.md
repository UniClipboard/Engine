# 规格 043：Engine 网络运行期与 Space 会话安全交接

## 状态

- **状态**：实施中；Slice 1–5 的主要生产路径已接通，正在完成分布与恢复验收
- **日期**：2026-09-15
- **对应问题**：GitHub Issue #68
- **前置规格**：[038 双设备配对本机耗时压缩到一秒](038-pairing-local-latency-budget.md)
- **相关架构工作**：Issue #34 负责发现与连接边界，本规格不实施该问题
- **完整负责人**：Engine 的 `SessionSupervisor` 负责普通 Space 会话交接、显式网络重建、挂起、恢复和最终关闭；Infra 的 `IrohNetworkRuntime` 负责唯一 endpoint、Router、协议代际门和连接清理
- **调用方唯一动作**：调用方继续只提交一次 Space transition、Device Reset、显式网络恢复、`suspend`、`resume` 或 shutdown，不编排协议切换、连接清理或 endpoint 重建
- **成功结果**：普通 Space transition 在同一个 endpoint 上安全发布新会话；新请求只进入新会话，旧请求完成、取消或拒绝；操作门重新开放
- **失败结果**：未提交前失败时从当前权威状态重建唯一会话；已提交后失败时保持操作门关闭并重试安装目标会话，不恢复旧权限，不创建第二个同身份 endpoint
- **重试与重启责任**：`SessionSupervisor` 在当前进程内重试会话安装；进程重启或 `resume` 从持久状态恢复。只有显式网络恢复、挂起、最终关闭或必须更换身份/配置时完整关闭并重建 endpoint

## 当前实施进度

- 2026-09-15：完成 Slice 1 的测试专用最小端到端版本。Infra 内部代际注册表能够在不重启 Router 的情况下封口当前处理环境、主动关闭旧连接、等待在途请求退出，再发布新处理环境；真实本机 Iroh 测试先失败后通过。生产 ALPN 和生产会话路径尚未接入，当前产品行为不变。
- 2026-09-15：Slice 2 已开始接入生产路径：固定 Router 只注册稳定分发入口，全部正式协议 handler 先构成完整集合再一次发布；endpoint 连接钩子统一约束所有入站和出站连接，避免在各 adapter 散落会话判断。当前 endpoint 仍由现有生产会话持有并在关闭时完整释放，跨 Space 保活尚未启用。
- 2026-09-15：长期网络已从 ProductionSession 移交给 SessionSupervisor。普通 Space transition 与 Device Reset 只替换完整会话能力；显式网络恢复、suspend 和最终 shutdown 仍完整关闭网络。真实本机双端配对取得二十份完整样本：端到端 p50 1.728 秒、p95 1.747 秒、最大 2.062 秒；本机处理 p50 1.596 秒、p95 1.614 秒；网络相关 p50 0.133 秒、p95 0.136 秒。本规格的 3 秒中位数目标达到，规格 038 的 1 秒本机处理门仍未达到。二十次循环中另有一次未产出完整样本，失败注入和实体设备矩阵尚未完成。
- 2026-09-15：首轮实现审查后的整理已完成。`SessionSupervisor` 用同一个运行状态共同持有网络与会话，关闭网络前显式拒绝仍有会话的非法状态；切换 watcher 在 `network=Some, session=None` 时重新安装当前权威会话，不再把可恢复失败当作无工作。运行期会话失败保留既有错误编号、类别和可重试语义，但不再伪装成启动失败。三秒交接门已拆为独立测试，整理后真实本机双端两次结果为 1.779 秒和 1.793 秒；失败注入、重复分布和实体设备矩阵仍待完成。
- 2026-09-15：完成正式提交后会话准备失败的首个真实故障验收。测试连续制造两次准备失败，并确认失败窗口内旧 Space 操作保持关闭、网络只构造一次，第三次后台尝试在当前进程恢复目标 Space，双方最终仍为有效成员。随后三秒交接门复测为 1.844 秒。持久提交失败、发布前取消等其余故障边界和实体设备矩阵仍待完成。
- 2026-09-15：故障验收扩展到旧会话封口、持久切换完成、会话准备和新会话发布四个边界，每个边界连续失败两次后都在当前进程恢复，旧权限保持关闭且长期网络只构造一次。验收发现并修复两处恢复缺口：持久切换的临时失败会错误停止重试；未发布的新会话没有完整关闭全部后台任务，会阻塞下一次准备。故障修复后三秒交接门两次复测为 1.695 秒和 1.842 秒。发布前取消、在途业务请求和实体设备矩阵仍待完成。
- 2026-09-15：补充生命周期取消与在途文件发送验收。切换恢复空窗内执行暂停后，旧恢复任务不会重建网络；恢复只重建一次并完成加入。慢宿主文件读取跨过两段排空期限后，旧发送确定返回取消、临时导入为零、固定网络不重建，新会话可继续接收正文。当前发布成功到监督状态写入之间没有异步让出点，不存在可被任务取消插入的中间窗口；本轮三秒交接门复测为 1.853 秒。实体设备和真实远端大文件传输仍待执行。

# 1. Overview

当前生产会话同时拥有 Application 运行期和完整 Iroh 网络节点。加入方完成准入、Device Reset 或显式恢复时，`SessionSupervisor` 会先销毁整个 `ProductionSession`，等待 `Endpoint::close()` 完成，再重新绑定相同设备身份并安装新会话。

2026-09-15 的本机双设备测量已经确认：四次准入交换约 1.3 秒完成，11 次完整配对中位数为 4.827 秒；慢样本中，旧 endpoint 的安全关闭约占 3.28–3.31 秒。撤回无收益的小修后，回归测试再次得到端到端 5.116 秒、本机工作 4.977 秒。等待发生在底层连接的正常排空期，不能通过缩短外层超时或强制中止规避；既有实验已经出现后继同身份 endpoint 121 秒不可用的反例。

根因不是准入协议轮次，而是网络入口与 Space 会话共享同一个销毁边界。普通 Space 切换不改变设备网络身份、Iroh 配置或底层 socket 所有权，却仍然承担完整 endpoint 关闭和重建成本。

本规格把网络运行期定义为一次“Engine 活跃期”：从启动或 `resume` 建立，到 `suspend`、最终 shutdown 或显式完整网络重建结束。Space 会话只拥有当前一代 Application 运行期及其协议处理上下文。普通切换只替换会话代际，不关闭 endpoint；旧代际的连接在切换时被明确取消或拒绝，QUIC endpoint 自身继续运行。

本规格是规格 038 的后续性能切片，也是 Issue #34 未来连接架构的基础，但不顺带引入发现提供者、多后端或新的领域连接接口。

# 2. Goals

- 普通 Space transition、成功的 Device Reset 及启动期待完成 Space transition 不调用 `Endpoint::close()`，也不重新绑定 endpoint。
- 同一次 Engine 活跃期内只存在一个 Iroh endpoint、一个 Router 和一套设备级网络后台任务。
- 所有绑定当前 Space 的入站协议处理能力作为一个不可拆分的代际整体发布，任何请求只能取得一个完整代际。
- 切换封口后不再接受旧代际的新入站或出站工作；已进入旧代际的工作必须完成、取消或得到明确网络拒绝。
- 旧代际请求即使迟到，也不能读取或修改新 Space 的 Application、成员、安全状态、剪贴板或文件传输状态。
- 切换失败后保持唯一可恢复状态：未正式提交时按当前权威状态重建；正式提交后只恢复目标会话，不重新开放旧权限。
- 显式网络恢复、`suspend`、最终 shutdown 和确需更换身份或配置的路径仍完整等待 Iroh 安全关闭，不降低现有关闭期限。
- 保留公开 `uc-engine`、UniFFI、HarmonyOS 接口、设备间消息格式、持久化格式、P2P 默认行为和现有错误分类。
- 预热后至少运行 20 次本机双设备配对，端到端中位数小于 3 秒；同时报告 p95 和规格 038 定义的本机耗时。
- 双设备重启、三设备离线恢复、文件传输中切换、移动端挂起恢复和真实手机/桌面配对均有明确验收结果；未执行项只标记为跳过。

# 3. Non-Goals

- 不缩短 Iroh/noq 的安全关闭期限，不以 timeout、abort、丢弃节点或并行绑定伪造关闭完成。
- 不允许两个使用同一设备身份的 endpoint 同时运行。
- 不修改准入消息、成员历史消息、剪贴板消息、文件票据或其他设备间格式。
- 不修改任何数据库、密文记录、持久化版本或迁移。
- 不新增公开 Engine、绑定、Application facade 或 Core 接口来查询代际、协议步骤或网络内部状态。
- 不把 Iroh 类型暴露给 Application 或 Core。
- 不实施 Issue #34 的 `DiscoveryProvider`、`ConnectivityManager`、mDNS 或多 backend 设计。
- 不改变 Application 对配对、成员历史、剪贴板、文件传输和恢复结果的完整责任。
- 不把正常会话封口、代际发布或 endpoint 保活提升为独立业务记录。
- 不合并或减少准入持久检查点，不用本规格代替规格 038 剩余的本机处理优化。
- 不预先增加可插拔网络运行期、可配置代际策略或通用热加载框架。

# 4. Current Architecture Context

```text
Component: SessionSupervisor
Path: crates/uc-engine/src/runtime/session_supervisor.rs
Responsibility: 串行处理会话安装、Space transition、Device Reset、显式恢复、挂起和恢复。
Relationship: 当前普通切换与完整网络恢复都通过 ProductionSession::shutdown 销毁网络，再调用 install_new_session 重建。
```

```text
Component: ProductionSession / ProductionSessionFactory
Path: crates/uc-engine/src/runtime/session_supervisor.rs
Responsibility: 持有 AppFacade、ApplicationRuntime、Engine 会话任务和 SyncEngineAssembly。
Relationship: Factory 每次 build 都调用 build_daemon_lifecycle，因而每次会话安装都会绑定新的 Iroh endpoint。
```

```text
Component: SyncEngineAssembly
Path: crates/uc-engine/src/assembly/sync_engine.rs
Responsibility: 一次构造 Iroh adapter、Application 网络对象图、全部协议 handler、进度翻译任务和 IrohNode。
Relationship: 当前同时承担设备级网络资源和 Space 会话资源，shutdown 最终关闭 IrohNode。
```

```text
Component: ApplicationNetworkBinding / ApplicationRuntime
Path: crates/uc-application/src/application.rs
Responsibility: 先构造 dormant Application 网络对象图，向 Engine 提供需注册的窄 endpoint，注册完成后启动领域运行期。
Relationship: 该两阶段构造已经提供“先准备处理能力、后启动会话”的安全边界，可用于准备新代际，不需要新增公开步骤接口。
```

```text
Component: IrohNodeBuilder / IrohNode
Path: crates/uc-infra/src/network/iroh/node.rs
Responsibility: 绑定唯一身份和 endpoint，构造 adapter，安装固定 ALPN handler，启动 Router、地址发现、连接观测与中继恢复，并在最终关闭时安全排空。
Relationship: Router 启动后不能替换 handler；现有 install_* 全部要求在 spawn 前完成。
```

```text
Component: ProtocolRouterBuilder
Path: crates/uc-infra/src/network/iroh/protocol_router.rs
Responsibility: 注册固定协议并按兼容优先级启动 Iroh Router。
Relationship: 当前把具体 handler 永久放入 Router；需要改为永久注册固定 dispatcher，再由 dispatcher 选择当前完整代际。
```

```text
Component: SessionOperationGate
Path: crates/uc-engine/src/runtime/session_supervisor.rs
Responsibility: 切换时拒绝新的 Engine 本地操作，等待既有操作，随后取消超时操作。
Relationship: 只覆盖经 Engine 操作入口取得 lease 的本机操作，不能覆盖 Router 已接收的请求和 Application 后台发起的网络工作。
```

```text
Component: PeerReachabilityPort / IrohPeerReachabilityAdapter
Path: crates/uc-core/src/ports/peer_reachability.rs
Path: crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs
Responsibility: 管理长期在线连接和本机缓存；disconnect_all 已能递增内部代际、拒绝新入站、关闭旧连接并清除状态，activate 恢复接收。
Relationship: 会话交接必须复用该现有能力，不能再建立第二套在线状态生命周期。
```

```text
Component: NetworkRecoveryFacade / RebuildNetworkSessionPort
Path: crates/uc-application/src/space/connectivity/recovery/
Path: crates/uc-engine/src/runtime/session_supervisor.rs
Responsibility: Application 负责显式恢复流程，Engine 负责执行完整网络会话重建。
Relationship: 本规格必须保留这一完整重建语义，不能把它静默改成只换 Space 会话。
```

当前普通 Space transition：

```text
发现待切换
  -> 关闭 Engine 本地操作门并等待/取消操作
  -> ProductionSession::shutdown
       -> 停止 Engine 会话任务
       -> ApplicationRuntime::shutdown
       -> SyncEngineAssembly::shutdown
            -> 停止进度翻译
            -> Endpoint::close
            -> Router::shutdown
  -> 提交持久 Space transition
  -> Factory::build
       -> 重新绑定同身份 endpoint
       -> 重装全部 handler
       -> 启动新 ApplicationRuntime
  -> 重新开放操作门
```

当前 Router 中与本规格有关的协议族包括：在线状态当前/旧版本、成员更新、剪贴板、活动剪贴板、传输进度、Blob、Space 准入、成员历史、成员分支恢复和活动剪贴板拉取。邀请解析与连接提示不注册 ALPN，但它们同样持有 endpoint 或当前会话能力，必须在会话构造时取得正确代际。

# 5. Proposed Design

## Components

### `IrohNetworkRuntime`

- **职责**：拥有一次 Engine 活跃期内唯一的 endpoint、Router、Blob store、地址发现、连接观测、中继恢复任务、固定协议 dispatcher 和唯一节点运行租约。
- **输入**：现有设备网络身份、Iroh 配置、网络分区测试门和 Blob store 目录。
- **输出**：一个可供会话准备使用的 `IrohNetworkHandle`；最终 shutdown 结果。
- **关系**：由 `SessionSupervisor` 持有，不再放入 `ProductionSession`；只有完整网络重建、`suspend` 和最终 shutdown 能消费并关闭它。

网络运行期创建时一次注册当前生产协议的固定 ALPN。每个 ALPN 对应一个稳定 dispatcher；Router 启动后不再修改协议表。没有活动代际时，dispatcher 明确拒绝连接，不等待未来会话出现。

Blob store 和出站进度翻译任务移到网络运行期。Blob handler 同样经过代际门：底层 store 可以跨 Space 会话存活，但每条入站连接必须属于一个仍有效的会话代际；切换时旧连接被关闭，不能依靠长期 store 绕过会话边界。

### `SessionProtocolRegistry`

- **职责**：原子保存“当前完整协议代际”，并为所有固定 dispatcher 提供同一个获取入口。
- **输入**：已经完整准备、尚未发布的 `PreparedSessionGeneration`。
- **输出**：`SessionNetworkLease`，或固定的 `Unavailable` / `Draining` 拒绝。
- **关系**：属于 Infra 私有实现；Engine 只能调用 prepare、activate、quiesce 三个完整动作，不能逐协议替换字段。

注册表只保存一个 `Option<Arc<SessionNetworkGeneration>>`。发布操作在同一个同步临界区内替换整个 `Arc`；禁止为四组或十组 handler 分别保存可替换槽位。这样任意请求取得的成员、安全、剪贴板和文件能力必然来自同一代际。

注册表使用标准库互斥与现有异步通知能力，不为单次指针交换新增依赖。同步锁只用于取得/替换 `Arc` 和更新短小计数，不跨 `.await` 持有。

### `SessionNetworkGeneration`

- **职责**：保存一个 Space 会话的全部入站 handler、出站工作门、取消令牌、活动请求计数和活动连接表。
- **输入**：当前会话构造出的具体 Iroh handler 集合及会话编号。
- **输出**：请求 lease；封口、取消和排空结果。
- **关系**：由 `IrohSessionBuilder` 一次构造，由 `SessionProtocolRegistry` 一次发布；旧请求只持有旧代际的 `Arc`，永远不会重新查询并跳到新代际。

每个入站 dispatcher 在调用具体 handler 前取得 lease，并登记连接。每个出站 adapter 在解析地址、拨号或复用连接前取得同一代际的 lease，并在整个请求结束前持有。封口执行以下原子顺序：

1. 把 `accepting` 设为 false；此后新 lease 全部拒绝。
2. 触发代际取消令牌。
3. 调用现有 `PeerReachabilityPort::disconnect_all()`，清除旧在线状态。
4. 主动关闭登记的入站与出站连接。
5. dispatcher 在具体 handler future 与取消令牌之间选择；取消胜出时丢弃旧 handler future，并返回固定网络拒绝。
6. 等待活动 lease 归零，不等待 endpoint 的 QUIC 全局排空。

正常完成、明确取消和明确拒绝都释放 lease。lease 的 `Drop` 必须减少活动计数并在归零时通知等待者，保证调用 future 被丢弃时也不会永久卡住交接。

### `IrohSessionBuilder` / `PreparedIrohSession`

- **职责**：复用现有 `IrohNodeBuilder::install_*` 的 adapter 构造知识，但不再修改已启动 Router；一次构造某个 Space 会话的出站 adapters、入站 handler 集合和 Application 网络输入。
- **输入**：`IrohNetworkHandle`、现有 `SyncEngineDeps` 和 Application assembly。
- **输出**：未发布的 `PreparedIrohSession`，其中包含 `ApplicationNetworkBinding`、当前代际 handler 集合和会话需要的客户端 adapter。
- **关系**：Engine 组合根负责调用；Application 仍通过现有两阶段 binding 暴露窄 endpoint，不知道 Router 或代际。

现有 `build_sync_engine_assembly` 拆成两个内部完整能力：

- `build_network_runtime`：每个 Engine 活跃期调用一次；
- `prepare_session_network`：每次 Space 会话构造调用一次。

所有协议字段必须集中在一个封闭结构中，构造函数要求一次给齐。新增 ALPN 时若没有明确归入设备级或会话级，编译或测试必须失败，防止未来 handler 绕过代际门。

### `ProductionSession`

- **职责**：只拥有当前 `AppFacade`、`ApplicationRuntime`、Engine 会话任务、可选 LAN 兼容 facade 和本代 `SessionNetworkGenerationHandle`。
- **输入**：已准备并激活的会话代际。
- **输出**：会话关闭和领域运行期关闭结果。
- **关系**：不再拥有 endpoint、Router 或最终网络关闭责任。`shutdown` 只停止会话任务和 Application；代际封口由 `SessionSupervisor` 在调用它之前完成。

### `SessionSupervisor`

- **职责**：成为普通会话交接和完整网络重建的唯一编排者，并确保两个路径不会混用。
- **输入**：现有 Space transition、Device Reset、显式恢复、挂起、恢复和 shutdown 请求。
- **输出**：现有稳定 `EngineError` / 操作结果；不公开内部阶段。
- **关系**：分别持有 `Option<IrohNetworkRuntime>` 和 `Option<ProductionSession>`，继续使用已有生命周期互斥和本地操作门。

它提供两个私有完整动作：

- `handover_space_session`：保留网络运行期，只替换会话代际；
- `rebuild_network_runtime`：封口当前代际，完整关闭唯一 endpoint，确认释放后再绑定新的 endpoint 并安装当前会话。

`transition_pending_session` 和 Device Reset 使用前者；`rebuild_session`、`suspend` 和最终 shutdown 使用后者或其关闭部分。`resume` 在没有网络运行期时创建一个，再恢复当前会话。

## Data Model

本规格不增加持久化数据。以下结构仅存在于一次 Engine 活跃期内：

| 字段 | 含义 | 生命周期 |
| --- | --- | --- |
| `generation` | 进程内单调递增、不可对外解析的会话代际 | 当前 Engine 活跃期；重启后重新开始 |
| `accepting` | 当前代际是否允许取得新 lease | prepare 时 false，activate 时 true，quiesce 后永久 false |
| `cancellation` | 取消旧代际的所有在途协议工作 | 当前代际 |
| `active_leases` | 已进入当前代际的入站和出站工作数量 | 当前代际；归零后允许销毁会话 |
| `connections` | 由当前代际启动或接受、需要在交接时主动关闭的连接 | 当前代际 |
| `handlers` | 当前 Space 的完整协议 handler 集合 | 当前代际；整体发布、整体释放 |

`generation` 不进入 Core、Application、日志、trace、设备间消息、公开错误或持久化。它只用于进程内防止旧工作穿越交接，不是业务身份、权限来源或恢复依据。

`PreparedSessionGeneration` 只允许 `Prepared -> Active -> Draining -> Retired` 的单向变化。`Retired` 永远不能重新激活。失败恢复必须重新构造一个新代际，而不是重新打开已经封口的旧代际。

Engine 运行组合允许以下状态：

| 网络运行期 | ProductionSession | 含义 |
| --- | --- | --- |
| 无 | 无 | 已挂起、尚未启动或最终关闭 |
| 有 | 无 | 正在交接、会话安装失败等待恢复，或启动恢复中；本地操作门关闭，网络请求被拒绝 |
| 有 | 有 | 正常运行，且注册表中恰好发布该会话代际 |
| 无 | 有 | 非法状态；必须由断言和测试阻止 |

## API / Interface

新增接口全部为 Infra/Engine 内部能力，名称允许实现时按现有模块命名调整，但职责和原子边界不得拆散：

```text
IrohNetworkRuntime::bind(inputs) -> Result<(IrohNetworkRuntime, IrohNetworkHandle), IrohNodeError>
IrohNetworkRuntime::activate(prepared: PreparedSessionGeneration) -> Result<SessionGenerationHandle, SessionGenerationError>
IrohNetworkRuntime::quiesce(handle: SessionGenerationHandle, reason) -> Result<SessionDrainReport, SessionGenerationError>
IrohNetworkRuntime::shutdown(self) -> NetworkShutdownReport
```

```text
IrohSessionBuilder::new(network: IrohNetworkHandle, deps: SyncEngineDeps)
IrohSessionBuilder::prepare(application: ApplicationAssembly) -> Result<PreparedIrohSession, SyncEngineAssemblyError>
PreparedIrohSession::complete(application_binding) -> Result<PreparedSessionGeneration, SyncEngineAssemblyError>
```

```text
SessionSupervisor::handover_space_session(reason, commit_action) -> Result<ProductionSession, EngineError>
SessionSupervisor::rebuild_network_runtime(reason) -> Result<(), EngineError>
SessionSupervisor::recover_current_session() -> Result<(), EngineError>
```

这些不是新增公开入口。不得把 `generation`、Iroh endpoint、handler 列表、内部切换步骤或排空计数加入 `AppFacade`、`uc-engine` 稳定接口或平台绑定。

错误处理规则：

- Infra 错误保留底层 source，并在 Engine 现有启动、会话恢复或操作错误边界转换。
- `NoActiveGeneration`、`GenerationDraining` 和 `GenerationMismatch` 只用于内部控制；对现有协议调用方表现为已有的不可用、连接关闭或取消分类，不新增 wire 错误格式。
- 排空任务失败或超时不得被记录为成功。dispatcher 自己持有取消分支，因此正常情况下不需要 abort handler task；若内部任务不响应，返回明确运行期失败并保持操作门关闭。
- 正式 Space transition 已提交后，后继构造失败不能返回并恢复旧会话。当前调用返回现有失败，后台或后续生命周期触发 `recover_current_session`。

## Workflow

### 首次启动或 `resume`

1. `SessionSupervisor` 在生命周期互斥内保持本地操作门关闭。
2. 若没有网络运行期，调用 `build_network_runtime`，启动固定 dispatcher；此时注册表为空，所有协议明确拒绝。
3. 调用 `prepare_session_network` 构造当前持久状态对应的 dormant Application 与完整 handler 代际。
4. 若发现待完成 Space transition，不发布旧代际；使用 dormant facade 完成原持久恢复，关闭该未启动 Application，再从新的权威状态重新 prepare。
5. 启动目标 `ApplicationRuntime`。启动失败时丢弃未发布代际，网络运行期保持空注册表。
6. 一次发布完整代际，调用 `PeerReachabilityPort::activate()`，安装 `ProductionSession`。
7. 重新开放本地操作门。

### 普通 Space transition

1. 检查并再次确认存在待切换状态；取得生命周期互斥，关闭本地操作门。
2. 对当前代际执行 `quiesce`：拒绝新工作、取消旧工作、断开在线连接、关闭登记连接并等待 lease 归零。
3. 停止 Engine 会话任务和旧 `ApplicationRuntime`；endpoint、Router、Blob store 和设备级后台任务继续运行。
4. 调用现有 `complete_pending_space_transition()`。这仍是持久业务切换的唯一入口。
5. 无论第 4 步成功或失败，都从当前权威持久状态重新 prepare 一个全新会话；不得重新激活已封口旧代际。
6. 若正式切换已经提交，只允许安装目标会话；若尚未提交，安装当前权威旧 Space 的新会话代际。
7. 启动 Application，原子发布新代际，恢复在线状态，安装 `ProductionSession`，重新开放操作门。
8. 返回现有 revision 或原始失败。若业务动作失败但恢复会话成功，返回业务动作失败；若会话恢复也失败，保持操作门关闭并返回恢复失败。

### Device Reset

1. 复用调用方当前 operation lease，关闭其余本地操作。
2. quiesce 当前网络代际并取消活动文件传输。
3. 停止旧 Application，但不关闭 endpoint。
4. 执行现有 reset 完整动作。
5. 从 reset 后的权威状态准备、发布新会话；若动作已提交但返回中断，继续复用现有 `has_committed_device_management_reset()` 判断和重试语义。
6. 失败恢复遵循普通 transition 的“只从权威状态重建”规则。

### 显式完整网络恢复

1. 关闭本地操作门并 quiesce 当前代际。
2. 停止当前 Application 与会话任务，取消活动文件传输。
3. 消费并完整关闭唯一 `IrohNetworkRuntime`，等待 `Endpoint::close()` 和 Router 收尾。
4. 只有第 3 步完成后才绑定新的 endpoint，保证同身份不双活。
5. 从当前持久状态准备并发布会话；成功后开放操作门。
6. 失败时保持无 endpoint 或唯一新 endpoint 的可恢复状态，不回退并行旧 endpoint。

### `suspend`、最终 shutdown 与 `resume`

1. `suspend` 复用完整关闭路径：封口代际、停止会话、取消传输、完整关闭网络运行期；成功状态必须是“无网络、无会话”。
2. `resume` 新建一次网络运行期并恢复当前持久会话；若已经正常运行，继续返回现有 skipped。
3. 最终 shutdown 与 `suspend` 使用同一唯一网络关闭 owner，之后清除 factory，禁止复活。

# 6. Implementation Plan

## Slice 0：固定基线、清单和失败测试

**File:** `crates/uc-engine/tests/space_membership_auto_pairing_e2e.rs`

**File:** `crates/uc-infra/src/network/iroh/protocol_router.rs`

**File:** `crates/uc-infra/src/network/iroh/node.rs`

**Change:** 保留当前可重复失败的配对时间测试；增加仅在 `test-util` / `dev-tools` 存在的进程内网络运行探针，记录 bind、完整 close、活动 endpoint 数量和会话代际发布次数。探针不得进入公开 Engine/绑定，不记录 endpoint identity。建立当前所有生产 ALPN、无 ALPN adapter、长期任务、入站/出站连接和关闭责任清单，并由测试固定清单完整性。

**Tests:**

- 当前普通 Space transition 的 close 计数先得到 1，形成红测。
- 同一时刻活动 endpoint 数不超过 1。
- 测试过滤器先通过 `--list` 确认实际运行数量。
- 运行撤回后的性能基线并保存 p50/p95，不以最快值作为对照。

**Risk:** 探针可能变成长期调试接口。只允许测试构建注入计数器；生产路径使用无额外状态的默认实现，并在最终切片删除不再需要的临时字段。

## Slice 1：建立固定 Router 与代际注册表的最小端到端版本

**File:** `crates/uc-infra/src/network/iroh/protocol_router.rs`

**File:** `crates/uc-infra/src/network/iroh/node.rs`

**File:** `crates/uc-infra/src/network/iroh/session_generation.rs`（新增）

**File:** `crates/uc-infra/src/network/iroh/mod.rs`

**Change:** 增加 `SessionProtocolRegistry`、`SessionNetworkGeneration`、lease、连接登记与稳定 dispatcher。先用测试专用协议跑通“空注册拒绝、发布后处理、封口后拒绝、旧在途取消、新代际处理”；生产 ALPN 注册和生产会话路径在本片保持原样，不形成部分生产协议走新门、部分协议走旧门的中间架构。

**Tests:**

- 空注册表立即拒绝，不等待 publish。
- 一次请求只取得一个代际；发布新代际后，旧请求仍引用旧代际且收到取消。
- 并发 acquire 与 quiesce 不会在封口后新增 lease。
- lease future 被丢弃仍减少计数；排空不会永久等待。
- Router 协议优先级及旧版在线状态协商保持不变。

**Exit gate:** 测试专用的真实本机 Iroh 入站和出站请求完成代际切换；生产协议行为不变，完整 endpoint shutdown 仍通过现有安全关闭测试。

**Risk:** acquire 与封口竞态可能让请求越过边界。`accepting` 检查和活动计数必须在同一同步临界区内完成，不能用先读布尔值、后加计数的两步原子操作。

## Slice 2：迁移全部协议和出站 adapter，形成完整会话代际

**File:** `crates/uc-infra/src/network/iroh/node.rs`

**File:** `crates/uc-infra/src/network/iroh/connect.rs`

**File:** `crates/uc-infra/src/network/iroh/space_admission/`

**File:** `crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs`

**File:** `crates/uc-infra/src/network/iroh/{group_update_adapter.rs,clipboard_dispatch_adapter.rs,clipboard_receiver_adapter.rs,membership_history_exchange_adapter.rs,membership_branch_recovery_adapter.rs,transfer_progress_adapter.rs,blobs.rs}`

**File:** `crates/uc-infra/src/network/iroh/active_clipboard/`

**Change:** 一次性把所有生产 handler 迁入一个封闭 `SessionProtocolHandlers`；把现有 `install_*` 的“构造 adapter”和“注册 Router”分开，形成 `IrohSessionBuilder`。`IrohNodeBuilder` 在首次构造时一次注册全部固定 dispatcher，并在 spawn 前发布第一代完整 handler；此时 endpoint 仍随 `ProductionSession` 重建，用户可见生命周期不变。所有入站协议经固定 dispatcher，所有出站拨号和请求取得同一代际 lease。复用 `PeerReachabilityPort::disconnect_all()` / `activate()`；Blob handler 和官方 store 仍由当前节点持有，但每条 Blob 连接受代际取消控制。邀请和 connection hints 从当前会话 builder 取得同一个 endpoint 句柄，不注册第二套 Router。

**Tests:**

- 对每个生产 ALPN 执行“旧代际成功、新代际发布后旧目标不再被调用”。
- 在线状态长连接、准入、成员历史、剪贴板、活动剪贴板、进度和 Blob 在 quiesce 时均完成、取消或拒绝。
- 所有出站拨号在封口后返回现有不可用/取消分类，不创建未登记连接。
- `disconnect_all` 清除旧在线缓存，`activate` 后新连接才能重新报告在线。
- no-subscriber、广播滞后和回执丢失保持原有明确结果，不把丢弃当成功。

**Exit gate:** 生产协议清单没有任何直接注册具体会话 handler 或绕过代际 lease 的 endpoint 拨号；仓库检查增加防回归规则或等价结构测试。

**Risk:** 一次迁移协议面较大。可以按协议族逐个编写和验证，但本 Slice 必须作为一个完整生产切片提交；在全部生产协议进入同一代际前不启用新路由方式，也不启用跨 Space 保活。

## Slice 3：拆分网络装配与会话装配

**File:** `crates/uc-engine/src/assembly/sync_engine.rs`

**File:** `crates/uc-engine/src/assembly/lifecycle.rs`

**File:** `crates/uc-engine/src/assembly/deps.rs`

**File:** `crates/uc-application/src/application.rs`

**Change:** 把 `build_sync_engine_assembly` 收敛为 `build_network_runtime` 和 `prepare_session_network` 两个内部完整能力。`SyncEngineAssembly` 改为 Engine 活跃期网络 owner；`PreparedIrohSession` 继续使用现有 `ApplicationNetworkBinding` 两阶段构造，不向 Application 增加代际概念。进度翻译和 Blob store 移到网络运行期；会话专属订阅、facade 和 Application owner 留在 `ProductionSession`。

**Tests:**

- 一个网络运行期连续准备两个会话，bind 计数保持 1，完整 close 只在最终 shutdown 为 1。
- 第二个会话未发布前，旧会话仍是唯一当前代际；发布操作不可部分成功。
- Application 启动失败时未发布代际被释放，注册表保持旧代际或空，不出现半装配 handler。
- 首次启动、无 Space、锁定 Space 和启动期 pending transition 均保持现有结果。

**Exit gate:** 首次启动所有现有 E2E 行为不变；本片尚未改普通 transition 时序，但网络与会话已经由不同 owner 持有。

**Risk:** Application 构造与 handler 构造存在双向依赖。必须复用现有 dormant binding：先构造出站能力和 dormant Application，取得窄入站 endpoint，完成 `PreparedIrohSession`，最后启动 Application；不得新增逐 endpoint 的公开装配接口。

## Slice 4：启用普通 Space handover 与失败恢复

**File:** `crates/uc-engine/src/runtime/session_supervisor.rs`

**File:** `crates/uc-engine/src/runtime/mod.rs`

**File:** `crates/uc-engine/tests/space_membership_auto_pairing_e2e.rs`

**Change:** `SessionSupervisor` 分别持有网络运行期和当前会话；实现私有 `handover_space_session`。`transition_pending_session` 改为先 quiesce、再关闭旧 Application、提交持久 transition、从权威状态准备新代际并一次发布。`install_new_session` 改为能在已有网络上恢复会话；session 为空但 network 存在时，后续 watcher、`resume` 或显式恢复会调用 `recover_current_session`，不再直接返回无工作。已封口旧代际不可恢复。

**Tests:**

- 普通配对 transition 前后 bind=1、close=0、active endpoint=1，endpoint 运行实例连续。
- 新代际发布后新请求只命中新 Application；旧代际迟到请求不能读取或修改新 Space。
- quiesce 前失败、持久提交失败、提交成功后 Application 构造失败、publish 前取消分别恢复到规定状态。
- 提交成功后构造失败保持旧权限关闭；重试或进程重启只安装目标 Space。
- watcher 在 `session=None, network=Some` 时能恢复，不产生空转或重复 bind。

**Exit gate:** 原始性能红测转绿：普通 transition 不关闭 endpoint；先运行 5 次确认方向，再运行不少于 20 次并报告端到端 p50/p95、本机 p50/p95、bind/close 次数和失败数。

**Risk:** 正式提交后的失败最危险。恢复判断只能读取现有持久事实；不能保存新的内存“应该回滚”标志，也不能重新开放旧 handler。

## Slice 5：接入 Device Reset、显式网络恢复和移动生命周期

**File:** `crates/uc-engine/src/runtime/session_supervisor.rs`

**File:** `crates/uc-application/src/space/connectivity/recovery/`

**File:** `crates/uc-engine/src/engine/` 下的 shutdown / lifecycle 调用点

**Change:** Device Reset 使用会话 handover；显式 `rebuild_session` 明确改名或内部收敛为 `rebuild_network_runtime`，保留完整 endpoint close；`suspend` 和最终 shutdown 完整关闭网络，`resume` 创建新网络后恢复会话。清除重复关闭责任，保证 endpoint 最终 close 只有 `IrohNetworkRuntime` 一个 owner。

**Tests:**

- Device Reset 成功时 bind=1、close=0；旧成员连接和旧权限全部拒绝。
- 显式网络恢复严格执行 close 旧 endpoint 后再 bind 新 endpoint，活动 endpoint 最大值为 1。
- `suspend` 结束为无网络、无会话；`resume` 恢复为一个网络、一个会话。
- 并发 `suspend`、Space transition、显式恢复继续由生命周期互斥串行，结果不双活、不死锁。
- 最终 shutdown 后不能 resume 或重新发布代际。

**Exit gate:** 现有连接恢复矩阵、启动/暂停/恢复矩阵、Device Reset 和 Engine shutdown 测试全部通过。

**Risk:** Issue #68 原描述的“进程级网络”会使移动端挂起仍保活。本规格明确采用 Engine 活跃期边界；任何让 `suspend` 保留 endpoint 的实现都不接受。

## Slice 6：真实流程验收、设计回写与归档

**File:** `crates/uc-engine/tests/space_membership_auto_pairing_e2e.rs`

**File:** `tests/` 下现有连接恢复、三设备与独立进程宿主

**File:** `docs/design-docs/automatic-peer-connections.md`

**File:** `docs/architecture/architecture-bible.md`

**File:** `docs/exec-plans/active/038-pairing-local-latency-budget.md`

**File:** `docs/exec-plans/active/043-engine-network-runtime-and-space-session-handover.md`

**Change:** 执行完整性能、安全、恢复和设备矩阵；把已经实施的稳定生命周期边界回写长期设计。修正 Issue #68 中两个已经被证据推翻或需要收紧的表述：网络生命周期是 Engine 活跃期而非无条件进程期；准入单向关闭小修不能直接移除约 3 秒。全部验收完成后更新状态并移入 `completed/`。

**Risk:** 本机快速样本不能代表真实设备。性能采用预热后重复分布；实体设备、系统挂起和产品宿主没有执行时明确标为跳过，不得归档为完成。

# 7. Edge Cases

```text
Scenario: 请求在 acquire 与 quiesce 同时发生。
Expected behavior: 请求要么在封口前取得旧代际 lease 并随后被取消，要么在封口后直接拒绝；不能漏计为无 lease 的活动请求。
Implementation: accepting 检查和 active_leases 增加在同一同步临界区内完成。
```

```text
Scenario: 旧请求在新代际发布后才完成网络读取。
Expected behavior: 它仍只持有旧 handler 和旧取消令牌，被取消或按旧代际结束，不能重新查询当前注册表。
Implementation: dispatcher 在开始时固定 Arc<SessionNetworkGeneration>，整个 future 不再次解析 current。
```

```text
Scenario: 旧 handler future 忽略取消或被外层直接丢弃。
Expected behavior: dispatcher 主动关闭登记连接；future 被丢弃时 lease Drop 归还计数，交接不永久等待。
Implementation: 每个 dispatcher select 取消令牌，lease 使用 Drop guard；增加故意挂起 handler 测试。
```

```text
Scenario: Space transition 持久提交失败，但旧 Application 已经停止。
Expected behavior: 已封口旧代际不复活；从当前权威持久状态准备一个新代际，恢复原 Space 的可用会话，然后向调用方返回原提交失败。
Implementation: handover 的恢复分支总是重新 load/prepare，不保存旧代际回滚句柄。
```

```text
Scenario: Space transition 已提交，新 Application 构造或启动失败。
Expected behavior: 旧 Space 权限保持关闭；endpoint 可以继续存在但没有活动代际，本地操作门关闭。当前进程重试或重启只恢复目标 Space。
Implementation: 使用持久 transition 事实作为唯一来源，SessionSupervisor 提供 recover_current_session。
```

```text
Scenario: 发布新代际后，安装 ProductionSession 前任务被取消。
Expected behavior: 不留下“有活动 handler、无生命周期 owner”的状态；新代际立即 quiesce，停止已启动 Application，再进入可恢复空会话状态。
Implementation: publish 后由安装 guard 负责，只有 ProductionSession 成功写入监督器后才解除 guard。
```

```text
Scenario: 切换期间第三台设备继续发送成员历史、剪贴板或在线检查。
Expected behavior: 旧连接被取消；空注册窗口明确拒绝；新代际发布后按新成员范围重新认证，不能使用旧在线缓存授予资格。
Implementation: 所有协议共享同一注册表，切换复用 disconnect_all/activate。
```

```text
Scenario: 文件发送、Blob 下载或进度回传正在进行时切换 Space。
Expected behavior: 当前传输得到现有取消结果，连接关闭，临时文件和持久传输状态由原 Application owner 收口；新会话不接收旧进度或复用旧传输权限。
Implementation: 出站和入站都持有代际 lease；先 quiesce 网络，再调用现有文件取消和 Application shutdown。
```

```text
Scenario: `suspend` 与普通 Space transition 同时触发。
Expected behavior: 生命周期互斥只允许一个动作先完成；最终若 suspend 获胜，状态为无 endpoint、无会话，不因较晚 transition 重建网络。
Implementation: 继续使用 SessionSupervisor 唯一 lifecycle mutex，并在每个 await 后核对最终关闭/工厂状态。
```

```text
Scenario: 显式网络恢复关闭旧 endpoint 失败或 Router watchdog 触发。
Expected behavior: 不并行绑定新 endpoint；返回现有恢复失败并保持操作门关闭。只有唯一旧节点确认释放后才允许后续重试绑定。
Implementation: IrohNetworkRuntime 保留现有安全关闭顺序和唯一 NodeRunLease；关闭结果决定是否能进入 bind。
```

```text
Scenario: 无 Space、Space 锁定或当前成员范围不可用。
Expected behavior: 网络运行期可以存在，注册表为空或发布只能明确拒绝业务的会话；不得从旧 handler、地址或在线缓存恢复权限。
Implementation: 复用现有 fail-closed Application 结果，dispatcher 不补造默认 handler。
```

```text
Scenario: 当前版本与旧设备通信。
Expected behavior: ALPN 优先级、旧版在线协议兼容、认证和业务错误保持不变；代际拒绝只表现为现有连接不可用，后续按现有恢复规则重试。
Implementation: 固定协议表保留当前/旧版 ALPN 排序，运行真实旧版本矩阵。
```

```text
Scenario: 日志或观测系统不可用。
Expected behavior: 会话切换和恢复结果不变，观测失败不阻塞 lease 释放、代际发布或 endpoint shutdown。
Implementation: 只复用现有本地会话诊断，不让观测对象参与状态机判断。
```

# 8. Testing Strategy

## Unit Test

### 代际注册表

- **输入**：两个带不同计数目标的测试代际，一个可控挂起 handler。
- **操作**：并发 acquire、quiesce、publish 和取消 handler future。
- **预期结果**：封口后活动计数不增加；旧请求只命中旧目标；新请求只命中新目标；所有 Drop 路径最终归零。

### 连接登记

- **输入**：入站和出站测试连接各一条，以及一个已自行结束的连接。
- **操作**：执行 quiesce。
- **预期结果**：仍活动连接收到关闭；已结束连接不重复影响结果；排空不等待 endpoint 全局关闭期限。

### 原子发布

- **输入**：每个协议字段带不同代际标记的两个完整 handler 集合。
- **操作**：高并发请求期间发布新集合。
- **预期结果**：单个请求观察到的所有能力来自同一集合，不出现部分新、部分旧。

### 关闭状态

- **输入**：Prepared、Active、Draining、Retired 代际。
- **操作**：重复 activate、quiesce 和 publish。
- **预期结果**：只允许单向状态变化；Retired 不可复活；重复 quiesce 幂等且不重复关闭无关连接。

## Integration Test

### 真实 Iroh dispatcher

- **输入**：两个本机 endpoint 和当前生产 ALPN 测试 handler。
- **操作**：建立旧连接、发布新代际、触发旧请求和新请求。
- **预期结果**：endpoint 与 Router 不重建；旧请求取消或拒绝；新连接只进入新代际。

### Application 两阶段装配

- **输入**：同一网络运行期上的两个不同 Space 测试对象图。
- **操作**：prepare 第一个会话并启动，再 prepare 第二个会话，注入启动失败和发布取消。
- **预期结果**：未发布会话不可达；失败不会污染当前注册表；成功发布后只有第二个会话可达。

### 普通配对 transition

- **输入**：两个真实本机 Engine，正式邀请和加入流程。
- **操作**：完成配对并等待双方成员状态稳定。
- **预期结果**：加入方 bind=1、close=0、活动 endpoint 始终为 1；双方显示两个有效成员并能双向发送。

### 失败恢复

- **输入**：在 quiesce、持久提交、Application 构造、Application 启动和 publish 边界注入一次失败。
- **操作**：重复 transition watcher、显式恢复或重启 Engine。
- **预期结果**：每个边界最终只有一个可用会话；正式提交后不恢复旧权限；没有 handler 的窗口保持关闭式失败。

### 生命周期

- **输入**：正常会话、挂起中的长连接和进行中文件传输。
- **操作**：`suspend`、`resume`、显式网络恢复和最终 shutdown。
- **预期结果**：需要完整关闭的路径仍等待 endpoint；resume 只重建一次；最终关闭后不复活。

## Regression Test

- 规格 038 的双设备性能门禁：预热 5 次后至少记录 20 次，报告端到端 p50/p95、本机 p50/p95、最慢值和失败数。
- 双设备双方重启后成员、剪贴板和文件双向传输。
- 三设备中旧成员在线与离线后恢复两种成员历史收敛。
- 文件上传、下载、活动剪贴板拉取和进度回传中途 Space 切换。
- Device Reset 后旧设备不能连接、查询或继续传输，新单设备 Space 正常运行。
- 连接恢复的直连、中继、长短断网、拒绝新拨号和旧版本矩阵。
- 当前/旧版在线 ALPN 协商顺序保持不变。
- 本地日志和 trace 不新增代际、endpoint identity、地址、设备、Space、成员、文件名或路径。
- `cargo metadata --locked --format-version 1`。
- 受影响的 Infra、Application、Engine 定向测试及真实本机 E2E。
- `cargo test --workspace --all-targets --locked --no-fail-fast`，基线失败与本次回归分开报告。
- `cargo check --workspace --all-targets --locked`。
- `cargo fmt --all -- --check`。
- `node scripts/architecture/check-rust-style.mjs`。
- `RUST_STYLE_BASE_SHA=$(git merge-base origin/main HEAD) node scripts/architecture/check-engine-repository.mjs`。
- `git diff --check`。

## 实体设备矩阵

| 场景 | iPhone + Desktop | Android + Desktop | HarmonyOS + Desktop |
| --- | --- | --- | --- |
| 首次配对 20 次性能分布 | 待执行 | 待执行 | 待执行 |
| 配对后立即双向文本 | 待执行 | 待执行 | 待执行 |
| 文件传输中切换/取消 | 待执行 | 待执行 | 待执行 |
| 应用挂起、恢复后同步 | 待执行 | 待执行 | 待执行 |
| 断网恢复与中继 | 待执行 | 待执行 | 待执行 |

未实际运行的平台最终必须写“跳过”，不能保留“待执行”后宣称规格完成。

# 9. Acceptance Criteria

* [ ] `SessionSupervisor` 是普通会话交接、完整网络重建、挂起恢复和最终关闭的唯一负责人；调用方仍只执行现有完整动作。
* [ ] 一次 Engine 活跃期内恰好创建一个 endpoint 和一个 Router；普通 Space transition 与成功 Device Reset 的 bind=1、close=0、活动 endpoint 最大值=1。
* [ ] `suspend`、最终 shutdown 和显式完整网络恢复仍执行安全 `Endpoint::close()`；新 endpoint 只在旧 endpoint 完全释放后创建。
* [ ] 所有生产 ALPN 经同一个会话代际注册表分派，不存在直接绑定会话 handler 的旁路。
* [ ] 所有生产出站协议在拨号或复用连接前取得当前代际 lease，不存在封口后的新出站工作。
* [ ] handler 集合作为一个整体发布；并发测试不能观察到新旧成员、安全、剪贴板或文件能力混合。
* [ ] 旧代际请求在新代际发布后只能完成旧工作、被取消或被拒绝，不能读取或修改新 Space。
* [ ] 在线状态切换复用现有 `disconnect_all` / `activate`，旧连接和旧缓存不会让旧成员在新 Space 显示在线。
* [ ] 在途准入、成员历史、成员分支、剪贴板、活动剪贴板、进度和 Blob 请求均有完成、取消或拒绝测试。
* [ ] 每个失败注入边界最终只有一个可用会话；不存在“双活 endpoint”“有会话无网络”或“已提交后恢复旧权限”。已验证旧会话封口、持久切换完成、会话准备和新会话发布各连续失败两次；发布前取消和在途业务请求仍待执行。
* [ ] 启动或恢复遇到 pending transition 时不发布旧 handler，完成持久恢复后只发布当前权威会话。
* [ ] 公开 Engine、UniFFI、HarmonyOS、Application facade 和 Core 接口没有增加代际或协议步骤；设备间消息和持久化格式不变。
* [ ] 预热后至少 20 次本机双设备配对端到端中位数小于 3 秒，并报告 p95、本机耗时、最慢值、失败数和每次 bind/close 计数。
* [ ] 双设备重启、三设备离线恢复、文件传输中切换、Device Reset、显式网络恢复和 shutdown 自动化测试通过。
* [ ] 真实手机与桌面配对、挂起恢复和文件场景有实际结果；未执行平台明确标为跳过。
* [ ] 日志、trace、公开错误和测试导出不包含新增敏感身份、地址、Space/member/device 标识、文件名或路径。
* [ ] 全部仓库交付检查通过；任何既存基线失败和跳过项单独列出，不改写为通过。

# 10. Risks and Trade-offs

## 技术风险

- **跨代际越权，严重度最高。** 长期 endpoint 失去“销毁一切”的粗粒度隔离后，必须由注册表、lease、连接关闭和当前成员授权共同证明旧请求不能访问新 Space。任何协议旁路都阻止交付。
- **同身份 endpoint 双活，严重度最高。** 完整网络恢复必须先成功消费旧网络 owner，再允许 bind。不能为缩短失败恢复并行准备第二个 endpoint。
- **正式提交后的错误回滚。** 旧代际已经封口且旧权限可能失效，不能通过 reopen 恢复。代价是目标会话构造失败时出现一个明确不可用窗口，但这比错误恢复旧权限安全。
- **长连接不响应取消。** dispatcher 必须自己选择取消并关闭连接，不能只把 token 传给现有 handler 后假定它会检查。
- **出站旁路。** 多数 adapter 共享 `connect_with_staggered_retry`，但 Space admission 和 Blob 还有独立拨号路径；清单测试必须覆盖所有生产入口。
- **Blob store 跨 Space 存活。** 保留 store 可避免昂贵重建和后台线程堆积，但连接必须受代际门控制，文件票据和内容授权仍按现有规则核验。
- **Application 构造环。** 现有 dormant binding 足以打断依赖；若实现新增大量逐 handler setter，说明边界设计错误，应停止并回到完整 `PreparedIrohSession`。

## 性能影响

- 预计普通 Space transition 可移除约 3 秒的 endpoint 安全关闭尾部，但实际配对仍包含约 1.3 秒通信和其他本机工作，只有 20 次分布能确认中位数是否低于 3 秒。
- 代际 lease、短锁和连接登记会给每个网络请求增加少量进程内成本。锁不跨 await，且只保存 Arc 与计数；验收需确认吞吐和 p95 没有明显回退。
- 完整网络恢复、挂起和最终 shutdown 不会变快，也不以变快为目标。

## 维护成本

- 新增长期网络 owner、会话代际和两条关闭路径，提高了生命周期测试量。
- 代价换来更清晰的边界：以后新增协议必须明确是设备级还是会话级，并自动受到同一交接规则约束。
- 不增加通用插件或多 backend 抽象，避免提前承担 Issue #34 的全部迁移成本。

## 替代方案

- **只调整 Space admission 的连接关闭方向：拒绝。** 20 次实验没有可测收益，不能消除其他连接触发的 endpoint 排空，也不能解决正常 Space 切换的结构问题。
- **缩短 `Endpoint::close()` 或增加更短外层 timeout：拒绝。** 已有 121 秒停滞反例，且会破坏 mDNS、socket 和同身份后继节点的安全释放。
- **先启动新 endpoint，再关闭旧 endpoint：拒绝。** 同身份双活，无法证明路由、身份和监听 socket 的唯一性。
- **为每个 handler 设置独立可替换指针：拒绝。** 发布过程中会产生新旧能力混合，风险不可接受。
- **在 Application/Core 建立通用网络会话接口：拒绝。** 本问题是 Engine 生命周期与 Infra adapter 装配问题，不应扩大业务接口。
- **等待 Issue #34 后一起重写：拒绝。** #68 的收益和安全边界可以独立交付；本规格只留下兼容未来连接边界的网络 owner，不预做多后端。

# 11. Open Questions

- **实体设备的 3 秒目标以哪个组合做正式发布门禁？** 本规格要求所有可用平台如实执行，但 iPhone + Desktop、Android + Desktop、HarmonyOS + Desktop 中哪一组是发布阻断项，需要产品发布负责人在 Slice 6 前确认。
- **显式网络恢复的关闭失败是否允许在同一进程内自动重试？** 当前安全底线是关闭未确认时不能绑定新 endpoint。若现有 Iroh shutdown 不能返回可区分的“已释放但清理报告失败”，实现阶段需要先用故障注入确认；无法证明时默认保持操作门关闭，等待下一次生命周期动作或进程重启。
- **长期 Blob store 在移动端 `suspend` 后的依赖退出时间是否满足平台要求？** 本规格要求随网络运行期完整关闭，但锁定依赖存在延后退出历史。自动化通过后仍需在实体设备观察线程、文件锁和耗电；不满足时作为 Iroh store 生命周期问题单独处理，不能让普通 Space transition 重新关闭整个 endpoint。
