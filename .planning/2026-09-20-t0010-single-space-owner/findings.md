# t-0010 唯一空间负责人发现

## 已确认事实

- 当前维护顺序由普通成员维护用例编排：配对恢复之后仍可能继续成员效果、成员更新和历史同步。
- `PeerContact` 存在绕过配对恢复、直接运行历史同步的路径。
- 最终确认第一次暂时失败会返回无立即工作，但当前轮仍继续普通维护；t-0025 真实日志显示两段约 4.5 秒等待，累计延后约 9.5 秒。
- 当前已有持久配对聚合、稳定加入编号、待发最终确认和可重放完成回复，足以作为运行模式的唯一事实来源。
- 邀请方在最终确认处具备唯一正式公布点；加入方只有保存最终完成回复后才应对产品显示完成。
- 当前测试仍允许未确认当前成员参与历史同步，说明候选资料与正式历史尚未完全隔离。
- 当前维护入口 `maintenance/use_case.rs` 在 `PeerContact` 时直接运行同步与清理；其他触发先恢复准入，仅在准入返回 `Yield` 时停止。网络连接暂时失败后准入可能返回普通延期并继续普通维护。
- 入站历史 `handle_history_message/use_case.rs` 在消息解析后直接读取或写入 ledger，它不经过后台维护入口；仅拦截后台轮无法满足入站写入禁令。
- `docs/PLANS.md` 要求 active plan 记录状态、完整负责人、唯一动作、成功/失败和重启/重试责任；`docs/exec-plans/active/index.md` 是正在进行计划的索引。

## 设计决定

- Application 层的空间负责人拥有：当前运行资格、串行工作、配对恢复与普通维护选择、下一次唤醒和统一状态。
- 调用方只表达“可能有变化”，不理解最终确认、让路、队列合并或维护步骤。
- Pairing 时所有普通成员维护以及入站成员历史均没有运行资格。
- Active 时普通维护只处理正式成员。
- NeedsAttention 必须包含稳定原因、恢复动作和重试时间。
- 正式实施规格：`docs/exec-plans/active/2026-09-20-single-space-work-owner.md`。其状态、公开接口、数据责任、边缘情形、分期删除及设备验收均已落盘；当前仍是规划，不误称已实施。

## 仍需在 Phase 0 核对

- 当前持久配对查询能够用哪个最小接口准确区分 Pairing、Active、NeedsAttention。
- 入站成员历史公开入口及其可重试错误契约。
- 终止意外 rebase 后，重新核对 t-0010 当前分支相对基线已有修改与不可覆盖范围。

## 仓库约定核对

- 正式执行计划放在 `docs/exec-plans/active/`；每份必须写明状态、完整负责人、唯一调用、成功/失败结果、恢复责任和验收条件。
- Runtime 只能负责触发、并发、暂停、恢复和关闭，不能掌握配对或成员维护步骤。
- 跨层行为必须由 Application 拥有；Engine 只组装，Infra 提供持久化和网络能力，Core 保存纯规则。
- 本任务改变恢复责任，必须有 active plan；持久格式若变化，同一未发布分支只能形成一个最终目标版本。
- 新模块必须通过删除检查；只增加门禁或转调不构成合格边界。
- 下层网络或存储错误必须保留 source chain；纯业务“Pairing 时不可维护”可以使用稳定结果，不伪造异常。
- `SpaceFacade` 是唯一公开 Space 入口；`SpaceApplication` 只组装；运行期不能掌握业务步骤。
- 准入协议状态由独立加密仓库负责，membership ledger 不得保存准入状态或 outbox；新负责人必须协调两者但不能复制事实。
- `CurrentSpaceMemberScopePort` 是普通成员消费者的唯一范围；在线状态和认证连接都不能授予成员资格。
- 现有稳定设计已经声明：同设备重新加入只有 Sponsor 验证最终确认后才接替旧实例；最终确认请求/回复重试必须幂等。
- 现有历史反熵负责人拥有持久欠账与确认水位；新空间负责人只能决定它是否有运行资格，不能吸收其内部同步步骤。
- 安全要求：新增持久字段默认敏感，必须随既有 MasterKey AEAD 保存；日志和公开错误不得包含身份、邀请、地址或消息材料。

