# 1. Overview

状态：已实施并验收；本文件保留批准时的规格，当前架构以[长期设计](../../design-docs/clipboard-connectivity.md)为准。基线：`main` 的 `c2a09cb7`（PR #176 已合并）。关联 Issue #34 第二切片；连接池收益评估仍归 #170。实施步骤和实际结果只写入[执行计划](2026-10-11-clipboard-connectivity-plan.md)，本文件是本切片规格的唯一正文。

实际场景：A 与 B 已配对，A 向 B 发送普通剪贴板密文，B 通过现有 Application 接收流程持久接受后返回回执。业务调用方只提交“发送给 B”的意图，不选择发现方式、直连或 Relay。

第一切片已经使 iroh 的生产地址查找消费 DiscoveryManager/PeerDirectory。当前 Application/Core 没有直接使用 iroh 类型的生产代码；本切片要隔离的是普通剪贴板协议 adapter 中的具体连接操作，不能把现状描述为上层业务尚未拥有发送 port。

实施前发送 adapter 同时承担地址解析、按设备合并拨号、原生连接登记、流交换和回执解释。其单飞领头方被取消时，跟随方重新竞争拨号。接收 handler 每条连接只调用一次 `accept_bi`，随后等待连接关闭。发送侧可能共享单飞成功得到的连接，但接收侧尚无同连接多流接收契约，现有并发接收测试使用的是多条连接。这是待用失败回归验证的具体缺口，不能只增加 trait 就宣称完成。

本切片建立 Uni 自有连接/多流边界，将普通剪贴板完整入站与出站迁移到生产路径，补齐多流、取消、背压和兼容语义，并删除被替代的直接拨号/流操作。正常发现、iroh 路径选择、成员授权和业务持久恢复保持原有唯一负责人。

# 2. Goals

- 普通剪贴板协议实现不直接依赖 `iroh::Endpoint`、`Connection`、`SendStream`、`RecvStream` 或 noq；其发送/接收均使用同一套 Uni 连接契约。
- 新新版本协商多流协议，同一连接至少两条流可独立接收、结算和回执；并发和等待数量有界。
- 取消一个调用方不取消其他调用方仍需要的共同拨号，不关闭其他操作使用的连接；全部等待者离开后停止无人需要的拨号。
- 消息超时、取消、格式错误、大小超限只结束对应流；认证失败、成员撤销、Space 换代关闭相应连接并使迟到拨号失效。
- 新旧和旧新版本双向内容成功；旧协议每条连接只执行一次内容交换，不假定旧接收端支持多流。
- 等待空位、拨号、打开流和交换受同次操作的截止时间及现有上层取消约束；无无限排队、静默丢弃或连接层自行重发。
- 回执仍表示现有 Application 的持久接受或去重结果；系统剪贴板写入属于原异步流程，不新增“对端可粘贴”的承诺。
- 普通剪贴板新路径使用第一切片发现入口、既有身份映射、成员准入与世代保护，完成真实进程/网络验收和旧代码删除检查。

# 3. Non-Goals

- 不实现长期健康连接池、跨 ALPN 复用、presence 连接用于剪贴板发送、性能收益结论或 #170 的完整交付。
- 不迁移 blob/file、active clipboard、group update、配对、presence 等协议的业务实现；它们仍可使用既有原生连接能力。
- 不新增稳定 `uc-engine` API、移动绑定 API、产品配置开关、附近设备列表、Nearby backend 或业务重试入口。
- 不重写第一切片的 provider/directory，不把 mDNS 等同于 LAN transport，不重新实现 iroh 的 NAT/路径选择或 Relay。
- 不变更消息帧、头版本、密文格式、ACK 字节、业务持久化格式或成员真相源；新增 ALPN 是传输能力版本，不是资料迁移版本。
- 不改变 Application 接收的串行应用顺序，不因多流而并行改写系统剪贴板；不承诺取消能撤回已经持久接受的内容。

# 4. Current Architecture Context

