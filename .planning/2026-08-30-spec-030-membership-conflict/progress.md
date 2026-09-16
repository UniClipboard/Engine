# Spec 030 Progress

## 2026-08-30

- 已完整读取规格、相关领域词表与 ADR，确认分阶段范围和测试接缝。
- 已检查工作树并标记现有未提交修改为用户工作。
- 已开始 Phase 1 Core 规则的 TDD 探索。
- 完成三个 Core 行为切片：sibling 顺序无关编号、Active/Removed/Absent 资格、Same/ancestor 拒绝。
- 新增敏感 ID 的脱敏 `Debug`，避免 branch/head 摘要进入诊断输出。
- Phase 2 新增加密 ledger conflict record/status/选择 intent 字段，并修正新 generation ledger 初始化。
- 新增关系与 conflict record 单次 CAS 提交测试；Infra ledger migration 定向测试通过。
- Phase 3 已建立并接入 Application facade 的单一 resolve action；本机分支完成、Removed 重新配对、重复幂等和相反选择均由同一 ledger CAS 隐藏。
- 远端 Active 选择当前只保存 `Selected/Pending`；恢复包、transition id 和维护续跑仍属于下一工作切片，Phase 3 尚未完成。
- 增加 Core branch transition 七阶段单调状态机；稳定 transition id 与 transition map 已进入加密 ledger。
- 远端 Active 重复选择测试确认 transition intent 和 ledger revision 均保持不变。
- 新增 `MembershipBranchRecoveryPackageV1`：绑定 conflict/branch/recipient/author/expiry/nonce、目标历史、MLS 恢复密文和内容密钥目录密文。
- Core 验证覆盖目标历史重验、branch 重算、双方 Active 资格、过期、错误 recipient 和损坏授权签名。

## Verification

- `cargo test -p uc-core --test membership_history_v2 conflict_ --locked`：2 passed。
- `cargo test -p uc-core --test membership_history_v2 same_or_ancestor_history_is_not_a_selectable_conflict --locked`：1 passed。
- `cargo test -p uc-core --test membership_history_v2 --locked`：33 passed。
- `cargo test -p uc-application diverged_relationship_and_conflict_record_share_one_ledger_commit --locked`：1 passed。
- `cargo test -p uc-infra space::membership_ledger --locked`：1 passed，其他目标按过滤条件运行 0 项。
- `cargo check -p uc-application -p uc-infra --all-targets --locked`：通过（仅既有 warning）。
- `cargo test -p uc-application resolve_conflict --locked`：2 passed。
- `cargo check -p uc-core -p uc-application -p uc-infra --all-targets --locked`：通过（仅既有 warning 及尚未接入 Engine 的公开 re-export warning）。
- `git diff --check`：通过。
- `cargo test -p uc-core --test membership_history_v2 membership_branch_transition_advances_one_phase_and_never_retargets --locked`：1 passed。
- `cargo test -p uc-application resolve_conflict --locked`：3 passed。
- `cargo check -p uc-infra --all-targets --locked`：通过（仅既有 warning）。
- `cargo test -p uc-core --test membership_history_v2 branch_recovery_package_binds_recipient_branch_expiry_and_authorization --locked`：1 passed。
## 2026-08-30 · Application recovery coordinator

- 新增恢复包获取与无副作用 transition preparation ports。
- coordinator 验证 conflict、branch、recipient、expiry、完整历史与授权签名。
- 单次 membership ledger CAS 原子消费 nonce、保存 `Prepared` transition 并推进为 `Transitioning`。
- 新增成功、提交后重试幂等、跨 conflict nonce 重放零账本副作用测试。

## 2026-08-30 · Maintenance recovery wiring

- `RecoverMembershipConflictUseCase` 实现统一 maintenance step port。
- startup/resume/state-changed/periodic 在 effects 后执行 conflict recovery；peer-online 同样执行。
- `SpaceApplication` 装配 coordinator，Engine 当前显式注入 deferred adapters，使选择保持 Pending 且不发生协议降级。
- maintenance 固定顺序 8 项测试通过，`uc-engine` all-target check 通过。
- `cargo test -p uc-application space::application_tests --locked`：1 passed。
- `cargo test -p uc-application space::membership --locked`：59 passed。
- `cargo metadata --locked --format-version 1`、workspace all-target check、fmt、架构检查和 `git diff --check`：通过（仅既有 warning）。
- `cargo test --workspace --all-targets --locked`：成员相关套件通过；`uc-engine` 115 passed / 3 failed，失败集中于既有 clipboard/search host-adapter 路径。
- 单独重跑 `host_clipboard_change_is_processed_by_the_engine_and_stops_on_shutdown` 仍以既有 QueryHistory unavailable code 1243 失败；失败发生在本切片未修改的 history search 路径。

