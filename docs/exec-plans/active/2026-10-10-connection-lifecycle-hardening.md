# 1. Overview

状态：实施中。问题 1、2 已按先红后绿完成并提交；问题 4、5 待做；问题 3 另开 issue。基线：`193e9d27f0be4157a28d9bc065507e753d8188aa`；依赖 iroh 1.3.0。

本规格处理连接生命周期中的五个已识别问题，其中问题 3（内容发送复用连接）已确定移出本分支，另开 issue，本分支实际处理四项。它们来自对连接实现的调研与只读代码审查，其中单飞拨号与传输进度的缺陷已由代码阅读确认，其余结论是否成立由各自的复现测试决定。

[连接候选发布、网络恢复与路径观测](2026-10-10-connection-path-and-network-recovery.md)已完成地址发布过滤与连接 selected 路径，其余项（网络矩阵、拨号策略对比、真机验收）仍归该计划；本规格不重复。

核心规则：**每个问题先写能复现它的测试，并在未修改生产代码的基线上确认测试失败**（红）。测试在基线上通过，说明问题不成立：该项带着证据降级或关闭，不实施修复。

# 2. Goals

1. 成员被移除或离开 Space 后，该成员的所有已建立连接（入站与出站、不限 presence）都被关闭，在途的出站内容发送被中止，且不会继续投递事件或内容。
2. 内容发送可取消、有分阶段超时；调用方放弃等待不会让同目标的后续发送永久挂起。
3. （移出本分支）内容发送复用健康连接，另开 issue 跟踪；本分支只保留现状基线数据，不实现。
4. 网络变化、回到前台、系统唤醒后，已存对端地址会被刷新，而不只在历史同步后更新；成员被移除时，其地址记录一并删除。
5. 一次失败的内容发送最多产生 3 次拨号尝试；每次移除、暂停或关闭之后，不再有任何负责人继续拨号。

# 3. Non-Goals

- 不修改 wire 格式、ALPN、持久化格式，不新增协议或服务端设施；对端地址签名声明属于调研报告的 P2-1，不在本分支。
- 不修改 Engine 稳定 API、绑定与 `PeerReachabilityPort` 等既有 port 的形状；复用现有 `forget`、`disconnect_all`。
- 不自行实现 ICE、打洞或新的 watchdog；不修改 LAN 兼容线。
- 不实现内容发送的连接复用（问题 3），该项另开 issue。
- 不改变配对（admission）自身的截止时间与围栏合同；它已有明确 owner。
- 不把本机、容器的结果描述为手机或真机验收。

# 4. Current Architecture Context

Component: 入站授权与连接关闭
Path: `crates/uc-infra-p2p/src/network/iroh/inbound_peer.rs`、`peer_reachability_adapter.rs`、`transfer_progress_adapter.rs`、`clipboard_receiver_adapter.rs`、`crates/uc-application/src/space/connectivity/peer_connections/runtime.rs`
Responsibility: `InboundPeerGate` 在各 ALPN 的 `accept` 开头授权一次；`ConnectionRuntime::reconcile` 对不再可用的 peer 调用 `forget`。
Relationship: `forget` 与 `disconnect_all` 只遍历 presence 适配器自己登记的连接。其他 ALPN 的 handler 各自持有连接，没有登记入口。

Component: 内容发送与单飞拨号
Path: `crates/uc-infra-p2p/src/network/iroh/clipboard_dispatch_adapter.rs`、`connect.rs`
Responsibility: `dispatch` 解析地址、经 `dial_single_flight` 拨号、写帧、读回执，随后丢弃连接。
Relationship: `dial_single_flight` 在拨号返回后才从 `in_flight_dials` 移除条目；`dispatch` 与 `send_and_ack` 没有超时或取消令牌。

Component: 连接保留与探活
Path: `peer_reachability_adapter.rs`、`liveness.rs`
Responsibility: 已建立的 presence 连接在 10 秒新鲜窗口内不重拨，重拨失败不关闭既有连接。
Relationship: 内容发送不查询该连接表，每次新拨。