| Component | Path | Responsibility | Relationship |
| --- | --- | --- | --- |
| 业务发送入口 | `crates/uc-core/src/ports/clipboard/sync_dispatch.rs` | `ClipboardDispatchPort::dispatch`，单目标密文交换和 DispatchReport | Application 调用；稳定业务意图继续保留 |
| 发送/恢复负责人 | `crates/uc-application/src/clipboard/sync/dispatch_entry/` | 多目标投递、结果、持久投递记录与既有恢复交接 | 不获得裸连接，不拼装连接步骤 |
| 现有发送 adapter | `crates/uc-infra-p2p/src/network/iroh/clipboard_dispatch_adapter.rs` | 地址、单飞拨号、登记、编解码、回执与 selected path | 本切片拆出连接责任并迁移协议交换 |
| 现有接收 handler | `crates/uc-infra-p2p/src/network/iroh/clipboard_receiver_adapter.rs` | 身份识别、准入、一次 accept_bi、解帧、交接与应用回执 | 本切片迁移同连接多流接收 |
| 接收业务负责人 | `crates/uc-application/src/clipboard/inbound/` | 权威接收 gate、解密、去重、持久接受与异步尾部 | `ClipboardDelivery` 和 receipt 继续是唯一业务交接 |
| 原生拨号 | `crates/uc-infra-p2p/src/network/iroh/connect.rs` | 三次错峰尝试、每次 3 秒、最坏约 4.5 秒、来源和连接诊断 | IrohBackend 复用；其他协议仍在使用，不整体删除 |
| 发现入口 | `crates/uc-application/src/discovery/`、`crates/uc-infra-p2p/src/network/iroh/discovery/` | 候选来源/期限和原生 lookup 桥 | 后端继续通过现有 endpoint lookup 获得候选 |
| 身份和登记 | `crates/uc-infra-p2p/src/network/iroh/inbound_peer.rs`、`admitted_connections.rs` | 传输身份映射、准入及撤销 epoch、关闭成员连接 | 复用事实和登记，禁止建立第二份授权表 |
| 会话换代 | `crates/uc-infra-p2p/src/network/iroh/session_generation.rs` | 设备级 Router 分发、HANDLING_GENERATION、入站工作和排空 | 多流子任务必须明确延续同一世代 |
| 协议与装配 | `crates/uc-sync-protocol/src/clipboard.rs`、`crates/uc-infra-p2p/src/network/iroh/protocol_router.rs`、`node.rs` | 帧与 ACK、ALPN 路由/优先级、节点生命周期 | 保留旧 ALPN，新增多流 ALPN；同时纳入 hooks 与 session 注册 |
| Engine 组装 | `crates/uc-engine/src/assembly/sync_engine.rs` | 注入完整能力，装配业务 port 与网络对象 | 不读取连接内部阶段或编排业务步骤 |

当前超时：出站 `ACK_TIMEOUT = 75s`，入站应用结算 `APPLICATION_SETTLEMENT_TIMEOUT = 60s`；Application fan-out 有 `FAN_OUT_DEADLINE = 5s`。这些是不同层的预算，不能相互替代。当前接收广播容量为 64；多流不能创建无界任务来绕过这个上限。

# 5. Proposed Design

## Components

### 所有权与模块布局

完整连接获取、共享拨号、租约、等待和关闭结果由 Application 的 `ConnectivityManager` 统一负责。业务发送/应用/持久重试仍由现有 clipboard owner 负责。技术身份映射、连接登记和世代边界复用既有实现，通过 manager 的专属能力 port 接入，不能要求协议调用方先读 epoch、拨号、再登记。

建议布局（命名可按编译结果调整，责任不可改变）：

- `crates/uc-application/src/connectivity/{mod,manager,connection,stream,backend,admission,error,tests}.rs`：Uni 契约、经理内部状态和完整获取/释放流程。`mod.rs` 只作入口与必要导出，内部定义通过 `deps` 提供组装，不暴露到 facade。
- `crates/uc-infra-p2p/src/network/iroh/connectivity/{mod,backend,connection,stream,admission}.rs`：IrohBackend、IrohPeerConnection、流转换和现有授权/登记的能力实现。传输身份解码、地址过滤、原生 ALPN、原生错误只在此及已有底层 helper 使用。
- `crates/uc-infra-p2p/src/network/clipboard/{mod,dispatch,receiver,exchange,error,tests}.rs`：传输无关的普通剪贴板交换，消费 Uni 连接与流，使用现有 codec 和 receipt。不承担业务解密、持久应用或重试。
- 原生 `ProtocolHandler` 只转换已完成密码握手的连接并交给统一入站入口；成员身份和准入在完成交接前验证。它不能另建独立接受循环/授权缓存。

