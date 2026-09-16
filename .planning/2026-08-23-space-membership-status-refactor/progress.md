# 进度记录：Space 成员状态查询与缺失状态恢复拆分

## 2026-08-23

### 阶段 1：查明旧实现原因

- **状态：完成**
- 追踪 `load_state_with_presence()` 的提交历史和调用位置。
- 确认缺失状态创建最初来自统一成员流程的懒初始化，而不是查询需求。
- 确认提交 `8203829` 将同一逻辑扩展为旧安装缺失状态恢复。
- 核对旧安装恢复和全新安装负向回归测试。
- 确认新 Space 初始化已经明确创建并保存成员状态。
- 确认旧安装缺失状态恢复仍在后台异步执行，查询存在先于恢复完成的窗口。
- 否决把旧方法整体迁入 `space/membership/state.rs` 的方案。

### 阶段 2：设计恢复边界

- **状态：进行中**
- 确认不能在 Engine 或 Space 后台任务构建时硬执行恢复，因为加密仓储可能仍被锁住，必须保留手动解锁入口。
- 确认同步恢复应接入 `PostSessionReadiness::prepare_data()`：手动解锁和安全存储恢复共用该入口，并且它发生在 presence 与成员活动恢复之前。
- 确认恢复应排在 Space 版本升级之后；升级若已经重建 Space，会先明确建立新成员状态。
- 确认恢复失败应中止本次会话就绪，查询保持不可用，不猜测旧成员资格。
- 确认必须移除后台运行时中的同一恢复步骤，避免两个负责人。
- 确认仅暂停后台任务不能覆盖配对和用户写入；由 `SpaceModules` 创建共享成员执行锁，同时交给旧成员负责人和新恢复用例。
- 阶段 2 完成，下一步进入恢复用例的失败先行测试。

### 阶段 3：先迁移缺失状态恢复

- **状态：进行中**
- 在 `space/recover_missing_space_membership_state/tests.rs` 新增独立用例级回归，不依赖 `WorkspaceMembership`。
- 旧安装回归要求返回 `RecoveredMissingState`，保存正确 Space 标识、迁移来源标记和时间。
- 全新安装回归要求返回 `NoStateForCurrentInstallation`，仓储保持为空。
- 保留原有大对象回归作为迁移期间的行为基线。
- 清理上一步删除 `space/membership/` 后遗留的模块声明。
- 定向编译仍被此前 66 个 application 迁移错误阻断，编译器尚未进入新测试模块；当前只确认测试文件格式和差异检查通过，不能记为红灯已执行。
- 实现 `RecoverMissingSpaceMembershipStateUseCase` 最小执行流程：已有状态原样保留，旧安装缺失状态时保存迁移来源，全新安装缺失状态时不写入。
- 将仓储锁定、状态损坏、仓储不可用和成员身份不可用收敛为 application 错误。
- 新模块生产代码未出现编译错误；当前只有尚未接线产生的未使用提示。
- 定向测试仍在进入测试模块前被相同的 66 个既有错误阻断，未声明通过。
- 将成员状态仓储接口和错误从 core 迁入 application 的 `space/membership_state`。
- infra 改为直接实现 application 接口，Engine 通过 `uc_application::deps` 组装，产品错误映射通过 Facade 类型完成。
- 删除 core 中旧接口和错误；core 独立全目标检查通过。
- 仓储内部失败统一映射为不可用，跨 Space 状态核对失败继续归为损坏，不再透传数据库错误文本。
- application 和 infra 的完整检查仍被相同的既有 application 编译错误阻断；输出中没有本次仓储接口迁移产生的新错误。
- 补充恢复用例的已有状态不覆盖回归，验证没有当前历史时状态内容和持久化写入次数均不变。
- 补充已有当前历史时清除旧迁移标记的回归；当前最小实现尚未实现该行为，预期后续实现使其转绿。
- 补充仓储锁定、状态损坏、写入不可用和成员身份不可用四类错误映射回归。
- 测试使用真实 core 成员历史推进生成已应用历史，不伪造不可到达的状态组合。
- 定向测试命令再次被相同的 66 个既有 application 编译错误阻断，未进入新测试，不能记录为已通过或已观察到红灯。

### 策略纠正

