# 只走中转时三秒内恢复双向连接

## 状态与实施合同

- **状态**：核心实施与二十轮验收完成；现有旧版配对基线阻塞完整套件收口
- **日期**：2026-09-18
- **问题来源**：本机只走中转的独立进程测试确认，当前版本能收到中转恢复通知并重新上线，但恢复时间不稳定；一次超过二十秒失败，四次成功样本分别约为 1.0、2.5、14.6、2.4 秒。
- **完整负责人**：Application 的 `PeerConnectionCoordinator` 继续完整负责单设备连接机会、在途尝试替换、失败重试与最终在线结果；Infra 只报告本机中转恢复事实并执行真实连接与确认。
- **调用方唯一动作**：宿主保持现有启动和网络机会通知方式；中转状态变化由 Engine 内部自动接入，不要求 Desktop、Mobile 手动刷新、发送内容或重启。
- **成功结果**：直连被阻断、双方成员资格正常且两端均已重新接入中转后，三秒内双方确认 Online，并完成 A→B、B→A 的精确内容传送。
- **失败结果**：三秒门未通过时保持离线或等待恢复，不伪造 Online；随后回到既有有界退避，不持续高频拨号，也不升级为整会话重建。
- **重试与重启责任**：本次即时替换和后续重试仍由 `PeerConnectionCoordinator` 统一负责；进程重启继续走既有启动恢复，不增加持久任务或第二套重试循环。

## 实施与验收记录

- 连接负责人已能区分本机中转恢复与普通网络变化。中转恢复会重置旧退避；有在途尝试时取消旧尝试并只安排一次新确认，暂停、成员移除和新成功均可阻止多余后继工作。
- Linux 隔离网络的 relay-only 聚焦验收连续 20 轮通过。两端重新接入中转后，双方 Online 为 57–353ms，平均 266ms；双向精确内容传送为 119–411ms，平均 322ms；二十轮整会话恢复次数均为零。
- 二十轮证据确认两端直连阻断规则实际命中，全部场景、命名空间和测试明文完成清理。失败证据与二十轮通过证据分别保存在仓库外，不会被后续运行覆盖。
- 完整网络回归中的直连三轮、已知设备换址三轮、中转健康直连三轮和 relay-only 三轮全部通过。旧版兼容在场景开始前的初始配对超时；修复版本重复两次失败，未修改的 `a13ff9d7` 对照版本在相同环境同样失败，因此确认是现有基线阻塞，不记为本次通过，也不归因于本次修复。
- 本地分层回归的连接层 38 项、路由 3 项、网络隔离 1 项和 Application 连接恢复 34 项通过。其后的 Engine 已有连接检查在启动资料尚未解锁时失败；未修改的 `a13ff9d7` 对照版本在相同环境和相同位置失败，因此同样作为现有基线阻塞，不记为通过。
- 仓库 metadata、全工作区全部目标检查、格式、Rust 规范、仓库结构、脚本语法和差异检查通过；全部临时容器、网络资源、对照源码和两套独立构建目录已删除。本机二十轮通过证据及失败对照证据保留在仓库外。
- 实体 Desktop↔Mobile 公网中转验收未执行，记为跳过。

# 1. Overview

当前中转恢复链路已经具备两个关键能力：Infra 观察本机中转从不可用变为可用，并把该事实转换为连接机会；Application 收到机会后重新检查当前成员并确认真实连接，只有确认成功才发布 Online。

缺口在于中转恢复被压成普通 `NetworkChanged`。如果某个目标当时已有一次在中转不可用期间启动的连接尝试，连接负责人只设置“完成后再跑一次”，不会终止这次已经过时的等待。旧尝试可能继续占用数秒，失败后又进入 1、2、5、10 秒递增等待。2026-09-18 的本地只走中转测试与该行为一致：离线识别稳定需要约 15.6–17.3 秒；中转重新可用后的连接恢复多数约 1–2.5 秒，但也出现 14.6 秒和超过二十秒的长尾。

本计划不降低在线确认标准，也不依赖更激进的离线判定。修复只处理一个明确事实：中转已经恢复时，仍在使用旧网络条件等待的连接尝试已经失去继续等待的价值，应由现有连接负责人替换为一次基于新网络条件的尝试。

三秒预算从“两端都重新接入本地中转”这一可验证时刻开始，不包含中转实际不可用的时间，也不包含此前的离线识别时间。测试同时记录中转进程启动、两端接入中转、双方 Online 和双向传送四个边界，避免用总场景耗时混淆产品恢复时间。