协议交换仍是 Infra 提供的完整 ClipboardDispatchPort/ClipboardReceiverPort 能力。Engine 只组装 manager、backend、专属准入能力和协议 adapter；可调整内部 builder 装配签名，但不新增公开 Engine 步骤接口。不得只移动文件而把旧原生拨号藏在同名 wrapper 中。

### 后端与发现

IrohBackend 根据逻辑 DeviceId 使用现有 PeerAddressResolver 获得已绑定传输身份/储存提示，经现有错峰拨号函数和 endpoint 的目录 lookup 拨号；地址不能由 protocol adapter 解码。不新增第二次发现调度，也不把过期目录候选固化为新缓存。

认证身份与预期传输身份必须一致，DiscoveryPeerId 不等于 DeviceId。陌生发现不能进入普通剪贴板准入路径。后端只负责传输和安全握手事实，成员许可仍由既有权威负责人判定。

### 会话准入能力

`SessionConnectionAdmissionPort` 是 manager 专属 capability，复用 InboundPeerGate、AdmittedConnections 和 session generation：

- `begin(target, protocol)`：返回不可读取的 AdmissionAttempt，捕获本次出站撤销/世代基线；协议调用方不可接触 epoch。
- `adopt(attempt, connection)`：原子核对迟到结果，登记并返回准入租约；成员在拨号中移除再添加也不能使旧 attempt 重新合法。
- `adopt_inbound(connection)`：使用真实远端身份识别，先登记再授权，按现有并发撤销规则返回准入租约；失败清理已登记资源。
- 租约内部 `check_current` 供 manager 在等待后、流获取/入站交接前重检；读取失败 fail-closed。移除/会话排空由现有 owner 触发注销/关闭。

以上是 manager 内部 port 操作，不是 facade 的步骤 API。登记表继续只有一份；可使其关闭句柄保存 Uni connection，其他协议只通过原生 wrapper 适配，不迁移其业务。禁止 manager 自建与原表并存的成员连接登记。

## Data Model

| 类型 | 信息与生命周期 |
| --- | --- |
| ProtocolKind | 本切片仅生产使用 Clipboard；能力族由 Uni 标识，原生 ALPN 映射留在后端/协议常量 |
| ConnectionMode | `SingleExchange` 或 `Multiplexed`，仅由本条连接实际协商结果决定 |
| ConnectionIdentity | 后端不透明标识及认证 transport identity；Debug 隐藏材料，不推导业务资格 |
| ConnectionLease | 已准入的 peer、协商模式、连接及内部登记/使用权；克隆/取消不能使其他使用者失效 |
| StreamLease | 一对 Uni 收发半流及并发 permit；成功、错误或 Drop 都释放本次资源，不拥有整体连接关闭权 |
| Flight | 同一 session、DeviceId、ProtocolKind 的共同拨号，保留有限等待者、结果与受 owner 管理的任务 |
| AdmissionAttempt | 不透明撤销/世代基线；只在本次获取流程有效，不持久化 |
| Deadline | `tokio::time::Instant` 绝对截止时间；等待、开流和交换共用，不能每个阶段重置 |

所有结构仅存内存。不要新增数据库、日志身份字段或持久能力缓存。能力不从软件版本字符串、presence Online 或历史成功推断。

连接共享只限同一会话、同一 peer、同一协议且明确协商为多流的并发操作；本切片不保留空闲健康连接。最后一个等待者/活动流/交换租约离开后停止拨号或关闭连接并排空任务。正在用的连接可让同范围新并发请求加入；释放与新请求竞争必须在 owner 内同步，不能得到半关闭连接。

### 有界性与截止时间

