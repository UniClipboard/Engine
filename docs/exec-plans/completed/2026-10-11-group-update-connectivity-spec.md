# 1. Overview

本规格定义 Issue #34 第三切片：把组密钥更新的完整发送、接收和恢复流程接入 Uni 连接边界。
基线为 `origin/main c0351c0e`，第二切片已合并。本文定义实施契约；当前进度和验收范围以执行计划为准。
执行顺序及交付证据只在[执行计划](2026-10-11-group-update-connectivity-plan.md)维护。

典型场景：B 停在 epoch 3，收到 epoch 5，缺少 epoch 4。B 应反馈暂不可用，并主动联系当前
合格成员 C，验证双方签名历史是否具有连续祖先关系，补齐缺失历史，然后依次安装 4、5。
A 收到 B 的主动联系后仍须重新验证 B 的资格；联系本身不证明 B 已应用任何更新。

当前组更新 adapter 同时负责原生拨号、协议解析和安全应用。缺少前置更新被归为明确拒绝；
`current_epoch >= incoming_epoch` 的快捷返回也不能证明某份更新经过了验证。
这些行为会把可恢复缺口变成阻断，或者把未经验证的重放误认作成功。

源码核对还发现一个范围决策：成员历史 `/4` 的 V4 后缀每页最多一条记录，且逐页请求、确认。
直接复用它能恢复缺口，不能实现多份缺失更新一次发送。2026-10-12 用户已选择沿用 `/4`：本次只交付主动逐页恢复，批量协议和减少往返留到后续。

# 2. Goals

- 调用方只提交“向 B 发送组更新”或交付完整入站交换；发现、拨号、授权、结果和恢复由唯一 owner 承担。
- 生产组更新收发通过 Uni 连接和双向流；Application 不引用 iroh/QUIC 类型。
- 只有当前 Space 会话允许的认证成员能进入接收流程；消息仍需验证格式、MLS commit 和密钥目录。
- 明确区分应用成功、临时不可用、明确安全拒绝及带 source chain 的本地/传输失败。
- 缺少前置更新由成员恢复 owner 主动补齐；收到较新 epoch 不隐含携带中间更新。
- 历史完整验证后持久接纳；本地安全更新顺序应用，每一步保存且安装成功才报告该步应用完成。
- 正确、完整验证的恢复能解除对应阻断；单纯在线、主动询问或历史 ACK 不能解除明确拒绝。
- 保留组更新 `/2`、`/1` 兼容和单次交换；冻结旧格式向量并用真实新旧节点验证。
- 无恢复任务时无额外轮询；有任务时复用成员调度，采用有界退避并能由明确事件提前唤醒。
- 连续缺口沿用 `/4` 逐页同步；批量协议版本不在本次范围。

# 3. Non-Goals

- 不完成整个 Issue #34；不迁移所有历史、配对和文件协议，不引入另一套密钥 bundle 真相。
- 不新增组更新 `/3`，不让 `/2` 或 `/1` 变成多流会话。
- 不因 P2P 失败自动切换 LAN，不因网络可达授予成员资格。
- 不把冲突证据消息当普通连续后缀同步，不用整数 epoch 替代签名历史祖先校验。
- 不扩大稳定 Engine facade 暴露内部阶段、恢复状态或业务标识用于测试和观测。
- 物理平台验收留在实施矩阵，未执行标为“跳过”。

# 4. Current Architecture Context

以下记录实施前基线，路径相对仓库根目录；落地后的所有权见成员历史设计文档，验收见执行计划。

```text
Component: Uni 连接负责人
Path: crates/uc-application/src/connectivity/
Responsibility: 准入、拨号合并、租约、排队、流及关闭生命周期。
Relationship: manager 缓存键只有 DeviceId；当前原生 backend 使用 Clipboard ALPN。
```

```text
Component: 组更新原生 adapter
Path: crates/uc-infra-p2p/src/network/iroh/group_update_adapter.rs
Responsibility: 发现、拨号、单次交换、wire、ACK，以及直接调用 GroupRevocationPort 应用。
Relationship: 同时承担技术能力和接收业务；这是本切片需要拆开的边界。
```