# 2. Goals

- 只走中转且两端均已重新接入中转后，三秒内自动恢复双方 Online，并在同一预算内完成双向精确内容传送。
- 中转恢复机会能够替换在旧网络条件下仍在等待的单目标连接尝试，不等待其原截止时间，也不继承故障期间累积的长退避。
- 重复或并发的中转恢复通知对每个目标最多形成一次替代尝试，不产生拨号风暴、并行状态写入或第二套恢复循环。
- Online 仍只来自当前成员资格下的真实连接确认；中转已连接、地址已发现或旧缓存均不能直接写 Online。
- 三秒失败后自动回到既有普通退避，整会话重建次数保持为零，正常直连和 LAN-only 行为不变。
- 自动化证据明确保存直连阻断、中转重新接入、双方 Online、双向传送、每段耗时和资源清理结果。

# 3. Non-Goals

- 不把离线显示时间从当前约 15–17 秒压缩到三秒；这是独立的活性检测与功耗取舍。
- 不修改 Iroh、QUIC、中转服务器实现或设备间协议。
- 不降低成员资格、准入确认、挑战回应或内容回执的真实性要求。
- 不新增公开 Engine 操作，不要求产品仓编排刷新、重试或会话重建。
- 不因 P2P 失败自动切换 LAN 兼容线，也不改变用户选择的网络模式。
- 不新增持久字段、数据库版本、后台恢复表或可由调用方组合的阶段式接口。
- 不通过放宽二十秒旧测试预算、增加观察宽限或自动重跑掩盖三秒门失败。

# 4. Current Architecture Context

```text
Component: 普通成员连接负责人
Path: crates/uc-application/src/space/connectivity/peer_connections/
Responsibility: 统一拥有目标选择、连接机会合并、单目标在途任务、在线检查、失败退避、暂停、恢复和关闭。
Relationship: 当前把所有网络变化交给同一个 opportunity 路径；在途时只设置 rerun，不替换旧尝试。
```

```text
Component: 本机中转状态观察
Path: crates/uc-infra/src/network/iroh/net_recovery.rs
Responsibility: 观察 Iroh home relay 状态；从不可用转为可用时发布 LocalRelayRecovered。
Relationship: 该事实真实且已接入，但下游目前把它折叠成普通 NetworkChanged，丢失“旧拨号应被替换”的语义。
```

```text
Component: 连接提示装配
Path: crates/uc-infra/src/network/iroh/node.rs
Responsibility: 把本机地址、已知设备发现和网络恢复观察转换为 Application 的 ConnectionHint 流。
Relationship: LocalRelayRecovered 当前映射为 ConnectionHint::NetworkChanged。
```

```text
Component: 真实可达性确认
Path: crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs
Responsibility: 核对成员准入、检查既有连接或建立候选连接，完成确认后维护连接证据并发布 Online/Offline。
Relationship: 修复继续调用同一 verify_reachable，不增加“中转在线即设备在线”的捷径。
```

```text
Component: 分批连接能力
Path: crates/uc-infra/src/network/iroh/connect.rs
Responsibility: 使用现有地址在有界时间内分批发起连接，成功即停止其余尝试。
Relationship: 本计划先复用现有 0、500、1500 毫秒分批尝试和三秒单次预算；不预先改写公共拨号策略。
```

```text
Component: 独立进程网络验收
Path: scripts/testing/connection-recovery-network.mjs
Responsibility: 用 Linux 隔离网络、本地中转和防火墙规则验证真实 Engine 连接恢复与双向传送。
Relationship: 现有 E10 只给 Online 二十秒，没有独立记录“双方接入中转到双向恢复”的三秒结果，且直连阻断证明没有写入最终 evidence。
```

# 5. Proposed Design

## Components

### `ConnectionHint::RelayRecovered`

- **职责**：表达“本机中转刚从不可用变为可用”，保留其可替换旧连接尝试的语义。
- **输入**：Infra 已有的 `NetworkRecoveryObservation::LocalRelayRecovered`。
- **输出**：现有受管提示流中的无身份事件。
- **关系**：它是连接机会，不是对端在线、成员资格或准入事实；不新增公开 Engine 事件。

### 中转恢复替换动作

