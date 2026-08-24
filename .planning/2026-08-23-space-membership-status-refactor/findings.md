# 调查结论：Space 成员状态查询与缺失状态恢复

## 当前需求

- 将容易误解的 `DeviceTrust` 应用模型改为 Space 成员状态模型。
- 建立 `QuerySpaceMembershipStatusUseCase` 作为唯一完整查询流程。
- 查询用例不能只是转调 `WorkspaceMembership` 的投影方法。
- Engine 必须始终通过 application Facade 调用，不能直接看到用例。
- 最终逐步拆除承担过多责任的 `WorkspaceMembership`。

## 2026-08-23 策略纠正

- ADR-023 已经取代 ADR-020 的旧成员自动恢复方案。
- 当前正式升级流程会重建为只包含本机的新 Space，清除旧设备关系，并持久提示全部设备重新配对。
- 因此 `migrated_from_pre_adr_020` 不再代表有效业务状态；继续使用它会让旧成员表重新参与成员资格判断，与现行安全策略冲突。
- `RecoverMissingSpaceMembershipStateUseCase` 建立迁移来源的方向错误，应整体删除。
- 缺少成员状态的旧资料应继续或重新进入单设备重建，而不是恢复旧成员范围。
- application 成员状态仓储 Port 的迁移仍然正确，予以保留。

## 原实现为什么在缺少记录时创建状态

### 第一阶段：大对象的懒初始化

- 提交 `39b0733` 首次引入统一成员流程负责人。
- 当时 `load_state()` 在仓储返回空值时创建一份内存中的 `WorkspaceConvergenceState::fresh(...)`。
- 创建出的状态不会立即保存；后续加入、决定或历史处理产生变化后才保存。
- 设计目的不是查询，而是让所有成员操作不需要先调用单独初始化步骤。

### 第二阶段：恢复旧安装缺失的成员状态

- 提交 `8203829` 修复了真实资料形状：旧安装仍有成员、可信设备和地址记录，但成员状态表完全为空。
- 新增 `WorkspaceConvergenceStateOrigin`，区分全新安装和旧安装。
- 新增 `was_persisted`，让启动恢复知道状态是读取出来的还是临时建立的。
- 旧安装缺少状态时建立带 `migrated_from_pre_adr_020` 标记的状态，并由启动恢复加密保存。
- 全新安装缺少状态时不保存，避免误入旧资料升级。

以上是 ADR-020 旧方案的历史背景，不再是当前目标行为。ADR-023 已明确用单设备隔离和全部重新配对取代该流程。

## 当前必须保留的行为

- `recovery_persists_missing_state_for_an_existing_installation`：旧安装缺少记录时必须保存迁移来源。
- `recovery_does_not_persist_missing_state_for_a_fresh_installation`：全新安装缺少记录时不能保存。
- 已有状态记录永远优先，不能被安装来源重新解释。
- 恢复不能补造签名成员历史，只能建立进入受控旧资料升级所需的来源标记。
- 当前 Space 标识为空的旧状态会使用当前成员身份补齐。

## 为什么不能直接让查询对 `None` 返回不可用

- `WorkspaceMembershipRuntime` 启动后异步执行 `recover_legacy_migration_marker()`。
- `ProfileSpaceAdmission` 可能在恢复完成前已经附加活动 Space 并接受查询。
- 旧查询在这段窗口中通过临时生成带迁移标记的状态，仍能解释旧成员范围。
- 如果直接删除这条逻辑，旧安装启动时可能短暂显示成员不可用或空列表。

## 当前结构问题

`load_state_with_presence()` 同时负责：

1. 从仓储读取状态。
2. 为没有状态的流程创建内存初始值。
3. 判断旧安装缺失状态并建立迁移来源。
4. 为旧记录补齐 Space 标识。

这四项不应继续作为普通读取的隐含行为。

## 正确拆分方向