建议实施起始参数：每条多流连接最多 4 个正在处理/等待应用回执的入站流、每目标最多 4 个活动出站流、每会话最多 16 个活动入站交换、每目标最多 32 个等待者、每会话最多 128 个等待者/最多 64 个 flight。它们是工程初始值，不是用户承诺或稳定 API；全部集中在 policy 中，以小上限 fixture 验证机制，实际资源结果写计划。

等待队列只持有元数据/permit 等待，不额外复制密文；等待槽位也必须有界。等待槽已满返回固定 Busy/资源不足分类；已有等待者到截止时间返回超时，不驱逐别人的内容。公平获取 permit，取消移除等待者，不长期占位。

出站在一次 dispatch 开始时建立总 deadline（初始沿用 75s 数值），地址/准入/等待/开流/写帧/回执都使用剩余预算；错峰拨号仍有自己的更短上限，并受调用 deadline 限制。上层现有 5s fan-out 取消可更早终止本调用，不能为了多流放宽。共享 flight 不采用最先取消的请求作为唯一任务 owner；独立等待者截止，各自脱离，全部离开即取消并 join。

入站交换设置有限的读帧 deadline（起始 60s），应用结算仍最多 60s；慢读或应用挂起必须释放其 slot。它们不承诺发送方一定等待到此时。已有广播容量 64 保留；持有交换 permit 到 receipt/失败结算，不创建绕过上限的后台 payload 队列。

## API / Interface

以下是接口语义，不是要求直接抄写的 Rust 声明；实现不得新增未被消费的通用操作。

| 接口 | 输入 → 输出 | 约束 |
| --- | --- | --- |
| 既有 ClipboardDispatchPort::dispatch | DeviceId、ClipboardHeader、SyncPayload → DispatchReport | 业务调用唯一动作与当前结果保持，不返回连接 |
| ConnectivityManager::connect | target、ProtocolKind、deadline → ConnectionLease | 内部完成共享拨号、模式判定、准入与迟到结果检查 |
| ConnectivityManager::accept | backend 提供的 PeerConnection → 已准入 ConnectionLease 或固定拒绝结果 | 入站走同一边界，使用当前会话；未知或不可授权 peer 不能交业务 |
| ConnectivityBackend::connect | target、ProtocolKind → PeerConnection | manager 管理生命周期；IrohBackend 内部解析地址并协商 |
| PeerConnection::open_bi / accept_bi | 连接 → Uni BiStream 或 typed error | 保留多流，不退化为“一条 AsyncRead+AsyncWrite 代表整个连接” |
| PeerConnection::mode / identity / selected_path | 本条连接 → Uni 枚举/不透明身份/ConnectionChannel | 不暴露 Endpoint、ALPN bytes 之外的原生对象或 peer 汇总 Online |
| PeerConnection::close / closed | 固定关闭原因 → 关闭动作/关闭结果 | 仅 manager/准入 owner 持有整体关闭权；幂等，不记正常清理为业务成功 |
| Uni send/recv 半流 | AsyncWrite/AsyncRead；finish、abort/stop | FIN 与 RESET/STOP 分开；未完成流 Drop 必须结束该流，已完成半流不被误重置 |
| ConnectivityManager::shutdown | 无业务输入 → 完整关闭结果 | 拒绝新请求，取消/等待 flight 和活动流；不能让迟到结果重新登记 |

协议调用拿到 ConnectionLease 后只通过租约申请受限流，不直接调用 backend 或 admission port。租约是能力持有者，不是让业务调用方手动拼接连接步骤。

连接错误须区分 AddressUnavailable、AddressResolution、Establish、Admission、SessionClosed、Busy、TimedOut、Stream、Shutdown 等固定分类；下层失败携带完整 source。共享失败用 `Arc<Error>` 传播，不字符串化。纯无地址/已关闭状态不伪造 source。新增错误实现 ErrorClass，分类穷举。

既有 DispatchReport 映射保持：未建立可用连接沿现有 Offline 语义；流、等待预算或交换失败沿现有带 source 的 Io/Internal；消息不兼容/拒绝保持现有分类。新增拒绝原因只用固定文本/typed reason，不输出设备、地址、内容或原生错误正文。不得顺手扩充公开 Engine 错误契约；底层错误证据保留在内部错误链和固定诊断分类。

