# 进度：Application 用例全量梳理

- 成员历史同步 UseCase 的职责、内部执行锁、共享状态写锁、固定总预算和单设备失败策略已补全中文注释；运行行为不变。
- 按 `rebuild_space` 模式重构成员历史同步：新增独立 UseCase、业务错误和单设备深接口；网络交换实现移入内部 `exchange.rs`。运行期与准入前同步统一调用 `execute()`，旧总对象的整链同步入口已删除，应用层生产编译通过。
- 旧诊断快照迁入 `query_workspace_membership_diagnostics`，旧兼容移除决定迁入 `decide_membership_removal_legacy`；产品路径仍使用三个标准用例。旧 `membership/` 与 `projection/` 子目录已清空并删除，生产库编译通过。
- 通用成员流程错误已迁为顶层 `membership_error`，准入、查询和后台调用不再通过旧总目录取得错误类型；保留旧目录内临时转出以支持尚未迁完的方法，生产库编译通过。
- 成员历史收发迁入 `synchronize_membership_history`，待处理决定与群组更新迁入 `recover_pending_membership_effects`；两者暂时仍实现于旧总对象，生产库编译通过。
- 新 Space 成员基线与准入基线迁入现有 `membership_history`，成员运行壳迁入现有 `membership_runtime`；旧目录不再拥有生命周期和运行期文件，生产库编译通过。
- `workspace_membership/discovery` 已整体迁为顶层 `membership_convergence`，组装、会话和对外依赖使用新归属，生产库编译通过。
- 当前有效成员范围判断已从旧投影目录迁为顶层 `current_membership_scope`；暂时仍作用于旧成员负责人，待总对象拆除时改为独立共享模块。
- 完成 `workspace_membership` 第一轮只读梳理：确认产品标准用例已有查询、发起移除和决定移除三个；旧总对象仍保留重复查询/决定入口，并混合生命周期步骤、历史同步、后台效果、当前范围投影和发现运行期。已形成保留、提取与删除方案，等待用户确认后实施。
- 加入方推进与共享准入规则拆分完成：`joiner/progression.rs` 只持有加入方三阶段和所需能力，`durable/protocol.rs` 统一保存双方复用的数据格式、消息校验与错误转换；旧 transaction 模块和名称已从当前源码清除，生产库检查通过。
- 应用层全部目标检查越过原测试接口和测试时钟问题后，继续发现旧测试构造的对象转换写法及 Facade 测试直接访问私有成员能力；这些现有测试断点仍需收口后才能完成验收。
- 为应用层回归恢复测试专用组合存储接口，并启用异步测试时钟；这两项只修复测试接线，不改变生产能力。下一步重新运行全部目标检查并处理剩余真实断点。
- 加入方可靠推进实现已从 `admission/durable/transaction.rs` 迁入 `admission/joiner/progression.rs`，旧文件已删除。共享准入工具仍需从加入方文件中拆回 durable 内部，完成后移除临时模块转出。
- 可靠准入总对象已收缩并改名为加入方推进负责人；生产持有者、加入方三段调用和协议回归辅助统一使用新名称，不再把仅剩的加入方流程表达成双方共享事务。下一步将实现迁入加入方目录并删除旧文件。
- 邀请方在成员提交前按明确原因拒绝加入的完整规则迁入邀请方取消与拒绝模块；可靠事务删除最后一项邀请方决定，回归直接验证新的负责人。下一步迁移剩余三段加入方可靠推进。
- 取消加入用例已直接读取当前加入记录、幂等保存取消请求、查询最终状态并发布修订，不再依赖 `DurableAdmissionProjection` 或收敛错误。可靠事务中的旧取消转发和 projection 内旧查询/取消方法已无生产调用，下一步可直接删除；projection 本身仍被成员状态查询用于读取待处理入站成员。
- 当前加入状态查询已直接依赖准入记录仓储并自行解释等待、拒绝和活动结果；删除 `SpaceAdmission` 与可靠事务中的旧查询转发。取消加入暂时继续使用旧内部查询，待下一小步迁移。同步修正迁移时误带换行的错误文本。
- 完成 transaction 首批表面清理：删除三个无生产调用的旧加入入口及其专用建档、重放匹配实现。保留的加入准备与恢复材料测试改走真实入口，取消、恢复、切换等测试改用直接预置记录的夹具；删除只验证旧入口重复调用的测试。旧名称残留、格式、Cargo 元数据和差异检查通过；按约定未运行构建和测试。
- 复核 `admission/durable/transaction.rs`：确认它混合查询、两侧协议状态转换、取消、空间切换、恢复、终态压缩和消息编码，不应继续作为单一总对象。确定按“先清无生产调用入口，再拆查询/取消，再迁角色转换，最后收窄共享保存与恢复工具”的顺序推进。本步只形成分析与路线图，未修改运行代码。
- 记录两次文档补丁定位失败：目标段落位置与预期不同；随后按当前稳定标题分别追加，失败补丁未写入文件。