- 新 Space 初始化负责明确创建并保存初始成员状态；现有 `initialize_new_space_membership()` 已经这样做。
- 旧安装恢复负责识别“已有安装但没有成员状态”，创建并保存迁移来源。
- 普通查询只读取真实状态，不创建、不保存。
- 恢复必须在活动 Space 对外可查询前完成，或者查询必须明确返回恢复中的稳定结果。
- 查询用例组合活动 Space 成员事实、当前加入、入站候选和 profile 版本，但不拥有恢复写入。
- 成员状态仓储接口只被 application 流程消费，因此归 `space/membership_state`，不留在 core。
- infra 已经正式依赖 application，可以直接实现该接口，不形成反向依赖。
- 仓储错误只保留锁定、损坏和不可用三类，不向 application 传递数据库错误文本。

## 同步恢复入口结论

- `SpaceApplicationRuntime` 在 `build_sync_engine_assembly()` 内启动，此时加密成员仓储可能仍然锁住。
- 因此不能把缺失状态恢复作为 Engine 或 Space 后台任务构建的硬前置，否则会阻止用户进入手动解锁流程。
- `PostSessionReadiness::prepare_data()` 同时由手动解锁和安全存储会话恢复调用。
- 调用该位置时 Space 密钥已经可用，并且成员在线状态和 Space 活动尚未恢复。
- 恢复应放在 `upgrade_space.execute()` 之后、移动端资料补齐和成员仓储检查之前。升级可能已经执行 Space 重建并明确建立新成员状态，恢复随后只处理仍然缺失的旧安装。
- 恢复失败必须让本次解锁或会话恢复失败；不得启动在线状态、成员交换或接收活动。
- `ProfileSpaceAdmission` 可以提前附加活动 Space。仓储仍锁住、恢复尚未执行或恢复失败时，成员状态查询统一返回不可用，不根据旧成员表猜测结果。

## 恢复用例责任

建议名称：`RecoverMissingSpaceMembershipStateUseCase`。

唯一入口：

```text
execute()
```

它负责：

1. 读取真实成员状态。
2. 已有状态时清理已经被当前历史取代的旧迁移标记。
3. 旧安装缺少状态时，根据当前 Space 身份建立并加密保存迁移来源。
4. 全新安装缺少状态时不建立旧资料来源，并返回明确结果。

它不负责查询产品成员状态、不生成签名成员历史、不启动后台任务。

依赖为安装来源、成员状态仓储、当前成员身份、时钟，以及与成员写流程共享的串行约束。

期望的稳定结果：

- `ExistingState`：已有真实状态，不创建替代记录。
- `RecoveredMissingState`：旧安装缺少状态，已保存迁移来源。
- `NoStateForCurrentInstallation`：当前安装缺少状态，未写入任何迁移记录。

首批用例级回归只固定后两种结果和持久化副作用；已有状态及错误分类在实现下一轮补齐。

## 失败处理

- 仓储锁住或暂时不可用：恢复失败，允许用户之后重新执行解锁或恢复。
- 状态损坏：恢复失败并保持关闭，不覆盖原记录。
- 当前 Space 身份不可用：恢复失败，不使用空 Space 标识创建记录。
- 全新安装缺少记录：不写迁移来源；该状态不能被解释为旧成员资格。
- 任何失败都发生在 presence 和成员活动恢复之前。

## 仍需解决的并发问题

- 当前 `WorkspaceMembershipRuntime` 在构建后立即启动，旧恢复也在后台任务中执行。
- 新恢复用例接入时必须同时删除后台任务中的同一恢复步骤，保证唯一负责人。
- 其他成员写流程仍可能与同步恢复并发。过渡阶段要么共享同一执行锁，要么让成员后台任务在会话就绪前保持暂停。
- 仅让后台任务保持暂停不能覆盖配对入站和用户操作，因此不足以保证恢复不与写入并发。
- 过渡阶段由 `SpaceModules` 创建一把共享成员执行锁，同时注入旧 `WorkspaceMembership` 和新的恢复用例。
- 新恢复用例接管后删除后台任务中的旧恢复调用，保证恢复行为只有一个负责人。
- 后续成员写用例逐个迁出时继续使用同一把锁，直到持久化能力本身提供可靠的并发写入约束。

## 相关位置