## Git 基线与最新授权

- 进入任务时不是普通分支状态，而是 `hp/uni/t-0010-android` 将唯一提交 `2d994c8a` rebase 到 `af25532c` 的最后一个冲突阶段。
- staged 区的大量文件是正在重放的 `2d994c8a`，不是本轮新增修改；唯一未解决文件是 `docs/architecture/architecture-bible.md`。
- 上述判断适用于当时的用户指令；用户随后明确要求先 rebase onto `origin/main`，这是对旧基线约束的更新。
- 已 fetch 并确认 `origin/main=af25532c5a04b990f9805541f17c8d5cc3e50264`；冲突只在架构圣经的两处维护记录，双方记录均已保留。
- rebase 完成后 HEAD=`d64a9046`，其父提交为 `af25532c`，旧提交 `2d994c8a` 保留为可追溯的重放来源；`range-diff` 仅显示主分支上下文与文档冲突合并。

## Phase 1 代码 seam

- `MaintainSpaceMembershipUseCase::execute` 是后台维护的可观察行为 seam；现有 recorder 能证明实际步骤调用顺序。
- 当前 `AdmissionMaintenanceOutcome::Continue(Deferred)` 会继续 restricted、effects、conflicts、group updates、history sync 和 cleanup，正好对应 t-0025 最终确认连接失败后的插队。
- `PeerContact` 在进入 admission 之前直接运行 history synchronization 和 cleanup，是独立绕行；Phase 1 的 Pairing 测试必须覆盖，Phase 2 再把入站消息统一裁决。
- 准入恢复只在达到单轮即时交换上限、等待 Space transition 或配对完成时设置 Yield；建连失败和 exchange 暂时失败返回 `NoImmediateWork`，最终映射为 Continue(Deferred)。
- `LoadedAdmissionRecovery` 已同时提供 joiner pending、Sponsor confirmation pending 和下一 deadline；可从同一持久摘要派生本轮是否仍由 Pairing 占有，无需新增持久模式字段。
- 第一个红灯测试在维护 seam 使用“未终结且本轮暂时失败”的 admission fake，断言只调用 admissions，普通步骤和网络 recorder 均为零；当前实现会失败并列出全部普通步骤。
- 最小修复不需要新增持久字段：`LoadedAdmissionRecovery::pairing_in_progress` 可由当前 joiner pending 和 Sponsor 等待确认事实直接派生。
- 旧 `Continue/Yield` 同时表达步骤结果和运行资格，容易让暂时失败误走 Continue。Phase 1 已将其拆为明确 `SpaceWorkMode` 与独立步骤结果；普通维护只在 Active 时有资格。

## Phase 2 入站历史 seam