- **职责**：由 `PeerConnectionCoordinator` 对当前合格目标统一处理中转恢复。
- **输入**：`ConnectionHint::RelayRecovered`。
- **输出**：每个目标最多一次新的即时检查安排。
- **行为**：清除该目标在中转故障期间累积的失败次数；若无在途任务，在现有合并窗口后立即安排；若有在途任务，标记一次后继工作并取消旧任务，旧任务结束后立即安排新任务。
- **关系**：仍使用同一目标状态、并发上限和 `verify_reachable`；不增加旁路 worker。

### 取消后的后继结算

- **职责**：区分“因新中转事实被替换”与“暂停、关闭、成员移除导致取消”。
- **输入**：目标已有的 `rerun`、`next_trigger` 和取消令牌。
- **输出**：前者准确安排一次 `relay_recovered` 尝试；后者不再启动工作。
- **关系**：不允许旧取消结果覆盖该轮之后收到的 Online，也不结算成普通连接失败。

### 三秒中转验收记录

- **职责**：只在测试 evidence 中记录三秒门所需的时间边界与证明。
- **输入**：中转进程 ready、两个隔离节点到本地中转的 TCP 连接、只读 Online 观察和双向内容核对。
- **输出**：每轮 `relay_transport_ready_to_online_ms`、`relay_transport_ready_to_bidirectional_transfer_ms`、直连阻断计数和最终结果。
- **关系**：不增加生产公开查询，不把测试记录混入产品业务记录。

## Data Model

生产不新增持久数据。`ConnectionHint::RelayRecovered` 与目标的“替换后重跑”标记只存在于当前进程、当前 Space 会话内；暂停、成员移除或关闭时随既有目标状态清理。

测试 evidence 在每个 `E10-relay-only-N` 记录内增加：

```text
proof
- direct_paths_blocked: bool
- direct_drop_packets: number
- both_relay_transports_ready: bool
- relay_transport_ready_to_online_ms: number
- relay_transport_ready_to_bidirectional_transfer_ms: number
- bidirectional_transfer: bool
- whole_session_recovery_count: number
```

计时使用测试运行器的单调时钟。设备身份、地址、正文和原始错误不写入 evidence；合成正文仍只存在于测试私有资料，并在清理检查中确认删除。

## API / Interface

- 在 Application 内部 `ConnectionHint` 增加 `RelayRecovered`。该变化不进入 `uc-engine` 稳定操作、绑定或产品 API。
- `IrohSessionBuilder` 的恢复观察映射把 `LocalRelayRecovered` 转成 `RelayRecovered`，不再转成泛化的 `NetworkChanged`。
- `ConnectionRuntime` 增加私有的中转恢复处理函数，例如 `relay_recovered()`；调用方只提交完整事实，不传“是否取消”“等待多久”等步骤参数。
- `PeerReachabilityPort`、`verify_reachable`、成员范围接口和在线结果类型保持不变。
- 取消是内部调度结果，不新增公开错误；真实资格、存储、拨号与确认错误继续保留原 source chain 和稳定分类。

## Workflow

1. 本地中转不可用期间，连接负责人可以按既有规则检查或重试目标。
2. Iroh 报告本机中转从不可用变为可用，Infra 发布一次 `LocalRelayRecovered`。
3. `node.rs` 保留语义并交付 `ConnectionHint::RelayRecovered`。
4. `PeerConnectionCoordinator` 读取当前合格目标；成员范围为空、已暂停或正在关闭时不启动工作。
5. 无在途尝试的目标清除旧失败等待，在合并窗口后执行一次真实确认。
6. 有在途尝试的目标先标记一次后继工作，再取消基于旧网络条件的尝试；取消完成后只启动一次新确认。
7. 新确认复用当前资格核对、既有连接检查、分批连接和准入确认。任何方向的新连接成功都可按现有修订规则发布 Online，迟到失败不能覆盖它。
8. 双方确认 Online 后，普通内容通道可双向传送；测试在不使用刷新或发送促成上线的前提下验证这一结果。
9. 新确认失败时，从第一档普通退避重新开始；后续仍由同一负责人恢复，不触发整会话重建。

# 6. Implementation Plan

## Step 1：先把三秒缺口固定为可失败的验收