```text
Component: Space 成员 owner 与发送
Path: crates/uc-application/src/space/membership/group_update_delivery.rs
Path: crates/uc-application/src/space/membership/worker.rs
Responsibility: 从验证账本选择收件人、推进 pending 更新、重试、历史同步和本地效果。
Relationship: 维护周期先处理本地效果，再待发送更新，再网络历史；不得另建并行恢复系统。
```

```text
Component: 成员授权与历史规则
Path: crates/uc-application/src/space/membership/access.rs
Path: crates/uc-core/src/membership/ledger/present.rs
Path: crates/uc-core/src/membership/versioned_membership_history/
Responsibility: 当前成员关系、签名历史、祖先关系、后缀与冲突判定。
Relationship: 使用既有 admits_inbound_peer 规则，保留它允许的待决关系，不新增“必须完全同历史”条件。
```

```text
Component: 历史同步与持久接纳
Path: crates/uc-application/src/space/membership/worker/history_sync.rs
Path: crates/uc-application/src/space/membership/handle_history_message/
Path: crates/uc-infra-p2p/src/network/iroh/membership_history_exchange_adapter.rs
Responsibility: summary 比较、逐页后缀交换、完整校验、CAS 提交和本地效果排队。
Relationship: Confirmed ACK 证明历史接纳，不证明随后排队的 MLS 效果已经应用。
```

```text
Component: 安全更新与存储
Path: crates/uc-infra-security/src/access.rs
Path: crates/uc-infra-security/src/group_update_error.rs
Path: crates/uc-infra-security/src/membership_update.rs
Path: crates/uc-infra-storage/src/db/repositories/space_security_store/delivery.rs
Responsibility: MLS/目录验证、先保存后安装、加密待发送状态和拒绝分类。
Relationship: 历史本地效果另有 apply 入口；必须与直接入站共享应用协调，避免并发推进。
```

```text
Component: 成员记录与组装
Path: crates/uc-application/src/space/membership/record.rs
Path: crates/uc-infra-profile/src/space/membership_record/
Path: crates/uc-engine/src/assembly/sync_engine.rs
Path: crates/uc-infra-p2p/src/network/iroh/node.rs
Responsibility: owner 记录/CAS、当前 v5 加密格式、生命周期装配及协议注册。
Relationship: Engine 只注入完整能力，Node 不编排补历史、应用或重试步骤。
```

# 5. Proposed Design

## Components

### 完整负责人

Space 的成员模块继续拥有组更新发送、恢复选择及调度。在其内部新增 `group_update` 子模块，
由 `GroupUpdateReceiver` 接收完整交换，内部 `GroupUpdateApplyCoordinator` 统一直接入站和
已验证历史效果的顺序应用。它们属于同一个成员业务 owner，不把步骤交给 Engine 拼接。
`mod.rs` 仅声明模块和必要导出，模型、port、流程、错误和测试分别组织。

- 输入：完整 `ConnectionLease`、当前会话工作许可和不透明观测上下文；历史路径输入为 owner 产生的已验证安全效果。
- 输出：一次交换的最终结果；必要时在同一成员记录提交恢复需求并唤醒原 worker。
- 依赖：Uni、权威成员准入、协议 codec、安全应用能力、成员记录/CAS 与原调度器。
- 恢复：重启从记录、待处理效果和安全材料重新构造；不要求调用方记住恢复阶段。

### 协议隔离的原生连接能力

同一 endpoint 上分别装配 Clipboard 和 GroupUpdate 的 manager，生命周期再绑定当前 Space
generation。原因是当前缓存仅按 DeviceId：共用一个 manager 会把 Clipboard 连接误交给组更新。
两者共享真实发现目录、身份解析、权威准入和连接登记，不复制授权账本。

Infra backend 增加有限协议 profile，仅覆盖已有 Clipboard 与 GroupUpdate。连接必须提供
实际协商协议的有限能力标识，供协议适配层判断 `/1` 与 `/2`；不得把请求的首选版本当实际版本。
GroupUpdate 两个版本均为 `SingleExchange`。只有协商不支持新协议才选旧协议；坏数据、拒绝、
超时或认证失败不能触发协议降级重发。