- `HandleMembershipHistoryMessageUseCase::execute` 当前会直接处理摘要、分页、冲突和受限消息，并读写成员 ledger；它没有读取持久配对状态，也不经过后台维护裁决。
- 生产装配当前先创建 `MembershipHistoryAntiEntropy`，之后才创建 `SpaceAdmissionProtocol`，两者分别作为网络端点公开；只在后台维护入口加门禁无法阻止入站历史写入。
- 入站公开端点必须在解析或写入历史之前返回稳定可重试结果，并与配对消息和后台维护共享同一串行裁决；仅新增一个会释放锁的布尔查询存在“查询后开始配对”的竞态，只能作为短期迁移层，不能作为最终负责人边界。
- 当前错误契约只有 Locked、RecoveryRequired、Rejected、Unavailable；Phase 2 需要固定一个明确表示“配对仍在进行，请稍后重试”的稳定结果，并验证 ledger 写入次数为零。
- Core 网络端点当前把所有应用层处理失败压成 `MembershipHistoryExchangeError::Rejected`；要让发送方稳定重试，Core 契约也必须区分“配对占用中”和永久拒绝，不能复用 Rejected。
- `SpaceAdmissionProtocol` 已有自己的串行锁，后台维护用例另有一个串行锁，历史入站处理还有第三个串行锁；这正是复杂度和竞态来源。最终负责人应提供一把共享的空间控制锁，并由三个入口共同使用，而不是再增加第四个独立布尔门禁。
- 本地准入、入站准入、准入恢复与普通维护若未全部纳入同一控制锁，成员历史可能在“查询为 Active”后、配对刚开始时继续写入。Phase 2 需要先列全现有准入公开动作，再选择能逐步替换且不产生循环加锁的所有权方式。
- 本地加入、取消、完成本地空间切换和 Sponsor 入站消息都经 `SpaceAdmissionProtocol::execute_exclusively`；后台恢复另用可被本地动作中断的恢复锁，刻意不占用本机动作锁等待网络。统一负责人不能简单拿一把 Mutex 包住所有网络等待，否则会破坏取消/替换能够中断恢复的现有保证。
- Phase 2 采用可删除的查询适配层：成员历史公开端点在任何解析/ledger 访问前，从现有加密准入记录派生模式；Phase 4 再由事件驱动的唯一负责人吸收该查询与现有三把入口锁，保留取消中断能力。
- Iroh 成员历史帧目前只有 ACCEPTED/REJECTED 两种首字节。可以增加 BUSY：新版发送方把它视为可重试的 PairingInProgress；旧版发送方会把未知首字节视为传输失败并重试，因此混合版本不会把暂时占用误判为永久拒绝，也不会允许写入。
- 新版向旧版发起成员历史时，旧版仍无法知道新版本机正在配对；安全保证依赖新版接收端本地门禁。旧版邀请方是否在正式提交前传播候选由 Phase 3 的 admission 协议版本协商负责，不应通过历史网络错误掩盖。
- Phase 2 红灯实际返回 `Rejected`，证明公开端点没有配对占用语义；现有处理已经触及 ledger 后才由无当前历史状态失败。正确实现必须在 `HandleMembershipHistoryMessageUseCase::execute` 之前裁决。
- 入站门禁现在通过 `SpaceAdmissionProtocol` 读取同一 `PendingAdmissionRecoveryStatePort`，没有新增持久开关；Pairing 阻止处理，Active 继续，NeedsAttention 失败关闭，暂时读不到状态则返回可恢复失败。
- Iroh framing 的 BUSY 值只改变暂时占用的表达；ACCEPTED、REJECTED 和消息编码版本均不变。旧客户端遇到 BUSY 会沿既有未知状态路径得到 Transport，因此可安全重试。
- `unconfirmed_current_member_is_included_in_membership_history_sync` 名称中的 unconfirmed 指已经在正式历史内、但点对点关系为 `RelationshipUnconfirmed` 的成员，不是等待 CompleteAck 的候选。正式提交后的历史核对必须保留；候选隔离由 admission 暂存和 CompleteAck 唯一提交保证。
- 原提交 `d64a9046` 已包含三设备端到端测试 `pending_join_is_not_published_before_final_confirmation`：Sponsor 重启并被第三设备联系时，候选只出现在待确认视图，不进入正式设备列表，第三设备 15 秒内看不到；释放最终确认后才三端收敛并双向传输。

## Phase 4 唤醒与状态收口