- **File**：`scripts/testing/connection-recovery-network.mjs`
- **Change**：为 relay-only 场景记录直连阻断计数；中转重启后等待两个节点都重新建立到本地中转的连接，以较晚完成者作为三秒起点；随后只读等待双方 Online，再执行双向精确传送，并把两段耗时和整会话恢复次数写入 evidence。
- **Risk**：轮询本身可能跨过截止点；断言使用动作实际时间，观察开销只可作为单独且很小的测试宽限，不能改变三秒产品预算。
- **Exit**：当前主线至少一次明确失败，失败位置是三秒恢复门而不是构建、配对、隔离或清理。

## Step 2：保留中转恢复语义

- **File**：`crates/uc-application/src/space/connectivity/peer_connections/mod.rs`、`crates/uc-infra/src/network/iroh/node.rs`
- **Change**：增加内部 `RelayRecovered` 提示并替换现有泛化映射；更新所有穷尽匹配和测试构造。
- **Risk**：不能把该提示公开给产品，也不能让它直接修改在线状态。
- **Exit**：单元测试证明中转恢复与普通网络变化可区分，提示丢失仍由既有周期重试兜底。

## Step 3：替换旧网络条件下的在途尝试

- **File**：`crates/uc-application/src/space/connectivity/peer_connections/runtime.rs`
- **Change**：实现 `RelayRecovered` 的单目标替换规则：重置失败档位、取消现有在途尝试、保留一次后继工作，并在取消完成后通过现有合并窗口启动；暂停、关闭、移除和资格失效优先，不能后继重跑。
- **Risk**：当前 `DialResult::Cancelled` 直接返回；实现必须只对明确标记的替换取消安排后继工作，不能让关闭取消重新启动任务。
- **Exit**：受控时钟测试证明长退避和被阻塞的旧尝试都能被替换，千次重复提示仍只形成一次后继尝试。

## Step 4：固定并发成功与迟到失败规则

- **File**：`crates/uc-application/src/space/connectivity/peer_connections/tests.rs`
- **Change**：补充中转恢复与入站成功、旧任务取消、成员移除、暂停和关闭的竞态测试；复用观察修订，确认新 Online 不被旧取消或失败覆盖。
- **Risk**：不能通过直接写在线缓存伪造真实 Infra 结果；Application 测试只证明调度，真实连接留给后续层次。
- **Exit**：每个目标最多一项在途工作，全局并发上限不变，所有等待中的手动刷新仍能正确结算。

## Step 5：真实连接与中转恢复回归

- **File**：`crates/uc-infra/src/network/iroh/net_recovery.rs`、`crates/uc-infra/src/network/iroh/peer_reachability_adapter/tests/liveness_tests.rs`（仅在现有测试入口需要补证时修改）
- **Change**：证明中转恢复边沿只发一次；被替换后的新尝试仍经过成员准入和真实回应；无需修改 `connect.rs` 预算时不改拨号策略。
- **Risk**：若三秒门仍失败，必须先依据现有连接诊断区分中转尚未可路由、连接准备、握手或准入确认，不能直接缩短安全确认预算或堆叠更多并发尝试。
- **Exit**：真实两个 endpoint 的恢复测试通过，未知或已移除设备不能因中转恢复变成 Online。

## Step 6：连续中转验收与全套回归

- **File**：`scripts/testing/run-connection-recovery-e2e.sh`、`scripts/testing/connection-recovery-network.mjs`
- **Change**：保留一键入口；专门执行 `relay-only` 连续二十轮，完整 `all` 套件仍按仓库标准连续三轮。失败 evidence 不被后续自动重跑覆盖。
- **Risk**：Linux 网络资源必须在失败、超时和信号退出后清理；不得修改宿主默认防火墙。
- **Exit**：二十轮全部满足三秒门和双向传送，全套三轮无回归，临时进程、规则、命名空间和明文资料均清理。

## Step 7：稳定文档与交付收口

- **File**：`docs/design-docs/automatic-peer-connections.md`、`docs/architecture/architecture-bible.md`、本计划及索引
- **Change**：把中转恢复替换旧尝试、三秒起点、失败回退与验收入口写回稳定设计；记录实际样本分布和未执行平台；完成后移动本计划到 `completed/`。
- **Risk**：计划完成不能代替真实设备、发布或产品采用；未执行项目必须明确记为跳过。
- **Exit**：稳定事实来源、代码行为、测试断言和执行记录一致。

# 7. Edge Cases

### Scenario：中转恢复时目标正在旧拨号