原生 handler 只包装连接并调用完整 receiver，跟踪任务并关闭收束；移除直接 MLS apply。
身份从认证连接取得，不信任报文中的设备字段。排队前、取得租约后、应用提交前和 ACK 前
检查会话 generation/授权，迟到的旧会话结果不能改写新会话或新尝试。

### 传输无关组更新交换

`uc-sync-protocol` 新增 `group_update` topic，机械搬移现有 framing、postcard trace carrier、
长度校验及 ACK codec，先固定旧代码产生的向量。Application 使用 Uni 流，不引用 ALPN 字节或原生连接。
保留 `UCT1`、现有长度前缀、4 MiB 更新上限和既有额外 wire 预算；不得先改格式再生成兼容向量。

收发超时和关闭均有界；现有 I/O 10 秒预算作为初值，整个交换也必须有明确 deadline，
不能让 handler 在 `connection.closed()` 上无限等待。取消发生在持久应用中时，不能丢弃
仍运行的 blocking 工作：由当前 SessionLocalWork 跟踪到完成，关闭流程负责收束。

### 现有恢复 owner

接收暂不可用时登记恢复需求，复用成员 worker 的 summary/后缀和 branch recovery 流程。
先找当前允许的认证成员，比较签名历史位置与祖先关系；C 不必是原发送者 A。
连续缺口走后缀；分叉、撤销或不一致进入既有相应规则。网络候选来源、发现顺序不是信任证据。

一次完整历史接纳完成后，继续运行已有因果效果队列，通过同一 apply coordinator 顺序应用。
禁止利用 FullConflictEvidence 多页封包绕过 V4 连续后缀规则。本切片沿用 `/4`，不新增批量线上扩展。

## Data Model

以下名称是拟定内部模型，不是新增稳定 facade。

| 模型 | 字段/含义 | 生命周期与约束 |
| --- | --- | --- |
| `GroupUpdateReceiveOutcome` | `Applied`、`Unavailable(reason)`、`Rejected(reason)` | 一次接收；对外仅发送现有版本能表达的 ACK |
| `GroupUpdateRecoveryRecord` | 可信 base_position、target_epoch、payload_digest | 与当前 Space 成员记录及修订绑定；重复目标合并，禁止按每份陌生报文无限建任务 |
| 应用证据 | 材料中的 epoch→确切 payload digest；旧材料另需当前可信历史匹配 | 与实际验证后的材料同存，绑定 Space/保护组；摘要不是授权证据 |
| 本机移除暂存 | 确切 digest→已生成后继私有材料 | 与签名移除历史同 CAS、同加密成员记录保存；真正应用后删除，分支切换清除 |
| 应用阶段 | 待验证、已保存待安装、已安装待确认 | 使用既有材料和效果进度表达；若缺字段统一在最终格式增加 |
| 调度状态 | 下次 deadline、连续无进展次数、合并唤醒标记 | 当前会话内存状态；重启根据持久需求重建初始退避，不存平台单调时钟 |

恢复附录和本机私有暂存随成员记录使用 MasterKey AEAD；应用证据随现有加密 SpaceKeyMaterial 保存。
成员 codec 只写最终 v6，v1–v5 一次迁移至最终形状。
不得以实现阶段再加 v7，不新增明文 epoch、设备、分支或摘要索引。旧记录缺应用证据不能
补造“已验证”；应从真实历史、安全材料和待完成效果恢复，必要时进入安全的再验证流程。

应用证据复用真实安全材料与可信成员历史，不另建公开恢复账本。
其不可变契约是：仅 `current_epoch >= incoming_epoch`、历史存在或历史 ACK 都不足以确认
该入站 payload 已应用。证据不足时先验证其与可信历史对应关系，并完成材料安装检查，
才允许对确切重复返回 `Applied`；无法证明时返回暂不可用并触发恢复，恶意不匹配返回拒绝。

## API / Interface

以下为职责级签名，实施时沿用仓库已有 port/result 命名风格；不得增加调用方必须执行的步骤。