- 用户确认此次升级要求所有设备重新配对。
- 核对 ADR-023、升级用例、单设备重建和重新配对提示，确认旧成员自动恢复已被正式取代。
- 决定删除 `migrated_from_pre_adr_020` 和错误的缺失状态恢复用例，不再实现迁移来源清理。
- 成员状态仓储 Port 已迁入 application，这项边界调整保持不变。
- 删除错误新增的缺失状态恢复模块和 8 个旧策略测试。
- 删除 core 领域状态中的 pre-ADR-020 标记、application 后台恢复和安装来源判断。
- 删除当前成员范围对旧成员表与旧保护关系的回退；没有当前签名历史时统一返回不可用。
- infra 旧 V2/V3 存储结构保留字段位置用于解码，写入固定为 false，转换为领域状态时丢弃。
- 既有宿主回归继续验证升级后 Space ID 变化、只保留本机、重新配对提示跨重启保留及本机历史保留。
- 阶段 3 完成，下一步回到 `QuerySpaceMembershipStatusUseCase`，删除其对 `WorkspaceMembership` 投影的依赖。
- 新增 `QuerySpaceMembershipStatusDeps`，只收纳查询所需的 8 项依赖，不包含旧迁移来源、保护状态、成员身份或执行锁。
- 该依赖集合保持 application 内部可见，并直接引用成员状态模块，不经过对外组装依赖出口。
- `cargo fmt --all -- --check` 与 `git diff --check` 通过。
- `cargo check -p uc-application --lib --locked` 仍被此前目录迁移留下的 66 个错误阻断；输出没有指向新依赖文件的错误，不能记录为 application 编译通过。
- `cargo metadata --locked --format-version 1` 通过。
- 架构检查仍因脚本读取已删除的旧 `space/convergence/connectivity/reachability.rs` 路径而中止，与本次依赖集合无关。
- 查询用例新增当前签名成员历史的读取与验证：历史缺失返回不可用，签名历史无法解码或验证时返回损坏，不读取旧成员资料推断资格。
- `cargo check -p uc-application --lib --locked` 仍被未完成的 admission、查询构造和成员目录迁移阻断；新增的成员历史读取与验证方法没有产生编译错误，不能记录为 application 编译通过。
- 查询用例新增本机成员身份解析：当前安全身份不可用、无效和仓储失败分别映射为不可用、损坏和查询失败；本机成员资格只由对应成员实例是否仍在已验证历史的活动集合中决定。
- 为查询用例补充 Rustdoc，明确它组合保存进度、已验证历史、设备资料、在线状态和准入状态生成临时产品结果，在线状态不参与成员授权。
- 本机成员身份解析与 Rustdoc 的格式、差异检查通过；application 检查仍被此前 67 个未完成迁移错误阻断，输出没有指向新增身份解析逻辑的问题。
- 按维护要求将成员状态查询用例的 Rustdoc 改为中文，职责和安全边界说明不变。
- 优化查询用例内部方法命名，明确区分当前 Space 状态、当前已验证历史、profile 修订号、本机成员判断、无活动 Space 查询和不可用结果构造；行为不变。
- 新增 `query_space_membership_status/active_space_status.rs`，以已验证成员事实一次生成活动 Space 产品状态；查询用例和成员变更结果共用同一实现。
- 查询用例不再调用 `WorkspaceMembership` 生成产品状态，只保留活动 Space 是否已接入的会话判断。
- 删除 `workspace_membership/projection/membership_status.rs`；旧内部快照查询迁到 `projection/snapshot.rs`，成员变更所需的结果加载留在变更流程附近。
- 没有当前签名历史时，设备信任决定直接返回不可用，不再进入旧 `membership_reconciliation` 分支修改成员状态。
- 用户明确后续目标是把 `WorkspaceMembership` 拆成多个独立标准用例，不保留新的大对象或转发用例。
- 查询用例的 profile 依赖与活动 Space 依赖按生命周期分开；用例内部可替换活动依赖，不再保存 `WorkspaceMembership`。
- `SpaceModules` 保存活动成员状态查询所需能力，`ProfileSpaceAdmission` 接入完整模块并同时维护查询依赖和事件订阅，Engine 不组装查询用例。
- 将 `pending_inbound_member` 从修改准入流程的对象迁到只读准入投影，成员状态查询的活动 Space、当前加入、入站候选和 profile 修订号组合已闭环。
- 独立目标目录的 application 检查完成，错误数由 66 降为 65；查询模块、Facade 接线和 `SpaceModules` 没有本次新增错误，剩余错误仍属于此前未完成的准入与成员目录迁移，不能记录为 application 编译通过。
- `cargo metadata --locked --format-version 1`、`cargo fmt --all -- --check` 和 `git diff --check` 通过。
- 架构检查仍因脚本读取已删除的旧 `space/convergence/connectivity/reachability.rs` 路径中止，与本次查询用例独立化无关。
- 新增 application `MembershipHistoryRepositoryPort`，普通读取和版本比较替换返回锁定、损坏、冲突或不可用；不再返回准入 profile 元数据。
- 从 core `AdmissionAttemptRepositoryPort` 删除普通成员历史读取和替换，保留准入记录与成员历史的联合提交方法。
- `DieselAdmissionAttemptStore` 直接实现新接口，继续在同一加密事务内保存历史并推进 profile 修订号。
- 查询、决定、准入事务、准入恢复、成员同步和测试夹具改接成员历史接口；Engine 从同一个具体存储组装两个独立 trait object。
- 用户明确本轮不运行构建；只执行格式化、静态调用搜索和 `git diff --check`。
- 将未完成的 `decide_space_membership_change` 重命名为 `decide_pending_membership_removal`，删除内部泛化的成员变化和产品选择词汇。
- 用例、依赖和错误统一改为 `DecidePendingMembershipRemoval*`；内部加载结果改为已决定、不再等待和仍待决定。
- `PendingMembershipRemoval` 直接保存移除事件、本机成员实例和签名凭据；二次确认判断直接使用 `RemovalDecision::Accept` 与 `confirm_self_removal`。
- core 新增 `create_unsigned_local_removal_decision`，集中计算父历史、接受或拒绝摘要、lineage 和凭据字段。
- `verify_and_record_local_decision` 重命名为 `apply_signed_local_removal_decision`；core、旧 application 调用和测试全部迁移。
- 新决定用例新增 `sign_removal_decision`，只向 core 请求未签名决定并调用当前成员签名能力，不修改或保存历史。
- 新决定用例新增 `prepare_history_commit`：由 core 将签名决定应用到内存历史，要求结果为新保存，再编码预期旧历史和下一版历史；该阶段仍无持久化副作用。
- 新决定用例新增 `commit_removal_decision`：版本比较替换成功表示决定正式成立；并发冲突返回 `HistoryChanged`，留给 `execute()` 重新加载业务状态。
- 新决定用例新增 `apply_committed_removal_effects`：用已提交历史解析移除发起设备，接受写入一致关系、拒绝写入分叉关系；相同关系重试不推进修订或重复保存。
- 将 `WorkspaceMembership` 的成员状态锁改为共享 `Arc<Mutex<()>>`，并统一命名为 `state_write_lock`；所有旧成员状态写路径继续使用同一锁。
- `SpaceModules` 创建并保存唯一生产成员状态写锁；待决定成员移除依赖该锁并在本地效果完整读改写期间持有，Engine 不接触锁。
- 删除 `WorkspaceMembership` 无监听者的 wake 字段、handle、notify 方法和全部调用；保留 discovery 中被运行期实际监听的独立 wake。
- 新增共享 `SpaceMembershipStateEvents`；`SpaceModules` 创建并交给旧成员流程与待决定移除用例，旧订阅入口保持不变。
- `apply_committed_removal_effects` 只在 core 返回真实状态变化、状态保存成功后发布快照；幂等重试不重复发布。
- 新增 `MembershipRecoveryRequests`，`SpaceModules` 创建并交给旧运行期和新决定用例；运行期新增明确的 `Requested` 触发来源。
- 决定历史版本替换成功后立即请求后台恢复，本地关系效果随后执行；状态保存失败不再导致发送请求丢失。
- 将根级 `membership_recovery.rs` 移到 `membership_runtime/recovery_requests.rs`，由后台运行期模块拥有；调用路径只改变归属，行为不变。
- 新增 `DecidePendingMembershipRemovalResult`，只使用具体移除语义并统一携带最新成员状态；旧泛化结果留待 Facade 接线时一次删除。
- `QuerySpaceMembershipStatusUseCase` 改为由 Profile Facade 通过 `Arc` 持有，供查询入口和待决定移除用例共享同一实例。
- 新增标准 `execute(removal_event_id, decision, confirm_self_removal)`：完整处理已决定、不再等待、本机确认、提交冲突、接受和拒绝，并统一返回最新成员状态。
- Profile Facade 在活动 Space 接入时用共享历史、状态、锁、事件、恢复请求和查询实例构造决定用例；无活动 Space 时返回不可用。
- Engine `DecideDeviceTrustChange` 改走 Profile Facade，并将新用例五种结果映射回原公开结果；AppFacade 和 MemberRosterFacade 旧转发已删除。
- 删除 `WorkspaceMembership::decide_device_trust_change()`、`device_trust_decision_lock` 和旧 `SpaceMembershipChangeDecisionResult`；生产决定链路只保留新用例。
- 删除四组依赖旧成员大对象和旧成员历史的决定测试；新用例新增接受本机移除确认、拒绝后分叉、重复或冲突提交返回已保存决定、并发提交只保存一次的直接回归。
- `cargo fmt --all -- --check` 和 `git diff --check` 通过；按用户要求本轮未运行构建或测试。
- 新增 `InitiateSpaceMemberRemovalUseCase`，一次负责本机资格、目标成员、签名移除、版本保存、状态失效和后台传播请求。
- core 新增本机移除事件生成规则，application 不再手工拼装父历史、深度、成员摘要、lineage 和凭据字段。
- Engine `RemoveMember` 改经 Profile Facade 调用新用例；AppFacade 与 MemberRosterFacade 的旧转发已删除，外部操作和结果结构不变。
- 成员后台运行期在收到显式恢复请求时执行成员历史同步；保存成功后即使目标设备离线，也可由后续上线、请求或周期恢复继续传播。
- 删除旧大对象中重复的新历史发起实现；旧格式测试所需分支改为测试专用方法，不进入生产调用链。
- 新用例新增保存后产生传播请求与状态通知、未知目标不写入、本机目标不写入的直接回归；core 新增事件绑定和非法目标规则回归。
- 开始阶段 7：将成员历史仓储和历史签名验证组合为 application 共享访问对象。第一小步只实现可信读取并迁移查询，不修改保存流程。
- 诊断可信读取首版的类型错误：修正方向是从 `super` 导入仓储接口与错误、显式导入 `Arc`，并将 store 类型的导出范围保持为 `pub(crate)`；尚未由 Agent 修改实现。
- 完成可信成员历史读取骨架：共享对象持有原始仓储和历史验证器，加载后对象封装原始版本与已验证历史；内部导入和可见范围检查通过。
- 两个成员移除用例测试已接入各自现有的成员历史仓储和测试验证器，用于后续查询读取迁移；测试场景和断言未改变。
- Profile 测试接线已使用活动 Space 自己的成员历史仓储和历史验证器构造共享可信读取对象；查询逻辑尚未切换。
- 为共享历史可靠保存新增两条测试先行回归：有效签名事件基于加载版本提交并可重读；加载后发生并发更新时提交返回冲突且不覆盖新历史。生产方法尚未实现，按约定未运行构建，红灯未实际执行。
- 实现共享历史的签名事件应用与可靠提交：加载对象内部持有验证能力，提交统一编码并按加载时原始版本比较保存，成功返回已提交历史；尚未迁移生产写用例。
- 发起成员移除已改为只依赖共享成员历史对象；原始仓储读取、手动验证解码、手动编码和版本比较保存均已删除。签名、提交后通知、后台传播请求和返回结果保持原顺序。
- 待决定成员移除已改为持有加载后的可信历史并通过共享存储提交；准备对象不再保存原始字节和替换字节，提交成功返回已提交历史，冲突继续转换为“待处理项已变化”。
- 查询、发起移除和决定移除均完成共享成员历史迁移；活动查询依赖中临时保留的原始仓储和历史验证器字段及四处组装已删除。
- 阶段 7 完成：共享成员历史对象统一隐藏读取、验证解码、签名事件或决定应用、编码和版本比较提交；各用例仅保留面向自身稳定错误的简短映射。
- 发起成员移除已删除 `MemberRepositoryPort` 依赖；core 从当前有效成员集合和签名准入事实解析目标设备，成员被移除或缺少签名事实时不再由资料投影补齐。
- 决定成员移除已删除 `MemberRepositoryPort` 依赖；提交后的关系效果直接从签名历史取得移除发起设备，缺少可信准入事实时按历史损坏处理。
- 成员历史版本替换成功后返回同一事务写入的新修订号，共享提交结果同时携带已提交历史与修订号。发起成员移除据此生成原有快照，已删除完整成员状态查询依赖及对应测试组装。

