# 发现：Space Application 用例全量梳理

## 初始发现

- Space 当前有 11 个标准 `use_case.rs`：initialize、unlock、lock session、recover session、reset、rebuild、upgrade、query access、query membership、initiate removal、decide removal。
- Space 的完整行为数量大于 11：issue/redeem/cancel invitation、join/cancel join、ensure reachable、network recovery、presence refresh、旧开发工具查询与决定仍定义在其他文件形态中。
- admission handshake、durable transaction、session activity、membership runtime 属于内部流程或后台运行期候选，不能因为有公开方法就直接提取成用例。
- “有 `UseCase` 后缀”不是唯一判断标准；需要从 Engine Space 操作、Space Facade、Profile Space Facade 和后台入口反向建立行为清单。
- 当前只稳定 Space application 行为和接口，其他 application 领域与 infra 实现暂缓。
- 刚才的架构检查会隐式执行 `openmls-validation` 构建；已停止相关进程，后续在用户解除限制前不再运行。

## 分类口径

- 标准用例：一个入口负责一个完整结果。
- 内部流程：只服务某个用例，不应独立暴露。
- 后台运行期：负责触发、合并、重试、暂停和关闭。
- 共享模块：隐藏多个用例共同需要的复杂规则或可靠访问。
- Facade：对外提供完整动作和稳定结果，不逐步编排。
- 纯转发：删除后行为几乎不变，应合并或重新划分。

## Space 生命周期第一批清单

| 行为 | 当前负责人 | 当前判断 | 后续动作 |
| --- | --- | --- | --- |
| 初始化 Space | `InitializeSpaceUseCase` + `SpaceFacade` + `AppFacade` | 标准入口已存在，但完整成功仍由三层拼接 | 收口存储就绪与 Space 活动恢复，Facade 只转换输入输出 |
| 解锁 Space | `UnlockSpaceUseCase` + `AppFacade` | 用例已负责解锁与数据准备，但 Space 活动恢复仍在 Facade | 将活动恢复纳入完整解锁结果 |
| 锁定 Space 会话 | `LockSpaceSessionUseCase` | 完整用例，包含暂停、锁定失败回滚和稳定错误 | 保持 |
| 静默恢复 Space 会话 | `RecoverSpaceSessionUseCase` | 完整用例，包含恢复、数据准备和活动恢复 | 保持 |
| 重置 Space | `ResetSpaceUseCase` | 完整用户命令，取消邀请后执行可恢复重建 | 保持 |
| 重建 Space | `RebuildSpaceUseCase` | 完整可恢复内部流程，不是独立产品动作 | 保持为 reset/upgrade 的内部用例 |
| Engine 版本升级 | `UpgradeSpaceUseCase` | 完整内部迁移流程，由会话就绪阶段调用 | 保持内部，不作为 Facade 用户动作 |
| 查询 Space 访问状态 | `QuerySpaceAccessStateUseCase` | 完整只读用例 | 保持 |
| 查询 Setup 状态 | `QuerySpaceSetupStateUseCase` | 已提取完整产品查询 | 保持，Facade 只调用 `execute()` |
| 查询重置是否已提交 | `SpaceFacade::has_committed_device_management_reset` | Session Supervisor 使用的完整内部查询藏在 Facade | 迁入 reset 模块，单独评估名称和结果 |
| 会话数据准备 | `PostSessionReadiness` | unlock/recover 共用的内部流程，不是用户用例 | 保持内部，避免独立暴露 |
| Space 活动开关 | `SpaceSessionActivity` | lock/recover/initialize/unlock 共用的运行协调 | 保持内部，由完整生命周期用例调用 |

## 生命周期关键发现

- `InitializeSpaceUseCase::execute()` 完成加密 Space、身份、成员基线和当前 Space 激活后，`SpaceFacade` 仍执行关系存储读取与 presence 激活，`AppFacade` 再恢复 search、receive 和 membership 活动；对外成功由三层共同完成。
- `UnlockSpaceUseCase::execute()` 已负责升级、回填、成员仓储检查和 presence 激活，但 `AppFacade` 仍在返回前恢复 search、receive 和 membership 活动。
- `RecoverSpaceSessionUseCase` 已把会话恢复、数据准备和 Space 活动恢复放在同一入口，可作为 initialize/unlock 的完整性参考。
- `LockSpaceSessionUseCase` 在锁定失败时恢复已暂停活动，隐藏了调用顺序和补偿逻辑，属于有效的深模块。
- `QuerySpaceAccessStateUseCase` 只回答是否初始化与会话是否就绪；`query_setup_state` 还回答当前邀请、设备名和重新配对要求，两者不是重复查询。