## 2026-08-31 · Real transition preparation adapter

- Infra 新增 `DefaultMembershipBranchTransitionPreparation`，从加密 active manifest 读取真实 source database generation。
- target generation 使用非零随机 128-bit 标识，并保证不同于 source；准备阶段无目录、数据库或 manifest 写入。
- Engine 已替换 transition deferred adapter；recovery transport 仍显式 Deferred。
- Infra 定向测试 2 项通过，Engine all-target check 通过。

## 2026-08-31 · Recovery package issuer

- Application 新增唯一 recovery package issuer 与材料 preparation port。
- issuer 从 ledger 验证本机目标 branch、认证 source device 与 Active recipient instance、Active 本机签发者。
- 生成五分钟有效期与随机 nonce，绑定完整持久历史，并用当前成员签名能力授权。
- 测试确认错误认证设备在材料 preparation 前被拒绝，合法请求产出的包可通过 Core 完整验证。

## 2026-08-31 · MLS external recovery primitive

- Infra 可从目标 MLS 状态导出带 ratchet tree 的签名 GroupInfo，且不导出成员私钥。
- recipient 使用自身现有签名凭据创建 external commit；OpenMLS 原子替换目标树中相同凭据 leaf。
- 定向测试确认目标端应用 commit 后双方 epoch 和 wrapping key 相同，而各自私有 MLS snapshot 不同。
- 首次编译发现 `VerifiableGroupInfo` 未由 prelude 导出，改为从 `openmls::messages::group_info` 显式导入。
- `uc-infra` MLS 定向测试、workspace all-target check、fmt、架构检查与 `git diff --check` 通过。
- 复审修正 external recovery 新增路径的错误吞噬：稳定分类改为携带 `anyhow::Error` source，脱敏 `Debug` 不输出底层细节；测试验证 InvalidMessage 分类、`source()` 非空和稳定显示文本。

## 2026-08-31 · Authenticated recovery server

- Application issuer 新增 begin 动作；在导出 GroupInfo 前执行与最终签发相同的 branch、source device、recipient 和本机签发者校验。
- 最终签发要求非空 external commit，并把它传给材料 adapter；两阶段不共享未经重新验证的内存授权状态。
- Iroh handler 从连接公钥解析已知成员设备，未知连接、畸形帧、错误消息方向或 Application 拒绝均只返回脱敏 Rejected。
- 共享 Iroh node 已注册 recovery ALPN；生产组装测试增加协议可达性断言。
- Application issuer 定向测试与 Engine 生产协议装配测试通过；workspace all-target check、fmt、架构检查和 `git diff --check` 通过。

## 2026-08-31 · Two-phase recovery wire contract

- Infra 定义专用 Iroh ALPN 和五类有界帧：GroupInfo 请求/响应、external commit 提交、最终恢复包和拒绝。
- 两个请求阶段均显式绑定 conflict、target branch 与 recipient；所有 `Debug` 对绑定与密码学负载脱敏。
- postcard 解码错误保留 source，空密码学载荷、错误版本和超过 4 MiB 的帧稳定拒绝。
- 定向 wire round-trip 与损坏帧 source 测试通过。

## 2026-08-31 · Encrypted recovery session state

- recipient 与 target 的两阶段 staged state、external commit 摘要和幂等恢复包进入 membership ledger 的现有 MasterKey AEAD 载荷。
- session 以 transition id 为稳定键，并绑定 conflict、target branch 与 recipient；ledger 提交前拒绝键错配、空载荷、超限载荷和恢复包绑定错配。
- session 自身负责 recipient completion 与 target commit 的单调、幂等推进，调用方不能直接构造或改写内部状态。
- Space rebuild/reset 原子清除未完成恢复事务；旧 ledger 反序列化时以空 session map 兼容。

## 2026-08-31 · Narrow Iroh recovery client channel

