# Findings

## Scope Rules

- 目标是业务规格和一次性重写设计，不是继续整理现有目录。
- 调查对象是 `crates/uc-application/src/space` 的实际行为，同时追踪其产品入口、Core 规则、Infra 适配和 Engine 组装。
- `crates/uc-application/src/space/rebuild_space` 只作为标准用例的结构参考。

## Initial Evidence

- 当前 `space` 目录同时存在旧成员关系总对象、按行为迁出的新模块、准入流程、生命周期用例和运行期协调，属于明显中间态。
- `synchronize_membership_history/exchange.rs` 夹入了工作记录文本，证明当前树不能直接被视为目标结构或健康编译基线。
- 既有架构记录已把成员传播、当前范围、历史同步、待处理效果、历史基线和成员运行期迁到不同目录，但用户明确要求停止这种渐进迁移，重新固定完整业务规格后一次性替换。

## `rebuild_space` Reference Shape

- 外部入口只有 `RebuildSpaceUseCase::execute()`，调用方不负责内部步骤排序。
- 用例内部拥有执行锁，完整负责 prepare -> stage -> rebuild -> commit -> finalize。
- `prepare` 先持久保存唯一目标；重启或重试复用同一 Space ID。
- `stage` 只写隔离目标，`promote` 才切换当前状态，`finalize` 清理安全状态、标记重新配对并清除进度。
- 成员重建内部完成关系清理、远端成员删除、本机成员保存和历史基线初始化。
- 内部能力按真实变化点设接口，但这些接口不泄露给上层调用者。
- 错误按准备、暂存、重建、生效和收尾分阶段，能区分“尚未生效”与“已生效但待收尾”。

## Target Design Implication

- 成员关系重写必须按完整业务结果划分用例，而不是按历史表、网络消息、效果步骤分别暴露公共入口。
- 每个用例必须拥有执行锁或明确共享锁规则、持久化检查点、幂等重试和最终结果。
- runtime、facade、engine 只能触发完整用例或查询结果，不能掌握内部步骤顺序。

## Current Entry Surface

- `SpaceFacade` 暴露创建、解锁、邀请、加入、待切换查询/完成、取消邀请、普通重置、设置状态、在线刷新、设备列表、加入完成确认和关闭。
- `SpaceJoinFacade` 单独暴露加入状态查询、取消和完成恢复。
- `SpaceMembershipFacade` 单独暴露成员状态查询、待确认移除决定和发起成员移除。
- `MemberRosterFacade` 仍暴露成员列表、在线信息、同步偏好、旧移除决定、旧收敛查询和保护状态。
- Engine 运行期分别持有这些 facade，并把稳定操作分派到不同 facade；当前 facade 划分不是最终业务归属证据。
- `SpaceModules` 同时构造并暴露成员状态总对象、当前成员范围、准入总对象、历史同步、传播、效果送达及两个后台 runtime，说明组装层掌握了过多内部知识。

## Required External Entry Categories

- 用户命令：创建、解锁、加入、取消加入、移除成员、确认待处理移除、重置、彻底重置。
- 产品查询：设置状态、加入状态、当前成员状态、成员列表/偏好、访问状态。
- 网络接收：准入消息、加入完成恢复、成员历史页、成员传播/证明、安全组更新。
- 后台恢复：待发准入消息、未完成加入、历史同步、待处理成员效果、在线触发的成员维护。
- 生命周期：启动活动 Space、停止旧 session、切换/重绑、锁定和关闭。

## Early Ownership Problem

- 同一成员行为跨 facade、assembly、runtime 和多个 owner 暴露，调用方需要知道哪些对象先构造、哪些端点安装、哪些 runtime 启动。
- 当前 `MembershipStateCoordinator` 同时承担状态写入、历史交换、移除和部分网络处理，接口规模接近实现复杂度，不满足目标模块的删除检查。

## Lifecycle Facts

