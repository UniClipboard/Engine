# Spec 030 Findings

## Initial state

- 当前分支领先远端 18 个提交，并有 7 个代码文件及架构圣经未提交修改；主要属于 Spec 029/Admission 重启诊断，Phase 4/5 可能重叠，必须保留。
- `VersionedMembershipHistory` 已是签名单父历史、关系与成员事实的事实来源；新 policy 应建立在验证后的 history 上，不复制验证逻辑。
- `MembershipLedger` 已是加密原子边界；conflict record 应成为 ledger 模型的一部分而非独立明文 repository。
- 规格的 Open Questions 不阻止核心实现：本地选择语义、短期恢复包时长和 CI 分层可以采用保守内部默认，不改变公开 contract。

## Invariants

- conflict id 与传输 peer、到达顺序无关。
- branch id 只绑定 lineage 与完整目标 head。
- Removed 不能恢复旧成员实例；Absent 不是可选目标。
- 用户选择 intent 一经持久化，后台不得改写。

## Phase 1

- `current_position().history_digest` 包含完整持久历史与决定，branch id 必须同时绑定它和 head；只使用 event head 会把同一移除的 Accept/Reject 分支误判为相同。
- 共同祖先使用事件链证明，不使用 branch-specific history digest；conflict id 对排序后的两个 branch id 做摘要，因此观察顺序不影响结果。
- 本机实例在目标 `active_members` 中才可恢复；保留历史凭据但已不在 `effective_members` 中表示 Removed，需要重新配对；尚未激活或完全缺席均不可选择。

## Phase 2

- SQLite `membership_ledger_state.encrypted_payload` 已对整个 ledger 使用 profile MasterKey AEAD；conflict record 追加到 `LoadedMembershipLedger` 即进入既有密文与 transaction/CAS 边界。
- conflict record 的 `Debug` 只输出选择分类、阶段、revision 和计数，conflict/branch/peer/transition 标识全部脱敏。
- 多个 peer 的同一分支证据使用集合保存，不把 transport 来源纳入 conflict id。

## Phase 3/4

- 远端 Active 选择的 transition id 可由 conflict id 与 target branch id 领域分隔摘要稳定产生；不依赖随机数即可跨 CAS 重试和重启保持唯一。
- branch transition 使用独立七阶段 Core 状态机，source/target generation 必须不同，且只能推进到直接 successor；阶段对象随 ledger 整体加密。
- 恢复包不能信任 transport 提供的 branch 声明：验证入口必须重新解码完整目标历史、验证全部历史签名并重算 branch id。
- recipient 与授权签发成员都必须位于目标 `active_members`；只保留历史 credential 的 Removed 实例不能签发或接收恢复。
- nonce 是否已使用依赖持久状态，归 Application ledger CAS；Core 只拒绝零 nonce 并把 nonce 纳入授权签名载荷。
- 恢复包 nonce 必须绑定首次消费它的 conflict，而不能只保存一个无归属集合；这样才能区分同一流程重试与跨 conflict 重放。
- transition preparation port 只允许返回无外部副作用的 `Prepared` 计划，generation 文件写入必须留给 CAS 成功后的后续阶段。
- conflict recovery 必须位于 membership effects 之后、group update 与受限交付之前；这样先恢复已有本地安全欠账，再决定是否具备切换准备条件，且损坏状态能阻断后续权限扩展。
- peer-online 是恢复包重新获取的直接触发条件，不能像 admissions/effects 一样跳过。
- active manifest 的 `database_generation` 是当前完整数据库 generation 的真实来源；transition preparation 不应从 ledger revision 或随机 source 推断。
- recovery package 的安全签发能力尚不存在；在它能绑定当前有效成员签名、MLS 恢复密文和内容密钥目录前，不应只安装一个会稳定拒绝的 Iroh 协议外壳并宣称 transport 已完成。
- Iroh endpoint 只负责把认证连接映射成 source device；recipient instance 与 source device 的对应关系必须由 Application 使用目标完整历史重新验证。
- 恢复材料密封方式归 Infra capability，Application 只接受两个非空 opaque ciphertext，并负责 package 绑定、时效、nonce 与成员授权签名。
- OpenMLS 0.8 external commit 会按相同签名公钥自动移除旧 leaf；因此 Active recipient 可使用 sibling 状态中自己的签名私钥重新加入目标 group，不需要也不得复制目标设备的 `MlsClientState`。
- 内容密钥目录必须在 external commit 后使用新 epoch exporter wrapping key 密封：目标端先给签名 GroupInfo，recipient 返回 external commit，目标端应用到 detached state 后才得到与 recipient 相同的 wrapping key。因此真实恢复协议是两阶段握手，不是单次请求响应。
- Iroh handler 只能把认证连接公钥映射为 source device；begin 与 submit 两阶段都必须重新调用 Application issuer 复核 conflict、branch、recipient 和 Active 状态，不能让第一阶段认证结果跨网络往返隐式延续。
- 两阶段 Iroh wire 必须在两个请求中重复绑定 conflict、target branch 和 recipient；连接身份只证明 source device，不能替代 Application 对目标历史的成员资格复核。
# 2026-08-31 · 恢复事务持久化边界