## 2026-08-23

- 将加入方邀请兑换从 admission 根级迁入 `admission/joiner/`；旧根级模块删除，Facade 和 Join 用例只通过 `joiner` 模块使用该内部流程。
- `JoinSpaceUseCase` 改为返回已保存状态和明确的会话切换要求；正常加入由 Engine 直接转换结果，不再重新查询状态或扫描切换记录。
- 新增内部 `CompletePendingSpaceTransitionUseCase`；Engine 关闭旧会话后经 Space Facade 调用并取得最终活动状态，启动时的中断恢复也复用该入口。旧 `SpaceTransitionRecoveryPort` 及 Engine 直接装配出口删除。
- 补充加入状态与切换要求必须一致的回归：活动状态不能要求切换，等待状态必须要求切换；邀请兑换回归同时验证它保留本次握手算出的切换要求。
- 本步已通过 Cargo 元数据、Rust 格式和差异合法性检查；确认旧恢复接口、旧兑换路径和 Engine 成功后补查均无残留。按当前约束未运行构建、测试和会隐式构建的架构脚本。
- 拆除 `admission/durable/flow.rs`：加入方 6 个可靠推进阶段迁入 `joiner/durable_flow.rs`，邀请方 9 个阶段迁入 `sponsor/durable_flow.rs`；共享事务和完成恢复位置不变。
- 原 `WorkspaceAdmissionOwnerPort` 按角色拆为加入方与邀请方内部接口，握手、运行期和测试替身只看到各自所需能力。取消加入和待处理切换判断同时迁出共享流程。
- 删除 `ProfileSpaceAdmission`：新增 `SpaceJoinFacade` 组合查询加入、取消加入和完成确认恢复；新增 `SpaceMembershipFacade` 组合成员状态查询、发起移除、决定移除和活动 Space 接入。Engine 字段、操作入口、启动恢复和事件转发已分别改接。
- 既有持久加入回归改为通过 `SpaceJoinFacade` 查询、取消和验证未找到错误；既有成员状态与完成确认回归改接两个新门面。Cargo 元数据、格式、差异和旧名称残留检查通过；按当前约束未运行构建、测试和架构脚本。
- 准入尝试仓储、待发送消息投递、完成恢复通信、跨 Space 切换、安全状态切换和加入方成员材料 Port 从 core 迁入 application；infra 实现和 Engine 组装统一改从 `uc_application::deps` 导入。core 只保留相关准入模型、错误和领域验证规则。
- 精简加入方成员材料接口，删除没有 application 调用者的成员接纳和安装方法。格式、差异、Cargo 元数据和旧 core Port 路径检查通过；按当前约束未运行构建、测试和架构脚本。
- 将准入 Port 配套的错误、原子变更、查询投影、投递结果及安全/空间切换请求结果全部迁入 application，infra 统一从 application 导入；core 删除重复定义和导出。随准入记录持久化的安全投递数据保留在 core 并迁入准入记录模块，序列化结构不变。Cargo 元数据、格式、差异和单一定义检查通过；按当前约束未运行构建、测试和架构脚本。
- 删除浅层 `SpaceAdmissionCoordinator`，建立 `JoinSpaceUseCase` 作为加入 Space 的唯一 application 入口；设备名保存、加入前网络准备和邀请兑换由该用例完整负责，其余邀请操作由 `SpaceFacade` 直接调用各自用例。
- `AppFacade` 不再持有第二个准入协调器；本步保持现有加入结果不变，持久加入状态和会话切换要求留到下一小步收口。