- 创建 Space：拒绝已完成设置或口令不一致；解析并保存设备名；建立 Space 访问材料、本机身份、本机成员和初始成员历史，最后才标记设置完成并发布识别信息。
- 解锁 Space：要求已有当前 Space，解锁密钥后恢复 Space session；失败必须区分未设置、口令错误、密钥材料损坏和内部错误。
- 冷启动恢复：读取当前 Space，尝试恢复 session；成功后运行升级并恢复活动任务；密钥材料损坏和钥匙串缺失不能伪装成未设置。
- 锁定：先暂停活动任务，再锁 Space；锁失败时必须恢复活动任务。
- 普通重置：先取消当前邀请，再执行可恢复的单设备 Space 重建；保留本机允许保留的数据，清除远端关系并要求重新配对。
- 升级：跨指定版本且已有设置时执行单设备重建，成功后才记录新版本；旧成员关系不转换为新授权事实。

## Admission Facts

- 加入对外只给当前结果，不公开内部协议阶段；新的用户加入请求与后台恢复是两种动作。
- 加入记录持久保存双方角色、阶段、消息、结果、目标历史和 Space 切换进度；重试与重启继续同一记录。
- 取消只请求正式提交前结束；已进入终态时返回现有结果，不伪造回滚或自动移除。
- 跨 Space 加入在当前 session 排空后完成持久化切换；重复调用返回同一活动结果。
- 后台恢复扫描未完成记录，恢复帮助完成、邀请方激活、待发消息和送达结果；产品端不负责编排内部步骤。
- 活动结果仍可能需要补送完成确认，补送资料从持久终态重建，不依赖内存。

## Design Consequence

- “成员关系”目标模块必须同时覆盖正式成员事实、当前授权投影、准入产生成员事实、移除产生成员事实、网络核对和待处理效果；否则复杂度会重新散回准入与 runtime。
- 加入协议可以有双方内部负责人，但 profile 层必须有一个完整加入入口和一个完整恢复入口；它们不得暴露消息阶段给 facade。

## Membership Truth and Authorization

- 经签名验证且属于当前 Space lineage 的 V2 成员历史是当前成员资格的唯一正向来源。
- 成员资料表只帮助把稳定成员身份映射到设备，不能独立授予当前成员资格。
- 加入记录中的激活/切换状态只会缩小权限；未完成本机加入时不能因目标历史已落盘而提前运行。
- 本机不是活动成员时，普通远端范围必须为空。
- 历史缺失、损坏、无法解密、Space lineage 不一致或成员身份映射不完整时必须失败关闭，禁止普通内容交换。
- 关系级 `PendingRemovalDecision`、`Diverged`、`Invalid` 阻止与该设备的普通交换；其他一致关系仍可继续。
- 被移除设备仍可在受限通道发送对既有移除事件的签名决定，但不能借此恢复普通内容、邀请、Presence 或成员历史交换权限。

## Removal Facts

- 发起移除要求本机仍是活动成员、目标存在且不能是本机；创建并签名移除事件，按历史版本提交，成功后请求后台传播。
- 发起成功只表示本机正式事实已保存，不表示远端已接受或全网同时一致。
- 收到的有效远端移除不会自动替用户决定；产品查询公开一项当前待确认变化。
- 接受本机移除必须二次明确确认；拒绝和接受都要签名并按加载版本提交。
- 并发历史变化不能覆盖，必须返回“待处理项已变化”并给出最新状态。
- 重复决定必须幂等返回原决定，并继续补做尚未完成的效果。
- 决定保存后，安全组更新、受限决定投递和普通范围变化由后台恢复负责，不能由产品调用方拼接。

## Synchronization and Runtime Facts

- 全量历史同步读取当前有效远端范围，排序去重，并让整轮共享固定 10 秒预算；单设备离线、拒绝、超时或协议失败不让整轮失败。
- 设备上线时可触发单设备同步；读取、验证、合并和保存仍受共享写入串行规则保护。
- runtime 在启动、恢复、定时和显式请求时运行恢复；暂停必须中止正在执行的恢复，恢复后重新触发，关闭最多等待有限时间。
- 恢复顺序目前是准入恢复、受限移除决定送达，部分触发再做全量历史同步；目标规格必须显式固定顺序与各步失败是否阻止后续。

## Document Conflict