### ALPN 与字节兼容

| 角色 | 协议标识 | 单条连接语义 |
| --- | --- | --- |
| 原有应用确认协议 | `uniclipboard/clipboard-applied/1` | 一份内容交换；保留生产支持 |
| 新多流应用确认协议 | `uniclipboard/clipboard-applied/2` | 多条独立流；每条流仍一份原消息帧和 ACK |
| 更早历史协议 | `uniclipboard/clipboard/0` | 当前生产已不注册；不借此切片恢复支持 |

常量和能力映射在 `uc-sync-protocol` 的 clipboard 主题集中定义，版本测试固定；codec 不依赖 iroh。新发送侧握手同时提供新/原应用确认 ALPN，服务端必须明确优先新标识；使用实际协商结果，不按请求顺序猜测模式。保留 presence 的原有优先级与回退测试，不把其它协议加入这次 clipboard offer。

若共同 flight 协商为 SingleExchange，结果只能被一个调用方独占，其他调用方各自获取新的独占连接，不能把同一连接广播给所有成功等待者。失败结果仍可合并传播。该旧版分配应在一次等待者批次内有界进行，不能每次释放旧 flight 再把剩余所有人放回队列形成无限竞争；每次独占拨号也计入会话的并发/任务上限。只有握手协商选择旧标识才能使用旧模式，不因拒绝、坏帧或超时换协议重发。

新入站为新 ALPN 开启多流循环，原 ALPN 保留一次交换模式；新新/新旧/旧新都要证明选中的真实 ALPN 和内容结果。新 ALPN 同时进入节点 managed ALPN、SessionProtocolEndpointHooks 和 generation handler 路由；不能只改 Router 注册而使旧世代绕过围栏。

## Workflow

### 出站完整流程

1. 现有业务 owner 完成范围判定和密文准备，调用 ClipboardDispatchPort；adapter 做原有本地帧大小校验、建立 deadline。
2. 协议 adapter 向 manager 提交 target、Clipboard、deadline；manager 获取不可读取的准入 attempt，进入有界 flight/连接活动状态。
3. backend 经既有地址解析、第一切片 lookup 和错峰拨号完成握手，返回真实身份与协商模式；manager adopt，成员撤销/世代失效时立即清理迟到连接。
4. 多流模式分发共享租约；旧模式分发独占租约。等待并发 slot 后再次核对当前准入并开流，绝不让旧等待者使用撤销后新登记的连接。
5. 协议写原帧并 finish 发送半流，读取同一流的原应用回执。超时/取消/协议错误结束本流；不自动重发内容。
6. 用本条内容连接的 selected path 形成 DispatchReport，释放 StreamLease/ConnectionLease；最后使用者离开时 owner 关闭连接。路径存在不代表内容已被接受。

### 入站完整流程

1. 设备级 Router 与既有 session dispatcher 验证当前 generation；薄 iroh handler 将握手后的原生连接转换为 Uni handle，进入 manager.accept。
2. 使用真实 transport identity 识别 DeviceId，先登记再授权；失败返回固定拒绝结果，不进入 ClipboardDelivery。对原 /1 保留既有有界 Rejected ACK 出口，再由 owner 关闭连接；拒绝回复只读取回复所需的一条流，不读入/应用 payload，不等待对端无限关闭。新模式采用同一安全拒绝分类，不能把拒绝连接交给正常多流循环。
3. SingleExchange 只接收一次；Multiplexed 持续接受独立流，获取连接/会话 permit 后才读取并分配 payload。满载时背压到流/传输等待，不创建无界已接受流队列。
4. 每次流交接前重检当前准入，解码原帧，构造现有 ClipboardDelivery/receipt。Application 接收运行期仍串行解密、去重和持久接受。
5. 每条流独立等待 receipt，回原 ACK，finish 本流；坏帧/超限/慢读只拒绝或终止此流，其它流继续。应用返回失败不是整个连接的认证失败。
6. 同一连接的子任务由一个显式 owner/JoinSet 持有。会话退休或成员撤销关闭连接、取消子任务、等待排空并释放 permits；不留无人持有的 tokio::spawn。