- `crates/uc-application/src/space/workspace_membership/mod.rs`
- `crates/uc-application/src/space/workspace_membership/membership/bootstrap.rs`
- `crates/uc-application/src/space/workspace_membership/runtime.rs`
- `crates/uc-application/src/space/workspace_membership/membership/tests.rs`
- `crates/uc-application/src/space/query_space_membership_status/`
- `crates/uc-application/src/space/admission/profile.rs`
- `crates/uc-engine/src/runtime/mod.rs`
- `crates/uc-engine/src/assembly/sync_engine.rs`
- `docs/architecture/architecture-bible.md`

## 不采用的方案

| 方案 | 不采用原因 |
| --- | --- |
| 将原方法整体移动到 `membership/state` | 只是移动混合责任，没有澄清行为所有权。 |
| 查询遇到空记录时直接创建状态 | 查询产生持久化前置事实，职责错误。 |
| 查询遇到空记录时直接返回空结果 | 会破坏旧安装恢复完成前的启动窗口。 |
| 新增一个由 `WorkspaceMembership` 实现的查询接口 | 形成新的转发层，查询复杂度仍留在大对象中。 |
| Engine 直接组装和执行查询用例 | 穿透 application Facade，暴露内部组织。 |

## WorkspaceMembership 后续拆分方向

- `WorkspaceMembership` 不作为长期负责人保留，后续按完整用户行为拆成多个独立用例。
- 当前先闭环 `QuerySpaceMembershipStatusUseCase`，再提取 `DecidePendingMembershipRemovalUseCase` 和发起成员移除等写用例。
- 新用例不得只转发到 `WorkspaceMembership`；必须接管校验、写入、失败分类、恢复和完整结果。
- 查询与写用例需要返回相同产品状态时，共用 `active_space_status` 的结果生成规则，不保存两套判断。
- 活动 Space 的依赖由 `SpaceModules` 持有并交给 Facade 内部用例，Engine 不接触用例依赖。

## 成员历史仓储归属

- `AdmissionAttemptRepositoryPort` 中的普通成员历史读取和替换没有 core 调用者，且让查询与决定用例看到整个准入仓储。
- 新的 `MembershipHistoryRepositoryPort` 归 application 的 `space/membership_history` 共享模块所有，只提供读取和版本比较替换。
- 新接口不返回 `AdmissionProfileMetadataV1`；普通替换调用者没有使用该返回值，infra 仍在同一事务中推进 profile 修订号。
- infra 的 `DieselAdmissionAttemptStore` 直接实现准入仓储与成员历史仓储，不增加转发 Adapter。
- `compare_and_advance_with_membership_history_v2` 继续属于准入仓储，因为准入记录推进和成员历史替换必须原子提交。
- `HistoricalMembershipSignatureVerifier` 保留在 core，core 的成员历史验证算法直接依赖它。
- `CurrentMemberSignaturePort` 与 `MemberRepositoryPort` 暂留 core；前者是跨 application/网络 Adapter 的成员安全能力，后者后续应单独评估读写接口拆分。
- `ClockPort` 没有 core 调用者，长期应迁入 application，但涉及约 49 个文件，不能混入当前决定用例。

## 待决定成员移除的领域表达