- Application 新增单 peer 两阶段 channel port，明确输入指定 peer 与不可变 conflict/branch/recipient 绑定。
- Infra `IrohMembershipBranchRecoveryChannel` 只负责解析已保存 Iroh 地址、认证连接、有界请求响应和超时，不选择 peer、不运行 MLS、不接触 ledger。
- GroupInfo 与 recovery package 响应方向严格校验；拒绝、不可用和畸形响应保留 source 并稳定分类，Debug 不输出底层详情。
- 保留旧的一步 fetch adapter 作为尚未切换的 coordinator 接口；下一切片完成 Application 编排后删除该浅接口，避免长期双实现。

## 2026-08-31 · Restart-safe recovery client coordinator

- Application coordinator 现在确定性选择 evidence peer，依次驱动 GroupInfo、recipient MLS preparation、external commit 和 recovery package 验证。
- external commit 发出前，recipient staged MLS state 与 commit 已通过 membership ledger CAS 加密保存；重启直接复用该状态，不再请求 GroupInfo 或重新生成 commit。
- recovery package 验证后先进入加密 session，再由最终 CAS 消费 nonce、创建 Prepared generation transition 并推进 conflict。
- 删除旧的一步式 fetch port；Engine 组装真实 Iroh channel，recipient MLS preparation 在真实 adapter 接入前显式 Deferred，不做 LAN 回退。
- nonce 已被其他 conflict 消费时不覆盖 nonce、不创建 transition；此前已完成的必要 staged checkpoints 保留供诊断/安全恢复，因此契约不再错误宣称零次 ledger commit。

## 2026-08-31 · Real recipient MLS recovery adapter

- `DefaultSpaceAccessAdapter` 从当前 generation 的安全仓库加载 Ready MLS state，并调用既有 OpenMLS external recovery primitive。
- staged payload 同时保存 recipient MLS snapshot、共享 exporter wrapping key 和 epoch；该 payload 只返回 Application，并在 external commit 发送前进入 MasterKey AEAD ledger。
- Engine 的 recipient deferred adapter 已删除，组装根复用同一个 space access adapter 的窄 recovery port。
- 复核 target 侧发现直接 apply commit 后再构造/保存响应存在崩溃窗口，因此没有接入不安全实现；下一切片必须使用现有 TargetPrepared/TargetCommitted session 完成 prepare/commit 分离。

## 2026-08-31 · Shared recovery transaction key

- transition id 推导从 Application resolve use case 移入 Core `MembershipBranchTransitionV1::derive_id`。
- recipient 与 target 现在可仅凭 conflict/target branch 得到同一稳定事务键，无需扩展 wire 携带另一份可错配标识。
- Core 测试覆盖重复推导稳定、非零和不同目标分支隔离。

## 2026-08-31 · TDD target recovery transaction

- 第一轮红测确认旧 issuer 会返回 package 但不保存 target session、也不提交安全材料；绿色实现增加 prepare payload、TargetPrepared checkpoint、material commit 和 TargetCommitted checkpoint。
- 第二轮红测确认重复请求会重新 prepare 并因 session 冲突被拒绝；绿色实现改为在材料计算前读取 target session，并按 external commit digest 返回缓存 package。
- 故障注入测试确认 material commit 首次中断后，重试从 TargetPrepared 续跑，prepare/签发只发生一次，commit 可安全重试。
- Application port 已明确 prepare 返回 staged target material，commit 接受该 opaque payload；真实 Infra 实现属于下一切片。

## 2026-08-31 · Real target recovery material adapter

- 红测先固定 target prepare 无持久副作用、commit 后 epoch 前进、相同 staged material 重试幂等，以及无效 payload 保留稳定分类与 source。
- `DefaultSpaceAccessAdapter` 在 MLS 快照上应用 recipient external commit，生成下一 epoch 内容密钥并使用双方共享 wrapping key 密封内容密钥目录和恢复确认。
- staged `SpaceKeyMaterial` 只进入 Application 已有 MasterKey AEAD recovery session；commit 重新验证 space、MLS state、key catalog 与单步 epoch，再持久化和安装。
- Engine 已直接注入真实 target material port，并删除 `DeferredMembershipBranchRecoveryMaterial`。

## 2026-08-31 · Phase 4 generation transition completion