## 生命周期整改顺序

1. `QuerySpaceSetupStateUseCase` 已完成提取。
2. 收口 initialize 的关系就绪与活动恢复，删除两个 Facade 的后处理。
3. 收口 unlock 的活动恢复，参照 recover 的完整入口。
4. 将重置提交状态查询迁入 reset 模块。
5. 最后复核 `PostSessionReadiness`、`SpaceSessionActivity` 的命名和可见范围，不把它们提取成用户用例。

## 已完成：Query Space Setup State

- 模型、错误和四项读取编排已从 `SpaceFacade` 迁入 `space/query_space_setup_state`。
- 用例唯一入口为 `execute()`，输入为空，成功返回当前 Space、邀请、设备名和重新配对要求，读取失败返回稳定查询错误。
- `SpaceFacade` 只保留一次转发，原 `uc_application::facade` 导出路径保持兼容。
- 新增全新安装、已完成状态和待处理邀请三个用例级回归；按当前约束未运行构建或测试。

## Space 准入第一批清单

| 行为 | 当前负责人 | 当前判断 | 后续动作 |
| --- | --- | --- | --- |
| 签发普通邀请 | `admission/invitation/issue` | 完整用户用例 | 保持，后续迁出 Facade 命令模型 |
| 按地址签发邀请 | `admission/invitation/issue_for_address` | 已提取独立 dev 用例 | 保持内部，不向正常产品暴露 |
| 查询邀请地址 | `admission/invitation/query_addresses` | 已提取独立查询用例 | 保持，普通 issue 不再依赖查询 Port |
| 取消邀请 | `admission/invitation/cancel` | 已提取完整用户命令 | 保持，Facade 只调用 `execute()` |
| 加入 Space | `SpaceAdmissionCoordinator::join_space` + `RedeemPairingInvitationUseCase` + Engine | 完整结果跨 application 与 Engine 拼接 | 建立 `JoinSpaceUseCase`，收口设备名、准入与持久结果 |
| 执行邀请兑换 | `RedeemPairingInvitationUseCase` | Join Space 内部通信与持久化流程 | 保持为 Join Space 内部用例，不直接暴露 |
| 取消本机加入 | `CancelSpaceJoinUseCase` | 已提取完整用户命令 | 保持 |
| 查询当前加入 | `QuerySpaceJoinStatusUseCase` | 已提取持久准入投影查询 | 保持；普通 Join 直接返回自身结果 |
| 完成跨 Space 会话切换 | Application transition recovery + Engine Session Supervisor | Engine 必须拥有会话 drain/rebuild，application 拥有持久转换 | 保留跨层协作，但让 Join 结果明确是否需要切换，避免完成后再次探测 |
| 重建并发送完成确认 | `pending_joiner_complete_ack` + `deliver_join_completion_ack` + Session Supervisor | 重启可恢复的会话安装后动作，当前分散在两个 Facade | 后续收口为一个明确的会话安装后准入恢复入口 |
| Sponsor 入站处理 | `SponsorAdmissionOrchestrator` | 后台运行期，不是用户用例 | 保持内部 Runtime |
| Joiner/Sponsor handshake | 两个 handshake coordinator | Join/入站准入的内部协议流程 | 保持内部，不独立暴露 |
| Durable admission transaction | `DurableAdmissionTransaction` | 保存检查点、幂等推进和恢复的共享 durable workflow | 保持深层内部模块，不把每个阶段提取成用例或 Port |

## 准入关键发现

