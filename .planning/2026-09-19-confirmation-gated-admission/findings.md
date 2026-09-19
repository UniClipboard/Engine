# Findings

## 已知约束

- 成员资格只能来自已验证正式成员历史；待确认准入状态必须留在独立加密准入仓库。
- 同一设备可保留多个历史实例，但当前正式成员范围按设备唯一；旧异常历史不能静默重写。
- `SpaceAdmissionProtocol` 是加入、消息处理、查询、恢复和稳定结果的唯一完整负责人。
- 正式 Add 一旦提交不回滚，因此唯一提交点必须位于无法再安全拒绝之后。
- 现有长期设计已写明新实例只有在激活回执验证后才能接替旧实例；当前行为与此不变量冲突。

## 待定位

- Sponsor 当前在哪个消息阶段创建并提交正式 Add。
- Joiner 最终确认请求与 Sponsor 最终确认回复的精确消息和持久状态。
- 安全组加入与正式历史 Add 的顺序，哪些材料必须在提交前暂存。
- 当前 wire 版本如何识别新旧对端，是否可在正式写入前拒绝旧版本。
- 现有故障注入 seam 能否精确丢弃最终请求或回复。

## 初步代码地图

- 当前消息顺序为 JoinRequest → Candidate → Prepared → Commit → Applied → Complete → CompleteAck → Settled。
- Sponsor 在处理 Applied 时调用 `sponsor/complete.rs`；该实现已经验证激活回执，并准备 Complete。
- Joiner 收到 Complete 后在 `joiner/activation.rs` 执行本机激活并生成 CompleteAck；Sponsor 收到 CompleteAck 后只准备 Settled。
- `sponsor/complete.rs` 同时处理正式 Add 的效果，极可能是正式成员写入提前于 CompleteAck 的位置；需要继续核对 Application 的保存顺序。
- 现有工作区有 28 个已修改/新增路径，包含上一轮加入、恢复、旧身份清理和文档修复；本轮必须在这些改动之上工作，不能回滚。
- 首个责任断点已确认：`SponsorAdmissionService::handle_applied` 在收到 Applied 后、保存 Complete 回复前就调用 Sponsor 激活；激活会写安全状态、成员事实和正式成员 ledger。此时 Joiner 尚未发送 CompleteAck。
- Core 的 `complete_applied` 同时把效果标成 `ActivateSecurity + PublishMembership`；`settle_complete_ack` 反而没有效果。这使正式 Add 的提交点位于最终确认之前。
- 当前消息结构已经具备正确两阶段形状：Sponsor 可先持久保存 Complete 与待激活材料，Joiner 收到 Complete 后完成本机激活并发 CompleteAck；因此最小方向是把 Sponsor 的正式激活移到验证 CompleteAck 之后，并保留 Applied 状态供丢包/重启重放。
- Sponsor 激活实现本身包含安全状态激活、成员事实和 ledger 的幂等检查；若准入记录最终提交发生冲突，CompleteAck 重试可以再次调用同一激活能力，但仍需用测试证明所有子步骤均幂等。
- 现有 Application 测试 fake 的 Sponsor 激活不记录调用时机；增加一个明确事件即可先锁定“Applied 不激活、CompleteAck 才激活”的协议级红测。
- Engine E2E 需要确定性停在 Joiner 已收到 Complete、但尚未发出 CompleteAck 的位置。现有网络分区只能按连接阻断，无法可靠命中同一连接内的最终消息；应复用仓库已有故障注入模式，在 dev-tools 下给 Joiner 本机激活后、CompleteAck 发送前设置可等待/释放的测试闸门，避免靠竞态轮询。
- Engine 当前只把网络分区控制器贯穿运行时和 dev 操作；加入激活能力在 `sync_engine` 组装时直接构造。若新增完整 E2E 闸门，应由 Engine dev-tools 装饰既有完整 Joiner 激活能力并把同一控制器交给 dev dispatch，不能把测试开关加入 Application/Core 公开接口。
- 为保持最小切片，先让 Application 完整协议红测证明提交时机，再决定三实例闸门的最小装配范围；不修改正式协议后再补测试会失去红色证据。
- Application 红测已稳定失败在 Sponsor 处理 Applied 后已经触发正式激活，证明测试对当前错误行为敏感。
- Engine 会在每次 Space 会话准备时构造 Joiner 激活执行器；dev 闸门若沿 `prepare_daemon_session → prepare_sync_session` 传入，会同时覆盖首次会话和重启后的新会话，并可由 `ProductionRuntime` 持有控制器供测试操作。
- 最合适的暂停点是装饰 `ExecuteJoinerActivationPort`：先让真实 Joiner 本机激活完成，再在返回 Application 前暂停；此时 CompleteAck 尚未持久化/发送，而旧 Sponsor 已在 Applied 阶段提前传播，能稳定复现第三设备可见错误。
- 闸门只存在于 `dev-tools` 的 Engine 组装与 dev 操作，不进入 Core/Application 正式接口，也不改变非测试构建。
- 三实例红测已稳定停在 Joiner 本机激活后、CompleteAck 前。Sponsor 的正式成员数立即从 2 变成 3，证明完整运行路径同样提前提交；在线第三设备在首个 5 秒观察窗内尚未收到该历史，不能据此断言永远不会传播，需要覆盖维护周期再判断。
- 扩展到 35 秒后第三设备仍未自动取得新历史，但 Sponsor 始终已正式提交。为了覆盖用户要求的传播和 Sponsor 先重启，将在暂停窗口重启 Sponsor；启动维护若从已提前提交的 ledger 向第三设备同步即可稳定暴露第二段错误，同时验证修复后的待确认记录可跨 Sponsor 重启继续。
- Sponsor 重启并公开刷新连接后，第三设备仍未自动获取新历史；成员维护的目标同步由已认证 peer contact 精确唤醒。下一次用第三设备向 Sponsor 发送普通内容制造真实认证接触，再观察 Sponsor 是否把已提前提交的历史同步回第三设备。
- 真实认证内容接触也未在 15 秒内让第三设备取得新历史；当前红测仍完整覆盖三设备视图：第三设备在暂停窗口保持 2，而 Sponsor 错误显示 3。修复后两者都必须保持 2，释放确认后两者都到 3。
- 当前错误不依赖是否立即传播：一旦 Sponsor ledger 已提前写入，后续任意合法历史导出都可能携带该成员。把正式提交移到 CompleteAck 才能从事实来源上消除传播风险，单独抑制同步不是修复。
- 正式提交点移动到 Sponsor 验证 CompleteAck 之后后，Application 协议回归通过：处理 Applied 只保存待确认材料，不再激活；处理 CompleteAck 才激活正式成员。
- 三实例回归通过：Joiner 本机激活后、CompleteAck 前暂停，期间重启 Sponsor、公开刷新并让第三设备与 Sponsor 真实通信，Sponsor 与第三设备都保持 2 名正式成员；释放确认后三方才收敛为 3 名。
- 旧实现与新实现使用同一 V2 协议号，因此只移动提交点仍无法阻止新版 Joiner 连到旧 Sponsor 后被旧实现提前提交。当前协议提升为 V3，初始认证和后续消息都只接受当前版本。
- 两个真实通道混合版本回归通过：新版 Joiner 遇到旧 Sponsor 时在认证前得到稳定的升级结果；V2 Joiner 遇到新版 Sponsor 时在凭据消费和业务处理前被拒绝，endpoint 调用数保持 0。
- CompleteAck 请求失败时，Joiner 的加密记录保留同一待发送请求；重启后恢复同一尝试。Sponsor 激活成功但准入记录提交竞争失败时，重发同一 CompleteAck 幂等命中已经完成的成员提交，再保存 Settled；测试确认只产生一次正式激活事件。
- 完整 Joiner 重启场景通过：本机激活后暂停最终确认，双向网络隔离使 CompleteAck 无法送达，对外状态为“处理中”、Sponsor 仍只有 1 名正式成员；Joiner 正常关闭并重启、网络恢复后同一尝试完成，成员数变为 2 且正文可传输。
- 公开加入状态新增 Processing：初始待处理继续是 Pending，本机已准备但尚未收到 Settled 是 Processing，Settled 持久化后才是 Active；拒绝和终止保持独立结果。
- 公开设备快照原有 `pending_inbound_member` 字段此前未接入准入仓库，邀请方无法区分待确认候选与“什么都没有”。现已从加密保存的 Applied 记录投影该字段，并明确不把候选加入正式 `devices`。
- 严格审查发现 V3 初始帧与消息校验已经升级，但 OPAQUE 尝试上下文仍写死 V2；双方都使用同一旧值所以普通场景不会失败，却没有把当前协议版本纳入身份校验。现已绑定 `CURRENT` 并用独立测试锁定，旧新版仍在读取凭据前稳定拒绝。
- Sponsor 正式提交包含安全组采用、成员事实投影和成员账本提交，无法依靠单库事务跨越全部存储；现有负责人通过相同激活材料和账本历史摘要实现幂等。已覆盖正式成员提交成功但准入终态保存竞争失败后重放同一 CompleteAck，只产生一个正式成员。
- `pending_inbound_member` 继续遵守既有单准入槽契约；若仓库出现两个非终态 Sponsor Applied 记录则按损坏关闭，不能任意选择其中一个。