- Application coordinator 在已有 transition 时不再提前返回 Completed，而是每轮调用一次粗粒度 execution port，并只接受 Core 状态机的直接后继。
- recipient 使用已加密 checkpoint 中自己的 MLS snapshot 和 wrapping key，解封并验证目标恢复确认与内容密钥目录；目标 MLS 私有 snapshot 不跨设备传输。
- durable transition 依次完成来源 SQLite 备份、目标 SQLite/blob staging、安全材料与目标成员投影写入、active manifest 提升、数据库/blob/session 重绑和来源 generation 清理。
- manifest 提升前活动数据库不变；提升后的 ledger 从目标数据库继续 CAS，最终把 conflict 标记 Completed 并删除 recovery session。
- 新增真实 MLS + SQLite generation 全流程测试，以及 Application 六次重启式阶段续跑测试；错误转换验证稳定分类、source chain 与脱敏 Debug。
- Phase 4 完成，下一阶段进入 Engine 与三端 contract。

## 2026-08-31 · Phase 5 Engine and bindings contract

- 已确认测试接缝：Engine 公开 `Operation -> OperationResult/EngineError`；UniFFI 与 HarmonyOS 只做同版本薄映射。
- Application 已有单次 resolve facade，但尚无完整 conflict query；Engine 和绑定均未暴露两项能力。
- Application 查询现在从加密 membership ledger 返回 revision、候选分支、逐分支选择资格、选择状态和 transition phase；依赖错误保留 source。
- Engine 新增 `QueryMembershipConflicts` 与 `ResolveMembershipConflict`，稳定结果明确使用 `local_resolution_completed`，不宣称全局收敛。
- iOS/Android 共享 UniFFI mapping，HarmonyOS 使用同版本 N-API mapping；两者均直接序列化 Engine 稳定结构，不编排恢复步骤。
- iOS/Android/HarmonyOS 共用的移动 probe host 同步支持查询和单次选择，能通过真实宿主入口验收相同结果。
- Phase 5 完成，下一阶段进入 Desktop 确定性拓扑与 chaos 验证。

## Phase 5 Verification

- `cargo test -p uc-application query_returns_complete_branch_choices_without_claiming_global_resolution --locked`：1 passed。
- `cargo test -p uc-application query_error_preserves_stable_classification_and_source --locked`：1 passed。
- `cargo test -p uc-engine membership_conflict --locked`：2 passed。
- `cargo test -p uc-engine-uniffi membership_conflict_json_preserves_local_completion_semantics --locked`：1 passed。
- `cargo test -p uc-ohos-napi --lib --locked`：10 passed。
- `cargo check -p uc-engine-uniffi -p uc-ohos-napi --all-targets --locked`：通过（仅既有 warning）。
- workspace all-target check、fmt、architecture preflight 与 `git diff --check`：通过（仅既有 warning）。

## 2026-08-31 · Phase 5 unified device-group choice replacement

- 先以 Engine contract 红测删除四个旧 Operation，确认 dispatch、绑定、probe 与 E2E 都依赖旧名字。
- 新公开入口收敛为 `QueryDeviceGroupChoices` / `ChooseDeviceGroup`；Application facade 统一读取两类内部状态、校验一致 revision，并负责选择路由。
- 查询保留完整 device-trust snapshot；候选分支成员未知时显式返回 `members_complete = false`。
- UniFFI、HarmonyOS、移动 probe 与 Engine E2E 已替换为统一入口；旧 Engine operation mapping 文件已删除。

## 2026-08-31 · Phase 6 topology driver discovery

- 已核对规格 F0-F13、声明式动作集合、workspace 二进制和现有多节点测试。
- 确认仓库内没有可直接扩展的 Desktop CLI/daemon；Phase 6 将以 `uc-engine` 公开 contract 的多实例验收驱动器作为本仓稳定接缝。

## 2026-08-31 · Phase 6 declarative topology tracer bullet

- 红测先引用不存在的 `MembershipTopology` / `TopologyAction`，编译失败固定了声明式驱动器接缝。
- 绿色实现支持 `Start`、`Create`、`Join` 和 `AssertSnapshot`，所有推进和断言只调用公开 `Engine::execute(Operation)`。
- 两节点脚本真实完成 Space 创建与准入，并从统一 `QueryDeviceGroupChoices` 快照观察两个 Active member、零待定选择。

## 2026-08-31 · Phase 6 dev-tools membership diagnostics