### 世代上下文与观测

`HANDLING_GENERATION` 是 task-local，tokio::spawn 不会自动继承。每个入站流任务必须通过 Infra 内部显式 scope 延续 dispatcher 所持 generation，保持 `begin_session_local_work` 等既有围栏有效；不要在 Application/Core 暴露原生 generation 类型或为 Engine 增加查询。

每条流独立读取自己的 WireTraceContext，在认证后建立原接收关联；连接任务没有某一份内容的业务父关系。共享 dial 不能错误归属首个消息并把其他消息接到同一业务 trace；各等待者的业务上下文独立，拨号诊断按既有匿名连接/attempt 合同记录。ClipboardDelivery.capture 必须在该流 scope 内发生。

跨层计时仅使用 Engine 组装层既有领域 decorator，底层运行诊断复用既有 NetworkRecorder。正常 finish/连接释放不新增业务 root。任务 join 异常按观测标准由等待 owner 记录；若需新固定 task.kind，实施时同步合同、文档和 collector 测试，不能临时打印自由字段。日志/公开错误不含内容、设备名、身份材料、地址或路径。

# 6. Implementation Plan

执行顺序、依赖、风险、阶段退出条件和状态集中在[执行计划](2026-10-11-clipboard-connectivity-plan.md)。顺序必须是：冻结基线与失败回归 → 最小 Uni 契约/原生 adapter → 新旧 ALPN 的单消息双向生产链 → 多流/单飞/背压 → 取消/撤销/换代 → 删除旧入口 → 完整网络与交付门禁。不得先写全部产品代码再补用例。

# 7. Edge Cases

| Scenario | Expected behavior | Implementation |
| --- | --- | --- |
| 空密文或非法 header | 沿用 codec/业务校验，不特殊伪报成功 | 发前校验、原 codec 测试 |
| 没有储存传输身份、坏 addr_blob、仓储失败 | 无地址与损坏/读取失败明确分层，后两者保留 source；不把陌生观察当绑定 | backend 复用地址 resolver |
| 同目标多请求、领头请求取消 | 剩余等待者继续一次共同拨号 | manager 持有任务和 waiter guard |
| 所有等待者取消、deadline 各异 | 最后退出取消拨号；单个 deadline 不伤其他人 | 按 waiter 生命周期计数和受控 join |
| 旧协议成功拨号、多个等待者 | 每条连接单请求独占；全部内容可结算 | 旧模式分配器，无多流假设 |
| 未读帧/坏帧/超限/短读/RESET | 当前流有界结束，其它流可成功；不能分配超限 payload | per-stream deadline、codec 上限、流 guard |
| 入站/出站满载、等待槽满 | 背压或固定 Busy/超时，无丢掉其它内容的策略 | 有界 permit/等待表；取消释放 |
| 应用已接受，ACK 丢失或发送取消 | 本机不能声称成功，也不能声称对端未保存；业务 owner 后续按原幂等规则恢复 | 不在连接层重发，不改变 receipt |
| 一个流完成，另一个在等 receipt | 连接仍有效，前者 FIN 不关闭后者 | 租约计数和单流结束语义 |
| 撤销后重新添加、旧拨号迟到 | 拒绝旧 attempt，不能复活 | 现有撤销 epoch 与世代基线 |
| Space 切换时排队/拨号/本地持久工作 | 旧世代不能发送或写入新上下文；endpoint 保持 | 显式任务世代 scope、原排空负责人 |
| 某流成员范围读取失败 | fail-closed；身份/准入不可用时中止相关连接 | 原权威 port/source chain |
| 对端关闭、端点关闭、子任务异常 | 活动流各自失败，关闭有界排空，无自动内容重发 | manager/节点持有 JoinHandle/JoinSet |
| 迟到发现候选或网络变化 | 不绕过当前身份/准入，不缓存永久能力 | 沿用第一切片和现有恢复 owner |

# 8. Testing Strategy

状态：以下保留批准时的测试要求；实际执行证据、失败复核和跳过项见执行计划。真实网络、真实储存、模拟时间和测试 backend 的结果分开记录。