- 用户调整目标：先完整梳理 `uc-application` 用例，再逐步向外实现 infra。
- 停止继续验证、提交和 infra 改造。
- 已确认此前启动的架构检查和其内部构建进程均已停止。
- 初步扫描发现 14 个标准 `use_case.rs`，同时确认大量候选行为仍分布在其他文件形态中。
- 建立新的独立路线图，下一步从生产入口反向生成全量行为清单。
- 用户进一步收窄范围：先完整完成 Space，其余 application 领域全部不动。
- 已扫描 Space 标准用例、Engine Space 操作、Space Facade、Profile Space Facade、运行期和旧成员负责人，确认 11 个标准文件之外仍有多类候选行为。
- 完成 Space 生命周期第一批分类：lock、recover、reset、rebuild、upgrade、query access 可保持；initialize 与 unlock 仍由 Facade 补活动恢复；query setup 和 reset commit status 是藏在 Facade 中的完整查询。
- 生命周期整改顺序确定为 query setup、initialize、unlock、reset commit query，最后复核 readiness 与 activity 的内部定位。
- 完成 Space 准入第一批分类：普通签发与兑换已有深层负责人；取消邀请、取消加入和地址查询仍藏在 Facade；`SpaceAdmissionCoordinator` 只有 join 包含真实编排，其余均为转发。
- 发现 Join Space 的稳定结果和跨 Space 切换判断仍由 Engine 在成功后重复查询；后续应让 application Join 结果直接携带持久状态与切换要求。
- handshake、durable admission transaction 和 sponsor orchestrator 分别定位为内部协议流程、可恢复共享流程和后台运行期，不提取为用户用例。
- 完成 Space 成员关系分类：三个新成员用例保持；设备列表、同步偏好和保护状态仍藏在 Engine/Roster Facade；旧 dev-tools 决定和收敛快照形成旁路。
- 完成 Space 连接与运行期分类：EnsureReachableAll 保持标准用例；NetworkRecovery 保持深层状态负责人；MembershipConnectivity 与 SpaceApplication 保持 Runtime；两个无调用者 reachability 转发列为删除候选。
- Space 全量行为清单阶段完成，进入逐项职责评估；优先从 Query Space Setup State 开始。
- 提取 `QuerySpaceSetupStateUseCase`：模型、错误和当前 Space、邀请、设置、重新配对四项读取归入标准模块，Facade 改为单次 `execute()` 转发。
- 新增三个用例级回归；格式、差异、单一定义和残留编排检查通过，构建与测试按约定未运行。
- 提取 `CancelPairingInvitationUseCase`：取消规则和错误归入标准模块，Space Facade 改为单次 `execute()`，并删除 Facade 对 holder 的直接持有。
- 新增无邀请冲突和全部取消两个用例级回归；Facade 测试改从 Setup 状态验证可见结果。
- 将 invitation 相关行为内聚到 `space/admission/invitation/`：现有 issue 和新 cancel 各自成为子模块，共享 holder；删除旧顶层与 admission 根级路径。
- 提取 `QueryPairingInvitationAddressesUseCase`：地址查询 Port 和行为从普通签发用例移除，新增查询专用错误并保持 Engine dev 结果不变。
- 提取 dev-only `IssuePairingInvitationForAddressUseCase`；普通签发和按地址签发共享 `PairingInvitationIssuer`，普通用例不再持有 dev Port 或第二入口。
- 删除可靠事务中已经被独立用例取代的加入状态查询、取消加入及其转发。成员状态查询改为复用 `QuerySpaceJoinStatusUseCase`，加入状态的解释规则只保留一份，并继续区分存储锁定、数据损坏和普通失败。
- `DurableAdmissionProjection` 当前只剩待处理入站成员读取；下一步将这项读取收进成员状态查询后即可删除该对象。
- 待处理入站成员读取已收进 `QuerySpaceMembershipStatusUseCase`，由该查询直接筛选当前 Space 的未完成邀请方记录并生成产品视图；重复记录或损坏内容继续失败关闭。
- 删除无剩余职责的 `DurableAdmissionProjection` 及其导出。可靠事务不再承载任何产品查询投影。
- `QueryPendingSpaceTransitionUseCase` 已直接读取未完成加入记录并判断是否等待会话切换，不再转发可靠事务方法。
- `CompletePendingSpaceTransitionUseCase` 已完整负责逐阶段推进跨 Space 切换、原子保存成员历史与加入结果、终态压缩和最终活动状态确认。可靠事务中的查询、推进和恢复转发全部删除；跨 Space 回归改为验证两个真实用例入口。
- 修复 `JoinSpaceUseCase` 遗留的旧状态查询调用，统一复用 `QuerySpaceJoinStatusUseCase`。
- 简化两个 Space 切换用例的构造：删除接收整个 `SpaceAdmission` 后再转发到 `from_ports` / `from_repository` 的双入口，统一为单一 `new` 并直接接收准入记录仓储和必要的切换能力。生产组装与回归测试使用同一入口。
- 完成确认恢复所需的挑战读取、挑战保存、帮助记录创建和帮助完成已迁入 `durable/completion_recovery.rs`，作为恢复流程私有步骤直接使用准入仓储。可靠事务删除对应四个方法，不再暴露恢复专用操作。
- 邀请方的拒绝送达确认和完成送达确认迁入 `sponsor/durable_flow.rs`，实时握手、后台恢复和测试共用同一组邀请方规则。可靠事务删除 `sponsor_confirm_rejected` 与 `sponsor_confirm_active`，不再保存邀请方终态确认知识。
- 邀请兑换结果迁入 `joiner/durable_flow.rs`：临时失败保留待重试记录，已兑换、不存在或冲突结束该记录。后台恢复只转交传输结果，可靠事务不再解释加入方的邀请兑换规则。
- 可靠消息送达确认从 `transaction.rs` 拆出：旧加入被取代后的取消清理确认归入 `cancel_space_join`；五类常规消息的送达记录作为未完成准入恢复的私有步骤，收进 `durable/completion_recovery.rs`，不保留孤立消息模块，也没有按消息类型复制五套规则。
- 建立 `RecoverPendingAdmissionsUseCase`，接管启动、恢复、周期和显式请求触发的全部未完成准入恢复入口。成员运行器显式持有该用例，不再调用一个并不存在于成员负责人的准入恢复方法；原 `SpaceAdmission::recover_pending_admissions` 入口已删除。
- 未完成准入的消息扫描、重发、结果分派、送达记录和终态压缩已迁入 `RecoverPendingAdmissionsUseCase`。`transaction.rs` 删除生产恢复流程，只保留受测试条件限制的旧回归适配，实际运行不再经过它。
- `RecoverPendingAdmissionsUseCase::execute()` 收敛为只返回成功或失败，不再把“消息尝试数”和“完成恢复数”相加成无业务含义的数字。旧事务测试适配已删除，恢复细节回归直接验证新用例内部步骤及最终持久结果。
- 删除无生产调用者的 `enqueue_post_commit_delivery`。已有成员安全更新与历史/回执批次的送达记录迁入邀请方，并命名为 `record_sponsor_follow_up_delivered`；恢复用例只把对应回执交给邀请方处理。
- 加入方收到拒绝后的校验、旧加入清理、安全准备丢弃、切换准备回滚和终态保存迁入 `joiner/durable_flow.rs`，统一命名为 `record_joiner_rejection`。恢复用例直接调用加入方规则；事务中的旧方法只保留测试条件下的适配。
- 邀请方在候选资料产生前明确拒绝加入的保存规则迁入加入方，并统一命名为 `record_early_sponsor_rejection`。实时握手、加入方接口和实现使用同一业务名称，事务中的 `joiner_reject_before_candidate` 已删除。
- 五个加入方拒绝回归已直接调用 `record_joiner_rejection`，需要验证切换回滚的场景显式使用记录器；事务中的测试专用 `joiner_record_rejected` 转发已删除，旧名字完全清除。
- 删除无生产调用者且不修改任何状态的 `record_admission_unavailable` 及其转发测试。准入暂时不可用继续由待发送消息保持未完成、恢复流程稍后重试表达，不再伪造“已记录不可用”的能力。
- 对照成员移除入口后确认：现有命令只移除已进入成员历史的设备，待加入成员仅作为独立状态展示，没有拒绝操作入口。删除从未接通的 `sponsor_remove_pending_member` 和 `PendingMemberRemovalOutcomeV1`，保留持久协议兼容所需的 `RemovedBeforeActivation` 拒绝原因。
### 2026-08-24：取消加入规则收口

