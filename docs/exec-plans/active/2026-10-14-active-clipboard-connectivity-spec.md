# Issue #34 第五切片：当前剪贴板状态与按需拉取连接规格

状态：**本地实现与验收完成，待线上当前 head 检查**（2026-10-14）。本文落实本轮 grilling 决定；具体代码组织与容量参数是基于现有实现的工程设计。
审计基线：`main 2f9afc9956b0cda82e65c912f0c0f6dc6aa51256`（PR #180）。实现开工时重新核对最新 main，
在修改生产代码前冻结旧端版本与源码摘要，不自动沿用漂移的分支名。
[执行计划](2026-10-14-active-clipboard-connectivity-plan.md)只维护工作顺序、门禁和证据，设计正文以本文为准。

# 1. Overview

普通剪贴板、组更新和成员历史已经接入 Uni，当前剪贴板状态通知及按需拉取仍由四个 iroh adapter
直接解析地址、拨号和操作原生流。状态发送缺少完整交换期限；两个入站 handler 的开流、读帧及处理等待
也没有统一期限。多目标通知逐台发送，本机恢复广播在发送期间还使用无界队列积累新状态。

本片完成场景：A 恢复一条历史剪贴板，向 B 通知当前状态；B 缺少内容时向 A 请求加密传输封套，
经现有保存、文件物化和系统剪贴板写入流程完成应用。连接获取、准入、容量、期限与关闭收口到 Uni；
状态选择和应用仍由 Application 剪贴板负责人承担。

本片不承诺一次短暂失败后必然自动补齐。失败结束当前尝试，后续状态变化或上线事件仍按既有规则触发新工作。

# 2. Goals

- 状态发送、状态接收、拉取客户端与拉取服务全部经过 Uni，删除这四条生产路径的原生拨号/开流实现。
- 四类网络交换分别有 10 秒绝对期限，涵盖各自入口后的排队和网络步骤，不逐阶段重置。
- 普通状态广播及上线补发每轮最多并行处理 4 个目标，失败独立结算。
- 本机恢复广播最多保存一个当前轮状态和一个最新待发状态，移除无界通知队列；当前轮不被更新抢占。
- 入站业务仍串行处理，保留 LWW、新旧状态判断、接收策略与系统写入顺序。
- 状态与拉取采用独立的有限容量；网络取消后未完成本地工作继续占用原额度并被生命周期负责人回收。
- 保持旧 ALPN、线上字节、无 ACK 状态通知和拉取响应语义，验证固定旧端双方向互通。
- 独立进程直连、Relay、完整内容读回及故障验收进入必需 CI；本地门禁先于推送执行。

# 3. Non-Goals

- 自动重试、持久补偿队列、保证最终补齐，或拉取失败后推进当前状态。
- 新的业务 ACK、批量协议、多流 ALPN、空闲连接池、跨协议共享连接；跨轮保留仍仅用于成员历史。
- 文件/blob 本体及进度通道的 Uni 迁移，改变文件下载期限或要求大文件在 10 秒内下载完成。
- 并行执行多个入站状态用例，或让新入站状态抢占正在处理的拉取/系统写入。
- 修改稳定 Engine/绑定接口、剪贴板持久格式、内容身份规则、LWW 顺序、权限设置或防抖时长。
- 完成整个 Issue #34，扩展全部历史版本矩阵，替换 iroh，或自动从 P2P 降级到 LAN。

# 4. Current Architecture Context

Component: 当前剪贴板完整业务负责人
Path: `crates/uc-application/src/clipboard/active/`、`clipboard/sync/active_state/`
Responsibility: 启动接收/广播/上线补发 worker，比较状态、按需拉取、保存并交接系统剪贴板写入。
Relationship: `ActiveClipboardFacade` 持有完整生命周期；`ApplyInboundActiveClipboardStateUseCase::run`
逐条执行 `handle_one`。应用成功按既有写入负责人推进 register，拉取失败不推进、不转播。