- ADR-021 保留单一 `WorkspaceConvergence` 总负责人，并要求逐项迁移；其第 62、79-81、90-95 行明确反对一次性重写。
- 当前用户决定改为先固定完整规格，再一次性替换，因此新 Spec 必须明确取代 ADR-021/Spec-024 的渐进实施顺序。
- ADR-020、Specs 021-023 中已经验证的成员分支、用户决定、当前范围、持久恢复和安全规则仍是业务事实，不因实施方式改变。

## Legacy Aggregate Diagnosis

- `SpaceMembershipState` 仍把 Space lineage、本机实例、对端历史关系、分页接收进度、综合 phase、失败类别、revision 和 removed 混在一个持久对象中。
- `WorkspaceSnapshot` 又把产品状态、历史计数、待决定项、分叉设备、升级设备、摘要和失败类别混为一个投影。
- `MembershipStateCoordinator` 掌握该对象的读取、保存、发布、准入判断、历史网络交换和多项效果，形成需要删除的旧成员关系总对象。
- 当前成员资格实际可从已验证历史和加入激活门禁派生；关系状态和传输进度应分别持久化，不能继续让综合状态成为第二份成员真相。

## Preserved Protocol Responsibilities

- 历史事件和决定必须继续不可变、签名、按父关系验证、有界分页交换，并保留过去作者的验证资料。
- `known` 与 `applied` 的区别必须保留：收到并验证不等于已经接受和授权。
- 安全组更新、成员显示资料、路由资料等效果必须在授权公开前完成，且能跨重启补做。
- 旧候选/公告/gossip 代码同时做候选确认、设备公告、安全更新拉取、可信关系和地址提升；不能整块照搬为目标模块，需逐项判定由正式准入、历史同步、效果恢复或普通连接资料更新承接。

## Stable Product Contract

- 正式产品操作是 `QueryDeviceTrust`、`DecideDeviceTrustChange`、`RemoveMember`、`JoinSpace`、`CancelJoinSpace`、`ResetSpace` 和 `FactoryResetSpace`。
- `QueryDeviceTrust` 返回本机资格、当前待处理变化、当前加入、待接纳成员、每台设备的成员/关系/兼容/同步状态、可用动作和稳定修订号。
- `DecideDeviceTrustChange` 返回已应用、保留当前设备组、已完成、状态已变化或需要确认移除本机，并携带最新完整快照。
- `RemoveMember` 当前仍返回旧 `WorkspaceSnapshot`，与正式设备信任结果不一致；目标规格应让它返回同一正式设备信任快照，但这会改变稳定结果类型，实施前需明确版本策略。
- `QueryWorkspaceConvergence` 和 `DecideMembershipRemoval` 只在 `dev-tools` 下存在，属于旧综合状态和旧决定入口；一次性替换后应删除。
- 绑定和移动验收宿主已直接使用正式设备信任查询、决定和移除入口，因此这些名称、输入语义、事件与错误分类应保持。

## Public Projection Rule

- 产品只看到一个完整设备信任快照，不公开历史页、签名、内部阶段、持久进度、重试次数或网络拓扑。
- revision 在同一 profile 内单调递增，加入、移除、决定和相关投影变化不能倒退。
- reachability 只是观察，不能改变成员资格；设备名和地址只补充展示/连接资料，不能成为授权来源。

## Runtime Ownership Problem

- `SpaceApplicationRuntime` 当前同时启动成员 gossip、成员维护和成员连接三个 runtime；暂停、恢复和关闭还要按三个对象排序。
- `SpaceSessionActivity` 因此依赖一个组合 activity，并间接了解多个成员后台流程。
- 目标应只有一个 `SpaceMembershipRuntime`：它拥有在线事件、定时器、退避、恢复请求和关闭；会话层只调用 `pause()`、`resume()`、`shutdown()`。
- runtime 只负责调度，不掌握业务阶段；每次触发调用一个完整 `MaintainSpaceMembershipUseCase`。

## Persistence Facts

- `SpaceJoinRecordStorePort` 背后的加密 profile 状态已经同时保存加入记录、V2 成员历史和单调设备信任 revision，并支持加入记录与历史的条件原子提交。
- `WorkspaceConvergenceStore` 另存旧综合状态和分页进度；它是待替换的第二份成员状态。
- 旧 `membership_candidate`、`membership_announcement`、`membership_outbox` 和 `membership_applied_security_update` 使用关系存储中的独立加密记录，服务旧 gossip 流程。
- 目标持久化必须复用现有 profile 加密原子提交能力，扩展为保存历史之外的关系、传输和待处理效果；不得在旧综合状态旁增加第三套存储。
- 所有新增负载仍按 MasterKey AEAD 密文保存；日志只允许稳定类别和数量。