- 将生成并持久化取消请求的规则收进 `cancel_space_join`。
- `CancelSpaceJoinUseCase` 与协议场景测试改为复用同一规则。
- 删除 `DurableAdmissionTransaction::request_cancel`，避免取消流程存在两套入口。
- 将邀请方收到取消请求后的决定规则收进 `sponsor`：提交前结束加入，提交后继续既有提交，重复请求返回已保存结果。
- 消息入口改名为 `respond_to_join_cancellation`，不再把通用取消误写成仅处理“新加入取代旧加入”的清理。
- 删除 `DurableAdmissionTransaction::sponsor_decide_cancel`，邀请方流程与协议场景测试统一复用同一规则。
- 取消响应发送后的确认改名为 `confirm_join_cancellation_response_sent`；只有真正取消成功时结束拒绝记录，取消太晚时继续原有加入流程。
- 修正取消太晚仍被错误包装为拒绝消息的问题：现在按实际结果返回继续加入消息，不再让接收方误判为取消成功。
- 将被新加入取代的旧加入收到迟到 Candidate、Commit 或 Complete 时的证据记录归入 `joiner`，并改名为 `record_message_for_superseded_join`。
- 三个加入方入口直接复用该规则；可靠事务删除 `record_superseded_protocol_contradiction`，不再持有加入方专属的迟到消息解释。
- 删除名不副实的 `load_join_recovery_material` 及其专用结构和检查；正式加入直接使用刚保存记录中的恢复公钥，不再立即重读整份敏感材料。
- 原回归改为重新打开准入存储并比较完整加入记录，继续证明加入身份与恢复材料已持久保存。
- 加入方收到 Candidate、Commit 或 Complete 时，统一通过 `load_join_attempt_for_incoming_message` 区分活动记录、已整理的被取代加入和完全缺失记录。
- 删除可靠事务中的 `is_compacted_superseded` 转发；三个入口不再各自拼装相同的恢复判断。
- 未完成准入恢复用例直接从准入存储读取待恢复记录，消息重发内部流程复用同一存储输入。
- 删除可靠事务中的 `recoverable` 纯转发；恢复顺序、消息重发和结果处理不变。
- 新增共享终态整理规则 `compact_settled_admission`，统一判断消息已送达、恢复标记已清除、空间切换已完成且无待清理工作后才整理记录。
- 加入方、邀请方、后台恢复和待完成空间切换统一复用该规则；删除可靠事务与空间切换用例中的两份重复实现。
- 后台消息恢复不再接收整个可靠事务对象，只依赖准入存储及实际处理消息所需能力。
- 邀请方和完成恢复流程直接从准入存储读取记录，可靠事务内部的必有记录读取也直接使用自身存储。
- 删除 `DurableAdmissionTransaction::load` 纯转发，不新增替代接口；缺失记录与存储错误的原有结果保持不变。
- 邀请方在真正激活安全状态前，通过 `record_sponsor_security_activation_pending` 持久保存恢复标记；重复标记成功返回，冲突标记继续失败。
- 删除可靠事务中的 `sponsor_prepare_security_activation`，保存顺序和崩溃恢复语义保持不变。
- 固定测试邀请方地址的 `prepare_join_before_network_without_route` 从可靠事务移入回归测试辅助能力，既有 21 个测试调用保持原输入与断言。
- 生产事务不再包含仅为测试省略一个参数的转发入口，真实加入准备入口保持不变。
- 新增 `joiner/prepare_join.rs`，先接管加入前源资料检查及当前未完成加入读取，明确这是联网前准备的一部分。
- 正式加入方流程直接使用新模块；原子建立新加入暂留可靠事务，下一步迁移。测试期间保留一个仅用于既有回归的检查转发，不形成生产入口。
- 统一加入方表达：联系邀请方前的只读门禁命名为 `ensure_join_can_start`，取得通信信息后建立并保存加入命名为 `prepare_join`。
- 删除生产代码中的 `preflight_local_join_source`、`prepare_local_join_before_network`、`DurableLocalJoinPreparation` 及配套错误转换旧名；准备结果统一为 `PreparedJoin`。
- `joiner/prepare_join.rs` 接管新加入身份与恢复密钥生成、旧加入安全取代、原子建档及并发冲突重试，返回已保存的 `PreparedJoinRecord`。
- 正式加入流程直接调用 `prepare_join`；可靠事务删除原子建档实现，仅在测试条件下转交既有回归到同一实现，不保留第二套规则。
- 删除可靠事务中最后两个仅供测试使用的加入准备转发。准入回归由测试负责人直接调用 `joiner/prepare_join.rs`，可靠事务只保留后续可靠协议推进。
- 核对后保留可靠事务的安全切换和空间切换依赖：它们仍用于收到候选资料后的安全状态准备与跨空间切换，不属于加入前准备的残留依赖。
- 邀请方接受加入请求、验证候选资料、原子保存邀请占用与候选消息的完整规则迁入 `sponsor/durable_flow.rs`，统一命名为 `accept_join_request_and_offer_candidate`。
- 生产邀请方流程与可靠协议回归直接复用邀请方入口；可靠事务删除 `sponsor_accept_and_offer`，不再负责邀请方候选资料建档。
- 邀请方收到已验证确认后，检查成员历史未变化、原子提交新历史与提交消息、处理并发重放或保存“历史已变化”拒绝的完整规则迁入 `sponsor/durable_flow.rs`。
- 生产邀请方流程与可靠协议回归统一调用 `commit_verified_joiner_preparation`；可靠事务删除 `sponsor_commit` 及其历史变化拒绝分支。
- 新建 `sponsor/candidate_offer.rs`，完整接管加入请求校验、候选资料生成、重复请求恢复和候选建档；`durable_flow.rs` 不再掌握候选方案阶段。
- 将可靠成员历史与旧资料成员基线的验证迁入 `workspace_membership/membership/admission_base_history.rs`。成员修复和邀请方候选生成共用成员模块能力，不再由邀请方定义通用成员历史规则。
- 新建 `sponsor/membership_commit.rs`，完整接管加入方确认校验、成员历史一致性检查、成员变更与提交消息的原子保存、并发重放和历史变化拒绝。
- `durable_flow.rs` 删除正式提交阶段；邀请方运行入口和可靠协议回归继续调用同一提交能力，协议内容与保存顺序不变。
- 新建 `sponsor/completion.rs`，接管已应用结果校验、安全激活恢复标记、激活回执写入成员历史、完成消息、最终确认和后续安全/历史送达确认。
- 旧可靠事务中的 `sponsor_complete` 已迁入完成模块，并收缩重复的完成内容参数；`durable_flow.rs` 不再掌握完成阶段。
- 将剩余的取消决定、取消响应确认和拒绝送达确认改入 `sponsor/cancellation.rs`。
- 删除 `sponsor/durable_flow.rs`；邀请方可靠流程现在按候选、成员提交、完成和取消四个阶段组织，不再保留混合流程文件。
- 邀请方完成阶段不再分别操作成员历史仓库和历史签名校验器；现有成员历史模块统一负责读取、验证激活回执和编码更新，准入负责人只持有这一项成员历史能力。
- 清理 `sponsor/completion.rs` 的全部行内导入与通配导入，使完成阶段实际使用的领域类型和应用能力在文件入口可见；原子保存、恢复标记和消息确认顺序不变。
- 将当前成员签名能力及其稳定错误从 core 迁入 application 的 Space 共享目录；core 不再声明未使用的应用流程依赖，infra 直接实现 application 的入口，engine 只负责组装同一实现。