## Unit Test

| ID | 输入与操作 | 预期结果 |
| --- | --- | --- |
| U1 | 可控 backend，同目标三个等待者，取消首先进入者 | 一个有效 flight 继续；剩余等待者成功，取消者不占位 |
| U2 | 不同 deadline 的等待者及全部取消 | 各自到期；最后离开才取消并等待 backend，flight 清空 |
| U3 | 上限 1 的 permit/小等待队列，持续阻塞一个交换 | 等待计入绝对 deadline；满槽 Busy；取消/失败后下一请求获得 slot |
| U4 | 单流成功、错误、Drop 与整体关闭 | FIN/abort 与 connection close 区分；兄弟流不受损 |
| U5 | begin 后撤销/移除再添加，之后 adopt | 迟到连接拒绝，没有可发送租约 |
| U6 | backend 建立/开流/关闭失败 | 稳定分类正确，source chain 可追到原失败；共享失败保留同一来源 |
| U7 | 测试第二 backend 产出 Uni connection/多流 | 同一真实 clipboard exchange 代码可执行；只证明依赖边界，不声称 LAN 通过 |

## Integration Test

| ID | 输入与操作 | 预期结果 |
| --- | --- | --- |
| I1 | 真实 endpoints 与生产 Router，双方 offer 新旧 ALPN | 真实协商为 /2；只注册 /1 的旧端选 /1；不接受 /0 |
| I2 | 原 codec 固定向量分别经 /1 和 /2 单流发送 | header、frame、ACK 字节完全相同，应用接受/去重回执不提前 |
| I3 | 真实生产接收方，同连接两条流，第一条 receipt 延迟 | 第二份被接收；各自 ACK 对应自己的 receipt，不要求业务并行应用 |
| I4 | 同连接坏帧/超限/慢读/被取消一条，另有合法流 | 只失败本流；合法流接受；active/waiter/task 均回收 |
| I5 | 多流已准入后移除成员，及拨号中/排队中撤销 | 连接与所有相关流结束；迟到发送为零；保留既有成员移除回归 |
| I6 | 真实 generation dispatcher，子任务等待本地工作时切 Space | 子任务有原世代 context；排空等待正确，不污染新会话、不重绑 endpoint |
| I7 | 并发发送给仅 /1 生产旧接收端 | 每连接一次交换，所有请求完成，无第二条流挂起 |
| I8 | 跨层异常/队列/关闭，真实 SQLite/MLS 接收链 | 幂等、持久接受与失败恢复保持；取消不撤销已有提交 |
| I9 | 并发交换各有独立 trace，共享一条连接 | 各消息的远端上下文/receipt 不串线；无新业务清理 root，无敏感字段 |

## Regression Test

- 先在 `c2a09cb7` 的生产对象图建立旧行为基线；I3 先使用原 /1 同连接多流证明当前缺口，新增 /2 后证明新契约。基线失败原因必须准确记录，缺少新符号/ALPN 不是行为失败证据。
- 用受控拨号 probe 验证 U1 的取消问题；用现有 codec、授权、成员撤销、地址过期、关闭和 session tests 建立无回归基线。确定性等待使用事件，不以长 sleep 当同步。
- 冻结修改前 `c2a09cb7` 的 host binary/source SHA，先运行既有三节点配对、E02 双向文本/文件与 E10 Relay 基线；不修改网络 harness 的原断言来得到绿色。
- 最终使用现有 `tests/hosts/connectivity`、relay 和 `connection-recovery-network.mjs`，运行新新、新旧、旧新双向 E02；新增并发/取消/满载场景必须扩展原 harness，使用真实生产 facade 及内容长度/哈希读回，不另造合成地址替代 E2E。
- 模式均覆盖 direct（无公网依赖）和 relay-only（实际屏蔽 IPv4/IPv6 直连旁路）。复跑 E04 机会恢复和 E10 Relay 中断，证明无误伤现有恢复负责人。
- 在真实生产 facade/SQLite 场景补充在途 Space 切换与成员移除；网络 harness 缺少该动作时，扩展测试宿主既有命令，禁止添加稳定 Engine 测试开关。
- 工件保存源码/二进制 SHA、匿名场景/时间线、实际 ALPN/selected path、安全内容断言、故障规则、清理和重放命令，放共享 `target`；不提交 keys/token/业务内容。
- 物理设备、移动切网/睡眠与已发布版本矩阵未执行时记录“跳过”。基线宿主混合互通不是已发布版本验收。