## Target Persistence Shape Direction

- `MembershipHistoryV2`：唯一成员事实。
- `PeerReconciliationRecord`：每个对端的关系、最后确认位置、受限送达计划；不包含成员资格副本。
- `InboundMembershipTransfer`：每个来源唯一的有界分页进度；完成整体验证前不替换正式历史。
- `PendingMembershipEffect`：按事件编号保存待完成的成员资料、安全状态和激活效果。
- `device_trust_revision`：profile 内唯一单调产品修订号。
- 上述资料由同一加密 profile 提交器按条件更新；只读投影按需派生，不持久化综合快照。

## Target Module Shape

- 保留完整产品用例：`query_device_trust`、`remove_space_member`、`decide_device_trust_change`、`join_space`、`cancel_space_join`、`rebuild_space` 等；每个目录按 `model.rs`、`error.rs`、`ports.rs`、`use_case.rs` 组织，只暴露一个 `execute()`。
- 新增私有 `membership_ledger/`：负责加载并验证历史、条件提交历史与运行资料、派生当前授权范围；不决定产品动作或网络重试。
- `synchronize_membership_history/` 完整负责单设备/全量历史交换；网络端点由 `HandleMembershipHistoryMessageUseCase` 完整处理一条入站消息。
- `maintain_space_membership/` 完整负责待处理效果、受限决定送达、准入恢复和按触发条件的历史同步；runtime 只调这一个用例。
- `SpaceMembershipRuntime` 合并现有成员 gossip、成员维护和成员连接 runtime，对会话层只暴露 pause/resume/shutdown。
- `SpaceFacade` 只选择上述完整用例并转交输入/结果；删除 facade 内的持久读取、网络拨号、完成确认送达和后台任务编排。
- `SpaceApplication` 作为私有构造结果，只向 Engine 提供 `SpaceFacade`、网络端点和一个 runtime 生命周期句柄；不暴露 coordinator、内部用例或持久仓储。

## Interface Decisions

- 删除 `MembershipStateCoordinator`、`SpaceAdmission` 和 `MembershipConvergence` 这类大总对象。
- 删除 `ContentExchangeGatePort`；普通消费者统一读取一次 `CurrentSpaceMemberScopePort::snapshot()`，快照已经包含成员历史、激活门禁和关系限制后的最终可用对端。
- 将 `MembershipAdmissionGatePort` 的两次读取合并为一次准入快照，避免 generation 与 decision 跨读取变化。
- 网络传输端口只传输一条有界成员消息；分页、游标、重试、验证和保存全部留在同步/接收用例内部。
- 生产与内存测试适配都实现持久或网络 seam；不为只有一个调用者和一个实现的内部步骤新增 port。

## Logging Violation Found

- 当前邀请方入站流程会把邀请码和完整设备标识写入日志；成员同步、连接和错误日志中也存在完整设备标识。
- 一次性重写必须删除这些字段，只记录稳定错误类别、协议阶段、计数、耗时和不可逆短关联值。

## Scope Correction

- 用户明确要求当前 Spec 只保证 application 层完备。
- Core、Infra、Engine、绑定、数据库 migration、真实网络 adapter 和设备验收均移出本规格。
- Application 仍需用 ports 写清所需原子提交、网络、安全和持久能力，但不规定或实现外层 adapter。
- 外层暂时不兼容是后续接入问题，不能成为 application 保留旧总对象、兼容别名或双路径的理由。

## Evidence Queue

- 产品查询与命令入口
- 后台与网络回调入口
- Space 创建、解锁、恢复、重建、重置、升级、加入生命周期
- 成员历史写入、读取、交换、签名和授权
- 当前成员范围与内容同步候选
- 移除决定、待处理效果与安全组更新
- 准入双方流程及可靠消息恢复
- 组装、运行期任务、锁和并发语义
- 现有测试覆盖和应删除测试