## 当前工作树说明

- 当前存在尚未完成的成员状态查询重构修改。
- `space/membership/state.rs` 是基于错误前提创建的未完成草稿，后续实现前应删除或重新设计。
- `task_plan.md`、`findings.md`、`progress.md` 根目录文件属于此前工作记录，本计划使用独立 `.planning` 目录，不覆盖它们。

## 验证记录

| 检查 | 结果 |
| --- | --- |
| 提交历史调查 | 完成：定位 `39b0733` 与 `8203829` |
| 旧安装缺失状态回归 | 已存在，必须保留 |
| 全新安装负向回归 | 已存在，必须保留 |
| 生产代码修改 | 本轮未继续修改 |
| 新恢复用例回归测试 | 已写入；执行被既有 application 编译错误阻断 |

## 恢复上下文检查

| 问题 | 答案 |
| --- | --- |
| 当前在哪里？ | 阶段 3，将缺失状态恢复切换到唯一生产入口。 |
| 接下来去哪里？ | 先迁移恢复，再提取查询，最后验证提交。 |
| 目标是什么？ | 查询不再依赖大对象，同时保留旧安装恢复能力。 |
| 已经确认什么？ | 缺失状态创建是懒初始化和旧安装恢复的历史叠加。 |
| 已经做了什么？ | 完成历史、测试、调用时序调查并记录结论。 |