Component: 地址刷新
Path: `crates/uc-application/src/space/connectivity/peer_connections/runtime.rs`、`crates/uc-infra-p2p/src/network/iroh/net_recovery.rs`、`membership_history_exchange_adapter.rs`、`persistable_addr.rs`
Responsibility: 网络机会只调度重拨；对端地址写回只发生在已确认的历史同步之后，且只保存仍活跃的 relay。
Relationship: 拨号路径只读取存储的地址；记录没有过期机制。

Component: 重试负责人
Path: `connect.rs`（错峰拨号）、`runtime.rs`（逐 peer 退避）、`recovery/mod.rs`（`RETRY_DELAYS`）、`space/admission`（延迟重试）
Responsibility: 各自有独立节奏；内容发送失败还会经 `report_communication_failure` 触发 presence 复查。
Relationship: 统一目标见[统一生命周期计划](2026-09-12-unified-runtime-lifecycle.md)，现状尚未收敛。

# 5. Proposed Design

## 问题 1：移除成员后非 presence 连接仍存活

负责人：Infra 入站授权模块（`inbound_peer`）提供已授权入站连接的登记，出站内容发送在拨号成功后以同一设备键登记其连接与在途发送，`PeerReachabilityPort::forget` 与 `disconnect_all` 的既有实现据此关闭同一成员的所有登记连接。Application 的 `reconcile` 流程不变，port 形状不变。

移除成员时，`forget` 同时关闭登记的出站连接并中止该设备在途的发送，调用方得到失败结果而不是继续等待。登记在 `InboundPeerGate` 授权成功之处完成，连接关闭时自动注销，避免各 handler 各写一份。长连接处理（传输进度）在每次读取前后仍可被关闭中断，不依赖再次授权。

## 问题 2：内容发送不可取消，单飞 leader 被丢弃会卡死后续调用

- 单飞 leader 的条目由 drop 守卫移除；leader 被丢弃时，follower 收到 `LeaderLost` 并可重新竞争成为 leader，而不是永久等待。
- `send_and_ack` 增加写帧与读回执的分阶段超时；超时分类为既有 Offline 之外的可区分失败，不新增公开错误。
- 移除 peer 时中止该设备在途的发送，由问题 1 的出站登记承担。
- 各阶段超时预算由测试实测确定，见 Step 2。

## 问题 3：内容发送不复用健康连接（移出本分支）

改动面大且依赖网络矩阵数据，另开 issue。issue 需带上本规格里的约束：复用必须遵守问题 1 的登记与关闭，不能绕过授权，对端静默关闭时回退到新拨。

## 问题 4：对端地址只在历史同步后刷新

网络机会（NetworkChanged、Foreground、SystemWake）触发时，由既有连接 owner 对每个 peer 复核当前可用地址，并通过既有的 `refresh_verified_peer_address` 写回；写回仍只保存经验证的活跃 relay，不引入地址签名与新协议。不做时间过期：记录只是拨号提示，过期会丢掉仍然有用的 relay 提示，在关闭公共发现的配置下尤其有害；新鲜度由上述验证后写回保证。

另外，成员移除时 `apply_remove` 目前只删除可信 peer，不删除地址记录（`crates/uc-infra-profile/src/space/adapters/membership_member_facts.rs`）。移除时同步删除该成员的地址记录，属于清理，不是过期；该效果可能被重放，删除必须幂等。

## 问题 5：重试负责人分散

保持“单次发送最多 3 次拨号尝试（现有错峰拨号的次数）并报告失败，之后是否继续重试由 `ConnectionRuntime` 按自己的退避决定”。移除、暂停、关闭时，其余负责人必须停止拨号，由取消传播保证，而不是各自检查。

## Data Model

不新增持久字段与 wire 字段。登记表仅在运行期存活，键为设备标识，不写入日志；日志与观测事件遵守仓库的隐私规则，不含地址、设备名或内容。

## API / Interface

- 不改变 Engine facade、`PeerReachabilityPort`、`DispatchReport` 与绑定。
- 新增的登记入口与 drop 守卫是 `uc-infra-p2p` 内部接口。
- 下层错误保留 source chain，不字符串化。

## Workflow

1. 成员被移除：`reconcile` 取消该 peer 的在途拨号并调用 `forget`；`forget` 关闭 presence 连接和该成员在登记表中的全部连接。
2. 发送被放弃：future 被丢弃，守卫释放单飞条目，后续发送重新拨号。
3. 网络机会到达：既有 owner 重拨并复核地址，成功后写回。
4. 一次发送失败：向 reachability 报告一次，由 runtime 统一退避。