### 2026-08-24：Space 加入记录批量重命名

- 已确定只修改类型、接口、错误、格式常量、内部保存类型和两个文件名；不修改保存格式或普通局部变量名。
- 影响范围已统计，下一步执行精确符号替换并检查旧名残留。
- 第一次批量替换因 zsh 未按换行拆分文件列表而未修改任何文件；改用零字节分隔的文件列表执行，避免路径展开歧义。
- 核心记录、编号、完成记录、角色状态、取代错误、存储接口、存储错误、每记录数据密钥和取消存储对象已统一为 Space Join 命名。
- core 模型文件与 infra 存储文件已重命名；当前规格中的源码路径和领域名称已同步。
- 持久内部结构继续保留 `V1`，主要业务记录删除 `V1`；格式常量只改名，不改数值。
- Cargo 元数据、Rust 格式和差异合法性检查通过；源码与当前文档中的 `AdmissionAttempt`、旧文件路径和旧格式常量均无残留。
- 按当前计划约束未运行构建、测试或架构脚本。
- 用户指出加入记录内部仍有不必要的 `V1`；已完成逐项分类，下一步删除稳定业务状态和进程内结果的版本后缀，保留独立编码协议内容的版本后缀。
- 第二轮重命名已完成：加入阶段、角色状态、待发与已收消息、最终结果、拒绝原因、资料元数据和 application 进程内结果均删除 `V1`。
- 旧名检查确认只剩 7 个有独立编码与版本校验的完成恢复和身份绑定类型继续保留 `V1`。
- 第二轮 Cargo 元数据、Rust 格式、差异合法性和指定旧名残留检查通过；按计划未运行构建、测试或架构脚本。