- 先以 Engine contract 红测固定仅 `dev-tools` 可见的 `QueryMembershipDiagnostics` operation 和稳定名称。
- Application 单一查询返回 branch/head、group epoch、有效成员数、待处理 conflict/effect 数及 transition 阶段；ledger/security 失败保持稳定分类和 source chain。
- Engine 仅执行脱敏字符串与计数映射，不记录诊断标识，也不把入口接入移动绑定。
- 声明式双节点拓扑测试通过公开 operation 断言 64 字符 branch/head、非零 epoch、两个有效成员及零待处理恢复状态。
- 定向 Application 错误测试、Engine contract 测试和真实双节点拓扑测试均通过。

## 2026-08-31 · Phase 6 authenticated Partition/Heal

- 先以 Engine dev contract 红测固定本机 EndpointId 查询和认证 peer 阻断集合替换，缺少 variant 时按预期编译失败。
- Infra 在唯一共享 Iroh endpoint 安装可选 gate；出站连接在发包前拒绝，入站与出站握手后再次拒绝，建立分区时同时关闭匹配的存量连接。
- Engine `dev-tools` 持有跨 session 重建稳定的 gate，拓扑驱动器以节点 EndpointId 双向设置 `Partition`，以空集合执行 `Heal`；默认/生产组装传 `None`。
- 真实双节点测试确认已完成准入的连接在 Partition 后正文零 accepted 且接收端无内容，Heal 后同一 Engine 恢复发送并精确收到正文。

## 2026-08-31 · F0 red test

- 新增五节点公开 Engine 脚本：A-B-C 先达到相同 branch/head，随后 `[A,C,D]` 与 `[B,E]` 分区，并由 A/B 分别准入 D/E。
- 红测要求两侧形成不同 branch/head、各自四名有效成员、分支内正文成功、跨分支正文失败；Heal 后 A/B 都只出现一个统一设备组选择且不自动选主。

## 2026-08-31 · F0 sibling conflict discovery

- 第一轮绿色尝试证明摘要已稳定判定 `diverged`，但 Iroh wire 白名单遗漏新增证据消息，双方都无法落成冲突。
- 修复 transport 后第二轮只由先收到完整证据的一侧记录冲突，暴露先隔离的一端会阻断另一端依赖后续反向调度的非确定性。
- 最终协议在同一次摘要往返中双向交换有界完整签名历史；双方各自验证 sibling 关系和发送者，并各自在一次 ledger CAS 中同时写入唯一冲突和 `Diverged` peer 关系，远端历史从不应用到当前分支。
- 五节点 F0 已通过：两个分支各四名有效成员且 MLS epoch 前进，分支内精确正文成功，分区期间和 Heal 后跨分支正文均失败，A/B 各公开一个统一设备组选择且没有自动 winner。

## 2026-08-31 · F1 remove/add sibling

- 声明式驱动新增只调用 Engine `RemoveMember` 的 `Remove` 动作；首轮真实红测发现成员历史已移除 D，但本地 MLS epoch 未前进。
- 本地移除现在把已签名事件和保留接收者封装进新 effect payload，由可重启 effect executor 调用已有可靠 MLS revocation；没有改变既有 ledger struct 的 postcard 布局。
- 第二个红点是可靠撤销删除当前成员事实后，QueryDeviceTrust 把 Removed 设备缺少 observation 误判为整体 unavailable；现仅为非 Active 设备合成 Offline，Active 缺失仍失败。
- 第三个红点是撤销更新只存在于 `RevocationStage`，后台维护只读取通用 Space outbox；统一待投递查询现聚合两类持久欠账，确认时回写原撤销事务，不复制确认状态。
- F1 最终五节点测试已通过：保留成员 epoch 精确收敛，两分支成员视图分别保持移除/新增语义，分支内正文成功，Removed 目标与 Heal 后跨分支正文关闭式失败，且没有自动 winner。

## 2026-08-31 · F2 concurrent removals

- 已新增声明式 `ResolveConflict` 动作与五节点双移除选择红测；首次运行发现成员 branch 收敛早于 MLS epoch，B 尚不能安全发起移除，测试基线现同时等待五端安全 epoch 一致。
- 基线补齐后冲突形成与 Engine 选择请求均成功，但 chooser 在 60 秒内未切换到目标 branch/head；真实红点已收窄到选择后的恢复包或 generation transition 推进链路。
- 脱敏阶段日志定位到 Iroh 服务端完成 `finish()` 后立即结束 handler，客户端在读取响应长度前收到 stream error；服务端现等待 `stopped()` 确认对端完整接收。
- transition 原本每个固定维护周期只推进一个阶段；Application 完整流程负责人现于单轮内连续推进并逐阶段持久化，仅在真实依赖不可用时 Deferred。
- F2 五节点测试已转绿：C 明确选择 B 的移除分支后，branch/head、四成员视图和 external commit 后的最终 MLS epoch 精确一致。