- membership ledger 已是按 generation 绑定的整体 MasterKey AEAD 载荷，恢复 staged state 放入该 ledger 可复用现有加密、CAS 与重启恢复边界，不应另建明文表或文件。
- transition id 是恢复事务的稳定索引；session 内再次保存并校验它，能在反序列化和提交前识别 map key 与载荷错配。
- target 必须缓存签发后的 recovery package，才能在 external commit 已应用但响应丢失时返回同一结果；recipient 必须保留 staged MLS state，直到最终 generation 提升完成。
- 状态推进由 session 对象隐藏，recipient 与 target 角色不能互相转换，重复完成操作只接受同一绑定结果。

# 2026-08-31 · Recovery client 边界

- 现有一步 `FetchMembershipBranchRecoveryPort` 无法表达 GroupInfo 与 external commit 之间必须持久化的 recipient staged state，不能直接由 Infra 实现而不破坏重启安全。
- Iroh client 应是窄 channel：只对一个由 Application 指定的 peer 做一次认证请求。peer 选择、MLS preparation、ledger CAS 和重试属于 Application coordinator。
- 下一切片完成 coordinator 切换后应删除旧 fetch port；否则两套抽象会让 Infra/Application 边界继续含混。

# 2026-08-31 · Recovery coordinator checkpoint 顺序

- recipient external commit 属于不可安全重建的密码学结果，必须先把它和 staged MLS state 加密提交，再允许网络发送。
- 最终恢复包可以在单独 checkpoint 验证并保存；之后 generation transition preparation 即使失败或重启，也不必再次触发目标端 external commit。
- nonce 冲突只能在取得最终包后确定，因此此时 staged checkpoint 已合法存在；安全保证应表述为不覆盖 nonce、不创建 transition，而不是零 ledger commit。

# 2026-08-31 · Target apply-and-reply 崩溃窗口

- target 若在 material port 内直接持久化 external commit，然后由 issuer 才签包，会在“MLS 已前进、幂等包未保存”之间留下崩溃窗口；重试同一 commit 将无法可靠恢复。
- 正确顺序必须是：对当前 material 无副作用计算 staged target state 与 wrapping key，构造并签署 package，原子保存 TargetPrepared，提交 staged security state，标记 TargetCommitted，最后返回缓存 package。
- 因此 target material port 需要拆成 prepare/commit 两个窄动作，issuer 是该事务的唯一负责人；不能把整个事务藏进 Infra adapter。

# 2026-08-31 · Generation promote checkpoint

- target database 必须在 manifest 提升前包含 target history、安全材料、成员投影以及当前 `TargetStaged` transition checkpoint；否则动态连接切到目标库后 Application 无法继续 CAS。
- Infra 在提升并替换动态数据库后返回，Application 必须重新从当前数据库加载 ledger，再提交 `Promoted`；不能继续使用切换前的 history digest。
- package 过期只约束首次接受。nonce 已消费并建立 transition 后，重启续跑只重新验签目标历史，不因长时间文件迁移导致已经接受的事务过期。

# 2026-08-31 · Phase 5 public contract boundary

- `SpaceFacade`/`AppFacade` 已经拥有唯一 `resolve_membership_conflict` 动作，但 Engine dispatch 尚未映射；不应在 Engine 重做选择规则。
- 当前 `query_device_trust` 不包含 conflict 的候选 branch、选择资格、选择状态和 transition phase，不能作为规格要求的完整查询结果。
- 公开结果必须表达 `local_resolution_completed`，不得用全局 `resolved` 命名暗示所有设备已经收敛。

# 2026-08-31 · 统一设备组选择边界

- 对产品而言，待定成员变更和 sibling branch 冲突都是“选择继续使用哪个设备组”；分别暴露会泄漏协议原因并要求调用方维护两套页面和并发处理。
- 统一查询必须携带 revision，选择动作必须回传该 revision；Application 在执行前重新查询并拒绝过期选择。
- 远端 branch 在恢复包验证前没有可信完整成员名单，契约必须表达未知，不能仅为 UI 对称而推测设备列表。

# 2026-08-31 · Phase 6 Desktop 接缝盘点

- 当前 engine 仓库没有 Desktop CLI 或 daemon crate；workspace 中唯一二进制属于独立 LAN compatibility client，不能作为 P2P 默认能力的验收入口。
- 现有真实多节点公开接缝是 `uc-engine` integration test 中直接启动多个 `Engine` 实例；它支持真实建 Space、邀请、加入与查询，但没有 Partition、Heal、DropNextFrame 或 CrashAtPhase 驱动能力。
- 因此 F0 不能直接写成“现有 CLI 脚本”：首个 Phase 6 切片必须先在 `uc-engine` dev/test 边界建立声明式拓扑驱动器，并让动作只调用公开 Engine contract；网络分区与故障注入需要后续补充受控测试 capability，不能读写内部 ledger 冒充端到端结果。
- 当前公开快照能断言成员状态和待定选择，但不直接给出 branch/head 等价类、MLS group epoch、pending effect 数量；F0 前需要一个仅 `dev-tools` 可用、对敏感标识保持结构化且不写日志的诊断结果，否则无法满足规格的强断言。

