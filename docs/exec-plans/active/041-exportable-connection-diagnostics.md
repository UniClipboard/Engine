# 规格 041：可导出的连接故障调查记录

## 状态与执行约定

- **状态**：实施中（Engine 采集、错误来源、覆盖报告与平台接口已提交；桌面接入及内容级验收已完成，联合候选故障链、手机接入和两端真机仍待补）；本文件的设计目标不代表能力已经全部实现或通过真机验收。
- **日期**：2026-09-11。
- **源码基线**：Engine `288161390ebf54259b7044f4be3e3dd888fee4f6`。实施前重新核对目标分支、手机实际安装包来源及相邻未提交修改。
- **需求依据**：手机团队《手机连接故障：补齐可导出的诊断日志》交接。交接核查的 Engine 为 `ae0e2650a01d80bff433914bf8abd350248de301`，不能将其旧代码描述直接当作本分支现状。
- **完整负责人**：Engine 进程观测运行时拥有采集策略、身份脱敏、编码、覆盖报告、文件队列与刷新；既有网络和业务流程负责人提供事实。手机与桌面团队分别拥有宿主事实、采集入口、ZIP 组装及真实设备验收；桌面 daemon 是 Engine 采集的唯一宿主负责人。
- **调用方唯一动作**：普通业务继续调用既有完整能力；用户需要详细复现时，宿主提交一次限时采集请求；导出时调用一次诊断准备接口取得刷新结果和覆盖报告，不手工遍历 Engine 内部步骤。
- **成功结果**：导出包可以解释连接尝试、所用信息、失败位置及后续恢复；不足的覆盖有明确说明。
- **失败结果**：诊断不可用、写入失败或刷新超时返回固定状态；不改变连接、业务回执、重试和持久恢复结果。
- **重试与重启责任**：原业务 owner 继续负责业务重试；诊断运行时负责自身限时采集、容量控制和有界收尾。进程重启不自动恢复详细模式，不恢复匿名身份映射。
- **相关约束**：[业务记录组织标准](../../design-docs/observability.md#业务记录组织标准)、[工程原则](../../design-docs/engineering-principles.md)、[错误处理](../../design-docs/error-handling.md)、[安全架构](../../SECURITY.md)。
- **与 039 的关系**：本计划只迁移连接故障链路涉及的历史调用点；[039](039-local-debug-inventory-cleanup.md) 继续负责全仓历史记录清单，不复制其工作队列。

# 1. Overview

手机交接中的导出包显示日志文件已发现、已包含、刷新完成且丢弃计数为零，但内容主要是操作完成摘要。桌面同时段观察到候选地址和中转位置变化，之后连接恢复。缺失的是“使用了哪份连接信息、在哪一步失败、收到什么新信息后恢复”的证据。

交接时间线采用 UTC：2026-09-10 15:44:52–15:46:29。成员更新的 storage 错误出现在连接建立之后，必须独立调查，不能解释之前的连接超时。地址过期、发布延迟、消费延迟、中转不可达及手机生命周期影响均未被最终证明。本文不复制原日志的设备身份、地址和其他敏感内容。

本分支已增加 `uc.connectivity` 类型化记录、认证本地细节、会话及在线检查记录，并通过 `LocalLogProcessor` 保存关联信息。当前缺口是采集事件和错误类型不足，不是简单漏打包文件，也不是开启普通 debug 日志即可解决。

# 2. Goals

- G1：默认模式记录每次逻辑连接的起因、终态、尝试计数和已知失败阶段；详细模式记录逐次尝试及地址来源变化。
- G2：分别证明“发现信息”“接受/保存信息”“连接时使用信息”；没有观察能力时明确未知，不通过超时反推原因。
- G3：同一进程内，可以将匿名对端、连接尝试、连接关闭及恢复事件对应起来；已认证的业务关联继续使用既有观测上下文。
- G4：成员更新失败保留可追溯的源错误，本地记录固定操作阶段与原因；导出内容不包含原始错误正文。
- G5：远程关闭时本地仍能记录；限时详细模式可自动结束；队列、文件和匿名映射都有上限。
- G6：宿主取得真实覆盖报告，可区分未开启、观察到零事件、不可用、被过滤、队列丢弃及刷新未完成。
- G7：自动测试读取实际 JSONL 和 ZIP；手机团队以真机导出包证明一次同类失败和恢复，无法复现的根因仍为未知。
- G8：桌面 GUI 和适用的诊断 CLI 通过同一 daemon 控制采集；daemon 重启、不可用和系统睡眠唤醒时，采集状态与导出覆盖均真实可查。

# 3. Non-Goals

- 不修改配对协议、重试次数、连接超时、P2P/LAN 选择或成员更新回执。
- 不全量放开 `iroh`、`uc_infra` 等普通 target，不从原始错误字符串猜阶段。
- 不新增可让 Engine 编排 Application 内部步骤的 facade、port 或业务结果字段。
- 不将每个网络尝试提升为独立业务 trace；跨层整体耗时与结果只由既有 Engine port decorator 记录。
- 不在首版提供跨重启或跨设备通用身份匹配，不新增可回查设备的持久匿名映射。
- 不建设 PostHog 产品动作追踪、远程动态许可或 Collector schema v2；远程摘要合同保持不变。
- 本仓不实现手机或桌面界面、ZIP UI 或原生系统日志抓取。手机和桌面接入均是本规格必需的下游交付项，由各产品仓实施，不以 Engine 单元测试替代。

# 4. Current Architecture Context

```text
Component: 固定诊断合同
Path: crates/uc-observability-contract/src/diagnostics/connectivity/
Responsibility: 定义允许的认证、恢复、在线检查和会话记录；认证细节只附加到本地完成记录。
Relationship: Application/Infra 提供事实，运行时严格解码；不接收任意字符串字段。

Component: 本地及远程运行时
Path: crates/uc-observability-runtime/src/{runtime,filter,local_log_processor,local_file,remote_health}.rs
Responsibility: 进程安装、分层过滤、本地队列、关联编码、远程出口及健康状态。
Relationship: uc.connectivity 已独立进入本地文件；普通 target 仍拒绝。健康 fmt 层无 span 列表，不表示 SDK 完成日志没有关联。

Component: 连接、地址与恢复
Path: crates/uc-infra/src/network/iroh/{connect,peer_address_resolver,node,addr_filter,conn_path,net_recovery,presence_adapter}.rs
Responsibility: 真实地址选择、连接尝试、路径查询和运行恢复。
Relationship: connect 仍有字符串化错误和含原始地址的历史日志；resolver 只读存储，不能代表所有发现来源。

Component: 成员更新及安全存储
Path: crates/uc-infra/src/network/iroh/group_update_adapter.rs
Path: crates/uc-infra/src/space/security/access.rs
Path: crates/uc-infra/src/db/repositories/space_security_store/
Path: crates/uc-core/src/membership/revocation.rs
Responsibility: 接收更新、执行安全状态变更及持久化。
Relationship: KeyEpochError::Repository(String) 已在上游丢失错误类型；接收端最后只编码 storage，不能靠末端加日志还原。

Component: 稳定入口与平台绑定
Path: crates/uc-engine/src/contract/observability.rs
Path: bindings/uc-engine-uniffi/src/observability.rs
Path: bindings/uc-ohos-napi/src/observability.rs
Responsibility: 进程观测安装、健康查询、刷新和关闭的稳定宿主入口。
Relationship: Swift/Kotlin、HarmonyOS 使用同一 Engine 版本；新增诊断能力不得暴露业务内部阶段。
```

# 5. Proposed Design

## Components

### 5.1 采集策略与输出

在 `uc-observability-runtime` 内扩展一个进程所有的本地采集状态对象。复用既有 writer、轮转、flush/shutdown 和 SDK 关联，不新增独立日志线程池或 exporter。

- 输入：类型化事件、限时采集请求、明确的来源注册状态。
- 输出：本地记录、实际生效的采集状态、固定覆盖报告。
- 默认 `standard`；`detailed` 首版默认 10 分钟，允许 1–15 分钟，到期回到 standard。参数是设计上限，不是性能测量结论。
- 默认模式保留逻辑连接开始/结束、失败与恢复、地址/路径发生变化、成员更新失败、会话状态变化。详细模式增加每次拨号的开始/结束、各来源查询及无变化的有效结果。高频“状态未变”不默认写入。
- 同一进程只有一个有效详细采集 session。重复请求返回当前 session，不延长截止时间；停止必须携带对应 capture_id，旧请求不能停止新 session。
- 期限用单调时钟计算，每次接收记录和查询状态时检查。应用被冻结期间不承诺定时任务执行，恢复后第一条事件前必须校正模式。
- S0 必须核实该时钟是否计入设备睡眠；不能默认 Rust Instant 在所有平台都有相同睡眠语义。无法证明剩余期限的平台在宿主恢复通知时保守结束详细模式，报告 suspension_expiry_unknown，不能静默延长采集。
- 结束详细模式时，已开始的尝试仍写一次终态；进程被杀导致没有终态时，由覆盖报告标为中断/未知，不伪造完成。
- 本地模式与远程 gate 分离。所有本地新增字段在进入远程 exporter 前被明确排除，也不计作远程隐私拒绝。
- ordinary target 仍默认拒绝；详细模式只扩大已审查事件集合，不扩大自由字段、来源或错误正文的权限。

### 5.2 连接事实采集

在真实发生事实的 Infra 模块内记录，不把存储 getter、普通查询或错误转换变成业务入口。

| 位置 | 新增事实 | 不可宣称的含义 |
| --- | --- | --- |
| connect.rs | 逻辑 connect_id、attempt_index、purpose、开始与终态、超时预算、失败阶段、胜出尝试 | 未开始的错峰任务不算一次失败；输掉竞速的取消不算超时 |
| peer_address_resolver.rs 和地址存储写入边界 | record_present、record_observed_at、record_age、stored_generation、读取/保存结果 | observed_at 不是地址有效期；读取成功不是地址可用 |
| node.rs 中实际发现服务及候选过滤入口 | 来源、候选类型/数量、收到/接受/拒绝、来源内代次、变化原因 | 只有过滤回调时不能宣称观察了完整 DNS 查询或发布确认 |
| conn_path.rs 及连接生命周期 | direct/relay/mixed/unknown、实际观察到的路径变化 | 快照不能证明两次采样之间没有短暂切换 |
| net_recovery.rs | 本机中转状态、恢复触发、执行动作、退避、结果 | 本机 home relay 状态不等于对端中转位置 |
| presence_adapter.rs 及关闭观察任务 | connection_id、方向、关闭原因、已知本地关闭意图 | 不因连接失效就认定远端主动关闭 |

实施时先检查锁定版本 Iroh 的结构化错误、watcher、address lookup 接口。优先复用实际回调；只记录已观察事实。没有阶段接口则用 `connection_establish/unknown`，不得解析 Debug 文本补出 DNS、TLS 等细节。若确需升级/修改第三方库，先在 S0 明确差异和测试范围，不顺带升级整套依赖。

地址代次必须区分 source_generation、stored_generation、used_generation。连接收到哪个候选快照，就绑定哪个快照的代次；不能在结束时再次查询地址并当作开始时使用的信息。动态发现提供的附加候选有单独事件，缺少 API 时明确 coverage=partial。

### 5.3 关联与隐私

- 每个进程启动随机生成 run_id；每次详细采集随机生成 capture_id。两者均不取自设备身份、路径、时间或业务内容。
- 本地匿名对端、连接和候选集合在内存中分配不可推导原值的随机编号。映射不落盘、不导出、不发送给对端、不进入远程 attributes；禁止无盐地址哈希和固定设备身份哈希。
- 编号在同一 run 内可关联；进程重启后重新生成。导出窗口覆盖多个 run 时明确 `cross_run_peer_linking=unsupported`。若用户要求跨重启精确匹配，须另立隐私与加密持久化设计，不能在本计划中暗加稳定标识。
- 认证前只用本地连接上下文。认证后的业务 trace/span 继续由既有合法 `ObservationContext` 和协议传递；不接受未经认证的跨设备关联。
- 跨层完整动作耗时与结果仍由 Engine 装饰既有完整 port；底层尝试耗时是 Infra 局部运行事实。应用阶段及恢复判断由原流程 owner 记录，不向 Engine 暴露查询接口。
- 同进程重复读取同一候选集合不增加代次；集合比较在内存中规范化顺序，忽略列表重排。只有实际集合变化才增加代次。
- 进程注册容量首版上限：4096 个对端、4096 个在途连接、每个对端保留 8 个候选集合代次。活跃条目不可被误复用；超限时输出不关联记录并增加 correlation_capacity_exceeded，不因诊断拒绝业务连接。

### 5.4 成员更新错误

本路径的错误修复从源头开始：用携带 source 的分类错误替换 Repository(String) 承载的下层失败。目标错误类型拥有转换；纯规则拒绝继续使用规则 variant，不人为构造数据库错误。

- 内部 source 保留实际数据库、解码、密钥访问等错误；外部 Display/Debug 使用固定安全描述。
- `apply_group_epoch_update` 的原负责人确定失败阶段：decode_update、validate_update、load_state、apply_security_update、persist_state。
- 本地原因集合首版包含 unavailable、locked、not_found、conflict、constraint、corrupt、permission_denied、unknown；只有实际类型能证明的原因才能映射。
- 来源链输出为至多 4 层固定类别，例如 membership_update → persist_state → storage → unavailable；不序列化 `Error::source()` 正文或 backtrace，不输出 SQL 和路径。
- group_update_adapter 保留现有接收结果与回执，只给本地完成记录附加固定阶段和原因，复用认证诊断已有的本地附加机制；远程仍是原来的 storage/security 等摘要。
- 不全仓迁移所有字符串错误；只修改这条真实调用链。由编译暴露的 Clone/Eq 等依赖逐一处理，不以复制字符串保留旧错误旁路。

### 5.5 原生来源与导出准备

宿主通过固定枚举报告前后台、运行权请求/取得/释放、网络类别变化、安全状态准备开始/结束。运行时负责安全编码；宿主只报告自己拥有的事实，不重复记录 Engine 内部连接。

原生事件入口不接收 map、任意事件名、自由错误字符串或业务 ID。Engine 宿主来源为 main_application、share_extension、keyboard_extension、background_service、desktop_daemon；平台限制合法组合。desktop_gui 和 desktop_cli 在导出报告中作为独立宿主日志来源，不冒充 daemon 原生事件。原生动作可使用本进程产生的不透明 observation token 对应开始/结束，不通过 JS 传递 Core 身份。

需配对的原生开始事件由运行时返回 token，结束事件提交该 token；状态通知无需 token。运行时最多保留 1024 个在途原生动作，结束一次即移除；未知、过期、跨进程或重复结束的 token 返回固定拒绝状态，不建立假关联。

iOS 扩展是独立进程：安装自己的进程运行时、登记自己的来源，并在原有运行权释放前请求有界刷新。主应用不能声称刷新了其他进程的队列。复用既有跨进程运行权及共享目录安排，不为了日志改变网络运行权顺序。

### 5.6 桌面宿主适配

桌面必须接入本规格，不能仅作为人工查阅日志的对照端。桌面 PR #1653 中的基础日志装配是参考起点，实施前核对实际合并状态及固定 Engine 修订；基础接入不代表本规格的限时采集、控制接口和覆盖报告已经交付。

以下路径相对于 desktop 仓库，实施时以当前分支结构为准：

| 组件与参考位置 | 实施责任 | 边界 |
| --- | --- | --- |
| daemon、`crates/uc-bootstrap/src/observability/tracing.rs`、`apps/daemon/` | 安装运行时、登记 desktop_daemon、管理限时采集；报告实际启动/停止、系统睡眠/唤醒及其拥有的网络或安全状态通知 | 只有 daemon 控制 Engine 采集及写 Engine 文件；不重复推断底层连接事实 |
| `crates/uc-daemon-contract/`、`crates/uc-webserver/`、`crates/uc-daemon-client/` | 通过现有本机通信提供开始、停止、查询和导出准备；复用现有认证及固定请求/响应合同 | 不新增监听端口，不接收任意日志正文或文件路径，不扩大业务内部步骤查询 |
| GUI 诊断设置入口 | 提供详细采集开关，展示实际模式、剩余时间、来源状态和失败结果；重新打开页面时查询 daemon | GUI 倒计时只用于展示，不拥有实际期限；关闭窗口不停止 daemon 采集 |
| `apps/cli/` 的现有诊断命令（适用时） | 复用同一 daemon client，提供与 GUI 相同的采集控制和导出准备结果 | 不另启动 Engine 或第二份采集；未提供诊断命令时明确标记未提供，不声称 CLI 操作已验收 |
| 现有桌面诊断导出负责人 | 汇总 Engine 报告及 GUI/daemon/CLI 的已有日志，逐来源记录发现、包含、不可读、截断和刷新结果 | daemon flush 不代表其他进程已刷新；缺少确认时保留未知状态 |

桌面控制请求沿用本规格 API 的持续时间、capture_id 和截止预算。daemon 区分控制来源 desktop_gui/desktop_cli 与事件来源 desktop_daemon，不接受客户端自行填写运行身份或原生事实。两个客户端同时开启时复用同一 capture，停止沿用 capture_id 检查；响应丢失后先查询状态，不能直接显示成功或重新延长采集。

daemon 重启后返回新 run 和 standard 模式，GUI/CLI 清除旧采集展示，不自动恢复详细模式。daemon 不可用时控制请求明确返回不可用；已有日志仍可通过现有离线导出路径导出，报告未获得本次 Engine 刷新和实时状态，不为导出强行启动 daemon。

睡眠或普通暂停不调用进程日志 shutdown，最终 daemon 退出才执行有界收尾。GUI/CLI 保留自己的日志，不把整批宿主记录跨进程重新发送给 daemon 以伪造共同关联；同一 ZIP 不等于同一 trace。限时本地采集不自动开启 Sentry、PostHog 或其他远程上传许可。

## Data Model

以下为拟新增本地合同；名称可在 S0 与绑定生成约定对齐，语义不可削弱。

| 类型/字段 | 含义与生命周期 |
| --- | --- |
| local_schema_version | 新记录使用本地版本 2；旧版本 1 文件仍可被导出。与远程 schema v1 独立 |
| event、source、level、mode | 固定枚举；模式决定采集集合，不等同于宿主 console 日志级别 |
| timestamp_utc、monotonic_offset_ms | UTC 用于跨包时间窗口比较，单调偏移只在同一个 run 内可比较 |
| run_id、capture_id? | 随机本地关联；standard 可以没有 capture_id；不得包含机器标识 |
| peer_ref?、connect_id?、connection_id?、attempt_index? | 对端、逻辑连接、实际连接、物理尝试分别标识；缺少关联时不伪造 |
| observation_generation、observed_at?、age_ms? | 真实观察/接受/使用事件的代次及年龄；时钟回拨时年龄未知并标记异常 |
| trigger、phase、outcome、reason、duration_ms? | 事件专属枚举及实际计时；未开始或无法测量时省略耗时 |
| source_commit、engine_version、host_version、platform、host_role | 来源于构建及宿主的固定受限元数据；缺失修订为 unknown，不从版本号猜 SHA |
| trace_id?、span_id? | 仅来自合法观测上下文；缺少或未认证时省略 |

仍按具体动作定义互斥 variant，不恢复 Stage × Reason 任意组合。每条记录的编码上限为 4096 字节，字段/枚举未知时拒绝并计数。沿用现有日志总容量、保留天数与队列上限；不得通过 detailed 模式增大磁盘上限。无法继续保存时有丢弃证据，不阻塞网络主循环。

覆盖报告按 run、capture 和固定来源分别表达：

- capability：supported / partial / unsupported。
- collection：enabled / disabled / unavailable / not_registered。
- observed_count、accepted_count、policy_filtered_count、schema_rejected_count、queue_dropped_count、write_failed_count；饱和计数，不回绕。
- 计数口径限于类型化采集入口实际看见的记录；普通日志在前置过滤处被拒绝的总数未知，不能计为零。没有注册来源不能标记为“已启用但零事件”。
- 采集起止 UTC、当前模式、详细模式剩余时间、构建修订、flush_status、最近成功写入时间及文件窗口覆盖情况。
- 队列接受不等于写盘成功。历史 dropped_local_records 保持既有兼容含义，新报告独立说明明细，不能无说明相加。

按 run 写入起始覆盖记录、来源状态变更和导出检查点，复用同一文件队列。程序正常收尾写终态；被杀没有终态则覆盖结束未知。来源检查点记录失败时，宿主报告缺少该 run 的覆盖信息，不生成“零丢弃”占位值。

## API / Interface

下列均为拟新增的诊断专用接口，经 `uc-engine::observability` 出口及两套绑定提供。保持已有 install、health、flush、shutdown 接口；采用新增请求/返回类型，不要求现有 Swift/Kotlin 构造器新增参数。

| 建议接口 | 输入 | 返回与错误 |
| --- | --- | --- |
| start_local_diagnostic_capture | DetailedCaptureRequest：持续时间、固定触发来源 | CaptureStatus：capture_id、有效模式、期限、是否复用；InvalidDuration / NotInstalled / LocalSinkUnavailable / AlreadyShutdown |
| stop_local_diagnostic_capture | 当前 capture_id | 已停止 / 已结束 / 非当前 session；不关闭 writer 或 provider |
| query_local_diagnostic_status | 无 | 有效模式、来源能力及当前统计；只查询诊断状态 |
| record_host_diagnostic | 固定 HostDiagnosticEvent，结束事件携带开始时返回的 token | HostDiagnosticReceipt：accepted / policy_filtered / capacity_exceeded / invalid_token / not_installed / already_shutdown，开始事件成功时附 token；无业务副作用 |
| prepare_local_diagnostic_export | 截止预算，范围 1–5000 ms，默认 1000 ms | ExportDiagnosticReport：flush 状态、已知 run 的覆盖/统计；超时仍返回已有状态，不宣称覆盖全部文件 |

详细配置经已有进程 handle 更新，不重复安装 subscriber，也不用 shutdown 实现模式切换。绑定中的阻塞刷新必须运行在已有工作线程/异步调度路径，不能阻塞 UI 主线程。

导出准备以有界队列屏障区分“屏障前已接收记录”与之后并发到来的记录。报告注明截止点；不暂停业务、不要求多文件原子快照。当前进程返回自己的刷新结果；历史或其他进程覆盖从各自检查点取得，没有检查点的来源为未知。手机和桌面各自的导出负责人负责最终文件清单、不可读/截断/打包结果，与 Engine 报告分别保留。

## Workflow

1. 宿主安装既有进程运行时，登记平台角色及确实接入的原生来源；默认开始 standard。
2. 用户选择详细复现时请求限时 capture；返回实际模式和期限。
3. 网络 owner 在真实发现、尝试、连接、恢复和关闭位置提供类型化事实，运行时执行本地策略、脱敏与有界编码。
4. 成员更新发生错误时保留源错误，并把固定细节附加到本地完成记录；业务回执照旧。
5. 宿主导出前请求一次 prepare，取得刷新及覆盖报告，再收集文件。正在运行的其他进程若无法协同刷新，单独标记覆盖限制。
6. 手机或桌面 ZIP 清单保存 Engine 报告和自己的收集结果；详细超时或主动停止回到 standard，后续业务和日志继续正常运行。
7. 桌面 GUI/CLI 经 daemon client 请求诊断能力；daemon 返回的模式及 run 是展示依据，窗口生命周期不改变采集责任。

# 6. Implementation Plan

按顺序推进，各阶段提交必须包含非零测试结果及实际文件证据；完成一个最小链路后再扩大覆盖，不预先搭建通用可插拔平台。以下均未执行。

| 阶段 | 修改位置 | 修改及退出条件 | 风险与约束 |
| --- | --- | --- | --- |
| S0 合同和证据确认 | 本规格、锁定 Iroh 依赖、现有观测测试 | 核对手机构建修订；列出发现/连接/路径/关闭的真实 API 与覆盖限制；固定枚举、身份范围、字段审查和来源链映射 | 无底层 API 时标 partial，不解析原始日志；隐私字段未过审不得落盘 |
| S1 最小可导出连接链 | connectivity/、connect.rs、local_log_processor.rs、filter.rs、现有 writer 测试 | standard 下真实连接开始/终态、匿名编号、尝试失败分类写入 JSONL；远程关闭仍可导出；重复/取消终态正确 | 不扩大远程字段；不改变错峰顺序或实际超时；在默认线程栈验证 |
| S2 地址及恢复证据 | peer_address_resolver.rs、node.rs、实际地址写入方、net_recovery.rs、conn_path.rs、presence_adapter.rs | 已知旧候选失败、收到新候选、采用新候选后成功的受控链路可查；可观察路径变化与关闭有连接关联 | 观察/保存/使用代次分开；新增观察任务纳入既有任务收尾，不在 drop 中发起异步清理 |
| S3 成员错误来源 | revocation.rs 错误类型、space_security_store/、space/security/access.rs、group_update_adapter.rs | 真正存储失败有 source，固定阶段/原因落盘，原回执和恢复行为不变 | 不把规则错误伪装成存储错误；不增加业务步骤查询或字符串旁路 |
| S4 限时模式与覆盖 | runtime.rs、config.rs、status.rs、filter.rs、local_file.rs | 期限、模式切换、容量、来源登记及各类丢弃口径测试通过；生成可靠检查点和刷新屏障报告 | 高频事件不挤占无限内存；截断/过滤/未开启不能混为 dropped=0；重启回 standard |
| S5 稳定入口和平台合同 | Engine observability 出口、UniFFI/OHOS observability、绑定测试宿主 | 新增采集/原生事件/导出准备接口；现有构造器兼容；两套绑定同源；原生事件进入本地文件 | 不输出源业务 ID；不同进程只报告自己的覆盖；不增加第二个全局运行时 |
| S6a 手机接入与整包验收 | 手机仓对应原生启动/生命周期入口、诊断导出；本仓集成与回归测试、验收报告 | 手机团队接入真实来源、使用同源新产物；模拟 ZIP 验证后完成真机同类失败与恢复；逐项记录版本和结果 | 不复制 Engine 实现；不以模拟成功替代真机；未知根因与未执行平台保留标记 |
| S6b 桌面宿主接入 | desktop 的 bootstrap、daemon、daemon-contract、webserver、daemon-client、GUI 设置、现有导出负责人及适用的 CLI | 固定到具备新接口的 Engine；daemon 暴露同一采集能力；客户端控制/查询一致；宿主状态、正常导出和 daemon 不可用时的离线导出分别通过 | 不让 GUI/CLI 拥有第二份 Engine 采集；不恢复失效 capture；不将 daemon 刷新误作所有进程刷新 |
| S7 两端联合验收 | Engine、手机与桌面团队共同维护的实际版本矩阵及导出包 | 按同一时间窗口复现手机与桌面的连接失败和恢复；仅凭双方包核对事件、宿主在场情况及覆盖限制 | 本地匿名编号不能跨包直接匹配；未关联部分标未知；必需产品接入未完成不得关闭规格 |

S6a、S6b 均依赖 S5，分别由手机和桌面团队负责，可以独立交付；两者完成后进入 S7。本仓记录共同接口、验收约定和各产品状态。手机参考入口是 modules/uc-engine 的 iOS/Android 原生宿主和 src/support/diagnostics 的导出负责人；桌面参考入口见第 5.6 节。规格本身不授权发布、合并或修改其他团队正在使用的工作树。

每阶段完成时更新本规格清单和本次涉及的长期文档；全部验收后将计划移入 completed 并更新索引。不得在未实现时把 Proposed Design 改写成当前架构事实。

# 7. Edge Cases

| Scenario | Expected behavior | Implementation |
| --- | --- | --- |
| 没有任何候选地址 | 记录 address_missing，尝试数为零，不报网络握手失败 | resolver 结果与 connect 入口组合，缺字段省略 |
| 多次错峰连接，一次成功 | 一个逻辑连接终态；已开始的其他尝试记录 cancelled_by_winner | owner 为 attempts 保存取消原因，有界守卫一次结算 |
| 网络切换与拨号并发 | 该次拨号绑定自己的 used_generation；新的候选另记 | 只读候选快照，不在结束时替换开始事实 |
| 只有路径快照，没有切换通知 | coverage=partial，并记录采样观察时间 | 不把不同快照之间的时刻当作精确切换时刻 |
| 系统时钟回拨/两端时钟偏移 | 本地 duration 仍有效；不能按墙钟确定跨设备严格先后 | 单调计时；负年龄为未知；保留可验证关联 |
| 存储损坏或 source 含路径/SQL | 固定 corrupt/unknown，原始内容不进入任何输出 | 类型映射、source 链测试和敏感哨兵扫描 |
| 旧记录、旧宿主或缺构建修订 | 旧日志仍可导出，未接入来源为 not_registered，修订为 unknown | 新接口新增，不要求重写旧构造器或旧日志 |
| 重复开启、停止与新 session 并发 | 重复开始复用且不延期，旧 stop 不影响新 session | 原子状态转换及 capture_id 匹配 |
| 详细模式到期/应用被冻结 | 恢复后的首个记录前回 standard；在途记录终态仍可写 | 单调截止点在采集和查询入口检查 |
| 原生扩展被杀/主应用导出 | 缺少扩展终态或刷新确认时覆盖未知 | 每 run 检查点；不将主进程 flush 充当跨进程确认 |
| 队列/匿名表/文件容量耗尽 | 固定丢弃分类，业务不阻塞，不复用错误身份 | 有界注册、饱和计数、沿用 writer 限额 |
| 导出期间仍有新日志 | 报告屏障截止点及非原子窗口，保存已读文件的统计 | 不停业务；手机报告读取/截断结果 |
| 远程禁用或 exporter 失败 | 本地关键记录仍可用 | 本地策略与远程 gate 独立测试 |
| 操作发起者不明或不同 run 的同一设备 | 匿名关联缺失而非猜测补齐 | 明确 unknown / cross_run_peer_linking=unsupported |
| 桌面 GUI/CLI 同时控制采集或响应丢失 | 只有一个有效 capture，查询确认状态，旧 stop 不影响新会话 | 复用 daemon client 与运行时状态转换，不维护客户端采集权威 |
| 桌面窗口关闭或 daemon 重启 | 窗口关闭不停止采集；daemon 重启回 standard，客户端展示新 run | daemon 拥有采集，客户端恢复时重新查询 |
| 桌面 daemon 不可用但日志文件存在 | 允许导出已有文件，明确未取得实时状态及刷新确认 | 复用离线导出，不强行启动 Engine，不填充成功状态 |

# 8. Testing Strategy

## Unit Test

| 输入 | 操作 | 预期 |
| --- | --- | --- |
| 固定事件组合及未知枚举/超长数据 | 编码/解码 | 合法记录往返；非法输入拒绝，计数准确，无原文回显 |
| 顺序变化但内容相同的候选集合 | 比较并分配代次 | 重排不增代次，真实变化增代次，来源之间不混用 |
| 同一 peer、多 peer、映射容量耗尽 | 分配及清理匿名编号 | run 内稳定且无碰撞误关联，超限有退化报告，无持久映射 |
| 可控单调时钟、重复 begin/stop | 推进限时模式 | 到期自动降级，旧 stop 无效，重复开始不无限续期 |
| 实际源错误与纯规则拒绝 | 错误转换 | 下层错误 source 可追溯；规则分类不变；输出只有固定原因 |
| 来源未注册、零事件、过滤和丢弃 | 查询报告 | 各种状态可区分；不存在全局无过滤的虚假结论 |

## Integration Test

- 使用真实 Iroh 端点和受控候选来源，触发一次失败、一次新候选采用和成功连接，读取实际文件，核对开始/结束、代次、关联和恢复原因；声明这是受控故障。
- 使用真实 SQLite，在成员更新实际保存边界注入可复现错误，验证源错误、阶段、回执及文件内容。解析错误不得被错误归类为 SQLite 写入失败。
- 关闭远程出口后，生成认证、连接和恢复记录，执行 flush，再从磁盘读取；打开 mock Collector 时验证新增字段未进入远程。
- 两个独立进程先后登记来源并写入受管日志，另一个进程被终止；导出报告不能将其缺失终态描述为成功刷新。
- 包含私密正文、设备名、路径、地址、邀请码、令牌及源错误的哨兵输入，扫描本地文件、模拟远程请求和手机测试 ZIP；任何未经审查的内容命中都失败。
- 队列满、磁盘拒绝写入、flush 超时、文件不可读、文件截断分别注入，验证 Engine 报告与手机 manifest 的责任边界。

## Regression Test

- 桌面通过真实本机 API/client 验证开始、重复开始、停止、查询及导出准备；GUI 和适用的 CLI 观察同一 capture，响应丢失后查询仍一致，未认证请求被现有通信边界拒绝。
- GUI 关闭后 daemon 继续采集；daemon 重启后查询 standard 和新 run；daemon 不可用时实际离线 ZIP 包含已有文件，并明确缺少本次刷新确认。
- 桌面实际 ZIP 分别核对 GUI、daemon、CLI 和 Engine 的文件、构建修订和覆盖情况；daemon flush 不得覆盖其他进程的未知状态，远程开关不得随详细采集改变。
- 桌面 macOS、Windows、Linux 分别记录启动/退出和睡眠/唤醒能力及结果；平台没有的事件源标 unsupported，不能报为已开启且零事件。未执行平台标跳过，联合验收使用实际手机与桌面导出包。

- 默认线程栈下的暂停、恢复、取消、重启后传输；不得提高 RUST_MIN_STACK 作为通过手段。
- Apple 系统日志、嵌套宿主 span、独立健康记录、单个完成事件、不重复安装和关闭后的行为。
- 原有准入、成员更新结果及来源链；覆盖已提交但等待恢复，避免把“保存成功”写成“所有成员同步完成”。
- 手机安装来源与产物校验、iOS 主应用/分享/键盘扩展、Android 无 Activity/无 JS 的后台入口；HarmonyOS 至少验证绑定编译和合同，未执行设备标跳过。
- 必要仓库门禁：cargo metadata --locked --format-version 1；cargo check --workspace --all-targets --locked；cargo fmt --all -- --check；node scripts/architecture/check-engine-repository.mjs；git diff --check。按本机有效存储规则从仓库根执行、保留编译缓存，Cargo 验证串行。

# 9. Acceptance Criteria

* [ ] S0–S5、S6a、S6b、S7 分别记录实现提交、非零测试数、实际执行环境及输出证据；不得仅勾选编译成功。
* [ ] 默认模式的导出文件能找到连接起因、终态、已知失败阶段和恢复经过。
* [ ] 详细模式能区分发现、接受/保存、使用的候选信息及各次连接尝试。
* [ ] 中转状态、观察到的路径变化及关闭记录与正确连接关联；未知阶段明确未知。
* [ ] 成员更新失败包含实际阶段和安全原因，下层 source 保留，现有业务结果与重试责任不变。
* [ ] 本地详细采集不受远程开关影响，普通原始日志不能绕过白名单，新字段不进入远程合同。
* [ ] 限时采集、重启降级、有界存储、容量耗尽和可靠刷新均有测试；不存在未收尾的新观察任务。
* [ ] 关联编号的范围和容量可验证，未持久化身份映射，未伪造跨重启或跨设备关联。
* [ ] 新增绑定只暴露诊断动作，手机能够取得实际策略、覆盖、丢弃及刷新结果。
* [ ] 桌面 daemon 是 Engine 采集唯一负责人；GUI 和适用的 CLI 复用现有通信，取得同一真实状态，重复请求和重启恢复测试通过。
* [ ] 桌面宿主生命周期进入可导出记录；窗口关闭不终止 daemon 采集，正常暂停不关闭进程日志运行时。
* [x] 桌面实际 ZIP 包含应用与 Engine 记录及各来源覆盖；daemon 不可用时仍可导出已有文件并明确缺失，不冒充其他进程刷新成功。
* [ ] 真机 ZIP 区分无事件、未开启、不可用、未收集、截断及丢弃；flush completed 不被展示成完整覆盖证明。
* [ ] 至少一次同类真实连接失败及恢复与桌面同时间窗对照；未复现根因保持未知，模拟故障单列。
* [ ] 导出包不包含敏感哨兵；构建修订来自实际产物，主应用与扩展各自来源可核对。
* [ ] 手机、桌面接入和联合真实设备验收分别报告；任一必需产品接入或联合验收未完成时只标对应阶段状态，不将整个规格标完成。

执行状态：S0 证据核对已执行，手机新产物修订待产品提供；S1 已验证；S2 已实现并有组件验证，联合候选失败/更新/恢复证据待补；S3 已验证；S4 已实现并有运行时验证；S5 已实现，两套绑定宿主测试和生成验证通过，原生设备构建待产品执行；S6a 手机未执行；S6b 桌面已在 macOS 完成接入、真实后台与诊断包验收，Windows、Linux 和图形界面真机点击跳过；S7 联合验收未执行。

Engine S1–S5 实现提交：`9708c278`。该提交不包含手机、桌面产品接入或真机验收。

实施记录（2026-09-11）：锁定 Iroh 提供 ConnectWithOptsError、ConnectingError、home_relay_status、remote_info、watch_addr 及 AddressLookup 的结果流；publish 是 fire-and-forget，不能证明外部发布已完成。现有准入连接使用 NoSubscriber 隔离底层驱动，新的连接记录须在该隔离边界之外发出，不延长业务 span。S1 首个回归使用真实 SQLite、Iroh 连接及 JSONL，要求区分连接成功与之后的凭据认证失败。

S1 已验证的切片（其余阶段仍未完成）：

- 真实准入连接与 JSONL：先验证旧代码缺少 connection.started/finished，再验证连接成功与随后认证失败分开；关闭端点后再次连接记录 establish/endpoint_closed，并保持同一匿名对端、不同 connect_id。1 项通过。
- 错峰连接：三次失败与外层取消只计入真正开始的尝试；真实第二次连接胜出后，已开始的首个尝试记录 cancelled_by_winner。2 项通过。
- 本地/远程隔离：本地连接记录包含 run_id、peer_ref，远程仍只有原认证摘要，且不增加远程拒绝计数。1 项通过。
- Future 在 subscriber 作用域外被取消时，守卫保留原 dispatcher 发出终态，不持有业务 span；普通连接调用中的 NoSubscriber 已下移到真正的 Iroh 驱动，避免把安全记录一并屏蔽。
- 本地关联表由运行时持有，最多 4096 个匿名对端，不持久化原始映射；构建修订暂为 unknown，尚不能据此标记 S4/S5 完成。
- S2 可用 API 补充：Connection::path_events 提供 Opened/Closed/Selected/Lagged，WeakConnectionHandle::closed 可观察关闭；新增观察任务必须使用弱句柄，不能通过克隆 Connection 延长连接寿命。

S2 进行中的证据：已验证存储地址读取记录来源/数量/观察时间且不输出地址；实际 JSONL 验证同一对端的候选值使用随机编号关联，来源代次与使用代次独立，旧值重新出现仍增加来源代次。相同候选编号只证明值相同，不证明某发现服务被选择；连接调用点显式声明 stored、admission_route 或 provided，避免从协议名称推断来源。发现服务和连接路径监听仍在实施，尚未完成 S2。

S2 新增验证：节点真实 mDNS 发布与关闭记录可穿过驱动的 NoSubscriber 隔离，弱句柄观察不延长连接寿命；运行时文件验证底层连接 key 复用后分配新的匿名连接编号；中转恢复动作只结算 submitted/timed_out，真实中转状态另记；在线检查旧关闭事件已移除，节点是关闭记录的唯一生产负责人。真实地址仓储保存记录在写入成功后发出，不解析其不透明业务负载。

S3 进行中：KeyEpochError 的下层异常改为保留 source，纯状态拒绝使用封闭 StateIssue；原有维护延期分类和远程 Storage 分类保持不变。已验证真实源错误可以追溯，公开 Display/Debug 不包含源错误私密正文。具体 group update 阶段及本地完成附件正在实现。

本轮实现补充：

- S3：接收成员更新的实际解析、加载、应用、校验、保存和安装步骤保留源错误；68 项安全流程测试、21 项实际存储测试以及真实连接拒绝回执测试已通过。本地仅输出固定阶段和原因，远程分类保持原值。
- S4：限时采集重复开始不延期、旧停止请求隔离、到期后的在途终态、后台恢复保守结束、来源登记、配额/队列/实际写入分别统计，均有实际 JSONL 或队列测试。正常关闭结束采集并记录未完成宿主动作中断。
- S5：Engine、UniFFI 和 HarmonyOS 提供采集控制、来源登记、固定宿主事件和本地导出准备。HarmonyOS 大计数用十进制字符串；UniFFI 同步刷新须由产品放在后台队列。现有构造器未增加必填字段。
- 导出报告统计当前进程和查询时间窗口；它不是暂停业务后的文件快照。产品必须另报 ZIP 实际读取、截断和其他进程刷新确认。尚未提供逐记录的精确导出序号截止点，不能据统计反推每条记录必定入包。
- 进程 run、采集 capture、来源候选代次和物理连接编号已提供；节点重建的独立代次尚未提供。不能将同一进程内的所有来源状态误作单一节点生命周期。
- 底层内部 payload 继续限制 2048 字节，最终本地记录限制 4096 字节；格式拒收为全局计数，无法安全解码的事件不猜来源。来源文件计数只覆盖进入审查后的类型化输出，普通被拒 target 不计为完整采集。
- 本仓实现不证明手机已经升级；S6a 和 S7 保持未执行。未重新下载、复现或证实原事故根因。

桌面 S6b 与内容级补充验收（2026-09-11，macOS 本机）：

- Desktop 固定使用 Engine `9708c2786a604e76b19ab8dc63b2e923c0e9e35f`，后台服务唯一持有采集；GUI 与 CLI 共用本机认证入口。真实后台验证了开始、重复开始不延期、旧编号隔离、停止、重启回 standard、在线 ZIP 与离线退路。
- 真实 SQLite、真实 Iroh 端点和实际 JSONL 的缺失继续凭据场景同时检查两端：连接记录包含用途、候选来源、匿名对端、逻辑连接编号、尝试序号、耗时和终态；客户端写入认证拒绝，服务端写入 continuation_credential/record_missing，连接成功不会覆盖后续认证失败。
- Application 恢复流程验证记录 state_changed 触发、authentication_rejected 原因和 deferred 最终结果；运行时文件验证补充 wait_for_recovery_trigger，且不输出设备名、端点身份、地址、路径、凭据或原始错误正文。
- Desktop 归档测试逐字段回读 Engine JSONL，确认连接失败阶段、原因和尝试次数原样进入 ZIP；实际 ZIP 同时报告刷新、包含、不可读、截断和其他进程未刷新。
- 这组证据证明当前已覆盖的认证故障可被诊断，不补足 S2 的“旧候选失败、发现新候选、采用后成功”联合证据，也不代替 Windows、Linux、手机或两端真机包。

本轮验证记录（2026-09-11，macOS 本机）：

| 验证范围 | 结果 | 证据与限制 |
| --- | --- | --- |
| 观测合同与运行时 | 108 项通过 | 包含真实 JSONL、远程隔离、限时模式和原生动作；不等于真机包 |
| UniFFI、OHOS 与运行时联合回归 | 150 项通过 | 原有公开合同、宿主生命周期、重启和接口布局；与上一行有重叠，不合计 |
| Swift/Kotlin 生成与 Swift 编译检查 | 通过 | 从当前本机库生成两套文件，Swift typecheck 通过；Kotlin 未执行 Android 编译，本机缺 ktlint 仅跳过自动排版 |
| 最新 OHOS 原生诊断入口 | 4 项通过 | 开始/结束采集、来源登记、动作记录和异步导出调用 |
| 连接错峰与取消 | 2 项通过 | 已开始的失败与竞速取消准确计数 |
| 安全流程 | 68 项通过 | 保留 source 后重跑实际更新与恢复路径 |
| 准入连接与实际日志文件 | 1 项通过 | 连接建立和随后认证拒绝分开 |
| 配对后重启并传输 | 1 项通过 | 显式启用 dev-tools，在默认线程栈运行；未启用开关的零测试运行不计通过 |
| 文件刷新超时 | 1 项通过 | 超时单独返回，计入等待提交锁的预算 |
| 全仓编译、元数据、格式与仓库规则 | 通过 | 本机全局 Cargo 覆盖指向其他源码，使用外置源码副本和独立 Cargo 配置校验，复用本工作树既有 target |
| 手机、桌面新版 ZIP 与两端真机复现 | 跳过 | 未接入产品新入口，未声称真实根因已经确定 |

Engine 实现已提交但未推送或发布。编译存储检查提交 `4e689543` 与 Rust 编写门禁提交 `74f0fd02` 独立于本计划，不纳入诊断变更归属。

# 10. Risks and Trade-offs

- **底层可观察性不足**：固定版本未必提供中转握手、查询或切换的全部阶段。优先结构化 API；缺口写进报告。额外轮询仅在详细模式按有界频率进行，不能创建无上限逐 peer 任务。
- **匿名关联范围有限**：采用 run 内随机编号避免长期身份追踪，代价是跨重启只能并列时间窗口，不能保证设备精确对应。此限制必须体现在验收和报告中。
- **错误类型改动较深**：保留 source 可能影响内部 trait 约束，需沿成员更新路径逐步迁移；不要同时改 unrelated 错误或持久格式。
- **详细采集增加开销**：记录上限、默认集合及期限先固定；S1/S4 实测 enabled/disabled 的耗时、分配、队列峰值和文件增长，结果记录后再调整默认量。不得用增加超时掩盖退化。
- **共享目录与扩展收尾**：既有运行权保证不能被新日志任务破坏；文件接受不是跨进程原子快照。通过 run 检查点和刷新截止点表达真实能力。
- **通用日志捕获替代方案**：直接放开模块 target 实现更少，但会引入原始地址/错误并仍缺乏稳定关联，因此不采用。重新做一套日志 SDK 会重复生命周期和队列，也不采用。
- **字段合同维护**：本地版本独立演进，远程继续最小化；新增字段需同步本地 decoder、负向测试和手机清单消费，不能只改发送方。
- **规则与 heuristic 边界**：采集期限/容量是运行策略，来源/错误/路径是观察事实，业务重试仍是原规则。不新增“地址老了所以失效”“恢复后成功所以旧地址一定过期”等 heuristic。

# 11. Open Questions

1. 手机 build 180 实际链接的 Engine SHA、绑定版本及各扩展来源尚需手机团队从产物核实。不会阻塞 S1 的库内测试，但阻塞对原始故障包覆盖范围的最终结论。
2. 锁定 Iroh 版本能否完整观察各发现来源、候选接受和真实路径切换？S0 必须逐项填写 API 证据；不可观察项先标 partial/unsupported，不能声称已完成对应精确阶段验收。
3. 原始超时究竟是地址失效、发现发布/消费延迟、中转不可达还是生命周期影响？必须由新真机证据判定；本规格不预判根因。
4. 交接若要求跨重启精确认出同一对端，需手机团队明确接受首版限制或另立隐私设计；本计划不持久化设备映射，也不借构建修订或时间相近代替身份。
5. 联合真机矩阵的设备、复现窗口和下游版本由手机与桌面团队共同安排；桌面各系统事件能力和是否提供 CLI 诊断命令须在 S6b 明确。无法执行的项保留待验收，不由 Agent 虚构通过记录。