- `SpaceAdmissionCoordinator` 只有 `join_space` 包含真实编排；其余五个方法只是转发到 `SpaceFacade`。应替换为单一 `JoinSpaceUseCase`，而不是保留“准入总协调器”。
- `IssuePairingInvitationUseCase` 同时暴露普通签发、按地址签发和地址查询三个入口，混合产品命令、开发工具命令和查询，应拆成三个行为。
- `cancel_invitation` 已完整实现“没有邀请时报冲突、存在时全部取消”，但仍写在 Facade。
- `cancel_join_space` 已完成持久取消、profile 修订读取和事件通知，但仍写在 Profile Facade。
- Engine 当前丢弃 application `join_space` 的返回值，再通过 `ProfileSpaceAdmission::current_join` 查询一次持久状态；application 的 Join 结果没有成为单一事实出口。
- 跨 Space 加入成功后，Engine 再调用 `requires_session_transition()` 决定是否 drain 当前会话。该事实在 `RedeemPairingInvitationUseCase` 内已经计算过，应随 Join 结果返回，避免成功后重复探测。
- Engine 必须负责关闭和重建完整运行会话，因此跨 Space 切换不可能完全藏进 application；application 应只暴露“是否需要切换”和“会话已 drain 后继续恢复”的明确接口。
- Port 迁入 application 时不能只迁 trait：仓储错误、原子变更、查询投影、投递结果，以及安全和空间切换的请求/结果也属于该接口，继续放在 core 会让 application 的接口仍由 core 塑形。
- `SponsorAdmissionSecurityDelivery` 是例外：它被序列化进可恢复准入记录，属于持久领域状态，不是单纯的 Port 返回值，因此留在 core 的准入记录模块，由 application 的安全接口复用。

## 准入整改顺序

1. Cancel、地址查询和按地址签发均已完成拆分。
2. 已将 `SpaceAdmissionCoordinator` 收敛为 `JoinSpaceUseCase`，并删除其余转发。
3. 让 Join Space 直接返回持久 `CurrentJoinStatus` 和是否需要会话切换，删除 Engine 成功后的重复查询与探测。
4. 提取 `CancelJoinSpaceUseCase`，移出 Profile Facade。
5. 收口会话安装后的完成确认恢复入口。
6. 保持 handshake、durable transaction 和 sponsor orchestrator 为内部流程，不按阶段拆成公开用例。

## 已完成：Cancel Pairing Invitation

- 用例唯一入口为 `execute()`，输入为空；至少一个待处理邀请被清除时成功，holder 为空时返回 `NotIssued`。
- 错误归用例模块所有，原 `uc_application::facade` 导出路径保持兼容。
- `SpaceFacade` 不再直接操作或持有取消所需 holder，只保留用例字段和一次转发。
- Facade 回归改为通过 Setup 状态观察邀请是否清除，不再读取内部字段。
- 新增 holder 为空与多个邀请全部取消两个用例级回归；按约束未运行构建或测试。

## 已完成：Invitation 子域内聚

- `issue` 与 `cancel` 已迁入 `space/admission/invitation/`，与共享 `holder` 放在同一子域。
- admission 根目录旧 `issue_invitation.rs` 和 Space 顶层旧 `cancel_pairing_invitation/` 已删除，不保留转发模块。
- 后续 invitation address query 直接建立在同一子域。
- `QuerySpaceSetupStateUseCase` 继续位于 Space 顶层：它虽然读取 holder，同时还组合当前 Space、设置和重新配对要求，不属于纯邀请行为。

## 已完成：Query Pairing Invitation Addresses

- 查询用例位于 `admission/invitation/query_addresses`，唯一入口为 `execute()`。
- 输入为空，成功返回当前可签发地址候选；网络未启动、服务不可用、地址不可用和内部失败使用独立查询错误。
- 普通 `IssuePairingInvitationUseCase` 已删除地址查询 Port、字段、构造参数和 `list_addresses()`。
- Space/Application Facade 改为返回查询专用错误；Engine dev 操作仍只转换地址结果或稳定内部错误。
- 新增候选返回和网络未启动错误两个用例级回归；按约束未运行构建或测试。

## 已完成：Issue Pairing Invitation For Address

- dev 用例位于 `admission/invitation/issue_for_address`，唯一入口接收明确 IP 并返回现有邀请结果。
- 普通 issue 用例已删除 by-address Port、字段、构造参数和第二个 execute 入口。
- 两个签发用例共享内部 `PairingInvitationIssuer`，统一负责准入门禁、分析事件、领域邀请生成和 holder 保存，没有复制完整流程。
- 既有指定地址回归已改为直接执行 dev 用例；Engine `DevOperation::IssueInvitationForAddress` 外部行为不变。