# 6. Implementation Plan

每一步都遵循：先写测试并在基线上确认失败，再修改生产代码，直至测试通过。

Step 1（问题 1）：
File: 新增跨 ALPN 的真实 E2E（`crates/uc-infra-p2p/tests/`）；随后 `inbound_peer.rs`、`peer_reachability_adapter.rs`、`clipboard_dispatch_adapter.rs` 及各 handler。
Change: 测试在生产装配的节点（`node.rs` 的 `removed_member_connections`）上，用真实成员表和 `forget`，先以对照断言证明连接已通过授权，再移除成员并触发 `forget`；已复现（基线失败）的两个长连接案例是传输进度连接，以及已交给应用、尚未结算的剪贴板连接（最长保留到 60 秒结算超时）。`ACTIVE_CLIPBOARD_ALPN`、`ACTIVE_CLIPBOARD_PULL_ALPN`、`GROUP_UPDATE_ALPN` 是单次交换，是否存在可被利用的长时间悬挂尚未证实，先补案例再决定，基线通过则不改。断言连接关闭，传输进度不再产出事件；另加一条出站案例：发送卡在对端不回执时移除成员，发送必须被中止并返回失败。基线预期失败。
Risk: 出站发送被中止后不得重放；登记范围过宽会误关属于配对流程的连接；admission 与 branch recovery 不属于成员连接，不登记。

Step 2（问题 2）：
File: `clipboard_dispatch_adapter.rs` 的 E2E 测试；随后同文件与 `connect.rs`。
Change: 测试一：leader 在拨向不可达地址时被丢弃，随后对同一目标的发送必须在有限时间内返回，基线预期永久挂起（测试自身带外层超时）。测试二：对端接受连接后不回执，`dispatch` 必须在预算内返回，基线预期无限等待。
Risk: 超时过短会误判慢 relay。预算由实测确定：在隔离网络中注入 relay 高延迟与丢包，记录各阶段分布，取覆盖正常尾部的值，不得凭本机短 RTT 推导；缺环境时保留现有预算并标未执行。

Step 3（问题 3）：移出本分支。
Change: 另开 issue，附上第 5 节的约束与握手次数基线测试思路；本分支不改动。

Step 4（问题 4）：
File: `peer_connections` 与 `membership_history_exchange_adapter.rs` 的 E2E 测试；随后 `runtime.rs` 与写回路径。
Change: 测试让对端 endpoint 以同一密钥在新端口重启，发送方存储旧地址；触发 `NetworkChanged` 后，期望存储地址被刷新并能重新连通。基线预期：不刷新。若发现 iroh 自身发现机制已经使该场景通过，则说明问题不成立，按“测试通过则降级”处理。
另加一条清理案例：移除成员后，其地址记录仍存在于仓库，基线预期失败；修复后记录被删除，重复应用移除效果仍成功。
Risk: 写回错误地址会让可达的 peer 变得不可达；仍只保存经验证的活跃 relay。删除地址记录不得影响仍在 Space 内的成员。

Step 5（问题 5）：
File: 带计数接收端的 E2E 测试；随后 `runtime.rs`、`connect.rs`、`recovery/mod.rs`。
Change: 测试一：对不可达 peer 做一次失败发送，统计接收端观察到的拨号尝试，并断言这一次发送本身引出的尝试不超过 3 次。测试二：peer 被移除、运行时暂停、关闭之后，接收端不再观察到新的拨号尝试。基线预期：至少其中一项超出。
Risk: 3 次只约束一次发送自己的尝试；`ConnectionRuntime` 随后的退避重试单独计数，不得因为凑够上限而改弱重试。

Step 6：文档与交付检查
Change: 回写稳定结论到相关 Design Doc；运行根 `AGENTS.md` 交付前检查；已通过项勾选，未执行项标“跳过”。

# 7. Edge Cases

Scenario: 成员被移除时有一条内容帧正在写入。
Expected behavior: 连接关闭，发送方得到失败结果，接收方不应用该帧；不重放。
Implementation: 关闭先于应用结算；沿用既有回执规则，无回执不算送达。