- 当前只有从其他成员收到的 `RemoveDevice` 需要本机用户决定，不应提前抽象为通用 `membership change`。
- 用例名称为 `DecidePendingMembershipRemovalUseCase`，内部直接使用 core `RemovalDecision::Accept/Reject`。
- 加载结果只表达 `AlreadyDecided`、`NoLongerPending` 和 `Pending`；待处理对象命名为 `PendingMembershipRemoval`，不是尚未存在的“待处理决定”。
- 本机是移除目标时，接受决定必须得到 `confirm_self_removal` 二次确认。
- 外部 `DecideDeviceTrustChange` 契约保持稳定；application 新用例接线时再一次完成结果映射和旧入口删除。
- 成员状态仓储当前是普通读写接口，没有版本比较；迁移期间由 `SpaceModules` 创建唯一 `state_write_lock`，旧成员流程和新写用例必须共享，不能各自持有互不相关的锁。
- `WorkspaceMembership.wake`、`wake_handle()` 和 `notify()` 没有任何 `notified()` 消费者；旧恢复运行期实际只由启动、恢复、30 秒周期和在线事件驱动。该 wake 是死路径，可以删除；`discovery` 的另一只 wake 有运行期监听者，不能混删。
- `WorkspaceMembership.events` 有真实订阅者，不能随 wake 一起删除；应提升为 `SpaceMembershipStateEvents`，让旧流程和新写用例发布同一失效快照。
- 新恢复请求必须绑定到成员历史提交成功点，而不是本地关系效果完成点；否则历史已提交但状态保存失败时，后台发送只能等待周期恢复。
- 恢复请求的删除检查指向成员后台运行期：删除任一写用例后其他生产者仍可能存在，删除运行期后请求完全失去消费者。因此它归 `membership_runtime`，不漂在 Space 根目录或某个用例内部。
- 决定的父历史、接受或拒绝摘要、lineage、凭据绑定和本机历史推进属于 core 成员历史规则，不能迁入 application。
- core 提供 `create_unsigned_local_removal_decision` 和 `apply_signed_local_removal_decision`；application 只负责调用签名能力、版本保存、可恢复效果和结果查询。
- 不新增一层本机决定状态枚举。真正的状态由签名成员历史表达；额外枚举只会被立即转换成用例内部上下文，不能隐藏更多复杂度。
- 外部决定操作已完全切换到新用例。旧 `WorkspaceMembership::decide_device_trust_change()`、专用锁和泛化决定结果没有其他生产调用者，可以一次删除。
- 底层 `decide_membership_removal()` 仍保留。它处理较低层的成员决定与兼容恢复，不是产品决定入口，不能随旧转发一起删除。
- 发起成员移除的成功边界是“本机签名历史已保存”，不是“所有设备已经接受”。网络发送失败不能回滚已保存移除。
- 新的发起用例不需要直接持有网络交换能力。成员历史保存成功后发出恢复请求，后台运行期消费请求并主动同步当前成员历史；离线设备仍由上线和周期触发继续恢复。
- 发起事件的父历史、深度、lineage、成员结果摘要和作者凭据绑定属于 core 规则；application 只选择目标、请求未签名事件、调用当前成员签名并保存。
- `WorkspaceMembership::submit_removal()` 的新历史分支已删除。剩余旧历史分支仅由旧格式测试调用，并明确标记为测试夹具，不再形成第二条生产入口。
- 查询、发起移除和决定移除都重复执行“读取原始字节、验证解码、修改后编码、按原始字节比较保存”。下一阶段由 application `membership_history` 共享模块统一隐藏这些步骤。
- 共享模块不能只是把仓储和验证器包装后原样转发；它必须返回已经验证且携带不透明原始版本的加载对象，并统一完成可靠提交，三个用例不再接触持久化字节。
- 迁移顺序采用读取优先：先实现可信加载并迁移查询，再增加提交能力迁移两个写用例，避免一次同时改变读取和并发保存语义。
- `membership_history/store.rs` 的首版应从同级模块导入仓储接口和错误，并显式导入 `Arc`；经 `crate::deps` 绕行会让内部模块依赖公开组装出口。store 类型保持 crate 内部可见，`mod.rs` 必须使用 `pub(crate) use`。
- 三个用例对共享成员历史错误的映射虽然结构相似，但目标错误类型和公开语义分别属于查询、发起移除和决定移除；不为几行穷举映射新增通用转换层。
- 发起成员移除的目标设备可以直接由当前有效成员集合与签名准入事实解析；成员资料表是展示投影，不得补齐缺失身份或授予成员资格。缺少签名准入事实时按目标不存在处理。
- 决定成员移除的发起者身份已经包含在已提交移除事件对应的签名准入事实中；缺少该事实说明可信历史不完整，应按损坏处理，不能用成员资料投影猜测。
- 发起成员移除依赖完整成员状态查询仅为取得历史提交后递增的 profile 修订号。成员历史仓储本来就在同一事务中递增该值，因此提交成功应直接返回修订号，避免写用例再次执行完整查询。