## 已完成：Join Space 用例入口

- `JoinSpaceUseCase` 是加入 Space 的唯一 application 命令入口，依次负责可选设备名保存、加入前尽力激活连接，以及执行邀请兑换。
- 设备名为空时在联网前失败；设置保存失败时不开始准入。连接预热失败不会遮蔽后续邀请兑换给出的可操作错误。
- `SpaceAdmissionCoordinator` 已删除；邀请签发、地址查询、直接兑换和取消不再经过总协调器转发。
- `AppFacade` 只经 `SpaceFacade` 暴露这些完整动作，不再额外组装准入行为。
- 本步尚未改变 Join 的成功结果。Engine 成功后重复读取持久加入状态、再次判断会话切换的问题，按路线图在下一小步处理。
- 加入方邀请兑换只服务 Joiner，不属于 admission 根级共享流程；已迁入 `admission/joiner/redeem_invitation.rs`，与加入方握手共同由 `joiner` 模块隐藏。外层 `JoinSpaceUseCase` 不再依赖 admission 根级具体文件。
- 加入和跨 Space 切换之间存在必须持久化的中断边界，因此对内拆为两个完整用例：Join 保存并返回当前加入状态和明确切换要求；`CompletePendingSpaceTransitionUseCase` 只在旧会话关闭后推进切换，并要求最终状态为活动。Engine 不再扫描准入记录解释是否切换，也不再在正常加入后补查状态。
- 程序启动时仍需判断是否存在已经准备好的切换，因为此时没有上一次 Join 返回值；该内部查询通过 Space Facade 完成，确认后调用同一个完成切换用例，不恢复 Engine 对准入内部对象的直接依赖。
- `durable/flow.rs` 曾把加入方、邀请方、取消加入和空间切换 17 个入口实现到同一个负责人上；现已按协议角色拆除。正常握手与重启恢复继续复用同一份加入方或邀请方可靠推进规则，不把每个协议阶段伪装成产品用例。
- 加入方与邀请方不再共同依赖一个包含全部协议步骤的接口。两侧接口分别位于自己的模块，测试替身也只实现对应角色所需能力；共享 adapter 文件只保留两侧共用的值和稳定请求绑定。
- `ProfileSpaceAdmission` 已删除。`SpaceJoinFacade` 只组合查询当前加入、取消加入和恢复完成确认三个 profile 范围用例；`SpaceMembershipFacade` 只组合成员状态查询、发起移除、决定移除和活动 Space 接入。Engine 不再通过一个名称模糊的总对象混用两类能力。
- core 中原有准入仓储、投递、完成恢复、空间切换、安全切换和成员材料 Port 均没有 core 领域调用者；它们已迁入 application 并由 infra 直接实现。成员历史验签仍留在 core，因为版本化成员历史的验证规则直接使用它。
- `GroupAdmissionPort` 原有成员接纳和安装方法没有 application 调用者，迁移后删除；新的加入方接口只表达准备加入、读取准备中公开凭据和使用准备中身份签名，避免继续暴露底层 MLS 管理全集。

## Space 成员关系第一批清单