# 9. Acceptance Criteria

- [x] 普通剪贴板协议模块生产代码中无 iroh/noq 类型、原生地址解码或直接原生拨号；新模块为实际生产装配入口。
- [x] /2 真实协商优先于 /1；新新/新旧/旧新双向成功；未恢复 /0。
- [x] 同连接多流各自等待 receipt 并回复；串流/回执串线为零；消息级失败和单请求取消不关闭兄弟流。
- [x] 取消最先等待者不使共同拨号重启；最后等待者退出会取消并等待任务。
- [x] 旧模式连接不广播给多个发送者；并发内容全部结算，有界拨号分配。
- [x] 活动流、等待者、flight 与任务有集中上限；满载公平等待或 Busy，deadline 不重置，释放无泄漏。
- [x] 成员移除、移除再添加、Space 切换、迟到拨号与子任务 generation 回归通过；唯一 endpoint 不重建。
- [x] source chain 与既有业务结果保持；连接层无内容重发/消息丢弃/系统剪贴板成功承诺。
- [x] 旧路径和对应过期测试已删除；其他仍使用的 connect/admitted/session/发现 helper 未误删。
- [x] 真实进程/网络场景、SQLite/MLS 接收链、观测关联与安全断言均有可重放证据；未执行平台项明确跳过。
- [x] metadata、workspace all-targets check、fmt、Rust 风格、仓库架构与 diff 检查全部通过；架构正文回写后计划移入 completed。

# 10. Risks and Trade-offs

- 新 ALPN 使多流能力可明确协商，代价是长期兼容原 /1。旧协议删除必须另有发布/采用证据，不能随本地重构删除。
- 旧协议成功 flight 不能共享，因而并发旧 peer 的握手数可能高于当前有缺陷的共享路径；这是正确交换所需，不作为连接池性能收益。
- 单飞从请求 future 转为 manager 所有会新增技术任务，必须有明确取消/关闭/异常 join owner，禁止循环 Arc 和无人需要的后台拨号。
- 多流可能改变帧到达广播的先后顺序。本切片不并行 Application apply、不新增按 captured_at 重排；沿用现有去重/活跃剪贴板规则，不能以 transport FIFO 代替业务顺序保证。
- 并发读取最多 2 MiB 原 payload 时会增加峰值内存。per-session permit 必须覆盖读取至 receipt 结算，实测 active 计数与内存，不凭上限常量声称物理移动设备通过。
- task-local 世代和 trace 不自动随 spawn 传播，遗漏会破坏排空或串接业务记录；真实 dispatcher/context 和导出测试是交付条件。
- “共享多流”不等于“长期缓存连接”。最后用户离开即关闭可以限制新生命周期面，后续连接池必须按 #170 测量收益。
- 仅包装原生 connect 的转调层不足以承担这些责任；manager 必须真实拥有共享/模式分配/背压/取消/准入结果，否则应合并空壳。

# 11. Open Questions

用户范围和行为决策已收敛，无待用户回答的阻塞问题。以下保留实施前证据要求；实际 ALPN、有限资源、宿主覆盖与平台跳过结果见执行计划，不以本列表声称物理平台通过：

- 真实原生握手在新旧端上的选择顺序及错误分类：先用当前 iroh 和真实 Router probe，若结果不符合设计，修正映射并记录，不以业务超时触发降级。
- 4/16 等工程起始上限在真实同步中的内存和延迟：先做可控上限测试及网络记录；调整只限有限资源参数，不改变已确认的取消/兼容/结果语义。
- 现有 harness 对并发、撤销和切 Space 命令的覆盖缺口：Step 1 盘点并扩展原宿主；缺少场景不能以普通 E02 替代。
- 真实物理平台和发布旧版本可用性：在执行计划记录实际可执行范围；当前不宣称具备这些资源或已通过。