- **Expected behavior**：旧尝试被取消，只安排一次基于新网络条件的尝试；取消不记为离线失败。
- **Implementation**：先写后继标记，再取消令牌；取消完成后由原负责人统一排期。

### Scenario：中转恢复提示短时间重复到达

- **Expected behavior**：不增加并行任务，不不断推迟已经安排的尝试。
- **Implementation**：复用每目标单在途和合并窗口；已有 `relay_recovered` 后继时只保留一份。

### Scenario：一端先接入中转，另一端稍后接入

- **Expected behavior**：较晚一端的恢复边沿启动新尝试；三秒从两端都已接入中转时开始，不能把远端尚不可达计为恢复超时。
- **Implementation**：生产各端独立处理本地事实；测试使用两个节点中较晚的中转连接时刻作为统一起点。

### Scenario：新入站连接成功与旧任务取消同时发生

- **Expected behavior**：真实新成功获胜，旧取消或迟到失败不能写回 Offline，也不再发起冗余拨号。
- **Implementation**：继续使用观察修订与 `peer.online` 结算规则；后继工作在新 Online 后清除。

### Scenario：中转恢复时成员被移除、Space 暂停或关闭

- **Expected behavior**：不启动新连接，不出现迟到 Online，资源按既有生命周期结束。
- **Implementation**：资格与生命周期优先；目标移除或 `clear()` 删除后继状态和取消令牌。

### Scenario：中转已经恢复但真实对端仍不可达

- **Expected behavior**：三秒门失败并保持真实离线；之后进入既有 1、2、5、10、30、60 秒退避，不持续快速重试。
- **Implementation**：只重置一次失败档位；新尝试失败后使用原退避表。

### Scenario：直连仍然健康时中转恢复

- **Expected behavior**：健康直连保持 Online，不因中转变化被关闭或替换，整会话重建为零。
- **Implementation**：真实检查可复用健康连接；替换只针对在途检查任务，不直接关闭已确认连接。

### Scenario：LAN-only 模式

- **Expected behavior**：没有 home relay 恢复提示，行为完全不变。
- **Implementation**：中转观察任务仍只在启用 relay 时创建。

### Scenario：旧版本对端

- **Expected behavior**：协议协商、旧版活性边界和兼容规则不变；不能把三秒新/新验收冒充新/旧通过。
- **Implementation**：不修改协议和兼容分支，旧版矩阵继续按原计划单列。

### Scenario：资料为空、损坏或地址缺失

- **Expected behavior**：空成员范围不拨号；资格、地址读取和损坏错误保持现有分类及来源链，不被中转恢复掩盖。
- **Implementation**：新提示只影响调度，不绕过原资格与地址解析。

# 8. Testing Strategy

## Unit Test

1. **长退避被中转恢复重置**：目标已进入 60 秒等待；提交 `RelayRecovered`；合并窗口后只增加一次调用。
2. **旧在途任务被替换**：第一轮阻塞，提交中转恢复；确认第一轮取消，第二轮在短窗口启动，最大并发仍为一。
3. **通知风暴合并**：在旧任务阻塞期间提交一千次中转恢复；只出现一次后继调用。
4. **关闭优先**：标记替换后立即暂停、移除成员或关闭；放行旧任务后不得再调用、不得出现 Online。
5. **新成功优先**：旧任务被取消期间收到新 Online；不安排冗余后继任务，手动等待者按 Online 结算。
6. **普通提示不变**：Foreground、SystemWake、普通 NetworkChanged、PeerAddressChanged 和 CommunicationFailed 保持现有合并及退避语义。

## Integration Test

1. 两个真实 Iroh endpoint 只使用 relay 地址；中转恢复后新拨号仍执行准入确认，成功才 Online。
2. 一端先恢复中转、另一端延迟恢复；较晚端恢复后可以连接，先前失败不能覆盖新成功。
3. 已移除或未知设备收到中转恢复时仍不能 Online。
4. 健康直连存在时重启中转，现有连接和双向内容保持可用。

## Regression Test

1. Linux 隔离网络中阻断全部 UDP 直连，并用计数器证明规则命中。
2. 先建立只走中转的双向可用基线。
3. 停止中转并只读等待双方 Offline；离线识别时间单独记录，不纳入三秒门。
4. 重启中转，等待两个节点均建立中转连接，从较晚时刻起计时。
5. 不调用刷新、不调用整会话恢复、不通过发送促成 Online；三秒内只读观察双方 Online。
6. Online 后立即执行 A→B、B→A 精确内容传送，仍在同一三秒预算内完成。
7. 连续二十轮全部通过；每轮整会话恢复为零，事件不振荡，资源数量不持续增长。
8. 再运行完整 `all` 套件连续三轮，覆盖直连、已知设备换址、relay 和旧版兼容场景。