| 行为 | 当前负责人 | 当前判断 | 后续动作 |
| --- | --- | --- | --- |
| 查询成员关系状态 | `QuerySpaceMembershipStatusUseCase` | 完整产品查询 | 保持 |
| 发起成员移除 | `InitiateSpaceMemberRemovalUseCase` | 完整产品命令 | 保持 |
| 决定待处理移除 | `DecidePendingMembershipRemovalUseCase` | 完整产品命令 | 保持 |
| 查询设备列表 | Engine + `MemberRosterFacade::list_with_presence` | 初始化判断和列表查询跨层拼接 | 提取 `QuerySpaceDevicesUseCase`，application 决定未初始化结果 |
| 查询成员同步偏好 | `MemberRosterFacade::get_sync_preferences` | 完整查询藏在 Facade | 提取独立查询用例 |
| 更新成员同步偏好 | `MemberRosterFacade::update_sync_preferences` | 完整命令藏在 Facade | 提取独立命令用例 |
| 查询 Space 保护状态 | `MemberRosterFacade::query_space_protection` | 完整查询藏在 Facade，且使用全量资料投影作为输入 | 提取并重新核对当前成员范围 |
| 查询 peer 快照 | `MemberRosterFacade::list_peer_snapshots` | 完整宿主查询藏在 Facade | 提取独立查询或与设备列表统一产品模型 |
| 订阅 presence | `MemberRosterFacade::subscribe_presence_events` | 事件订阅，不是用例 | 保持 Facade 订阅能力 |
| 旧成员决定 | `WorkspaceMembership::decide_membership_removal` | 仅 dev-tools 使用，与新决定用例并行 | 最终删除或让 dev-tools 复用新用例，不保留第二套生产逻辑 |
| 旧收敛快照查询 | `WorkspaceMembership::query` | 仅 dev-tools 与旧内部流程使用 | 评估诊断需求后删除或改为明确诊断投影 |
| 成员历史交换与接收 | `WorkspaceMembership` history endpoint | 已认证网络端点内部流程 | 保持内部 endpoint，不提取为用户用例 |
| 成员效果恢复与决定投递 | `WorkspaceMembershipRuntime` | 后台恢复流程 | 保持 Runtime，后续随旧负责人拆分迁移 |
| 成员历史可靠访问 | `MembershipHistoryStore` | 多个用例共享的深模块 | 保持 |
| 成员状态与事件 | `membership_state` | 共享状态和进程内失效通知 | 保持 |

## 成员关系关键发现

- 三个新成员用例已形成清晰产品入口，Engine 通过 `ProfileSpaceAdmission` 调用；但该 Facade 同时承载准入投影和成员命令，名称与职责需要在最终 Facade 收口时处理。
- Engine `ListDevices` 先查询加密状态，再调用 Roster Facade；“未初始化返回空列表”仍由 Engine 决定，属于 application 查询结果的一部分。
- Roster Facade 中同步偏好读写和保护状态查询都已经包含完整规则，不是单纯输入输出转换。
- `WorkspaceMembership` 剩余公开方法多数是准入适配、网络 endpoint 或后台恢复步骤，不能逐个提取成用户用例；应跟随它们所属的 Join、Runtime 或共享模块迁移。
- dev-tools 的旧决定入口与新决定用例表达同一用户事实，长期保留会形成两套行为。

## Space 连接与运行期第一批清单

| 行为 | 当前负责人 | 当前判断 | 后续动作 |
| --- | --- | --- | --- |
| 刷新全部当前 peer 可达性 | `EnsureReachableAllUseCase` | 完整命令，返回逐 peer 汇总 | 保持，统一命名与 Facade 入口 |
| 手动恢复网络会话 | `NetworkRecoveryFacade::request_recovery` | 深层命令，包含合并与有界重试 | 保持行为，负责人改名为 Coordinator/Runtime 更准确 |
| 查询网络恢复状态 | `NetworkRecoveryFacade::status` | 同一恢复负责人状态查询 | 保持，不拆出无状态转发用例 |
| 网络变化观察 | `observe_*` | 自动恢复 Runtime 的内部输入 | 保持内部，不作为用户用例 |
| 成员保持连接 | `MembershipConnectivityRuntime` | 后台重试与退避运行期 | 保持 Runtime |
| 启停全部 Space 后台任务 | `SpaceApplicationRuntime` | 聚合启动、活动控制和关闭 | 保持 Runtime |
| 查询配对 peer 编号 | `SpaceFacade::list_paired_peer_device_ids` | 无生产调用者 | 删除候选 |
| 确保单个 peer 可达 | `SpaceFacade::ensure_reachable_one` | 无生产调用者 | 删除候选 |
| Space 关闭清理 | `SpaceApplicationRuntime::shutdown` + `SpaceFacade::on_shutdown` | Runtime 为关闭 pairing orchestrator 反向持有 Facade | 将 pairing orchestrator 句柄归入明确的 admission runtime，移除 Runtime 对 Facade 的依赖 |

## 连接与运行期关键发现