Component: 广播与状态入口
Path: `crates/uc-application/src/clipboard/sync/active_state/{fanout,restore_broadcast_worker,peer_online_resync_worker}.rs`、`clipboard/write/restore_broadcast.rs`
Responsibility: 发送权限和内容类型过滤、多目标广播、恢复通知合并、上线触发补发。
Relationship: 两处目标循环目前串行；恢复通知使用 `UnboundedSender`，300 毫秒静默防抖只在开始发送前消费队列。
上线补发保存目标集合，沿用 1500 毫秒静默防抖，发送时读取当前 register；两种入口不能混为同一种覆盖队列。

Component: 原生传输
Path: `crates/uc-infra-p2p/src/network/iroh/active_clipboard/`
Responsibility: 四个 adapter 分别发送/接收状态、请求/服务拉取，调用原 codec、身份 gate 和地址 resolver。
Relationship: 状态无业务 ACK；旧发送端最多等待 3 秒连接关闭。拉取客户端有 10 秒总超时，完整读响应后主动关闭；
服务端发送 FIN 后最多等 3 秒对端关闭，避免发送缓冲尚未排空就拆连接。

Component: 线上格式与完整业务 port
Path: `crates/uc-sync-protocol/src/active_clipboard_{state,pull}.rs`、`crates/uc-core/src/ports/clipboard/active_clipboard/`
Responsibility: 前者提供唯一无传输依赖的 codec；后者提供 dispatch、subscribe、pull 和 serve 能力。
Relationship: Application 不依赖 `uc-sync-protocol`；Infra wire 实现通过 Application 专属 port 提供编解码。

Component: 拉取内容准备及本体传输
Path: `crates/uc-application/src/clipboard/sync/active_state/serve_pull.rs`、`clipboard/outbound/{mod,payload_prep}.rs`、`clipboard/active/mod.rs`
Responsibility: 按内容摘要重建 snapshot，为文件和大图片发布指向本机的 blob 引用，生成新的传输密文；请求方解密后按现有入口保存。
Relationship: 响应不是磁盘密文副本；文件本体经现有 blob/文件流程物化。共享准备流程被普通发送、重发和拉取复用。

Component: Uni 与会话本地工作
Path: `crates/uc-application/src/connectivity/`、`crates/uc-infra-p2p/src/network/iroh/{connectivity,session_generation.rs}`
Responsibility: 统一租约、排队、撤销围栏、连接登记、关闭；既有 generation 本地工作登记阻止旧工作越过安全交接。
Relationship: `StreamLease::hold_for_work` 可保留额度；成员历史已有网络取消后保留本地工作的模式，但直接复制“整个 future 一直执行”
不足以满足本片“取消后不启动后续步骤”的要求。

Component: 装配与验收宿主
Path: `crates/uc-engine/src/assembly/sync_engine.rs`、`tests/hosts/connectivity/`、`scripts/testing/`
Responsibility: 组装完整能力、注册 Router 与生命周期；独立进程驱动稳定 Engine 操作并收集网络证据。
Relationship: 当前 connectivity host 有内容发送、读回和设置入口，尚缺历史恢复命令；旧端验收需显式解决测试驱动入口。

# 5. Proposed Design

## Components

| 组件 | 输入、输出与职责 | 原因与边界 |
| --- | --- | --- |
| `clipboard/active/transport/`（新增 Application 内部模块） | 完整 dispatch/pull、完整入站 receive/serve；输入业务请求或 Uni 连接，输出原 port 结果或入站交接结果 | 统一流程顺序、期限、取消及结果；按 `sender`、`receiver`、`pull`、`ports`、`model`、`error`、测试分文件，`mod.rs` 只作入口 |
| 状态 Uni manager 与拉取 Uni manager | 各自持有连接与等待额度，输出独立单次交换租约 | 共用实现、独立实例；慢拉取不耗尽状态协议额度，二者都不保留空闲连接 |
| Infra wire 与 iroh bridge | 原 codec 对接 Uni 半流，完成 TLS 身份绑定、原生关闭及 Router 转交 | 不决定内容、LWW、权限策略或重试；Application 不依赖 codec crate 或原生连接类型 |
| 恢复广播的最新值槽 | 接收已完成本机激活的状态及内容类别，输出下一轮唯一候选 | bounded latest-value 槽替代无界 mpsc；不丢失权限过滤所需的类别 |
| 共享的有界目标发送流程 | 输入一轮状态、类别、既有授权范围与目标迭代器；输出全部目标已结算 | 两种广播入口复用，最多 4 个目标 future，不为每目标无界 spawn |
| 可停止的拉取准备与本地工作桥 | 输入摘要及不透明停止控制，输出完整新密文或失败；实际本地工作持有原额度与 generation 许可 | 只中断尚未开始的步骤；已开始的持久工作由既有 owner 结算，不另建通用后台任务系统 |
| Engine 装配 | 注入两类 manager、wire、入站端点与完整准备能力 | 不读取内部阶段、不编排准备步骤、不增加稳定外部操作 |