| 能力 | 输入 | 输出/错误 | owner |
| --- | --- | --- | --- |
| 现有组更新发送 port | 目标 DeviceId、更新 payload、观测上下文 | 既有发送结果：确认、拒绝、暂不可用或离线/失败 | 成员 delivery + Uni adapter |
| `GroupUpdateReceiver.receive` | 已准入 `ConnectionLease`、当前会话工作上下文 | 完整接收最终结果；负责读取、应用、ACK 和清理 | Application 成员模块 |
| `GroupUpdateApplyCoordinator.apply_received` | 不可伪造的认证成员上下文、已解码更新 | `Applied` / `Unavailable` / `Rejected`，或保留 source 的本地错误 | 同一成员模块内部 |
| `GroupUpdateApplyCoordinator.apply_verified_history` | owner 持有的已验证历史安全效果 | 持久且安装完成结果；失败留在现有效果恢复链 | 同一成员模块内部 |
| 安全应用完整能力 | 经授权的更新、当前材料上下文 | 验证、保存、安装结果及结构化失败来源 | Infra security/profile |
| 现有成员维护入口 | 已提交恢复需求/唤醒原因 | 合并后的完整维护周期 | 现有成员 worker |

接收结果映射：

| 事实 | Application 结果 | `/2` | `/1` |
| --- | --- | --- | --- |
| 对应更新验证、保存和安装完成 | Applied | ACK 1 | ACK 1 |
| 缺前置历史、材料未就绪或可恢复安装故障 | Unavailable | ACK 3 | 关闭交换，不发送 ACK 2 |
| 身份/当前成员资格不合法 | 拒绝准入 | 不进入 payload apply | 同左 |
| 已就绪状态下可证明的签名、commit、目录或 lineage 非法 | Rejected | ACK 2 | ACK 2 |
| 带 source 的 I/O/存储/任务失败 | 类型化失败；不假装成功 | 按能力关闭或暂不可用 | 关闭 |

缺前置更新与空/损坏 group state 必须分型：不能把全部 `OutOfOrderUpdate` 都改成暂不可用。
Application 负责业务分类；Infra 提供可识别的失败，不通过字符串匹配原因。
发送方收到明确拒绝后仍沿用 `ByReceiver` 阻断，不能因 Online、历史确认而直接删除。
恢复后发送方重新提交原正确 payload，接收方凭实际应用/验证结果返回成功，才结清 pending。

为避免 `ByReceiver` 永久禁止这个确认交换，delivery 增加内部“验证后恢复确认尝试”资格：
只有来自当前认证 B 的完整历史交换证明与被拒更新对应的可信历史关系成立，才允许对同一
正确更新进行一次受退避约束的确认尝试。普通主动联系、Online 和未完整验证的历史不能授予
此资格；历史证据只允许尝试，不证明 B 已安装。资格绑定更新/目标/会话，消费后不得重复
绕过退避，B 返回暂不可用时保留原阻断和 pending。实际成功 ACK 才结清；后续更新仍不能
越过被拒更新。该资格归原 delivery owner，不在 Engine 或网络 handler 设置“解除拒绝”标记。

## Workflow

### 正常收发

1. 原成员 delivery 从当前验证账本选择 B 和待发送更新，沿用既有顺序、收件人和尝试围栏。
2. GroupUpdate manager 使用真实发现/拨号能力，认证 B 并按当前 Space 准入；检查实际协商版本。
3. 发送端取得唯一交换流、发送现有帧，等待有限 ACK；传输失败保留原错误链与 pending。
4. B 的 handler 将认证连接交完整 receiver；receiver 检查当前会话、成员资格和尺寸。
5. 接收与历史效果通过同一 coordinator 排序。安全能力校验、保存、安装；提交前重检围栏。
6. 只有对应更新真正完成才回复成功；发送方在当前尝试仍有效时结清。finally 释放流、租约和任务。

### B 缺 4 收到 5