# 2026-08-31 · Phase 6 真实网络分区接缝

- 仅在拓扑驱动器跳过动作或暂停 Engine 不能模拟 Partition：后台反熵、group update、recovery、presence 和正文各自持有 Iroh 通道，而且 F0 两侧必须能继续进行本地/分支内操作。
- Iroh 1.0 `EndpointHooks` 同时提供出站 `before_connect` 和双向 `after_handshake` 拦截；hook 可保存 `WeakConnectionHandle`，分区建立时关闭已经存在的匹配连接，从而覆盖新连接和存量连接。
- 最窄实现是在共享 Iroh endpoint 建立时注入一个按认证 EndpointId 阻断的可变 gate。Engine `dev-tools` 只负责查询本机 endpoint id 和设置阻断集合；所有业务 ALPN 自动共享该 gate，生产配置保持 `None`。

# 2026-08-31 · F3 重启就绪边界

- `Engine::start` 返回时，持久化 Space 的安全会话仍可能正在异步解锁；统一设备组查询会以可重试 `1211 unavailable` 明确表达该状态。
- Desktop 重启动作必须仅通过公开查询轮询 membership-ready，再断言持久化决定；固定 sleep 会把启动时序误当成业务结果，直接读内部 session 则会破坏端到端接缝。
- F3 分层 tracing 证明 restricted removal event 的地址解析、Iroh 连接、来源身份解析、Application 处理、`RestrictedApplied` ACK 与响应写回全部成功，排除网络链路根因。
- 根因是 restricted-event handler 直接调用 `verify_and_receive_event`：它把 `known_head` 推进到远端 RemoveDevice 并立即投影两成员；普通 history merge 则会在同一场景恢复父 head 以等待本机决定。两个接收入口没有共用“面向本机成员接收远端事件”的 Core 规则。
- F3 修复进一步发现分页 suffix 是第三条远端事件入口；它既要验证发送方声明的 target position，又不能强迫本机应用待决定移除。Core 因此用 sender projection 验证完整传输，再用唯一的 local-member 接收规则更新本机 projection。
- Engine 的选择操作返回 `retryable unavailable` 时，Desktop 驱动必须在统一 deadline 内重试；短暂安全会话不可用不是选择失败，非重试错误与超时仍立即失败。

## 2026-08-31 · F4 single-bridge topology

- F4 使用同 lineage 的六节点共同基线，再把网络分成两个三节点区域；A 与 D 各自在自己的合法链依次移除对侧三节点，形成两个各三成员的 sibling histories。区域内其他副本不需要为 bridge 不变量额外完成用户决定。
- 单 bridge 只开放 A-D 一条跨区连接，其余跨区连接继续由认证 endpoint gate 阻断。验收只观察公开 branch/member/choice 与正文结果，不读内部 ledger。
- A 与 D 必须在两条分支中都保持 Active，才能构成真正可认证的 bridge；若双方互相移除，成员认证会在业务协议前关闭连接。桥接后两端应各记录同一 sibling conflict，但不得应用或联合远端成员集合。

## 2026-08-31 · F5 ring conflict idempotency

- `MembershipLedger::exchange_conflict_evidence` 原先即使 conflict 与 evidence peer 已存在也会无条件 CAS，导致同一证据每次往返都增加 revision；幂等短路必须同时确认 conflict 已包含该来源，且 peer 已是无确认位置的 `Diverged`。
- 环拓扑中的全局 ledger revision 还会被不可达 E/F 的正常反熵退避账务推进，不能用“revision 完全静止”代表没有 conflict 消息环。精确判据是同一 evidence 不重复 commit、公开 conflict 数保持一、membership effects 不增加。
- 四个环节点必须都属于两条 sibling history 的 Active 交集；E/F 只负责制造不同分支并保持隔离，B-C 与 D-A 才是两个相反方向的冲突传播边。

## 2026-08-31 · F6 offline sponsor boundary

- 深链描述的是准入与历史来源，不代表设备承担网络路由；Iroh 成员之间仍是认证直连，中间 Sponsor 离线后其他 Active 成员可直接完成证据和恢复交互。
- branch recovery 的 external commit 由 target 生成，并作为持久 group-update 欠账定向投递给其他成员；接收者应用后不会转发该 MLS commit。因此可以用相邻链验证 evidence/recovery peer 选择，但最终安全 epoch 需要 target 能直连仍在线成员。
- F6 不应同时验收“所有中间成员离线时签发新邀请”；先形成两条合法 sibling 并让安全 epoch 收敛，再停 B/D，才能只检验 conflict 选择和恢复不依赖原 Sponsor。
- Membership branch/head 和 MLS epoch 收敛仍不足以证明正文可发；target 侧的 peer reconciliation 若被旧 conflict evidence 回退为 Diverged，发送 scope 会把 Active recipient 排除为零目标。
- 已提交的 target recovery session 是“该 recipient 已选择本分支并完成密码恢复”的唯一耐久证据；后续旧 sibling evidence 只能幂等应答，不能重新扩展冲突状态。