### 2026-08-24：准入消息送达结果收口设计

- 用户拒绝后台恢复逐项调用加入方、邀请方和取消模块的状态推进函数；开始只读调查和设计，不修改生产代码。
- 已确认恢复流程混合传输重试与完整协议结果路由；部分被调规则由实时握手复用，不能在恢复中复制第二套实现。
- 设计完成：使用 `AdmissionOutboxRecovery.execute()` 隐藏扫描、选路、发送和统计，使用 `AdmissionDeliverySettlement.settle()` 隐藏全部结果路由、状态推进和终态整理。
- 两个模块均为 application 内部具体模块，不新增 port；顶层恢复、Facade 和 Engine 不再理解消息类型矩阵。
- 已记录迁移范围、删除项、生产能力边界和接口级验收矩阵；本步未修改生产代码。
- 实现时整文件替换因补丁工具不允许同一路径同时删除和新增而未写入；改为精确修改文件头与调用点，再机械删除旧自由函数区段。
- 测试结算辅助函数最初尝试从消息推导所属记录编号，但消息不包含该编号；已立即改为显式接收记录编号，保持接口事实完整。
- 已新增送达结果结算和待发送消息恢复两个具体模块；顶层恢复改为一次调用，旧结果分支和测试自由函数已删除。
- 既有恢复测试改走 `AdmissionOutboxRecovery.execute()`，精确确认测试改走 `AdmissionDeliverySettlement.settle()`。
- 结算入口补齐原消息关联校验：普通确认必须与刚发送消息生成的确认一致，远端拒绝必须引用刚发送的取消消息。
- 记录整理成功后，待发送消息恢复立即停止处理该记录的旧快照，避免继续发送过期消息并防止重复统计。
- Cargo 元数据、Rust 格式、差异合法性和结构残留检查通过；人工审查未发现职责回流或正式发送能力扩张。
- 按当前计划约束未运行构建、测试或架构脚本；工作区包含用户正在进行的整体重构，因此未创建提交。
- 在用户修补后的完成恢复文件上继续收口记录编号：内部入口和辅助读写直接使用 `SpaceJoinRecordId`，形参统一为 `join_record_id`，移除拆字节后立即重新包装；生产调用与现有回归同步调整，协议消息字段不变。