1. B 确认这是缺前置更新，不是损坏状态或非法消息；返回暂不可用并合并恢复需求。
2. B 主动联系当前合格节点 C，经认证与准入后交换历史 summary，判断签名祖先关系。
3. 对连续后缀调用现有历史流程；其协议是 push/offer 协作，不能假设现有 request 直接返回完整后缀。
4. 完整传输校验通过并 CAS 保存历史，才确认历史接纳；不立即宣称 4、5 已安装。
5. 按因果顺序应用 4，完成保存和安装；随后应用 5。未完成阶段留在既有待处理效果中。
6. 已安装事实产生恢复唤醒。B 的主动交流和 A 的发送重试均须重检资格；正确 payload 获实际成功 ACK 后 A 结清。

B 本地恢复事实与 A 的发送状态不共用内存事件。B 通过正常、认证的历史交流给 A 提供可验证
关系，A 按上述资格发确认尝试；若 B 尚未安装仍返回暂不可用。无需新增组更新版本或把历史
ACK 扩展为安装证明；不得假设 C 的成功同步自动通知 A。

### 明确恢复事件与退避

| 事件 | 成立条件 | 可做什么 | 不能推出什么 |
| --- | --- | --- | --- |
| 当前 Space 启动/恢复 | 当前会话激活完成且存在持久恢复任务 | 重建 deadline、推进任务 | 网络已连接或密钥已齐全 |
| 缺失历史接纳 | 完整签名/祖先验证并 CAS 持久提交成功 | 推进对应本地效果 | 密钥已安装、sender pending 已成功 |
| 前置密钥应用完成 | 保存与实际安装均成功 | 重试下一因果更新 | 任意较新消息均合法 |
| 合格成员主动交流 | 实际认证、当前准入通过的成员交换完成 | 提前尝试历史/发送 | 明确拒绝自动解除 |

恢复任务存在时采用 2、4、8、16、30 秒退避，之后封顶 30 秒；复用原维护调度器 deadline，
不用单独永久轮询。真实进展才重置退避；重复通知合并到一次提前唤醒，普通账本提交不能持续
把 deadline 重置到 2 秒造成忙循环。失败后推进退避；任务完成、成员撤销或 Space 关闭取消。
重启首次恢复可立即执行，再按该序列退避。网络在线、发现候选、前台恢复本身不是 readiness。

这是一条调度 heuristic，不是安全规则：输入为任务存在、最近进展和四类事件；无进展失败案例
只能继续有界重试，不能因此放宽授权。未来调整时间常数只改变调度策略，不改变成功定义。

### 崩溃与并发

- 历史未完整验证：不推进可信历史或密钥；恢复暂存和现有 transfer 去重规则。
- 4 已保存但未安装：恢复 owner 从持久材料重装并验证，不能再对相同状态盲目 apply commit。
- 4 已安装、5 未完成：保留 4；从 5 继续，不回滚到 3，不重复发送所有历史。
- 安装完成、ACK 丢失：原 payload 重试需校验对应证据，返回成功而不重复推进 MLS。
- 两个来源同时补缺口：按 Space/分支和当前进度合并，一个 coordinator 排序；CAS 冲突重新读事实。
- generation 改变：停止旧网络交换，收束已经开始的 local work；旧结果不得提交到新会话。

### 观测