## 2026-08-31 · F3 opposite removal decisions red test

- Desktop seam 新增 `Decide` 与 `Restart` 声明式动作：前者仅调用统一 Engine 设备组选择，后者复用相同目录和安全存储重启 Engine。
- 红测要求 A 移除 C 后 B Apply、C Keep，两个决定跨重启保持、分支内通信成功、跨分支 exact text 关闭式失败。
- F3 已揭示并修复 Removed + Consistent 设备投影误报 `RecoveryRequired`：Removed 设备现在明确映射为不可同步，不再要求 active scope pause 条目；Application 回归已转绿。
- maintenance 顺序红测已证明 restricted removal event 原先晚于 removal effect；现改为完整轮次先尝试 restricted delivery，Deferred 仍继续离线移除，顺序回归已转绿。
- Desktop F3 tracing 已确认 restricted event 完整送达并收到 `RestrictedApplied`；两个决策端随后都错误投影为两成员且无 pending choice。根因是 restricted handler 绕过 Core 普通 merge 的本机待决定 head 规则。临时 `[DEBUG-F3-RD]` 探针已清理；下一步以 Core 单一接收入口红测锁定并修复。

## 2026-08-31 · F3 opposite removal decisions complete

- Core 红测覆盖 A 移除 C 后 B/C 各自保存同一远端事件、保持父 head 与三成员投影，并产生待决定项。
- 新增唯一 local-member 远端事件接收入口；完整 history merge、分页 suffix 与 restricted handler 均复用，旧 merge 手工回退逻辑已删除。
- suffix 使用独立 sender projection 校验传输 target position，本机 projection 可合法停在父 head。
- Application restricted 回归确认 `RestrictedApplied`、事件落盘、零提前 effect；maintenance 与 Removed 投影套件通过。
- Desktop F3 通过：B Accept、C Reject，分支内正文成功，跨分支正文关闭式失败，B/C 重启后各自 branch/head 保持。

## 2026-08-31 · F4 single bridge red test

- 已确认继续使用 Engine 公开 operation 与既有 dev-tools endpoint gate 作为测试 seam。
- 场景将从六节点共同历史构造两个各三成员的合法 sibling 分支，再仅开放一条跨区 bridge，锁定“不得联合为六成员假历史”。

## 2026-08-31 · F4 single bridge complete

- 声明式拓扑新增 `Bridge`：只解除 A-D 的相互阻断，A 仍阻断 E/F、D 仍阻断 B/C，其他节点保持原分区。
- 六节点准入改为每次扩容后等待 branch/member 与 MLS epoch 收敛，避免下一次邀请抢跑安全传播。
- 两条分支各保留 A/D 作为共同 Active bridge 端点，并分别形成不同的三成员集合；Removed↔Removed 不能通过普通成员认证，不再被误作 bridge。
- F4 Desktop E2E 通过：A/D 各记录一个 sibling conflict，成员数保持 3、branch id 不变、跨桥正文零 accepted，未形成联合历史。

## 2026-08-31 · F5 ring propagation red test

- 四个共同成员在分区内分别准入 E/F 形成 sibling，再将 A-B-C-D 配置为只有相邻边可达的环，E/F 完全隔离。
- 首轮红测发现分叉前 AddDevice effects 仍可能处于恢复队列，不能把“无重复 effects”误写为“全局 effects 必须为零”；改为环接通前后及重复刷新前后比较稳定计数。
- 第二轮红测确认同一 conflict evidence 会被无条件重复提交，四端 revision 随往返增长；Application 单测固定同一来源相同 evidence 只能提交一次，ledger 已增加幂等短路。
- 全局 revision 同时包含隔离 E/F 的正常 peer 退避账务，不能作为 conflict 防环判据；最终 E2E 以唯一公开 conflict、同 evidence 单次 commit 和 effects 不增加组成强断言。
- F5 Desktop E2E 通过，耗时 192.21 秒；A/B/C/D 各只公开一个 conflict，两个相反传播方向没有产生重复 effects。