- Runtime 仍把上线设备和已认证联系人身份编码进 `MembershipMaintenanceTrigger::PeerOnline/PeerContact`，普通维护据此跳过步骤或绕过持久退避；这让调度器理解业务顺序，与“只提交可能有变化”不符。
- 普通同步已有持久 peer 欠账、确认水位和 `next_attempt_at_ms`；统一为 StateChanged 后可以从正式 ledger 重新选择到期工作，不需要 Runtime 携带设备身份。代价是上线不会绕过尚未到期的持久退避，换来单一事实来源和稳定重启行为。
- 启动、恢复和周期仍可作为负责人内部的观测/扫描原因；外部设备上线、设备联系、历史变化和显式 wake 应合并为同一种 StateChanged。
- Phase 4 的红灯应证明多个不同设备的联系只排入一个通用变化轮次，而不是分别排三种带身份的工作；一轮运行中到达的重复变化必须合并。
- 统一后 `MembershipMaintenanceTrigger` 只保留 Startup、Resume、Periodic、StateChanged；设备上线和联系不再把设备身份或步骤选择带进普通维护。历史同步与群组更新按各自持久欠账选择到期工作，不再由运行期绕过退避。
- 准入交换原本已经持久保存重试次数和下次时间，但失败路径从未推进它，也没有据此跳过过早唤醒。最小修复复用原字段和原密文格式，没有增加第二份模式或新持久版本。
- 普通历史同步的持久结果足以派生三种稳定健康：Healthy、带绝对时间的 Retrying、带原因和恢复动作的 NeedsAttention。该结果已进入 Engine 稳定设备信任查询；产品展示不在 Engine 本轮范围。
- 严格复核确认共享许可不能覆盖配对网络等待，否则本地取消/替换无法触发原有中断。正确边界是：配对恢复保持可中断；仅在准备启动普通维护时取得许可并重新读取持久模式；普通维护和入站历史在许可内完成。
- 删除测试成立：移除共享许可后，后台普通维护与入站历史会重新出现检查后竞态；移除统一运行期后，PeerOnline/PeerContact 分支、定向步骤和退避绕过会重新散落。

## Phase 5 交付检查

- 仓库架构检查原先只接受准入内部的 `Mutex<()>` 字段，无法表达本轮跨配对、普通维护和入站历史的共享许可。检查已改为要求共享所有权字段、许可实现和持有式加锁标记，避免新边界以后被退回为只保护准入内部。
- 全仓并行测试在高负载下分别出现文件恢复等待超时和剪贴板接收计数提前读取；两项相同配置精确复跑通过，串行全仓运行中也通过。成员端到端长组串行执行时，同设备重加测试一次失败，精确复跑通过。这些失败未集中在本轮改动路径，按真实结果保留，未伪装为单次全绿。

## Phase 6 Desktop 独立进程验收 seam

- Engine 的 `dev-tools` 已有进程内 `JoinerFinalConfirmationGate`，它在加入方完成本地激活、保存 `CompleteAck` 前暂停；独立 `uc-connectivity-host` 只通过继承的 stdin/stdout 控制，已经满足“指定测试进程、无全局环境、无真实资料”的边界。
- 最小扩展点是同一 dev-only 控制对象：激活完成后把下一次 continuation `resume` 标记为最终确认连接；第一次返回 `Deferred`，后续让真实 transport 执行。这样故障发生在连接阶段，而不是通过固定延时碰运气。
- 普通成员更新与历史同步的可靠观测应放在 Engine 已有 adapter decorator 边界：分别记录 `GroupUpdateDispatchPort` 与 `MembershipHistoryExchangePort` 的实际网络调用；最终确认则记录 admission transport 的失败、重试和成功。所有事件共享单调序号。
- `MaintainSpaceMembershipUseCase` 已有当前修复的关键规则：admission 未允许普通维护时直接结束本轮。Application 单测 `unfinished_pairing_with_transient_network_failure_excludes_ordinary_maintenance` 证明普通步骤调用为零，但 Desktop 还需要上述独立进程能力复现同一真实顺序。
- 同一 Engine E2E 测试能力应用到 `d64a9046` 后得到确定红灯：事件 1 为最终确认连接失败，事件 2 为普通成员更新，事件 3–5 为成员历史同步，事件 6 才开始最终确认重试。
- 当前行为得到确定绿灯：最终确认首次失败后，下一项配对事件就是最终确认重试，随后收到回复；失败与重试之间普通成员更新和历史同步均为零，最终两端正式成员数收敛。
- 严格复核修正了事件命名：transport 成功只证明已收到最终确认回复，不能替代 Application 随后的状态提交，所以公开测试事件使用 `final_confirmation_reply_received`，最终完成仍必须查询产品状态。