Scenario: 同一成员多条并发连接。
Expected behavior: 全部关闭，登记表按设备收齐。
Implementation: 登记表允许同一设备多项。

Scenario: 登记与关闭竞争。
Expected behavior: 先授权后被移除的连接在关闭窗口内仍会被关闭；移除之后到达的连接在授权时被拒绝。
Implementation: 登记发生在授权判定之后且持锁检查成员状态。

Scenario: leader 在写回之前被取消，follower 已订阅。
Expected behavior: follower 不挂起；重新竞争拨号。
Implementation: drop 守卫移除条目并通知。

Scenario: 网络变化时对端确已离线。
Expected behavior: 不写回；保留旧记录，由退避继续。
Implementation: 只写回验证成功的地址。

Scenario: 暂停、关闭与恢复机会并发。
Expected behavior: 暂停与关闭优先，撤销后的工作停止，迟到结果被丢弃。
Implementation: 复用 `ConnectionRuntime` 的代际围栏。

# 8. Testing Strategy

先列失败方式：移除后连接仍存活、移除后仍有事件投递、leader 被丢弃后挂起、无回执时永久等待、重复握手、网络变化后地址未刷新、移除或关闭之后仍有拨号尝试。以这些断言驱动测试，所有断言先在基线上失败。

## Unit Test

仅用于纯函数与登记表的状态机（例如多项登记、并发注销）。不得用单元测试替代下面的端到端复现。

## Integration Test

主体是端到端：真实 iroh endpoint、真实成员表、真实 ALPN handler。接收端用带计数的 handler 统计新连接与拨号尝试，不依赖时序休眠，使用事件与超时上限。每个测试在基线上先红，修复后转绿。

## Regression Test

执行既有 `--suite local` 成员恢复套件、`uc-infra-p2p` 全量测试和根 `AGENTS.md` 的交付前检查。Linux 网络矩阵由 CI 的 `connection-recovery` 作业承担，不在本机宣称通过。

# 9. Acceptance Criteria

* [x] 问题 1：复现测试在基线上失败；修复后传输进度、等待结算的剪贴板入站连接，以及在途出站发送在成员移除后都被关闭或中止（`node.rs` 的 `removed_member_connections`）。活动剪贴板、pull、group update 未补案例。
* [x] 问题 2：leader 被丢弃与无回执两个复现测试在基线上失败（`clipboard_dispatch_adapter.rs`）；修复后均返回。回执等待上限为 75 秒，由接收端 60 秒结算上限加余量得出；写帧阶段与拨号的超时预算仍待隔离网络实测，保留现状。
* [x] 问题 3：已确定移出本分支，另开 [issue #170](https://github.com/UniClipboard/Engine/issues/170)。
* [ ] 问题 4：复现测试在基线上失败或被证明不成立；成立则修复后地址刷新并重新连通。
* [ ] 问题 5：尝试计数测试在基线上失败；修复后移除、暂停、关闭之后不再有拨号尝试，一次失败的发送最多 3 次拨号尝试。
* [ ] 既有成员恢复本机套件、`uc-infra-p2p` 全量测试与仓库交付门禁通过。
* [ ] Linux 网络矩阵有 CI 证据，或明确保留未执行状态。
* [ ] 实体设备切网、睡眠唤醒有双向内容证据，或明确保留未执行状态。

# 10. Risks and Trade-offs

登记表让关闭范围覆盖所有 ALPN 与出站发送，代价是新增一处必须与授权保持一致的运行期状态。地址刷新放宽了写回时机，若写入未验证的地址会降低可达性，所以限定为验证后的活跃 relay。收敛重试负责人会改变失败后的恢复节奏，必须有实测而不是靠改弱重试来通过测试。

# 11. Open Questions

已决定：移除 peer 时中止在途的出站发送（问题 1）；超时预算用实测确定；问题 3 另开 issue；一次失败发送最多 3 次尝试，口径为一次发送自己的错峰拨号，`ConnectionRuntime` 之后的退避重试单独计数；存储的对端地址不做时间过期，成员移除时删除其地址记录。

暂无待决问题。若 3 次的口径改为包含 runtime 退避，需要重新评审重试策略。
