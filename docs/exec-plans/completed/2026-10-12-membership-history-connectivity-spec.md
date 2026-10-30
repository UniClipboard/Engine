# Issue #34 第四切片：成员历史完整迁移与连接复用规格

状态：**实现与已执行的必需验收完成，已归档**（2026-10-29 核对结算）；[PR #180](https://github.com/UniClipboard/Engine/pull/180) 已合并，最终同 head CI、原失败及跳过项见[执行计划](2026-10-12-membership-history-connectivity-plan.md#最终线上验收与归档结算2026-10-29)。
基线：`origin/main edf0a296`，第三切片合并提交为 `247490af`。
[执行计划](2026-10-12-membership-history-connectivity-plan.md)负责实施顺序、门禁与证据；本文是本切片设计的唯一来源。

# 1. Overview

当前成员历史适配器直接持有 iroh Endpoint，每次历史消息重新拨号、打开一个双向流、交换后关闭连接。
第三切片已把组更新收发接入 Uni，并通过成员历史 `/4` 逐页恢复缺失更新；历史传输本身仍在 Uni 之外。
因此一次摘要和连续多页后缀会重复建立连接，历史连接的容量、取消及关闭责任也与已迁移协议分离。

本切片把全部成员历史交换接入 Uni：新端通过明确协商的多流能力，在成员历史范围内复用连接；
旧端继续使用 `/4` 单次交换。消息体、逐页回执与可信历史提交规则保持原合同。连接复用减少握手，
不减少历史页之间的因果往返，也不代表完成组更新安装。

# 2. Goals

- 全部八种 `MembershipHistoryMessage` 的合法请求/响应路径经过 Uni，不遗留生产原生拨号路径。
- 新端间实际协商多流历史 ALPN；同方向、同 peer 的独立交换共用连接，不同业务协议使用独立连接管理实例。
- 同一后缀 transfer 按页等待既有 ACK，再发送下一页；保留每来源一个入站 transfer、peer 锁及 ledger CAS。
- 已验证身份的当前成员可跨轮保留连接，空闲 30 秒后释放；身份介绍和受限恢复完成交换后释放。
- 旧新双方向互通保持 `/4` 原始字节、状态、分页、签名和回执语义。
- 流取消不破坏同连接其他流；撤销、Space generation 交接和关闭能停止网络并结算已开始的本地工作。
- 真实 SQLite/MLS、独立进程直连与 relay-only、失败后持久恢复及强制 CI 都有明确通过证据。

# 3. Non-Goals

- 批量历史、bundle、新的后缀消息体版本，或把多页合并成一次业务确认。
- 与剪贴板、组更新、presence、文件、配对共享连接；将双向连接合并成一条或增加拨号胜者选举。
- 新的业务重试循环、更改退避、扩大同步一轮预算，或以收到历史 ACK 代替组更新安装成功。
- 更改成员持久化格式、身份目录的权威来源、签名验证、移除规则或稳定 Engine/移动绑定 API。
- 替换 iroh、增加 Nearby/AWDL、P2P 失败自动转 LAN；新增产品设置或观测专用 facade 查询。

# 4. Current Architecture Context

Component: 成员历史业务负责人
Path: `crates/uc-application/src/space/membership/worker/history_sync.rs`、`handle_history_message/`、`worker.rs`
Responsibility: 摘要比较、逐页同步、冲突/受限消息验证、可信记录提交、结果结算及恢复。
Relationship: 调用 Core 历史 port，由成员 owner 保存加密记录、CAS 与待完成效果；维护 runtime 只调度。

Component: 原生历史传输
Path: `crates/uc-infra-p2p/src/network/iroh/membership_history_exchange_adapter.rs`
Responsibility: `/4` 拨号、framing、身份绑定、调用业务端点、返回状态、受限投递及已验证地址刷新。
Relationship: 当前直接接触 iroh 流；身份未解析时，仅 SummaryV3/SuffixPageV4 可通过 sender_admission 介绍身份。

Component: Uni 连接能力
Path: `crates/uc-application/src/connectivity/{manager,connection,stream,admission,backend}.rs`
Responsibility: 有界拨号/等待、请求撤销快照、连接和流使用权、容量、取消、关闭。
Relationship: 当前 outgoing active 表是 Weak，最后一份 lease 释放即关闭；尚无跨轮强引用空闲池。
入站连接不提升为本机 outgoing 连接。现有 grant 要求已知 DeviceId，不能直接表达未知身份的首帧介绍。

Component: iroh Uni 实现与会话交接
Path: `crates/uc-infra-p2p/src/network/iroh/connectivity/`、`admitted_connections.rs`、`session_generation.rs`、`node.rs`
Responsibility: 实际 ALPN 能力、地址读取、TLS 身份、撤销登记、协议路由及 generation 交接。
Relationship: 当前 Clipboard/GroupUpdate 准入要求普通成员授权；直接复用会拒绝合法受限历史路径。

Component: 消息规则与持久状态
Path: `crates/uc-core/src/membership/membership_history.rs`、`ports.rs`、`versioned_membership_history/`、`ledger/peer_link.rs`；`crates/uc-application/src/space/membership/record/`
Responsibility: 八种消息、回执、历史证明与限制、持久同步债务及退避。
Relationship: 本切片复用消息定义和当前加密成员记录，不增加连接池持久字段。

Component: wire 与装配
Path: `crates/uc-sync-protocol/src/`；`crates/uc-engine/src/assembly/`
Responsibility: 前者承载无业务依赖的 codec；后者装配完整 port 及其领域观测 decorator。
Relationship: 历史 codec 尚在 Infra，需机械提取；Engine 不接管分页、授权或恢复流程。

当前主链为：维护唤醒 → HistorySynchronizer → 原生历史 adapter → iroh 单流 → 身份绑定 →
HandleMembershipHistoryMessageUseCase → 验证/持久化 → ACK → owner 结算。受限投递使用同一 adapter，
不能只迁移常规同步而遗漏移除通知和决定。

# 5. Proposed Design

## Components

| 组件 | 职责、输入与输出 | 边界与原因 |
| --- | --- | --- |
| `uc-sync-protocol::membership_history`（新增） | 从通用 AsyncRead/AsyncWrite 读写请求封套、状态和响应；输出完整消息或结构化 codec 错误 | 唯一 codec；不含身份、拨号、业务重试或观测装配 |
| 成员模块私有 `history_transport/`（新增） | 完整 exchange、完整入站 receive/respond、受限投递映射；输入消息与 Uni lease，输出原业务结果 | 保持一个成员流程负责人；按流程、port、模型、错误、测试拆文件，mod.rs 只作入口 |
| 历史专用 Uni manager | 历史连接拨号、流容量、空闲持有、方向隔离和关闭 | 共用 Uni 实现，独立实例/策略；其他协议不启用本次空闲池 |
| Infra 历史身份能力 | TLS 公钥到既有目录身份的解析，或按原白名单绑定首帧介绍声明；输出明确区分的身份材料 | 绑定公钥不等于成员授权，不复制可信成员集合 |
| Application 历史授权/保留判定 | 每条消息交既有业务用例验证；只有目录已验证且当前成员的连接可保留 | 业务可接收、连接可保留是两项不同判断，防止剪贴板 gate 阻断恢复 |
| iroh profile/handler 与 Engine 装配 | 注册新旧 ALPN、包装 native connection、转交完整端点、跟随会话交接 | native 类型只在 Infra；Engine 只接线完整能力 |

规则归 Core 历史/ledger，流程和恢复归 Application 成员 owner；TLS、路由和流包装归 Infra，
连接生命周期与容量归 Uni。30 秒空闲回收和淘汰顺序是资源策略，不是成员资格 heuristic。
授权失败不能通过“设备曾在线”“上次有 ACK”或“连接还活着”推断为成功。

## Data Model

### 协商与 wire

本规格指定新 ALPN 为 `uniclipboard/membership-history/5`，旧 ALPN 为 `uniclipboard/membership-history/4`。
`/5` 表示连接允许多次独立双向流交换，**消息封套 WIRE_VERSION 仍为 4**；不增加 V5 历史消息。
命名沿用仓库 Clipboard 的能力协商方式，常量放在唯一 codec 模块；实现前以冻结向量确认这一分离。

| 协商结果 | 模式 | 行为 |
| --- | --- | --- |
| 历史 `/5` | Multiplexed | 每个双向流一条请求及一条响应，多流独立结算 |
| 历史 `/4` | SingleExchange | 一连接仅一条请求/响应，结束关闭；不入跨轮空闲池 |
| 其他 ALPN | Unsupported | 拒绝，不猜测对方版本或在已发请求后换协议重送 |

同一次 TLS 协商提供 preferred `/5` 和 fallback `/4`。保留原 length-u32-BE、请求 `4 + UCT1 + postcard`
封套及 trace_context，响应状态 ACCEPTED=1、REJECTED=2、BUSY=3 和 `4 + postcard` 响应体。
仅新 `/5` 增加传输状态 AcceptedClosing=4，随后仍是同一 length-u32-BE 与 V4 响应体：
表示本次消息已接受、但连接不再供下一交换使用。`/4` 绝不发送或接受此状态。
原因是两端成员视图可能不同，发送方自己的资格不能推断接收方的保留决定；没有明确提示会把
后续页发在已经结束的临时连接上，反复 Deferred。提示只能限制连接使用，不能授予成员资格。
状态对应的 body 有无、FIN 和完整消费要求从原实现冻结。长度 0 与超过既有
`MAX_MEMBERSHIP_HISTORY_FRAME_SIZE`（4 MiB）的输入在分配前拒绝；保留原消息判别值顺序。
八种消息为 SummaryV3、RequestSuffixV3、SuffixPageV4、AckV3、RestrictedEventV3、
RestrictedDecisionV3、RequestConflictEvidenceV3、ConflictEvidenceV3；请求/响应角色仍按既有用例，
不能因为 codec 支持某 variant 就允许把 response-only 消息作为主动请求。

### 内存状态与容量

连接索引为历史 manager 内的 `(generation, verified peer, direction)`；direction 是本机 Outbound 或 Inbound。
A→B 与 B→A 各一条多流连接。入站多流连接在原 handler 生命周期内保留，不进入 outgoing 表。
同方向重复拨号由原 flight 合并；重复入站先取得有界握手额度，完成身份/资格确认后保留先登记的可用连接，
关闭后来者，不关闭正在工作的先登记连接。单次/临时连接不参与长期保留表。

保留记录含连接强引用、活动使用权计数、最后活动结束时刻、失效标记和撤销快照；
活动交换和已登记等待请求共同计入使用权。计数归零才启动 30 秒空闲期限，新请求原子清除旧期限。
定时器在同一锁内核对计数及空闲起点，标记失效并关闭 transport，再清理表项；
失效使用权不能被新请求取得，避免新请求与旧 timer 竞态。
等待 accept_bi 不算活动请求，也不能占全局活跃流额度；未发送首帧的已开流在首帧期限内受有界额度约束。

历史策略沿用 Uni 当前界限：每 peer 出站流 4、每连接入站流 4、会话入站流 16、
每 peer 等待 32、会话等待 128、会话拨号 flight 64。新增保留连接上限 **会话合计 16 条，双方向合计计数**，
作为本规格的内部资源策略：上限提供硬界，不与成员数量无限增长。满额先按最早空闲回收；
全部忙碌时不登记新保留项，该连接本次交换后释放，不能驱逐活动流来腾位。
容量取得、排队和未完成首帧全部有取消和 deadline，不把未验证 peer 先加入无界 map。

连接池不持久化；身份材料、消息和 peer 索引仅用于内存处理，不进入日志或公开错误。
缓存条目不是身份目录，不以介绍声明建立永久目录映射。

## API / Interface

保留 Core 的完整业务边界：

| 接口 | 输入 → 输出 | 错误与责任 |
| --- | --- | --- |
| `exchange_membership_history` | target DeviceId、MembershipHistoryMessage → MembershipHistoryMessage | 保留 MembershipHistoryExchangeError；本地/传输转换保留 source chain |
| `handle_membership_history_exchange` | 绑定的 source DeviceId、完整消息 → 完整响应 | 仍由成员业务用例决定授权、拒绝、BUSY 及持久进度 |
| `RestrictedMembershipDeliveryPort` | 指定受限 Event/Decision → 原交付结果 | 只接受既有 RestrictedApplied/RestrictedConsistent ACK，不另定义成功 |
| `refresh_verified_peer_address` | 已由 owner 确认的 peer → `()` | 保留原 best-effort 刷新及固定分类诊断，不新增业务回执 |

新增内部接口按以下合同实现，不向 uc-engine 稳定入口导出：

- `ConnectivityProfile::MembershipHistory` 选择 preferred/fallback；ConnectionProtocol 增加两种实际历史能力。
- Uni 增加 `accept_pending(connection, deadline)`：先包装 TLS connection 并持有有界入站额度，
  返回 `PendingInboundLease`，允许一次首帧读入；它仅有 transport 身份、generation 和首帧前冻结的
  InboundAdmissionFence，不能伪造 DeviceId grant。身份识别后须核对原撤销 cutoff。
  本接口是通用的有限首帧能力，消息解码与业务判断由完整历史端点承担。
- 历史私有 `HistoryPeerBindingPort::bind(connection, first_message)` 返回 `Box<dyn AdmissionGrant>`；
  grant 内部明确区分目录已验证和仅介绍候选。仅 IdentityUnresolved 可介绍，且只允许原 SummaryV3/SuffixPageV4 白名单。
- `PendingInboundLease::bind(grant)` 消费首帧使用权并返回 ConnectionLease/首个 StreamLease，身份识别后核对
  入站前冻结的撤销 cutoff。候选只登记用于关闭的 transport 标签，不写身份目录，不取得保留资格。
- 历史私有 `HistoryConnectionRetentionPort::may_retain(peer)` 读取成员 owner 当前签名历史；
  `AdmissionGrant::can_retain` 同时重检目录身份和撤销。Uni `retain(lease)` 消费不透明 lease 自身的资格，
  原子登记有界强引用，没有供调用方伪造的授权布尔 setter。失败/超时仅使用临时连接，不能改写已计算业务结果。
- `HistoryWirePort` 接收完整请求/响应，内部 `HistoryReply { message, reusable }` 把 `/5` 连接提示交完整负责人；
  Core 的 exchange/endpoint 仍只返回 MembershipHistoryMessage。提示不进入业务结果、Core 记录或稳定 Engine API。

这些接口位于成员内部 transport 模块和 Uni 生命周期模块；合同不能退回裸 iroh handler。
对外调用方只提交完整 exchange 或完整接收动作，不负责拼身份解析、流写入和业务提交。
对于 outbound，target 和撤销快照在排队前冻结；实际握手公钥必须绑定目标，身份介绍例外不允许把
响应中的自报 DeviceId 当成期望目标。期望传输公钥取自唯一 PeerAddressResolver 读取的
EndpointAddr.id，冻结该拨号输入，iroh 握手必须认证这个 EndpointId；DirectoryVerified 资格另从
身份目录取得。目录尚未完成介绍时，可使用既有目标地址材料做临时历史交换，但不能保留连接；
地址缺失返回 Offline，读取/解码失败保留 source chain，不能把失败静默吞成“地址不存在”。

## Workflow

### 出站与复用

1. 既有 owner 选择同步目标或生成受限投递，调用一次完整历史 port。历史同步维持每轮 10 秒总预算、
   每轮最多 8 peer、最多并行 4 peer、同 peer 完整同步锁；transport 不能每页重置 round deadline。
2. 历史 manager 在排队前冻结请求围栏；在当前资格与连接健康检查后获取同方向可用连接，或有界拨号。
   单次交换的内部 deadline 不超过原 10 秒，并贯穿排队、握手、首帧、写入、读回执；owner 的剩余
   round budget 更短时优先截止。受限投递同样有界，不新增无限写入。
3. 按实际 ALPN 开一个 stream；写一次请求并 FIN，读响应状态和 V4 消息；同步上下文按该流隔离。
   `/5` 收到 AcceptedClosing 后立即结束本连接，下一 RPC 重新获取；仅 Accepted 才可申请保留，仍须
   读取本机当前成员权威。旧 `/4` 维持一次交换，无新传输状态。
4. 未开始写请求前发现缓存连接失效，可在原 deadline 内重新获取一次；从首次写入调用开始按“可能已发送”
   处理，连接层不重送，即使随后只观察到 write/read 错误也返回原失败。
5. owner 按原规则验证响应。SuffixPageV4 第 i 页获得合法 Continue/Confirmed 后才推进；错误 ACK 或
   错 transfer/position 不结清债务。独立业务请求可在 Uni 限额内并行，不能取消 peer 锁来并行同一同步链。
6. 接收端在写响应前决定本连接是否保留，并选择 `/5` 的 Accepted/AcceptedClosing；不更改业务 ACK。
   临时响应 FIN 后等发送确认或对端停止接受，再立即关闭，不等待对端主动关闭整条连接。
   流结算释放活动额度。仅 `/5`、DirectoryVerified 且当前成员通过保留判定后进入空闲保留；
   `/4`、介绍身份、已移除或受限 peer 的临时交换结束关闭。受限消息若对端确为当前成员，仍逐消息授权。
7. 失败按原 owner 结算 Deferred/Rejected 等结果；已持久进度保留。HistorySyncCompleted 的 transient
   defer 按 ledger 退避：首次失败记录后 1 秒，随后 2/4/8/16…最多 300 秒；成功重置。
   worker 按最早 due_at 安排下一轮，不必等 30 秒周期扫描；运行中轮次或暂停可使实际开始更晚。

### 入站、授权与取消

1. Infra Router 注册 `/5` 与 `/4`，包装 native connection，把完整 lease 交 Application 历史端点。
   Uni 在读首帧前就计入有界 pending 容量；首帧最长 10 秒且受 generation 取消。
2. 从 TLS 公钥和首帧取得明确身份材料；仅原白名单允许介绍身份。失败按旧状态有界拒绝并释放。
   未通过绑定前不采纳远端 trace parent。
3. `/5` handler 循环 accept_bi，每流独立请求上下文、超时、业务结果与响应；`/4` 只处理一次。
   已介绍身份的临时连接只处理首个交换，不能借更多流扩大权限。
4. 每条消息均进入原 HandleMembershipHistoryMessageUseCase；不能把首条成功缓存成后续消息授权。
   原 execution_lock、单来源 transfer、签名/lineage/移除验证与持久提交顺序保留。
5. 一流 decode/业务拒绝/取消只终止该流；已确认的整连接身份失效、peer 撤销或 generation 失效关闭整连接。
   受限恢复在新围栏下可新建临时连接，旧已撤销 lease 不能重新激活。
6. 接收业务提交使用既有 Space 工作 permit；一旦进入不可取消本地提交，复用第三切片 local-work
   交接机制，让操作结算后释放 generation。停止网络等待不丢弃已开始的 SQLite/MLS 工作；不新建第二套登记。
7. 当前成员入站连接等待后续流时按相同 30 秒规则与全会话保留上限回收；正常结束不再无界等待
   `connection.closed()`。shutdown/quiesce 先禁止新请求、关连接、取消等待，再 drain owned tasks/local work。

### 观测

恢复动作仍由成员 owner 表达触发原因、已一致/冲突/延期/失败及后续恢复责任。Engine 只装饰既有完整
历史 port；首帧、页和开流是动作内部工作，不创建独立业务 root，不为池事件扩大 facade。
每个并行流独立传播既有不透明上下文；连接建立、复用和回收只作固定分类运行诊断。
测试诊断与产品记录隔离，遵循[业务记录组织标准](../../design-docs/observability.md#业务记录组织标准)。

# 6. Implementation Plan

| Step | File | Change | 风险 |
| --- | --- | --- | --- |
| 1 | 原 adapter、connectivity/admission、真实宿主测试 | 固定旧向量、身份/授权矩阵、outbound 公钥绑定和取消断点；冻结内部 pending/bind/retain 合同 | 把介绍身份误当已验证成员 |
| 2 | uc-sync-protocol/src/membership_history.rs | 提取唯一通用 codec，原适配器暂委托它；新旧 wire 完全相同 | postcard 判别值/封套改变 |
| 3 | connectivity/{manager,connection,stream,admission} 及 Infra 实现 | 有界 pending、历史 scope、强引用空闲池、计数/代次、同方向唯一与复用 | timer/撤销竞态、无界 peer/任务 |
| 4 | space/membership/history_transport/、handle_history_message/ | 完整出入站与受限路径，逐消息授权，deadline/local-work，保持旧 port | 页乱序、取消遗弃提交、错误字符串化 |
| 5 | iroh/{node,session_generation,connectivity}、Engine assembly | 双 ALPN 生产注册、独立 manager、shutdown/撤销/地址刷新、删除原 native 历史业务路径 | 测试接线与生产接线分叉 |
| 6 | 现有独立进程/testkit 与 .github/workflows/pr-check.yml | 增加必跑历史多流/复用/双向旧新/失败恢复用例，跑完整本地 CI 等价门禁 | 只看 mock、不验证实际协商/连接数 |
| 7 | 对应设计文档与本计划 | 按实际事实回写稳定设计、逐项证据/跳过项，完成后归档 | 文档先声称未实现行为已完成 |

阶段与命令见[执行计划](2026-10-12-membership-history-connectivity-plan.md)。开发中的暂时委托只能是迁移步骤，
最终生产必须唯一 codec、唯一历史流程路径，不长期保留新旧业务实现。

# 7. Edge Cases

| Scenario | Expected behavior | Implementation |
| --- | --- | --- |
| 无历史差异/空后缀 | 返回原 Consistent/ACK，不能虚构一页或更改回执 | 原 owner 规则，codec 保留空字段 |
| 同时摘要、受限消息和多个流 | 限额内独立结算，流 A 取消不影响 B | per-stream context/lease/permit；peer 锁仍覆盖同一同步链 |
| 同来源两个 suffix transfer | 保留原单 active transfer/合法替换规则，不交叉提交 | 既有 execution_lock、transfer 检查、CAS |
| 空闲到期同时来了请求 | 只有一种状态迁移生效，不复用已关闭 lease | 原子活动计数和 timer 代次 |
| 池满且全部有活动流 | 新连接只做本次交换，现有活动流不被淘汰 | 合计 16 条保留上限，按空闲顺序淘汰 |
| 首帧不来/极长长度/非法版本 | 有界拒绝或关闭，分配和任务不无限增长 | pending 额度、deadline、先校验长度 |
| Directory IdentityUnresolved | 仅合法介绍消息可继续，仍须业务验证，不能跨轮保留 | 原 fingerprint 白名单及临时 scope |
| 其他目录失败/公钥不匹配 | 不走介绍 fallback，不调用业务 owner | 结构化身份失败与固定拒绝分类 |
| 已移除 peer 的合法 restricted 恢复 | 原业务规则允许时交换，结束释放；不能普通缓存复用 | 历史专用 scope、每消息授权 |
| 同步预算耗尽且缓存连接未报告关闭 | 淘汰复用资格，在途兄弟流结算；下一原业务轮次新拨号，不能透明重送 | 私有轮次截止点与 Uni evict；排空期间允许旧连接短暂存在 |
| 已发送但 ACK 丢失 | Deferred，保留债务，后续轮次处理重复并返回既有 ACK | 不透明重送禁止，原 durable transfer/replay |
| 已提交页后断线/重启 | 保留 staging、可信记录/效果和债务，从原 owner 恢复 | 无新增连接池持久化；真实 SQLite 测试 |
| 撤销后恢复重新联系 | 旧请求快照失败，新受限请求按当前围栏处理 | 复用既有撤销登记；不复活旧 grant |
| Space 切换中本地提交 | 停止旧网络，等已开始本地工作结算，再释放旧 generation | 原 local-work/Space permit 交接 |
| 两端成员视图不同/池已满 | 正常业务回复仍成功，本次连接关闭，后续页不陷入失效连接循环 | `/5` AcceptedClosing，`/4` 单次模式；双连续真实 RPC 测试 |
| 旧端不支持 `/5` | 握手协商 `/4`，一次交换后关连接，全部业务消息仍可互通 | fallback ALPN；固定未修改旧端测试 |
| transport 健康但资格读取失败 | 不保留；返回/记录原来源错误，不猜测授权 | 当前权威判定和 source chain |

# 8. Testing Strategy

## Unit Test

| 输入 | 操作 | 预期结果 |
| --- | --- | --- |
| 原实现冻结的八种消息/状态向量 | 新 codec 编解码 | 字节一致；trace 封套、判别值和 FIN 合同一致 |
| 0、边界、超界长度及截断/非法 postcard | 用受控 reader 读取 | 先拒绝长度再分配；错误类别和 source 可检查 |
| 同方向并发请求、30 秒虚拟时钟 | 取得/释放/重新请求 lease | 一条保留连接，多流限额有效；29 秒仍在、满 30 秒无活动才回收 |
| 第 16/17 条保留连接、全部忙碌 | 登记、取消与清理 | 硬上限守住，忙碌连接不被驱逐，额度最终归零 |
| 网络取消但本地工作未结束 | 丢弃网络流并发起下一交换 | 原全局额度仍被工作持有，结算后才归还，不积累无界工作 |
| 取消一流/等待者、空闲 timer 与新请求竞态 | 受控调度交错 | 另一流成功，无 permit 泄漏、无复用已关闭对象 |
| 目录已验证/未解析/其他失败与介绍变体 | 绑定并授权 | 只允许原介绍白名单；candidate 不入权威目录/空闲池 |
| 资格变化、撤销、generation 改变 | 复用旧 lease、提交 retain permit | 旧使用权失败，连接被退休，新 restricted 请求单独验证 |
| 发送前故障/首写开始后故障 | 获取缓存连接并交换 | 前者最多一次重获且同 deadline；后者零透明重送 |

## Integration Test

| 输入 | 操作 | 预期结果 |
| --- | --- | --- |
| 两个独立进程、真实 Endpoint/SQLite/MLS | 多页同步及 30 秒内下一次同步 | 实际 `/5`；逐页 ACK 正确；同方向实际一连接，不仅断言调用次数 |
| 两端同时同步及独立受限消息 | 双向并发交换 | A→B/B→A 各一条；相同方向独立流可并行，因果页不并行 |
| 同一连接两个请求，一流故障 | reset/丢弃一流 | 另流完整成功；未确认债务后续按原退避恢复 |
| direct 与 relay-only 独立组 | 限制网络候选分别运行 | 两组都证明协商、复用、失败隔离；P2P 不自动转 LAN |
| 未解析合法介绍、移除/决定/冲突证据 | 执行真实完整消息路径 | 原授权/签名行为不变；非当前成员结束释放；无错误池化 |
| ACK 丢失、页持久后 kill、重启 | 故障注入并重开相同数据 | 原债务/transfer/效果恢复，最终真实可信确认，无重复错误安装 |
| 有在途流/已开始本地提交 | 移除、切 Space、shutdown | 旧网络关闭，不遗弃提交；join/drain/额度回收有界 |
| 新宿主与固定 edf0a296 旧宿主 | 双方向摘要、后缀、冲突及受限交换 | 旧端未改；实际 `/4`、单交换、字节/回执语义一致 |

使用现有独立进程 harness/testkit；测试私有探针可以读取连接序号、实际 ALPN、活动流与池计数，
不新增生产 Engine 查询，不把 peer/public key/内容加入产品日志。证据必须来自实际建立的连接和
实际业务持久结果；“拨号函数调用一次”不能单独证明复用。

## Regression Test

运行完整 workspace、evidence、membership-e2e、connectivity、upgrade-matrix smoke 和根门禁。
重点验证第三切片真实组更新安装/拒绝恢复、Clipboard 多流/取消/重启、配对与受限身份、旧格式启动、
地址刷新和跨协议隔离。失败先保留全套证据再诊断，隔离通过不能替代原失败解释；不扩大超时掩盖竞态。
桌面/移动设备的网络切换、休眠和前后台实际未跑项单列“跳过”，不算本次 Linux CI 证据。

# 9. Acceptance Criteria

- [x] 八种消息的全部合法路径接入 Uni；生产原生历史拨号/codec/独立业务 handler 被删除或收口为纯 Infra wrapper。
- [x] 新端实际 `/5` 多流、固定旧端双方向实际 `/4` 通过，消息体仍为 WIRE_VERSION=4，无批量协议。
- [x] 同方向一连接、反方向独立、跨业务协议隔离；多页 ACK 顺序及同来源单 transfer 保持。
- [x] 30 秒空闲回收、16 条合计保留上限、等待/流/首帧界限及取消后额度归零具备确定性证据。
- [x] 当前成员才可保留；身份介绍、受限恢复、目录故障、公钥不匹配和逐消息撤销矩阵通过。
- [x] 首写后不透明重送；原 10 秒 round budget 与 1 秒起步、最大 300 秒历史退避保持。
- [x] ACK 丢失、页持久后强杀和重启用真实 SQLite/MLS 恢复；历史 ACK 不冒充更新安装结果。
- [x] 直连与 relay-only 独立进程复用/失败隔离通过；generation/移除/shutdown 本地工作交接通过。
- [x] Engine 不扩大稳定 API/观测阶段查询；错误保留 source chain；敏感数据、日志及测试隔离合同通过。
- [x] 新用例进入强制 CI；本地最终完整 CI 等价门禁通过后才推送，线上同提交必需检查通过后才交付。
- [x] 证据包含命令、提交、退出码、旧宿主摘要、实际 ALPN/连接事实；设备未跑项明确跳过；稳定文档按事实回写。


# 10. Risks and Trade-offs

强引用空闲池减少握手，但增加 socket/任务持有；30 秒与 16 条合计上限限制成本，复用效果依请求间隔
而定，不能承诺每轮一定命中。弱引用只覆盖重叠请求，不能满足已确认跨轮复用需求。

双方向各一条避免拨号选举和业务之间抢所有权，代价是两端都主动联系时存在两条连接。
多流不能加速因果链页间 ACK；去掉等待会改变可信提交合同，因此批量问题留在范围外。

未知身份首帧和已移除恢复是当前 Uni grant 的真实缺口。有限 pending lease 与明确临时 scope
增加内部生命周期类型，但可以让所有网络容量都由 Uni 管理；继续裸 handler 会留下绕过点。
必须防止临时身份被注册为永久可信 peer，也不能把连接首条成功提升为所有消息授权。

`/5` ALPN 和 V4 body 的分离需要清晰常量与兼容证据；根据应用版本猜能力或失败后换协议重送
都会掩盖真实协商并造成重复业务。旧端保留单次交换，减少握手的收益仅新新路径具备。

取消不可取消提交时会有短暂 drain 等待；复用原 local-work 责任比新增独立提交任务可靠。
保留条目的准入/撤销与 timer 必须同一生命周期下原子结算，不能靠后续定期扫描才关闭失效连接。

# 11. Open Questions

- 物理桌面与 iOS/Android/HarmonyOS 设备、实际网络和前后台测试资源尚未确定；不阻塞本切片
  已接受的独立进程/CI 交付边界，实施时按实际资源列跳过，不假设覆盖。

30 秒、方向、完整迁移、成员历史独占池、新 ALPN/旧 fallback、逐页顺序与原退避均已确认；
实现命名与内部拆分不需要再次征求这些范围决策。上述身份事实若推翻规格安全前提，则先更新
具体受影响合同，再实现，不能将待验证条件当作已通过。
