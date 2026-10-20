# Issue #34 第六切片：设备可达性、准入与存活检查连接规格

状态：**生产迁移及本地行为回归完成，完整网络／平台／线上验收进行中**（2026-10-15）。
本文落实本轮 grilling 共识；内部类型名称和文件划分是基于代码的工程设计。
审计时最新 main 为 `68de511a77dee74b8b6ba0360fa07f9637241c90`，第五切片 [PR #181](https://github.com/UniClipboard/Engine/pull/181) 已合并。
实现开工时重新冻结最新 main，不使用可漂移的分支名作为旧端锚点。
[执行计划](2026-10-14-peer-reachability-connectivity-plan.md)维护顺序、门禁和实际证据，设计正文以本文为准。

# 1. Overview

本片处理设备列表里的 Online／Offline／Unknown，以及支撑该判断的 presence 连接、成员准入和存活确认。
例如 A 与 B 已配对，A 自动联系 B；B 核验 A 的当前成员资格后回复，双方保留连接并持续识别失联与恢复。

Uni 是本仓 `uc-application/connectivity` 的连接接口和管理模块，iroh 仍提供底层 QUIC、直连与 Relay。
“完整迁移”要求发送、接收、准入、存活确认、取消、撤销及关闭全部由同一 Application 业务模块通过 Uni 完成。
Infra 只保留实际联网与协议桥，删除被替换的原生 presence 流程。

当前 Application 已负责自动连接调度，但 `IrohPeerReachabilityAdapter` 仍决定准入、在线证据与长期连接保留。
入站读取和回复分别超时，没有覆盖身份／成员读取的统一准入期限；等待准入额度也没有在首流之前限制。
本机资格读取失败被 `.is_ok()` 合并为明确拒绝，会把本机故障错误传播为对端无资格或 Offline。
本片在迁移时修正这些已确认的缺陷，保持正常在线判断、调度和线上协议。

# 2. Goals

- presence 主动连接与被动接收、v2 存活检查与应答、v1 关闭观察均使用 Uni，生产流程不再直接操作 iroh 连接／流。
- 完整流程及唯一在线证据表归 Application `space/connectivity/peer_connections`；保持稳定 Engine 查询、刷新、事件和机会通知接口。
- presence 独立 manager，复用现有管理实现及同一个设备 Endpoint，与剪贴板、成员历史的额度及连接池分开。
- 当前获准的 N 个远端，每端最多两条入站、一条出站长期连接，最多 3N；暂态拨号和等待准入另算。
- 保持全局 4 个完整目标尝试；等待入站准入全局 16、识别后每对端 2，满额立即拒绝且不影响健康连接。
- 入站完整准入绝对期限 20 秒，各网络步骤最多 5 秒；出站完整目标尝试仍为 20 秒，v2 存活轮次仍为 5 秒。
- 本机身份／资格读取失败保留最后有效显示状态，不产生新 Online 证据、不授权新连接，错误保留 source chain。
- 保持 `/2` 与 `/1` ALPN 和线上字节；真实旧新双角色覆盖 Direct／强制 Relay，v2 的三秒自动恢复进入本地和 CI 必需门禁。

# 3. Non-Goals

- 文件／blob 本体、反向进度通道迁移；完成整个 Issue #34；替换 iroh 或自动降级 LAN。
- 新配对协议、修复 rc.15 与新版现场配对不兼容、伪造旧资料成员资格、另加资料迁移格式。
- 扩大稳定 Rust／移动绑定接口，新增 Connecting／Degraded 状态，或者把产品重试责任交给调用者。
- 共用剪贴板／成员历史连接池、新固定全局长期连接上限、新空闲回收时长、新保活字节或业务 ACK。
- 改变正常目标选择、检查周期、退避、刷新合并、明确资格拒绝优先级和已准入第三条入站的替换规则。
- 用编译和 Linux 网络结果宣称 iOS／Android／HarmonyOS 真机切网及前后台验收通过；扩大全部历史版本矩阵。

# 4. Current Architecture Context

Component: 自动连接完整业务模块
Path: `crates/uc-application/src/space/connectivity/peer_connections/{mod,record,runtime,tests}.rs`
Responsibility: `PeerConnectionCoordinator` 负责目标、刷新、机会合并、周期检查、重试、暂停、恢复和关闭。
Relationship: 调用 Core `PeerReachabilityPort`；当前真正的连接准入与证据还留在 Infra。完整目标预算 20 秒，包括 1 秒资格读取；并发 4、检查周期 10 秒。

Component: 既有完整可达性合同
Path: `crates/uc-core/src/ports/peer_reachability.rs`
Responsibility: ensure／verify、通信失败报告、forget／disconnect_all／activate、纯读状态及订阅。
Relationship: 查询不拨号；通信失败仅请求检查，不能直接判 Offline、关闭健康连接或重放业务。

Component: 原生 presence 与活性流程
Path: `crates/uc-infra-p2p/src/network/iroh/peer_reachability_adapter.rs`、`peer_reachability_adapter/liveness.rs`、`peer_reachability_protocol.rs`
Responsibility: 原生拨号、首流准入、每端 2 入站／1 出站、在线证据、v2 challenge、v1 连接关闭观察。
Relationship: 共享状态表防止旧失败覆盖新成功；目前身份／准入错误与存活 IO 来源存在丢失，待准入限额取得过晚。

Component: 身份、成员权威及统一撤销
Path: `crates/uc-infra-p2p/src/network/iroh/{inbound_peer,admitted_connections,session_generation}.rs`、Application 当前成员范围能力
Responsibility: 认证公钥到设备身份、权威成员准入、连接登记、撤销和会话代次围栏。
Relationship: `PeerIdentityResolver` 已保留来源；旧 gate 的 `LedgerUnavailable` 丢失下层错误。迁移复用现有权威及登记，不另造成员表。

Component: Uni 连接管理
Path: `crates/uc-application/src/connectivity/`
Responsibility: 连接／流租约、有限等待、拨号合并、准入 grant、撤销与 task shutdown。
Relationship: 当前 `PendingInboundLease::bind` 把入站连接许可移入长期 record；空闲保留按方向只存一条且有固定容量，不直接满足 presence 的 2 入站／1 出站。

Component: 组装和真实环境验收
Path: `crates/uc-engine/src/assembly/sync_engine.rs`、`crates/uc-infra-p2p/src/network/iroh/node.rs`、`tests/hosts/connectivity/`、`scripts/testing/run-connection-recovery-e2e.sh`
Responsibility: Engine 装配完整能力；builder 注册 ALPN；独立宿主和隔离网络验收内容与路径。
Relationship: 当前 `install_peer_reachability` 构造原生 owner；`--mode legacy` 只证明 E09 现场配对拒绝，不能证明已配对 rc.15 的 presence 互通。

# 5. Proposed Design

## Components

| 组件 | 输入、输出和职责 | 原因与边界 |
| --- | --- | --- |
| `peer_connections/transport/`（新增内部模块） | 实现原 `PeerReachabilityPort`，完整处理入站；统一准入、证据、存活及连接保留 | 与 coordinator 同属一个业务负责人；按 owner、admission、liveness、ports、model、error、tests 拆分，`mod.rs` 仅声明和必要导出 |
| 既有 coordinator | 既有刷新／机会／成员变化输入；产出尝试结果并安排后续检查 | 保持调度单一来源，transport 不启动第二套退避或周期调度 |
| presence Uni manager | 通过既有 backend／grant 获取和登记租约；提供 pending、流及长期保留能力 | 独立实例、同 Endpoint；扩展明确资源生命周期策略，不复制另一套连接管理器 |
| Infra presence backend／Router bridge／wire | backend 提供原生连接事实；bridge 一次转交完整入站；wire 读写原字节并保留 IO 来源 | 不判断 Online、决定重试或维护业务证据；认证握手不等于成员获准 |
| 唯一纯 codec | `uc-sync-protocol` 内提取 presence 编解码及 ALPN 常量 | 当前协议模块混合原生 IO 与 codec；提取后 Application 通过专属 wire port 使用，不依赖 codec crate |

既有通用撤销表仍负责资源撤销；Application 的内存观察表只记录 reachability 证据，不能作为第二个授权来源。
暂态原始连接也须接入停止／代次控制，不能等获得 DeviceId 后才可被 shutdown 回收。

## Data Model

以下均为会话内存状态，不新增持久字段或格式。

| 模型 | 含义及生命周期 |
| --- | --- |
| Presence 观察表 | 每设备最后有效显示状态、事件、近期验证时间、成功修订；forget 或会话退出清除 |
| 在途观察快照 | 会话代次、设备资格 epoch、开始时成功／检查修订；结束时比较，旧失败不能覆盖较新成功 |
| Pending admission | 入口时刻、绝对 deadline、全局许可、识别后的每端许可、临时连接和取消围栏；成功原子转为已准入连接，失败／取消关闭并释放 |
| 已准入保留集合 | 每端入站最多 2、出站最多 1 的 Uni 租约；保持强引用到关闭、替换、撤销或退出，不因首轮 FIN 释放 |
| 存活轮次结果 | Alive、真实对端拒绝、网络失败／超时、本机资格读取失败、取消；错误分支携带 typed source，仅固定类别可输出日志 |

长期集合不保存永久撤销墓碑。已移除设备和旧会话在途结果靠现有 epoch／generation 失效，重复加入不复用旧 grant。
实现迁移原成功修订及并发保护，不另加旁路 Online 缓存。

## API / Interface

Core `PeerReachabilityPort` 和稳定 Engine／绑定接口保持原合同；其实现从原生 adapter 改为 Application 完整能力。

| 内部接口 | 输入／输出 | 失败与边界 |
| --- | --- | --- |
| 完整入站 `accept` | 已完成加密握手的 `Arc<dyn PeerConnection>`、入口时刻及停止控制；返回完整处理结果 | Router 不拼接识别／授权／回复；失败保留来源，释放临时连接；该方法不成为 Engine 的阶段 API |
| `PeerReachabilityWirePort` | Uni 半流、业务帧及绝对 deadline；完整读／写原准入和 challenge 帧 | 精确长度、类型、nonce、FIN 校验；IO／timeout 保留来源；不返回原生 stream |
| Uni 暂态准入与长期保留策略 | 未绑定临时连接转为当前 grant 的受管租约，按 presence 策略保留 | 入口先 try-acquire；失败不可排队；长期保留不继续占用 pending 许可 |
| 协商能力 | `ConnectionProtocol` 增加明确 v1／v2 presence 能力，由实际 ALPN 映射 | v1 保留连接且不发新版 challenge；v2 允许重复存活流；不按版本名或 Online 猜能力 |

`ConnectionMode::SingleExchange` 目前表示整个连接只供一次交换。不得把 v1 当成该模式后在准入 FIN 时关闭，
也不得将它伪报为支持 v2 多流。Uni 要显式区分“可用流能力”和“连接保留生命周期”，现有协议默认策略保持原义。
内部详细错误最终包装成既有 `PeerReachabilityError`，不能字符串化或借日志输出私有标识。

## 资源、期限与保留

| 资源／阶段 | 合同 | 获取与释放 |
| --- | --- | --- |
| 完整出站目标尝试 | 全局并发 4 | 沿用 coordinator；资格读取、已有连接核验及必要新拨号结算后释放，长期连接不持有这 4 个名额 |
| 待入站准入 | 全局 16，识别后每端 2 | 加密握手完成、Router 接到连接时先非等待获取全局许可，之后才等待首流／首帧；绑定身份立即获取每端许可 |
| 长期在线连接 | 当前获准 N 个远端，最多 3N | 2 入站／1 出站；第三条已获准入站按原选择规则替换，不改为拒绝第三条或先关健康连接再验证候选 |
| 每连接入站活性处理 | 最多 2 个并行请求 | 满额只 reset／stop 当前流，不排队，不关闭整条健康连接；自然随长期连接数增长，不套用固定 16 总额 |
| 入站完整准入 | 入口绝对 deadline = received_at + 20 秒 | 开流、读首帧、身份／资格读取、回复、重检、拒绝与关闭都在同一期限；网络步 deadline 取整体与 now + 5 秒较早值 |
| 出站完整尝试 | 20 秒，资格读取最多 1 秒 | 原 coordinator 外层期限包含整个调用，内层取消／超时不得重新延长；显式内部 deadline 由完整 owner 传递 |
| v2 存活轮次 | 5 秒 | 同轮候选连接并行确认，整个轮次共用期限；不是每连接顺序各等 5 秒 |

无首流／首帧的连接也占 pending 额度。全局满额立即关闭，未收到有效请求时不等待创建流发送拒绝。
每端满额在已读有效请求且剩余期限允许时可按原拒绝帧结束；不占用既有长期连接额度。
成功提升时在同一受控转换内先校验当前 grant／generation、登记长期租约，再释放 pending 许可；失败自动回收。
现有 `_inbound_connection` 许可不可原样留到长期连接上，否则 16 会变成长期总上限。

长期保留使用 presence owner 的受管强租约和明确策略，不套用成员历史的空闲超时／固定容量；首轮流结束不关闭租约。
没有无界独立任务：准入任务受 16 限制、出站尝试受 4 限制、活性任务按已准入连接每条 2 限制，均由 owner 等待回收。
超时后不开始后续网络步骤；确已开始且不能安全中断的本地读取由既有本地工作负责人结算，持有原许可直至实际结束，迟到结果不放行。

## Workflow

1. 成员变化、周期、手动刷新或既有恢复机会进入 coordinator；目标范围、合并和退避沿原规则。
2. 冻结当前观察和完整尝试期限，先检查权威资格。近期有效证据按原快速路径复用；需验证时先核验已有连接。
3. 已有连接不足以证明可达且当前仍获准时，通过 presence manager 拨号；保留 `/2` 优先、`/1` fallback 及既有网络恢复能力。
4. 远端 Router 入口取得 pending 许可及 20 秒 deadline；读取原请求、解析认证身份、核验当前资格，再回复。
5. 双方交接前重检 grant、代次与接受状态，登记并提升受管连接，更新唯一观察表并发出原状态事件；FIN 结束准入流而不结束长期连接。
6. 后续 v2 使用 nonce challenge 核验并服务请求；v1 只保留原连接关闭观察，不伪造新鲜 v2 确认证据。
7. 本次失败结算后由 coordinator 沿原规则安排重试。forget、切 Space、锁定／暂停及 shutdown 使在途许可失效，关闭并等待任务退出。

## 在线、失败与兼容

| 结果 | 本机显示／证据 | 对端回复及连接处理 |
| --- | --- | --- |
| 当前获准且验证成功 | 原规则 Online；只有真实 v2 确认刷新对应证据 | 保留受管长期连接；旧失败／旧关闭不覆盖本次成功 |
| 本机身份／资格查询出错或超时 | 本轮返回失败，保留最后有效状态；无历史则 Unknown，不更新时间 | 不伪发“无资格”；新连接关闭，已在线连接只结束当前存活流，保留完整 source |
| 权威资格明确不允许 | 原拒绝及撤销语义 | 沿原拒绝字节；撤销不被“保留显示”策略豁免 |
| 收到真实对端拒绝 | 保持原同轮拒绝优先级，不能由另一条成功抵消 | 原处理规则；区别于本机故障或格式错误 |
| 单连接失败／无效帧／超时 | 原规则结算；不让旧失败抹去其他较新有效连接证据 | 回收该失败交换／失效连接；不能把无效帧分类为明确资格拒绝 |
| 停止／旧 epoch／旧 generation | 不发布新 Online | 丢弃迟到结果并释放；未识别的暂态连接同样可停止 |

保持正常线上格式：ALPN `uniclipboard/presence/2` 和 `/1`；首轮请求 `0x01`、接受 `0x01`、拒绝 `0x02`；
v2 帧恰为 17 字节，类型 Request `0x10`、Accepted `0x11`、Rejected `0x12` 加原样回显的 16 字节 nonce，要求原 FIN 边界。
不增加第三种“本机错误”回复或附带观测字段。旧 codec golden 向量必须先取自修改前实现再迁移。

## 观测及安全

沿用[业务记录组织标准](../../design-docs/observability.md#业务记录组织标准)和[错误处理规范](../../design-docs/error-handling.md)。
明确触发的完整恢复动作记录触发、已完成工作、结果及后续恢复责任；例行检查、开流和正常清理属于运行诊断。
Application 完整 owner 提供既有不透明关联；Engine 只装饰完整 port 输入／输出，不读取步骤、状态表或业务标识。
跨层计时和分类由组装层 decorator 提供，任务 join 失败由等待方按固定类别记录。
使用 `uc_*!`，不得输出设备名／标识、地址、文件或资料路径、密钥、邀请、内容和原始错误正文。
真实测试只输出受控模式、版本／构建摘要、固定失败分类及合成内容的验收结果；配对资料和密钥不上传为 CI 工件。

# 6. Implementation Plan

顺序与退出条件见[执行计划](2026-10-14-peer-reachability-connectivity-plan.md)。

| 步骤 | 修改位置与内容 | 主要风险 |
| --- | --- | --- |
| 0 | 现有宿主和恢复驱动：冻结版本、真实旧旧配对、原资料单端升级、正常基线与 red 信号 | 旧资料升级可能不具备既有支持；未证明前不能修改生产实现 |
| 1 | `uc-sync-protocol`、Infra wire：提取唯一 codec，固定字节和 typed IO 来源 | 把 v1 误写为新版多流或改变 FIN／关闭语义 |
| 2 | `uc-application/connectivity`：分离 pending／长期额度，增加 presence 能力与受管保留策略 | 固定 16 持久占用、只留一条入站、改变其他协议默认生命周期 |
| 3 | `peer_connections/transport`：完整收发、身份／准入、观察表及错误分类 | 多份状态表、本机故障变拒绝、迟到结果越过撤销 |
| 4 | 同模块 liveness／lifecycle：v2 双向确认、v1 关闭观察、统一暂停关闭 | 单流故障误关整连接、任务逃逸、取消刷新误停共享工作 |
| 5 | Engine assembly、iroh builder／Router：装配唯一 owner，删除原生 presence 业务实现 | 只改测试而生产仍走旧 adapter；为观测泄露内部流程 |
| 6 | 恢复／互通 harness、PR workflow、长期设计：真实矩阵、本地必需 CI、最终证据 | 将 legacy 拒绝测试冒充互通、通过手动触发掩盖三秒失败、记录未测为通过 |

# 7. Edge Cases

| Scenario | Expected behavior | Implementation |
| --- | --- | --- |
| 无成员／无地址／从未核验 | 无目标保持空闲；无历史 Unknown；无地址按原合同处理 | 使用原成员范围和 NoAddress 合同；查询不产生连接 |
| 超过 4 个获准对端 | 分批尝试后全部可保持 Online，不因暂态并发限制逐出其他端 | 至少 6 对端测试，暂态尝试结束释放 4 个名额，长期按 3N |
| 大量握手后不发首流、半帧或慢本地查询 | 最多 16 pending，最迟入口 20 秒结束；每网络步不超过 5 秒 | 入口非等待许可、单绝对 deadline、真实工作结束前不虚报容量释放 |
| 同端 pending 已有 2／第三条已获准入站 | pending 满额拒绝；已准入候选按原规则替换一条长期连接 | 两种额度和转换分开，候选成功前健康连接不被驱逐 |
| 本机真实 SQLite 读失败／身份映射读取失败 | 保持最后有效显示，但当前检查失败；不发明确无资格 | typed source 路径及真实存储错误测试；禁止 `.is_ok()` 混同 |
| 错 nonce、超长／短帧、错类型、缺 FIN | 当前交换失败，不视为 Alive 或明确对端拒绝 | 唯一 codec、有限读取、剩余期限及来源分类 |
| 同轮一个真实拒绝另一个 Alive | 原明确拒绝优先级保持 | 聚合分类独立测试，不把本机资格失败加入 Rejected 分支 |
| 新成功与旧失败／关闭交错 | 新有效状态不被旧观察覆写 | 原成功修订、epoch 和检查修订语义迁移 |
| 移除再加、切 Space、锁定、暂停、关闭 | 旧工作不得重新上线或拿新 grant；健康共享工作不因单个刷新 waiter 取消而停止 | 原撤销围栏／代次、受管任务组；最终所有暂态／长期租约回收 |
| rc.15 已配对资料不可被新版正常读取 | Phase 0 阻塞，保留实际错误，不造资格或偷偷更改升级／配对规则 | 每角色从原始旧资料重新复制；先定位既有升级契约支持范围 |
| Relay 静默中断与恢复 | v2 原失联上界 20 秒，恢复后按三秒门；v1 原传输关闭上界 70 秒 | 独立计时，真实中转恢复证据；v1 不套新版三秒门 |

# 8. Testing Strategy

## Unit Test

| 输入 | 操作 | 预期结果 |
| --- | --- | --- |
| 修改前 golden 帧及畸形帧／nonce／FIN | 新纯 codec 编解码、分类 | 正常字节完全相同；格式错误与真实拒绝分开 |
| 测试第二 backend、16 个暂态及第 17 个、同端 3 个 | 完整准入、超时、取消及提升 | 不依赖 iroh；立即限流、许可无泄漏、长期集合不持 pending 许可 |
| 6 对端及 2 入站／1 出站每端 | 分批连接、替换、释放首轮流、后续检查 | 全部可保持 Online；至多 3N；v1 首轮后仍存活，v2 能重复确认 |
| 当前 grant／旧 epoch、本机读取错误、真实拒绝／成功并发 | 完整 owner 结算及调度 | source 可追溯，显示／证据符合结果矩阵，旧失败不覆盖新成功 |
| 受控时间、慢网络、慢查询、共享刷新取消 | 从入口执行完整尝试 | 单一 20 秒期限、网络步 5 秒、保留原调度常量，无共享工作误取消 |

## Integration Test

使用真实 Iroh、SQLite、当前 Engine 及独立进程；测试第二 backend 只证明接口可替换，不能代替真实网络。
存储故障从真实下层制造，断言顶层 source chain 和固定失败分类，不只返回手写错误。

| 组合 | 资料与角色 | Direct／强制 Relay | 恢复验收 |
| --- | --- | --- | --- |
| 新－新 | 双方正常配对，两个发起方向 | 两种路径，精确双向内容与最终读回 | v2 静默失联 ≤20 秒；Relay 三秒自动 Online 并完成双向内容 |
| 新－冻结 main | 各自担当 presence 主动方／接收方，记录实际 ALPN | 两角色 × 两种路径 | 双方实际协商 v2 时使用同一三秒门 |
| 新－a07／rc.15 | 两台真实旧版先正常配对；每角色独立复制原资料，只升级一端 | 两角色 × 两种路径 | 证明 `/1` 正常准入、连接保留、关闭与恢复；传输关闭 ≤70 秒，不强加 v2 三秒门 |

计时起点为两端重新建立到 Relay 服务的底层连接的较晚完成时刻；从该时刻到双方自动 Online 且双向内容精确读回不超过 3 秒。
上线前只做状态查询，不手动 refresh、不预先发内容／nudge、不完整重建会话来促成恢复。
隔离网络同时阻断 IPv4／IPv6 的 UDP 直连绕路，保存规则命中计数及实际 selected path；Online 或可达 IP 不能替代路径与内容证据。
新新沿用现有三秒门 20 轮；每个 v2 旧新角色至少 3 轮。网络／互通矩阵必须进入 PR 默认必需验证，不能只放可选 label 或 nightly。

真实资料建立顺序：旧 A／旧 B 正常配对并关闭，保存原始加密资料；复制后升级 A 验证新版主动，再重新复制原始资料升级另一角色验证新版接收。
不得手工插入成员／身份／密钥，不让旧端读取新版写过的资料来混入降级场景。保留原 E09 新旧现场配对拒绝测试。

## Regression Test

- 输入原 coordinator 测试和原恢复网络用例，执行机会合并、检查、退避、暂停／恢复、只读查询，预期原合同不变。
- 输入普通剪贴板、组更新、成员历史及当前状态／pull 的既有矩阵，执行完整回归，预期独立 manager 扩展不改变其容量、保留和取消语义。
- 输入恶意字段、真实资格读取错误和完整恢复动作，检查固定诊断分类、source、实际导出及列表／详情，预期隐私边界、结束时间和关联符合唯一观测标准。
- 执行完整 workspace、架构、格式、现有升级 smoke 与跨平台编译门禁；真机和额外历史版本未执行列跳过。

# 9. Acceptance Criteria

* [ ] 开工冻结最新 main／a07 的精确源码与驱动／二进制摘要；真实旧旧配对及原资料单端升级可执行，正常基线成立。
* [ ] 主动／被动准入、v2 存活双方、v1 长期关闭观察全部通过 Uni；原生业务流程删除，仅 bridge／backend 保留原生类型。
* [ ] 唯一完整负责人、唯一在线观察表、权威成员来源和统一撤销登记；Engine 不拼步骤或扩大观测接口。
* [ ] 超过 4 个对端仍可保持在线；2 入站／1 出站上限及原替换规则成立；长期连接不受固定 16 总额或空闲超时限制。
* [ ] 入站 pending 16／每端 2 在正确入口取得；满额立即拒绝，健康连接保留，取消／超时／提升后无许可泄漏。
* [ ] 出入站完整 20 秒、网络步骤与 v2 轮次 5 秒；不续期、不遗留绕过容量的实际本地工作。
* [ ] 本机身份／成员读取故障保留显示、失败本轮、不发无资格、不刷新证据；真实错误来源及明确拒绝优先级验证通过。
* [ ] 移除／再加、切 Space、锁定／暂停／关闭及共享 waiter 取消的竞态通过；旧结果不会重新上线。
* [ ] 原线上字节与 v1／v2 能力兼容；新新和两个真实旧锚点双角色 Direct／Relay 真实内容及路径矩阵通过。
* [ ] v2 失联上界及必需三秒自动恢复门通过；v1 原传输关闭上界通过；保留 E09 配对拒绝测试。
* [ ] 本地全部对应必需 CI 通过后再推送；最终提交线上必需检查通过，首次失败与最终证据完整留存。
* [ ] 稳定设计回写并归档；物理设备、额外历史矩阵及其他未执行项明确跳过；不关闭整个 Issue #34。

# 10. Risks and Trade-offs

- 独立 manager 增加装配对象和连接数量，但让 presence 故障与其他业务容量隔离；复用实现及 Endpoint 避免第二套底层网络。
- 动态 3N 保留成本随成员数增长，符合已确认在线合同；用 pending 16、目标 4、每连接活性 2 控制暂态压力，不暗加固定长期总额。
- 本机读取故障暂时保留旧 Online，显示可能滞后；本轮失败、无新证据／授权及正常存活／撤销约束防止它被解释为新鲜可用证明。
- v1 缺少 v2 challenge，关闭识别更慢；保留已配对旧版兼容，不通过伪造能力缩短其验收边界。
- Uni 当前 pending、单交换和空闲保留语义不同，扩展风险主要在其他协议回归；显式 presence 策略及默认行为测试比复制独立 manager 更可维护。
- rc.15 资料升级可能先于网络互通失败。该问题是必须取得真实证据的前置，不能用配对拒绝绿灯或人工资格替代。
- 三秒门受测试机负载、Relay 就绪观测和路径限制影响；保留首次失败、准确起点和重复证据，不放宽协议期限或使用手动触发使其通过。

# 11. Open Questions

已确认设计选择没有待回答项；以下事实必须在 Phase 0 实测，当前不能宣称通过：

1. 两台 a07／rc.15 能否在当前验收环境正常配对，真实资料能否由开工版本的既有升级流程读取并保留原成员关系？失败时先报告来源与阻塞，不扩大本片到资料升级修复。
2. 开工最新 main 的精确 SHA、实际 presence 能力、旧端统一测试驱动可用命令及二进制构建摘要是什么？记录冻结值，不沿用本文的审计时间值冒充实现基线。
3. 默认 CI 的隔离网络权限、混合版本资料入口与运行时长是否满足整个必需矩阵？先证明基线并把真实约束写入计划，不通过跳过必需格或只放 nightly 规避。