Core 保留状态比较与权限规则，Application 决定应用、传播、取消和恢复；Infra 提供具体传输、存储和本地工作执行能力。
容量和防抖属于显式资源策略，不推断设备资格；在线状态只作原有发送提示，不代替当前成员授权。

## Data Model

### 协议与连接

| 协议 | ALPN 与帧 | 保持的合同 |
| --- | --- | --- |
| 当前状态 | `uniclipboard/active-clipboard/0`；magic `0xC3`、u32 BE 长度、V1 postcard，body 上限 4 KiB | 一连接一条双向流，但只发送状态，无业务 ACK |
| 内容拉取 | `uniclipboard/active-clipboard-pull/0`；请求 magic `0xC2`、u16 BE 摘要长度；摘要上限 1024 字节 | 一连接一次请求/响应；响应 0/1/2/3 为 Envelope/NotAvailable/Locked/Internal，成功封套上限 16 MiB |

新增 `ConnectivityProfile` 与 `ConnectionProtocol` 的状态、拉取枚举项，均映射 `SingleExchange`。
ALPN 常量机械移入对应唯一 codec 模块，更新全部使用方，不维护多份字符串或兼容转调入口。
沿用原 FIN、错误状态、字段顺序与解析语义；冻结向量覆盖空值、坏 magic、超限、截断、未知状态及尾随输入的既有行为。
不得借迁移悄悄改变对既有字节的解释，也不在拒绝/超时后换 ALPN 重送。

### 最新待发状态与广播轮次

- `PendingActivation` 保存完整状态、内容类别、入槽版本和到达时刻；仅在内存中存放。时间用单调时钟，协议时间戳保持原规则。
- 本机恢复入口的触发器保持非阻塞。最多一份当前轮状态和一份待发状态；同一临界区更新状态及类别。
- 待发候选按既有 `ActiveClipboardState` 的激活顺序比较，迟到的旧回调不覆盖较新激活；完全相同的重复不反复刷新静默期限。
- 当前轮期间，新候选替换待发槽，不取消本轮尚未完成的目标。轮次结束后按最后一次有效更新计算剩余 300 毫秒静默期；
  若静默期已过，立即开始下一轮，不额外等待一个完整窗口。持续新激活仍沿用原静默防抖，不新增最大等待策略。
- 槽用“版本 + 原子取走/标记已消费”避免发送完成时清空刚到的新值；网络等待期间不持槽锁。
- 该槽替换现有本机恢复通知队列。上线补发仍合并目标集合、发送时读当前 register；不能用覆盖一个状态的方式丢掉待补发目标。
  入站成功后的转播沿用原触发与顺序，只复用 4 目标并发发送流程，不改成新的全局广播总线。
- 每轮 4 目标不是整个进程仅有 4 个请求；不同既有触发入口可并存，总资源仍受各 manager 限额约束。
  等候进入本轮 4 个目标槽的设备尚未调用 dispatch；其 10 秒预算从调用 dispatch 开始，整轮不承诺 10 秒结束。

### 初始容量

以下复用 Uni 默认值，是本片初始内部配置，实施中不得通过放宽它们让测试通过。