## 2026-08-31 · F6 deep-chain red test

- 继续使用稳定 Engine operation 与认证 endpoint gate：A→B→C→D→E→F 逐级准入后停止中间 Sponsor B/D。
- A/C/E 接受移除 B 的共同分支，F 建立移除 D 的 sibling；只开放 A-C-E-F 相邻链路，要求 F 从 E 恢复到共同 branch/head/epoch，并验证三段相邻正文通信。
- 红测先引用尚不存在的 `Stop` 与 `Chain` 拓扑动作，固定离线生命周期与非全连接传播 seam。
- 首轮运行定位到 A/C/E 分别接受同一移除后无法形成单一目标 branch；该构造混入了本机决定与 MLS 时序，不适合作为 F6 单变量验收。
- 场景改为 B/D 停机后 A/F 分别准入 G/H 形成 sibling；A/C/E/G 与 F/H 各自通过普通新增收敛，再开放 A-C-E-F 链。

## 2026-08-31 · F6 deep-chain complete

- 共同基线时由 A/F 预签发邀请，分区后 G/H 分别形成两条七成员 sibling；待安全 epoch 收敛后再真实停止 B/D。
- `Stop` 会关闭 Engine 并从拓扑移除节点；`Chain` 只开放 A-C-E-F 相邻连接，端点身份缓存保证停机后仍能建立精确分区。
- Target 恢复材料在 external commit 后为其他 Active 目标成员生成持久 group-update outbox，不再只更新 target 与 chooser。
- TargetCommitted 作为唯一恢复事实：完成 target 侧 conflict，将 recipient 恢复为 Consistent，并阻止旧 sibling evidence 重新把它暂停。
- F6 Desktop E2E 通过，耗时 402.19 秒；A/C/E/F 的 branch/head/epoch 一致，A→C、C→E、E→F 正文均精确接收，B/D 全程保持停机。

## 2026-08-31 · F7 three-branch fairness red test

- 十节点先形成 A–G 七成员共同历史，再由 A/B/C 使用预签发邀请分别准入 H/I/J，形成三条八成员 sibling。
- D 暂停在共同祖先；打开 A↔B 冲突边的同时重连 D 与 A/G/H，要求 A/B 记录冲突而 D 仍能补齐 H 事件和 MLS epoch。
- 最终遍历十节点完整有向正文矩阵：同分支精确接收，跨分支零 accepted 且无正文泄漏。

## 2026-08-31 · F7 three-branch fairness complete

- 公开 Engine 拓扑驱动新增任意分组分区和单跨组 bridge；每个节点只保留组内连接，bridge 端点额外保留彼此，所有业务 ALPN 共用同一认证 gate。
- 共同基线改用 A→B→C→D→E→F→G 链式 Sponsor 来源；十 Engine 负载下只将 admission completion 的测试观察窗口放宽到 120 秒，公平性业务窗口仍为 60 秒。
- A/B/C 并发准入 H/I/J 形成三条八成员 sibling；D 暂留七成员祖先，随后在 A↔B 冲突边存在时补齐 A 分支的 branch/head 和 MLS epoch。
- F7 Desktop E2E 通过，耗时 462.19 秒；合法 peer D 未被冲突 peer B 饿死，十节点 90 条有向正文矩阵中同分支精确接收、跨分支全部关闭式拒绝。

## 2026-09-01 · two-device pairing performance acceptance

- 在稳定 Engine operation seam 新增显式性能门禁：邀请签发不计时，从 Joiner 提交完整邀请开始，到 Sponsor/Joiner 均公开两名有效成员结束，预算为 1 秒。
- 红测连续得到 7.56 秒与 7.53 秒，确认当前实现不满足目标；用例标记为显式 `--ignored` 性能门禁，避免共享 CI 机器把性能结果混入功能正确性。
- 诊断运行曾显示首次信道建立约 157ms，后续恢复信道约 3–4ms；调用点探针随后删除，正式观测改由 Engine 组装的恢复状态 port decorator 承担。
- 2 秒和 10 秒维护周期实验分别在 332.05 秒与 372.62 秒令 F7 同分支正文矩阵失败；实验代码已撤回。维护 runtime 本来就立即执行 Startup round，并由 StateChanged 主动唤醒，缩短持续周期只增加 Iroh/SQLite 竞争。