### 2026-08-24：成员历史 V1 完全删除评估

- 用户要求先评估完全删除成员历史 V1；当前只读调查，不修改生产代码。
- 已确认旧成员模型仍有多个生产调用者，V1 证据也承担 V2 历史中的旧签名保留；删除范围远大于两个证据类型。
- 完成评估：V1Evidence 包装没有生产调用，可独立删除；旧 MembershipHistory 仍服务生产兼容，且加密工作空间 V3 布局包含该字段。
- 推荐两阶段移除：先发布 V2-only 的 V4 状态并保留 V3 迁移，真实升级覆盖后再删除旧读取器。一次性全删等同于放弃所有现有 V3 资料兼容。
- 用户补充下版产品本就要求重置 Space；据此取消旧历史迁移要求，完成 application/core 可删除清单和实施顺序评估。
- 关键前置条件是先让新建/重置直接创建 V2 根历史，并确保重置无需完整解析旧 V3 成员历史；随后可删除 application 旧分支和 core 旧状态机。

### 2026-08-25：成员历史旧实现删除

- 新增 `VersionedMembershipHistory::new_single_member_root`，根编号由 V2 域分离内容确定；新建和重置 Space 直接保存该 V2 根历史。
- 删除 Application 的旧升级、旧准入会话、旧历史写入、旧移除决定回退、旧效果恢复、旧当前范围回退和旧同步目标回退。
- 删除 Core 的旧 `MembershipEvent`、`MembershipOperation`、`MembershipDecision`、`MembershipReconciliation`、V1Evidence、旧检查点和相关错误/测试。
- 工作空间状态删除旧历史、旧效果、旧决定和旧准入会话字段；加密负载提升为 V4，旧单行或旧槽状态不再迁移并稳定要求重置。
- 迁出并保留 V2 仍需的编号、移除选择、关系状态、历史消息和待移除产品事实。
- 修复当前分支阻塞生产库编译的若干机械断点；Core 全部目标测试通过，Application 与 Infra 生产库检查通过。
- `cargo check -p uc-application --all-targets --locked` 仍被既有测试接线错误阻塞，错误集中在已迁移准入测试的旧 owner/port 构造及 SpaceSetup 测试字段，不属于成员历史旧实现残留。
- 最终残留扫描确认源码和当前架构文档中没有旧成员状态机、V1Evidence、旧检查点、旧基线分类或旧状态字段。
- `cargo check --workspace --lib --locked` 通过，覆盖 Engine、Application、Infra、三端绑定与兼容库生产代码。
- `cargo test -p uc-core --all-targets --locked` 通过：196 个库测试、17 个 key epoch 契约和 30 个 V2 成员历史测试全部成功。
- `node scripts/architecture/check-engine-repository.mjs` 通过，包含 6 个 OpenMLS 可执行验证和 4 个负向架构夹具。
- 格式、Cargo 元数据和差异合法性检查通过。