| 资源 | 状态类 | 拉取类 |
| --- | ---: | ---: |
| 每 peer 活动出站交换 | 4 | 4 |
| 会话活动入站交换（含保留额度的本地收尾） | 16 | 16 |
| 每连接业务流 | 1（单次协议） | 1（单次协议） |
| 每 peer 等待请求 | 32 | 32 |
| 每类会话等待请求/入站处理上限（按 Uni 既有分项约束） | 128 | 128 |
| 每类会话拨号 flight | 64 | 64 |
| 空闲保留连接 | 0 | 0 |

两类资源上限分别计算并合计评估：活动入站最多 32；不能把隔离后的每类 16 描述成全局 16。
入站原 broadcast 队列仍为 64，滞后丢失沿用原行为和固定分类诊断，不升级为可靠收件箱。
取得业务额度前不分配完整帧或构造封套；16 个拉取响应的线上封套字节上限合计 256 MiB，
这不是进程 RSS 承诺，还须检查重建 snapshot、内部副本及在途本地工作。不得复制密文到等待队列。

## API / Interface

| 接口 | 输入 → 输出 | 错误与完成含义 |
| --- | --- | --- |
| `ActiveClipboardDispatchPort::dispatch`（保留） | DeviceId、ActiveClipboardState → `Result<(), ActiveClipboardDispatchError>` | 只表示本机完成发送步骤，不确认对端接收、保存或系统应用 |
| `ActiveClipboardReceiverPort::subscribe`（保留） | 无 → 有界状态观察 receiver | 交接成功不等于业务采用；无订阅者或消费者滞后不伪造应用成功 |
| `ActiveClipboardPullClientPort::pull`（保留） | peer、snapshot_hash → 加密封套 | 10 秒内取得完整响应；返回密文不等于下载完文件或已应用 |
| `ActivePullPreparationPort::prepare`（拟新增，归 Application） | snapshot_hash、不透明停止控制 → 加密封套或准备错误 | 完整内容准备，含取消边界；替换仅为此流程使用的内部 Core `ActiveClipboardPullServePort`，同步迁移装配/测试并删除旧接口 |
| `ActiveStateReceiverPort::receive` / `ActivePullReceiverPort::receive`（拟新增，归模块） | Uni 连接租约、入口绝对 deadline → 传输交接结果 | 完整入站端点；Infra 只调用一次，不逐步骤调用业务 |
| `ActiveClipboardWirePort`（拟新增，归模块） | Uni 半流与状态/摘要/响应模型 → 解码模型或写入结果 | 状态读写、pull 请求/响应读写；纯 framing 实现调用已有 codec，错误保留 source |
| `ActivePullLocalWorkPort::execute`（拟新增，归模块） | 不透明本地工作 future → 完整工作结果 | bridge 负责 generation 登记及实际结算，不能根据模型内容决定业务步骤 |

上述新增类型只通过内部装配需要的出口提供，不进入 `uc-engine` 稳定 API。内部 Core serve port 并无稳定外部契约；
迁移它是为了让取消控制归真正调用者，避免向 Core 添加 tokio token 或让 Engine 查询步骤。
准备错误保留原 `NotAvailable`、`NotUnlocked` 和带 source 的失败语义，额外的 `Cancelled` 只作内部控制结果，不新增 wire 状态。
wire 模型是模块内部数据，不引入第二套权威状态或 codec。

错误映射集中在目标错误模块：地址确实缺失、明确网络不可达可落原 Offline/Unreachable；仓储、身份读取、
任务、容量及 codec/I/O 失败保留带 source 的 Io/Internal，不能把地址仓储报错吞成“设备离线”。
pull 超时保持 Unreachable，远端三个无内容状态保持 NotAvailable；这些原有无来源结果的边界转换须先保留内部错误，
按[错误规则](../../design-docs/error-handling.md)记录固定分类后再投影，禁止记录正文。状态新增加的总超时落带来源的 Io，
不新增稳定 Engine 错误码。没有下层错误的 Busy/Denied 用固定原因类型，不伪造网络错误。

## Workflow

### 出站状态与拉取