建议验证命令从仓库根目录串行执行并复用共享 `target`：

```bash
cargo test -p uc-application space::connectivity::peer_connections --locked
cargo test -p uc-infra peer_reachability --locked
bash scripts/testing/run-connection-recovery-e2e.sh --suite network --repeat 3
```

只走中转二十轮使用现有网络 runner 的聚焦参数；实施时把最终命令写入脚本帮助并记录在本计划，不另建未经维护的临时入口。

交付前还需运行根 `AGENTS.md` 规定的 metadata、workspace check、fmt、Rust 规范、仓库结构和 `git diff --check`。实体 Desktop↔Mobile 的真实公网中转验收单独执行；未执行时记为“跳过”。

# 9. Acceptance Criteria

* [ ] 当前主线的聚焦 relay-only 测试能稳定捕获超过三秒的原问题，而不是在构建或环境准备阶段失败。（实施前的旧验收已记录 14.6 秒和超过 20 秒样本，但收紧后的三秒门未在未修改版本上重复运行。）
* [x] `LocalRelayRecovered` 以独立内部提示进入连接负责人，不再丢失为普通网络变化。
* [x] 中转恢复能取消基于旧网络条件的在途尝试，并且只安排一次新尝试。
* [x] 暂停、关闭、成员移除和资格失效不会因取消后的后继工作产生迟到连接。
* [x] 重复提示不突破每目标一次、全局四次的现有并发上限。
* [x] Online 仍来自当前资格下的真实双端确认；没有新增缓存或中转状态捷径。
* [x] 直连被内核规则阻断且计数器实际命中，测试期间没有可用直连路径。
* [x] relay-only 连续二十轮均在两端接入中转后三秒内双方 Online，并完成双向精确内容传送。
* [x] 二十轮中整会话重建次数均为零，没有状态振荡或任务、线程、连接持续增长。
* [ ] 完整网络套件连续三轮通过，直连、换址、旧版兼容和生命周期行为无回归。
* [x] evidence 保存每轮三秒计时、直连阻断、双向传送和清理结果，失败不被自动重跑覆盖。
* [x] 临时进程、网络规则、命名空间、构建目录和测试明文均完成清理。
* [x] 稳定设计、架构圣经和计划状态已按实际结果更新；未执行的实体设备验收明确标为跳过。

# 10. Risks and Trade-offs

- **取消竞态**：替换旧任务会增加取消与新入站成功并发的机会。继续使用现有修订保护，并用受控时钟和真实连接两层测试固定“新成功优先”。
- **恢复风暴**：若底层反复报告健康变化，可能产生频繁拨号。提示只在不健康→健康边沿产生，Application 继续合并，并对每目标只保留一个后继工作。
- **三秒预算余量**：现有分批连接最晚一次在 1.5 秒启动，正常 relay 建连约 1–2 秒，预算较紧。先消除已经确认的旧任务等待；若仍失败，依据连接阶段证据修正现有拨号节奏，禁止直接放宽门限。
- **离线显示仍慢**：本计划刻意不改变约 15–17 秒的静默离线识别。优点是避免更频繁保活和移动端耗电；代价是中转长时间故障时 UI 仍可能较晚显示离线。
- **测试环境差异**：本地中转不能证明真实公网、移动后台或代理环境。Linux 二十轮是 Engine 合入门；真实 Desktop↔Mobile 公网中转是发布前补充验收，结果独立记录。
- **替代方案**：全局缩短健康检查、提高所有拨号并发或在中转恢复时重建整会话都能制造更快表象，但分别增加功耗、连接风暴或扩大故障面，因此不采用。

# 11. Open Questions

当前没有阻塞实现的问题。以下口径已在本计划固定，不留给实施阶段临时决定：

- 三秒从两端都重新建立本地中转连接的较晚时刻开始。
- 三秒门同时要求双方 Online 与双向精确内容传送完成。
- 离线识别耗时单独记录，不计入三秒恢复门。
- 核心合入以 Linux relay-only 二十轮和完整套件三轮为准；实体设备未执行时不得记为通过。