- `EnsureReachableAllUseCase` 同时用于用户刷新和会话就绪后的尽力恢复，核心行为一致；调用方只决定是否传播失败，属于合理复用。
- `NetworkRecoveryFacade` 实际拥有状态机、同轮合并、自动窗口、退避和关闭，不是普通 Facade；拆成多个小用例会泄露状态机，应保留深模块并调整定位。
- Engine 直接持有网络恢复负责人并转换稳定结果，这个入口本身完整；最终只需统一 Space Facade/Runtime 的对外组织，不应把观察方法暴露给产品。
- `SpaceApplicationRuntime` 正确聚合三个后台运行期，但只为调用 `SpaceFacade::on_shutdown` 而持有整个 SpaceFacade，说明 sponsor pairing runtime 的所有权仍不清晰。

## Space 全量梳理后的优先级

1. 生命周期：提取 Query Space Setup State。
2. 准入：提取 Cancel Invitation 与 invitation address query，收敛 Join Space。
3. 成员：提取 Query Space Devices、同步偏好读写与保护状态查询。
4. 生命周期：收口 initialize/unlock 的活动恢复。
5. 准入：提取 Cancel Join，并收口 Join 后会话切换结果。
6. Runtime：移除无调用者 reachability 转发，理顺 pairing shutdown 所有权。
7. 最后清理旧 WorkspaceMembership dev-tools 旁路和已迁出公开方法。

## Durable Admission Transaction 复核

- `durable/transaction.rs` 约 3400 行，同时承担查询投影、本机加入建立、邀请方状态推进、加入方状态推进、取消与拒绝、跨 Space 切换、待发送消息恢复、终态压缩、完成协助、消息编码和错误映射；它已经不是一个单一事务模块。
- 加入方与邀请方的正常协议推进已有各自 `durable_flow.rs`，但状态转换实现仍集中在共享 transaction，维护者必须在角色流程和共享文件之间来回追踪，角色内聚尚未真正完成。
- `DurableAdmissionProjection` 混合只读查询和 `cancel_local_join` 写入；当前加入查询应归查询用例，取消写入应归取消用例，不应继续以 projection 名义组合。
- `start_join`、`start_join_before_network`、`start_join_with_recovery_material`、`request_cancel`、`sponsor_remove_pending_member`、`record_admission_unavailable` 等入口没有生产调用者，只被旧测试直接调用。应先删除或改由测试走真实角色入口，不能继续作为共享模块表面。
- `recover_with` 负责扫描未完成记录并投递待发送消息是合理的共享恢复能力，但它内部直接调用多个 joiner/sponsor 状态转换，仍掌握两侧语义。后续应让恢复调度只判断待发送消息种类，再委托对应角色负责人确认结果。
- 真正应留在 `durable/` 的知识是：按版本原子保存准入记录与成员历史、加载与终态压缩、收件去重、待发送消息和确认的稳定编码、可恢复记录扫描，以及通用错误映射。这些能力是角色流程的内部保存工具，不是另一个公开用例。
- 跨 Space 切换只属于加入方完成阶段，应迁入 joiner 侧内部完成模块；完成协助恢复继续留在 durable 子模块，但只通过窄保存工具读写记录。

### 推荐拆分顺序

1. 先删除或收口没有生产调用者的旧 transaction 入口，并把测试改到真实入口，缩小表面。
2. 将 `DurableAdmissionProjection` 拆到查询加入与取消加入两个现有用例中。
3. 将 `sponsor_*` 状态转换迁入 sponsor 内部流程，将 `joiner_*`、加入准备和跨 Space 完成迁入 joiner 内部流程。
4. 提取仅供内部复用的准入记录保存工具，集中版本递增、原子历史提交、加载和终态压缩。
5. 最后重写恢复调度，使它只扫描和投递，再委托角色负责人处理确认；完成后删除 `DurableAdmissionTransaction` 总对象和 `transaction.rs`。

第一步已完成首批清理：`start_join`、`start_join_before_network` 和 `start_join_with_recovery_material` 及其共用旧建档实现已删除。仍有业务价值的“联网前保存”和“重启恢复材料不变”回归改走真实加入准备入口；其他协议回归通过测试夹具预置初始记录，不再把旧入口当作共享接口。