1. 广播 owner 按原当前成员范围、发送开关、内容类型及目标排除规则选择设备，通过共享有界发送流程执行，
   任一目标错误只结束该目标。上线补发复用同一流程，保留原触发规则。
2. 单目标 dispatch/pull 入口计算 `now + 10s`，在 Uni 排队前冻结请求撤销快照；地址解析、准入、连接、开流、
   写帧与等待结果使用同一个 deadline。最后一步超时也不能新增预算。
3. 两种协议都是 `SingleExchange`。并发调用可使用 Uni 原 flight 管理，但一个成功单次连接只能被一个交换取得，
   其余请求按既有有界独占逻辑取得自己的连接；不得把同一连接误当多流共享。
4. 状态帧写完并 FIN 后，保留原最多 3 秒的关闭/排空保护，期限取剩余总预算与 3 秒的较小者。
   在总预算内仅关闭保护期结束不升级为业务 ACK；总预算耗尽则本次网络等待超时，不能延续等待。
   Uni 若需暴露关闭完成能力，应是通用不透明 lease 能力，不泄露原生连接。
5. pull 写请求并 FIN，读取原响应；完整读回后由客户端关闭单次连接，服务方不能提前拆连接截断响应。
   请求可能已写出后绝不自动重送；保留当前“本次失败即结束”规则。

### 入站与权限

1. Router 将原生连接交给协议入口时记录绝对期限，两个协议均为入口时刻加 10 秒；先取得有限入站处理能力，
   再执行身份查询、准入、等流、读帧、处理等待及响应。任何排队都不刷新期限。
2. 复用同一身份目录、`AdmittedConnections`、成员权威与 generation。未知身份不允许通过首帧自行介绍；
   本片不套用成员历史的受限恢复授权。撤销前进入的旧请求不能在移除再添加后借新 grant 发送。
3. 状态帧完整读取后、交接前重检准入；绑定的来源 DeviceId 与帧内 `activated_by` 各守原语义。
   后者可以表示原激活者，不得错误要求它等于本次转发者；本地查内容只用摘要，不使用远端 entry_id。
4. 将状态交给既有容量 64 的接收队列，网络交换即完成。后面的串行 pull、保存、blob 物化和系统写入不属于这条通知的网络期限。
5. pull 在开始准备前及发送响应前重检资格。保留原权限差异：推送遵守发送开关/内容类型；显式拉取不因关闭主动发送就自动被禁，
   仍受当前成员、内容交换 gate、锁定和既有接收策略约束。迁移不得借用普通推送 gate 改变这个行为。
6. 拉取服务生成新的加密封套并按原 wire 响应；NotAvailable/Locked/Internal 保持原值。过载、无身份或 deadline 已过时
   有界关闭，不发未定义 Busy 字节。准备失败不会推进任何 active register。

### 取消、本地工作与关闭

1. 网络 timeout、peer 关闭、调用取消、撤销和 generation 退休都触发同一私有停止控制；已排队未开始的工作不启动。
2. 完整准备 owner 在查找/重建、每个文件/图片发布、编码、加密之间检查停止状态；共享 outbound 准备的唯一实现接收
   所需停止能力并在各发布边界检查，全部调用方同步迁移，不复制一份拉取专用准备流程。
3. 单次已经开始的 `publish_blob`/`publish_blob_path` 或持久能力调用视为一个不可拆开的本地动作：其 owner 保留实际任务，
   安全结束后才释放 generation 许可和 `hold_for_work` 使用权。外层取消不能提前归还额度。
   阻塞池任务在实际执行前也检查停止状态，排进阻塞池不等于必须执行。
4. 若文件 A 的发布已开始、文件 B 尚未开始，取消后只结算 A，不启动 B、后续编码或加密，不再发送响应。
   把整个 serve future detach 后照常跑到最后不符合合同；也不能用 timeout 丢弃外层 future 冒充底层写入已停止。