遵守[业务记录组织标准](../../design-docs/observability.md#业务记录组织标准)。
完整发送、接收和恢复的触发、动作、终态由 owner 延续不透明上下文；底层拨号、页传输和清理
作为运行诊断，不自动升级为独立业务动作。Engine 只装饰既有完整 port 的输入和输出做持续计时。
不记录 payload、身份、设备名、地址、路径、密钥或历史摘要，不增加查询内部阶段的公开 API。

# 6. Implementation Plan

下面定义修改位置；阶段门禁、命令和记录模板见独立执行计划。

```text
Step 1:
File: 现有成员、安全、协议测试；uc-infra-profile/src/space/membership_record/codec/
Change: 固定旧 wire/ACK 和 v1–v5 fixtures；写先失败的缺前置、错误重放、保存后安装断点测试；确定最小应用证据。
Risk: 不能用现有 epoch 快捷返回或历史 ACK 伪造成功证据；不得在证据设计未定时先改持久格式。

Step 2:
File: uc-sync-protocol/src/ 下新增 group_update topic；原 group_update_adapter.rs
Change: 机械迁移 codec，保留格式向量、尺寸与版本映射；新增传输无关完整交换。
Risk: 提取时改变 marker、trace、长度、旧端 Unavailable 表达会破坏兼容。

Step 3:
File: uc-application/src/connectivity/；uc-infra-p2p/src/network/iroh/connectivity/
Change: 有限协议 profile、实际协商能力、独立组更新 manager 和双版本 SingleExchange。
Risk: 复用 DeviceId 缓存导致跨协议连接误用；新旧 profile 必须共用权威准入。

Step 4:
File: uc-application/src/space/membership/ 下新增 group_update/；uc-infra-security/src/{access.rs,membership_update.rs,group_update_error.rs}
Change: 完整 receiver 和唯一 apply coordinator；区分缺前置、损坏、明确拒绝；直接入站与历史效果共用协调。
Risk: 独立 mutex 会留下双入口竞态；取消不能遗弃 blocking local work。

Step 5:
File: membership/{record.rs,worker.rs,group_update_delivery.rs,handle_history_message/}；uc-infra-profile/src/space/membership_record/
Change: 持久恢复需求、顺序进度、四事件和有界退避；必要时统一最终 v6 加密迁移。
Risk: 历史 ACK 提前结清、错误解除 ByReceiver、伪造迁移证据、频繁唤醒忙循环。

Step 6:
File: worker/history_sync.rs；membership_history_exchange_adapter.rs；相关 Core 历史规则与 sync-protocol topic
Change: 沿用 /4 逐页交换，复用已验证历史同步能力执行主动恢复；不改线上版本和字节。
Risk: 误用冲突消息作为连续后缀；突破既有帧/页/暂存限制。

Step 7:
File: uc-engine/src/assembly/sync_engine.rs；uc-infra-p2p/src/network/iroh/node.rs；旧 group_update_adapter.rs
Change: 接入真实生产收发、关闭和 generation fencing，删除被取代拨号/接收业务路径。
Risk: 测试使用新 owner 而生产仍注册旧 handler；空依赖构造导致启动窗无授权。

Step 8:
File: uc-engine/tests/space_membership_auto_pairing_e2e/group_update_delivery.rs；tests/hosts/connectivity/；现有网络/安全/profile 测试与设计文档
Change: 完成真实系统矩阵、确定性 G2、旧端互通、失败恢复与根门禁；回写长期事实。
Risk: 放宽超时或增加 tamper 次数掩盖竞态；用同版本双方冒充新旧互通。
```

# 7. Edge Cases

| Scenario | Expected behavior | Implementation |
| --- | --- | --- |
| 空、截断、超长、未知 marker/ACK | 有界失败，无应用及持久推进 | codec 长度前置检查与旧格式回归 |
| 5 缺 4，当前停在 3 | 暂不可用、主动恢复 | 结构化缺前置分类、合并恢复记录 |
| 空/损坏本地 group state | 进入相应本地故障恢复，不能当连续缺口 | 区分 security 错误来源 |
| 同 epoch 不同非法 payload | 拒绝，不能快捷 Accepted | 真实签名/commit/证据校验 |
| 已正确安装的确切重复 | 幂等成功，无二次 MLS 推进 | 绑定 Space/分支的验证应用证据 |
| 未完整收到历史或中途断网 | 保持已有可信进度，后续恢复 | 既有 transfer 暂存、完整验证和 CAS |
| 历史分叉 | 既有分支恢复规则，无跨分支拼接 | Core 祖先/签名判断 |
| 来源被撤销，或仅陌生候选 | 拒绝授权；不接收安全更新 | 当前成员准入与提交前重检 |
| 4 完成后 5 失败 | 保留 4、继续 5 | 顺序效果及安装检查 |
| 已保存未安装/ACK 丢失 | 恢复重装/幂等确认，不谎报成功 | 持久阶段及 coordinator |
| 明确拒绝后只有 Online/主动询问 | 阻断不自动解除 | 保留 ByReceiver；事件仅唤醒 |
| 正确历史恢复明确拒绝 | 应用成功后可重试正确 payload 并确认 | 验证事实驱动恢复，发送端实际 ACK 结清 |
| `/1` 无暂不可用 ACK | 关闭，发送端按原失败兜底 | 禁止发送明确拒绝 ACK 代替 |
| Clipboard 与组更新同一 peer | 不共用错误协议连接 | 独立 manager/profile |
| 两来源同时恢复/会话切换 | 合并、串行及 generation fencing | 唯一 owner、CAS 和取消收束 |
| 全部节点暂不可用 | 封顶退避，不绕过信任规则 | 原 scheduler deadline |
| 超 64 页或暂存 16 MiB | 明确有界结果，不无限内存/循环 | 保留既有边界，不能静默吞掉未覆盖目标 |

# 8. Testing Strategy

每项需记录输入、操作、预期和实际结果。真实安全/持久能力不能由内存 fake 的成功代替。

## Unit Test

| 输入 | 操作 | 预期 |
| --- | --- | --- |
| 冻结 `/2`、`/1` 正常、trace、边界帧 | 新旧 codec 双向解码/编码 | 兼容字节与 ACK 完全相同 |
| 缺前置/损坏/非法 commit/安装 I/O 错误 | 分类与 ACK 映射 | 暂不可用、拒绝及 source chain 分别准确 |
| pending 与四类事件、重复通知 | 可控时钟推进维护调度 | 2/4/8/16/30 秒、合并、无任务无轮询 |
| 同 peer 两协议与两个 generation | 拨号、排队、关闭旧会话 | 无跨协议复用，无旧结果污染 |
| v1–v5 和损坏 fixtures | 解码迁移、最终格式回读 | 一次到最终格式、AEAD、不补造证据 |

## Integration Test

| 输入 | 操作 | 预期 |
| --- | --- | --- |
| Linux 真网络、真实 SQLite/MLS 的 A/B/C，B 停 3 | A 发送 5；B 联系合格 C 恢复 4、5 | 历史验证、实际顺序安装、原 pending 最终成功 |
| 相同 epoch 的篡改/错误分支更新 | 通过真实 Uni 入站发送 | 不被 epoch 快捷返回误接受 |
| 在材料保存/安装/ACK 边界注入故障 | 杀进程并重启、重新发送 | 保持已完成进度，重装及幂等，未完成不提前确认 |
| 当前成员撤销/Space 切换时在途接收 | 暂停提交再推进切换 | 新资格与 generation 生效，旧工作有界收束 |
| 固定 main 旧端与新端，双方向 | `/2`、`/1` 单次交换及缺口失败恢复 | 真实协商和准确 ACK，不自动安全降级 |
| 原生接收端不关闭连接 | 等待 deadline、停止 Space | 无无限 handler 或后台任务残留 |

## Regression Test

| 输入 | 操作 | 预期 |
| --- | --- | --- |
| G1–G4 原成员更新场景 | 原真实 Engine 回归 | 原收件人、顺序和失败责任保留 |
| G2，正常历史旁路被确定性 gate 暂停 | 真正发送篡改更新，确认拒绝，再仅释放合格联系 | 不因联系解除安全拒绝，后续更新不越过阻断 |
| 同一 G2，再显式释放正确历史恢复 | 完整验证、安装，重试原正确更新 | 允许恢复且以实际成功确认结清；不要求恢复后永久阻断 |
| 第二切片 Clipboard 多流、重启与授权用例 | 原集合重跑 | 新 profile 无跨协议污染，现有能力保持 |
| membership workspace 历史失败记录 | 全套与隔离复核 | 保留结果和原因证据，不把隔离通过当全套通过 |

G2 必须确认 tamper 在真实发送处被消费；不得增加故障次数、sleep 或 timeout 来隐藏
提前消费与旁路应用。测试同步只用既有测试控制或 devtools-only hook，不增加生产 facade。
独立进程 host 若需新 fault 操作，只扩展其 devtools 映射，不要求旧端支持新测试命令。

设备矩阵：Linux 独立进程、真实网络/SQLite/MLS/旧端互通属于必需；桌面和 iOS/Android/
HarmonyOS 真机、网络切换、休眠与前后台单列。未执行项目记“跳过”，不能以 Linux 通过替代。

# 9. Acceptance Criteria

* [x] 批量范围已明确：沿用 `/4`，本次不新增批量协议版本。
* [x] 生产完整组更新收发通过 Uni；旧原生拨号和接收业务路径已删除。
* [x] 接收业务在 Application 唯一 owner；历史效果与直接更新共用应用协调。
* [x] 当前认证成员授权、generation fencing、source chain 和有界关闭均有真实测试。
* [x] 缺前置更新可主动恢复，损坏/非法状态不会被无条件当作缺口。
* [x] 历史 ACK、材料保存、安装成功和发送结清四个事实未混淆。
* [x] 明确拒绝不会因联系解除；正确验证恢复后能实际应用并确认。
* [x] 保存后崩溃、第四次完成后崩溃及 ACK 丢失均从真实持久状态恢复。
* [x] `/2`、`/1` 原格式、单次交换及旧端暂不可用行为通过冻结向量和真实互通。
* [x] 有任务才退避、四类事件可提前唤醒、无永久新增轮询或 busy loop。
* [x] `/4` 逐页恢复具备真实请求证据；批量协议版本不在本次范围。
* [x] 文档明确减少往返未交付，不把逻辑恢复称为一次发送 bundle。
* [x] G2 确定性验证拒绝与正确恢复两阶段，不靠扩大故障/超时兜底。
* [x] 真实系统矩阵、根门禁、跳过项和已知失败均保留证据。
* [x] 架构事实按实际实现回写，计划完成后归档；不关闭整个 #34。

# 10. Risks and Trade-offs

- **应用证明成本**：严格幂等需要验证来源和材料安装事实。仅用 epoch 很便宜却可能错误接受恶意重放；
  本方案优先复用历史/效果事实，必要时加密保存最小证据，不引入另一套更新账本。
- **协调器范围**：统一两条 apply 入口会触及现有效果执行依赖。只统一完整安全应用责任，
  不把所有成员 worker 代码塞进单文件，也不让 Engine 编排两条路径。
- **兼容性成本**：旧 `/1` 无暂不可用结果，只能有界关闭并走现有重试；不能以假拒绝换取易实现。
- **批量成本**：本切片沿用 `/4` 一记录一往返，交付可靠恢复；减少往返与新批量版本留到后续独立任务。
- **性能界限**：整批验证不意味着无限 bundle，也不意味着密钥一次大事务应用；页数、帧、暂存
  上限继续生效。传输失败时保持已验证进度，避免重复 MLS apply。
- **历史 G2**：第二切片记录全套失败及隔离通过；目前的提前 tamper 消费和旁路应用是源码风险，
  尚未证实为全部失败根因，实施时必须用确定性证据诊断。
- **上下文负担**：codec、协议 profile、接收流程、安全能力和恢复调度各自保持明确入口。
  Core 保存规则、Application 保存顺序与结果、Infra 保存技术能力、Engine 只组装。

# 11. Open Questions

1. **已解决的范围问题（2026-10-12）**：用户选择沿用成员历史 `/4` 逐页同步；本次只实现主动补齐与可靠恢复，批量协议版本和一次发送 bundle 明确留到后续。
2. **已确定的最小证据承载**：如何复用当前可信历史和效果记录证明确切 payload 已应用，
   尤其旧材料已有 epoch 而缺证据、保存后未安装的情况？最终方案为材料中的 epoch→确切摘要证据、成员 v6 中单一恢复需求及签名移除的本机私有暂存；旧无证据材料必须匹配可信历史，并以当前 MLS exporter 解开对应 catalog、校验材料和实际重装，不能仅比较 epoch。故障断点、真实 SQLite/MLS 与进程重启证据见已归档执行计划。
   这是内部实施核对，不需要用户选择存储布局；不能因此放宽已确认的成功契约。

批量协议及一次发送 bundle 的设计与验收属于后续独立任务，本次不新增协商版本。