5. 本地结果失败保留 source 与固定诊断；已完成的安全缓存依既有 owner/GC 规则回收或复用，不新增持久重试记录，不回滚本地历史。
6. shutdown/suspend/generation 交接停止入口、取消网络、关闭两类 manager，再按既有生命周期等待本地工作。
   本地收尾可以超过 10 秒；超出原生命周期等待能力时诚实报未完成/失败，不无界等待、不声明已经安全切换。
   已接受入站观察的串行应用继续遵守原 ClipboardWriteCoordinator 与成员许可围栏，不能由网络 handler 另开后台应用任务。

### 观测

遵守[业务记录组织标准](../../design-docs/observability.md#业务记录组织标准)，Engine 只装饰既有完整能力。
记录应区分恢复历史触发、上线补发、收到状态，及发送完成、未采用、超时、无内容、应用失败等结果。
正常连接关闭和本地收尾不另立业务入口。协议不含可用跨端 trace 字段时明确作为接收诊断，不用摘要/设备身份/时间拼接，
也不为观测修改 wire。来源错误、snapshot hash、entry_id、activated_by、文件名和路径均不写入自由文本日志。

# 6. Implementation Plan

Step 1:
File: codec golden tests、`tests/hosts/connectivity/`、执行计划证据目录。
Change: 冻结开工旧 main，先运行原正常流程；构造无限读等待、慢目标阻塞、待发状态积压的 red-capable 测试。
Risk: 新宿主命令在旧 binary 不存在；必须先完成双版本测试驱动方案，不能在最后用新端互测冒充旧端。

Step 2:
File: `clipboard/active/transport/`、`network/iroh/{connectivity,active_clipboard}/`、唯一 codec。
Change: 先完成状态完整收发：单次 Uni、准入与 10 秒 deadline、关闭保护，接回原串行观察入口。
Risk: FIN 尚未送完就释放租约、通知被误当应用成功、未知身份穿过队列。

Step 3:
File: `serve_pull.rs`、`clipboard/outbound/`、私有 transport 与本地工作 bridge、`application.rs`/装配。
Change: 迁移拉取完整双向链，替换内部 serve port，补停止控制及实际本地工作结算，保持 blob 本体链路。
Risk: detach 整条流程继续开始新步骤，或取消释放容量但磁盘任务仍在执行。

Step 4:
File: `fanout.rs`、`peer_online_resync_worker.rs`、`restore_broadcast_worker.rs`、恢复触发器及其装配。
Change: 共享每轮 4 目标并发；恢复通知换有界最新值槽；保留两种触发器各自防抖与授权来源。
Risk: 丢上线目标、状态与类别错配、迟到旧回调覆盖新值、清槽丢失并发到达的状态。

Step 5:
File: Engine 装配、Router、session generation、原四个 adapter。
Change: 一次迁移全部生产调用方，删除旧 native 与 serve 入口；生命周期统一关闭两类 manager。
Risk: 同时运行新旧路径、仅测试接 Uni 而生产仍走原生分支。

Step 6:
File: 宿主/E2E/脚本/CI、长期剪贴板连接设计与执行计划。
Change: 运行旧新双向与直连/Relay 内容读回、故障和安全验收；完成本地 CI 再推送；通过后回写设计并归档。
Risk: 只看发送返回值、路径未强制 Relay、旧库被测试补丁修改、跳过项写成通过。

# 7. Edge Cases

| Scenario | Expected behavior | Implementation |
| --- | --- | --- |
| 无当前状态、无合法目标、没有订阅者 | 无网络/应用成功假象，不新建重试 | 原空结果规则；必要固定诊断 |
| A 当前轮发送，B/C/D 连续进入待发槽 | A 完成本轮；下一轮只取最新合法 D | 版本化单值槽，状态与类别原子更新 |
| 旧回调晚于新状态入槽 | 不倒退为旧激活 | 复用既有激活比较，不靠到达先后或猜时钟 |
| 4 个慢目标和第 5 个健康目标 | 活动目标不超过 4；第 5 个等空位后独立计时 | 有界 future 集合，无全轮 10 秒承诺 |
| 慢拉取及其收尾占满额度 | 状态网络接收仍有自己的额度；状态应用仍串行 | 两类 manager，不假称业务完全无等待 |
| 半帧、无开流、写侧流控停滞 | 入口起 10 秒内结束网络等待 | 绝对 deadline 覆盖准入与排队，不刷新 |
| 容量或等待队列满 | 有界失败，无 wire Busy 新值，不无限创建任务 | Uni 限额及原错误映射 |
| 锁定、未知/移除成员、存储读取失败 | 不产生无权限内容；错误来源可追溯 | 原业务 gate + Uni 围栏，不以在线状态授权 |
| 准备途中移除/切 Space/对端取消 | 关闭网络，已开始动作结算，后续动作不启动 | 停止控制、原许可、容量和 generation 联合持有 |
| 拉取数据损坏、摘要不匹配、仅有占位文件 | 不推进 register 或转播，不把不完整内容写成可用 | 原解密/哈希/完整可用性检查 |
| 旧端接收/发送、无内容/锁定/内部状态 | 原字节及关闭顺序互通 | 固定旧库双方向真实进程验收 |
| 大图片、文件/目录 | 封套有界；本体沿用原传输；最终读回完整 | 原 tickets、manifest 和物化路径，不偷换成只验文本 |
| 连续取消再请求、进程重启 | 收尾占额度，不能无限积累；重启不恢复内存队列 | 既有任务回收及触发机制，无新持久队列 |

# 8. Testing Strategy

## Unit Test

| 输入 | 操作 | 预期 |
| --- | --- | --- |
| 可控虚拟时钟、排队/拨号/读写分阶段延迟 | 依次消耗同一预算 | 每个入口 10 秒截止，阶段不续期；无消息重送 |
| 6 个目标，前两个阻塞，一个失败 | 启动一轮广播并释放屏障 | 最大并发 4，健康目标按空位推进，错误互不取消 |
| 当前 A 阻塞、快速输入 B/C/D及旧回调 | 观察槽和轮次，插入“取走同时更新”竞态 | 待发最多 1，类别一致，下一轮 D，不丢更新 |
| 上线多个设备并伴随本机恢复广播 | 执行两个触发器 | 不丢目标集，1500/300 毫秒静默规则保持，最多 4/轮 |
| 入站两条状态、第一条 pull 阻塞 | 放行第二条网络通知 | 业务 handle_one 并发始终 1，顺序和 register 规则保持 |
| fake 地址库/身份库/codec 失败 | 调用原业务 port | 分类正确、source 可下钻；不得一律 Offline |
| 单次协议同时多个 connect 调用 | 领取租约并发送 | 同一连接只被一个交换消费，结束不留池 |

## Integration Test

| 输入 | 操作 | 预期 |
| --- | --- | --- |
| 真实 iroh 双端与明确第二测试 backend | 完整四向 port 合同 | Application 无原生类型；第二 backend 只证明边界，真实 QUIC 另验证 |
| 真实 SQLite/加密存储、两个待发布文件 | 文件 A 的真实依赖操作开始后取消，再释放 | A 结算、B 不启动、不加密/响应，额度直到 A 结算才归还 |
| 填满拉取收尾额度 | 新状态与新拉取同时到达 | 状态接收可完成，新拉取排队/失败符合限额，关闭能看到未结算工作 |
| 请求排队后移除再添加同一 peer | 放行旧拨号/开流/响应 | 旧请求不能借新授权；迟到连接关闭，无越代业务结果 |
| 真实存储、文本/图片/文件/目录 fixture | 恢复→通知→必要时拉取→物化→宿主读回 | register 与系统写入合同、实际长度/哈希/目录成员完整；未走普通推送偷渡 |

## Regression Test

- 从原实现提取状态和 pull 冻结向量，新实现双向编解码保持原字节及既有非法输入分类。
- 固定旧 main 库分别作为状态发起端/接收端及 pull requester/holder；覆盖已有内容、不持有内容、锁定与拒绝。
- 独立进程直连与强制 Relay 均验证最终内容、实际路径与网络约束；旧新兼容至少双方向直连，Relay 新新另作强制门禁。
- 保留接收静音、发送关闭但合法拉取、未来时间戳、相同激活、不完整条目、系统写入失败与“失败不转播”用例。
- 原普通剪贴板、组更新和成员历史（包括 30 秒保留池）回归；取消一个本片请求不能误关闭其他协议连接。
- 观测从恢复历史这一实际入口检查触发、部分结果和结束；无 ACK 或跨端关联材料时不伪造“对端已应用”。

# 9. Acceptance Criteria

* [ ] 四个生产方向均使用 Uni；旧直接拨号/开流与被替换的内部 serve port 已删除。
* [ ] 状态与 pull 保持原 ALPN、字节、单次交换及结果语义，固定旧端双方角色均通过。
* [ ] 出入站 10 秒 deadline 覆盖入口后所有网络等待，未因阶段、排队或关闭屏障续期。
* [ ] 两种广播入口每轮最多 4 目标；恢复队列有界且保留最新，接收业务仍串行。
* [ ] 两类容量独立且总量明确；本地收尾不提前释放额度或 generation 许可，取消后不启动后续准备步骤。
* [ ] 撤销、重复添加、锁定、切 Space、暂停及关闭的在途场景不产生越权/越代结果。
* [ ] 拉取或系统应用失败时 register、转播与内容可用性符合原规则，无新增自动重试。
* [ ] 文本、图片、文件、目录完整读回通过；文件本体实现与期限未被本片替换。
* [ ] source chain、脱敏日志、业务结果及不能跨端确认的边界可核实。
* [ ] 本地对应 CI 与根门禁全部通过，再推送；最终 head 的线上必需检查通过。
* [ ] 工件含版本/二进制摘要、场景命令、通过/失败/跳过、实际路径及合成内容断言；物理设备与未跑矩阵不标通过。
* [ ] 长期设计回写、计划与 spec 归档、索引更新；不关闭整个 Issue #34。

# 10. Risks and Trade-offs

- 保留无 ACK 与无自动重试，意味着连接发送完成不能保证最终应用；一次短暂失败仍可能等到下一触发才恢复。这是已确认范围。
- 单次连接保持旧端兼容，握手次数未因本片下降；Uni 的价值是统一所有权、期限和容量，不能宣称已取得连接池收益。
- 每轮 4 目标减少串行阻塞，但目标数较多时仍分批；不同轮次/触发器的总量由 manager 控制，不伪装为全局 4。
- 入站串行保留现有写入顺序，慢拉取仍可能延迟后续状态应用。状态额度隔离只保证网络接收，不消除该业务等待。
- 独立配额扩大合计资源上限；16 MiB 封套还可能伴随重建副本。实施必须用边界测试核实内存和任务数量，而非仅验证成功率。
- 取消后本地结算可能超过网络预算；直接 abort 全部任务会破坏已开始能力的责任，detach 整条流程又会启动不必要后续工作，均不采用。
- 把内部 serve port 迁到 Application 会涉及装配签名和测试，但避免把取消和准备流程塞入 Core 或 Engine。
- 不扩展 wire trace 字段导致部分跨端记录不能关联；接受独立接收诊断，不通过敏感数据推断关联。

# 11. Open Questions

本轮产品与行为边界已确认，无待用户决定的阻塞项。以下属于开工时必须产出证据的事实，不得预写“通过”：

- 实现启动时最新 main 的精确 SHA、旧库和测试驱动的摘要；若与当前审计基线不同，先检查相关差异再冻结。
- 真实 blob 发布依赖的取消/收尾边界及其临时资源回收：必须通过两文件屏障测试确认，不能仅据 async 函数返回推断。
- 旧端测试驱动入口：现有固定宿主缺少恢复命令，按执行计划为同一测试驱动分别链接冻结旧 Engine 和当前 Engine，
  明确标注测试宿主来源；不得修改旧 Engine 实现或把当前新增命令发送给不支持它的旧 binary。
- 本地与 CI 可执行的平台范围、物理设备可用性；不可用的设备矩阵只记跳过，不影响既定 Linux 必需验收的实现责任。
