# Findings

## Scope

- 规范只允许修改 `crates/uc-application/` 和相关文档。
- 当前分支在 `origin/refactor/reset-space` 之上已有 4 个本地提交，工作树同时包含大量跨层未提交改动。
- 必须以当前文件内容为准，保护不属于本任务的 Core、Infra、Engine、绑定和宿主改动。

## Initial Gap

- 强制删除项仍大量存在：`MembershipStateCoordinator`、`SpaceAdmission`、`MembershipConvergence`、`SpaceMembershipState`、`WorkspaceSnapshot`、三个旧 runtime 和多个旧 facade 入口仍被 application 引用。
- 当前目录结构是一次渐进搬迁的中间态，不满足 Spec 027 的一次性目标。
- 已有规格计划只负责产出 ADR/Spec，并明确没有实施生产代码；需要新的实施记录。
- `SpaceApplicationRuntime` 仍同时持有 gossip、maintenance 和 connectivity 三个 runtime，生命周期对象还要了解三套暂停、恢复和关闭顺序。
- `SpaceModules` 仍公开内部 owner、端点和两个成员 runtime 启动方法，外层能够绕过完整用例。
- `deps.rs` 仍公开旧 `SpaceMembershipStateRepositoryPort`，说明旧综合状态仍是 application 组装合同的一部分。

## Baseline Verification

- `cargo test -p uc-application --lib space --locked -- --list` 尚未列出测试，先在编译阶段失败。
- 首批失败包括旧测试模块层级失效、旧 facade 访问 coordinator 私有字段、旧 admission owner 方法缺失、测试仍调用已改名安全接口，以及旧 fixture 与新依赖结构不一致。
- 这些错误不是目标测试的预期 RED，而是中间迁移损坏；目标测试必须独立建立并因缺少 Spec 027 接口或行为而失败。
- `cargo check -p uc-application --lib --locked` 当前通过，但产生 41 条警告；说明生产库可作为逐步切换的编译基线，测试目标仍损坏。

## Reusable Core Rules

- `VersionedMembershipHistory` 已提供持久编码/验证解码、有界分页导入导出、当前成员集合、设备到成员实例映射、移除事件创建、决定创建与签名决定应用。
- application ledger 不应复制签名、父链、摘要或成员集合规则，只负责一次加载验证、条件提交、运行资料和最终范围派生。
- `SpaceJoinRecord` 与 `AdmissionProfileMetadata` 已可序列化，当前 profile 元数据包含唯一 `device_trust_revision`，可作为目标联合提交负载的一部分。

## Consumer Bypass

- 剪贴板发送和接收仍同时依赖 `MemberRepositoryPort` 与 `ContentExchangeGatePort`。
- 活动剪贴板和目标选择还额外依赖 `CurrentWorkspacePeerScopePort`，同一普通动作会组合三类成员判断。
- 目标切换必须让这些路径只读取一次 application `CurrentSpaceMemberScopePort`，成员表只补充偏好或展示资料。

## First Target Slice

- 新目标测试先因缺少 `MembershipLedger`、加载 port、错误类型和最终范围类型而失败，确认 RED 原因与需求一致。
- 为执行目标测试，停止编译旧 coordinator 聚合测试树及其依赖的旧决定/诊断/握手拼装测试；这些文件均在后续删除或目标测试替换范围内。
- 新 ledger 负载当前集中表达历史、加入记录、关系、入站 transfer、待处理 effect 和 profile revision；尚未接入写用例。
- `space::membership_ledger::tests::no_current_space_has_no_authorized_scope` 已确认列出 1 个并通过 1 个，证明无当前 Space 时普通范围失败关闭。
- V2 单成员根只有在本机设备映射一致且本机加入已激活时才产生本机活动资格。
- 普通对端先由 V2 当前成员集合产生，再要求对应关系为 `Consistent`；关系为 `PendingRemovalDecision` 时只暂停该设备并返回稳定原因。
- 聚焦测试二进制每次启动在当前 macOS 环境的动态加载阶段额外等待约 60 秒，但随后测试本身在 0.01 秒内完成；不能把启动等待误判为测试逻辑死锁。

## Ledger Foundation

- `CurrentSpaceMemberScopePort::snapshot()` 是普通消费者唯一读取接口；实现保留在私有 ledger 内，结果和错误通过 `deps` 暴露。
- ledger 写入口在提交前验证当前/替换历史、使用 `checked_add` 推进唯一 revision、携带期望历史摘要，并只调用一次条件提交 port。
- 内存适配在同一锁内同时比较 revision 和历史摘要，冲突不修改状态。
- ledger 聚焦模块当前 10 个测试全部通过，覆盖空 Space、本机激活、一致对端、关系限制、effect 门禁、本机未激活、修订号溢出、快照 port 和摘要冲突。

## Query Device Trust

- 新查询用例从同一份已验证 ledger 快照派生范围，不重复读取持久层。
- 无当前 Space 返回明确空状态且不读取观察资料；活动状态对完整设备集合只读取一次观察资料并严格检查重复/缺失。
- 当前待决定变化只从 V2 远端历史合并产生，公开变化编号、提议设备、目标设备和本机影响，不公开签名或内部阶段。
- 查询模块当前 3 个测试全部通过；当前加入和待接纳成员留待准入改写接入同一 ledger。

## Remove Space Member

- 新移除用例持有自己的执行锁；每次尝试完整执行读取、成员验证、事件创建、签名和 CAS 提交。
- 一次提交同时保存 V2 移除、Prepared 效果、关系变更、受限事件计划和唯一 revision；成功后才唤醒维护并查询最新状态。
- 第一次 CAS 冲突会从新快照重新签名并重试一次；第二次冲突返回 `StateChanged`，不无限重试。
- 提交后查询失败返回 `CommittedButPending(change_id)`，不会让调用方误以为移除未发生。
- 已移除设备仍出现在设备信任结果中，但标为 Removed/Paused；普通 scope 只由 V2 当前成员产生，受限计划不能恢复普通权限。
- remove_space_member 当前 2 个聚焦测试通过：完整原子提交与一次冲突恢复。

## Decide Device Trust Change

- 接受会移除本机的变化时，未明确确认直接返回 `LocalConfirmationRequired`，零提交、零维护唤醒。
- 接受决定一次提交签名决定、应用位置、Prepared RemoveDevice 效果、关系和精确受限决定计划；查询立即显示本机 Removed。
- 拒绝决定不创建 RemoveDevice 效果，本机成员分支保持活动，只把提议方关系标为 Diverged。
- 重复决定先读取历史中的原决定，不再提交；会再次唤醒维护以继续未完成效果，并返回 `AlreadyCompleted`。
- 一次 CAS 冲突会从新快照重试；第二次冲突转换为携带最新状态的 `StateChanged` 结果。
- 决定模块当前 5 个聚焦测试全部通过。

## Membership History Receive

- 入站用例只接收已认证成员和一条有界消息；ACK 消息作为入站请求被稳定拒绝。
- 单页在一个 ledger 提交内完成页保存、整体验证、关系更新、正式历史/效果提交和幂等 ACK 回执；重复最终页零写入返回同一 ACK。
- 多页每页先保存再 Continue；重复页不重写，乱序页只请求缺失索引，transfer 替换清除活动进度并持久标记 Invalid。
- Core 4 MiB 限制按页执行；application 另设固定 16 MiB transfer 总上限，允许合法两页历史同时防止无限累积。
- 普通历史扩展通过合并前后成员集合和事件链创建 Prepared Add/Remove 效果，与正式历史同事务提交。
- handle_membership_history_message 当前 4 个聚焦测试全部通过。

## Membership History Send

- 新发送用例直接拥有 ledger、最终 scope、传输、固定 10 秒整轮预算和每对端串行锁；调用方只选择 AllCurrentPeers 或 AuthenticatedPeer。
- AllCurrentPeers 只读一次 scope，排序去重；单设备离线/传输失败记 deferred 并继续，协议拒绝或错误 ACK 记 stable failure。
- 一轮所有对端导出同一份历史快照；最终 ACK 后条件提交确认位置和 Consistent 关系。
- 旧 assembly/runtime 尚未切换，旧步骤型同步器临时只以 `LegacySynchronizeMembershipHistoryUseCase` 名称保留；最终删除，不作为兼容 API。
- synchronize_membership_history 当前 1 个聚焦测试通过。

## Test Isolation

- 编辑会触发外部 `cargo check --workspace --all-targets` 并占用默认 `target/` 锁。
- Spec 027 聚焦测试改用 `CARGO_TARGET_DIR=/tmp/uc-spec027-target`，首次编译后反馈稳定在约 7-13 秒，且不终止或干扰外部检查。

## Maintenance And Runtime

- `MaintainSpaceMembershipUseCase` 固定 Startup/Resume/StateChanged 顺序为 admission -> effects -> restricted delivery -> history sync -> cleanup。
- Deferred/StableFailure 计数后继续；Corrupt 立即停止后续可能扩大权限的步骤；PeerOnline 只运行受限投递和精确同步。
- 新 `SpaceMembershipRuntime` 统一启动、定时、在线、显式唤醒、暂停、恢复和 5 秒有界关闭；暂停确认会等待当前维护轮完成，避免中断本地提交。
- maintain_space_membership 当前 4 个测试全部通过。

## Scope Consumers

- clipboard send/receive gates、active-state fanout、dispatch target selector、delivery view、manual resend、outbound facade 和 roster 已切换到 `CurrentSpaceMemberScopePort`。
- scope 缺失或目标不在 `usable_peer_device_ids` 时发送/接收失败关闭；成员表、可信关系、地址、过滤器和偏好只允许继续缩小目标集合。
- clipboard/roster/transfer 范围内对 `ContentExchangeGatePort` 和 `CurrentWorkspacePeerScopePort` 的搜索已归零。
- 迁移后 45 个既有消费者回归通过；send gate 6、receive gate 1、target selector 9、delivery view 20、resend 13、roster 3。

## Target Assembly

- 新 `space/application.rs` 一次构造 ledger、设备信任查询、移除、决定、历史收发、维护和唯一 runtime。
- `SpaceApplicationDeps` 只公开被动 ports；历史 endpoint 通过已认证 Core endpoint 适配调用一次入站用例。
- `cargo check -p uc-application --all-targets --locked` 在消费者迁移后通过。
- 目标 assembly 尚未接入唯一 `SpaceFacade`；旧 `SpaceModules`、旧 facade、旧 admission owner 和三旧 runtime 仍是强制删除缺口。

## One-shot Cutover

- `SpaceModules`、`MembershipStateCoordinator`、`MembershipConvergence`、旧 membership state/runtime、旧 admission owner 和步骤级 store 已从 application 源树物理删除。
- `SpaceApplication` 现在同时拥有历史网络入口、准入网络入口、完整用户用例和唯一成员 runtime；`SpaceFacade` 只保存稳定业务入口与两类规范网络 endpoint。
- 准入入站 preparation port 只负责验证/准备协议资料；application 用例验证来源绑定并负责唯一原子提交与维护唤醒，未把持久顺序推给 adapter。
- `MemberRosterFacade` 已降为内部实现并由 `SpaceFacade` 持有；顶层 `AppFacade` 不再保存第二个 Space 门面。
- 旧连接 runtime 已删除。统一 runtime 通过被动网络活动 port 协作取消网络等待，同时等待当前维护轮到达保存边界；关闭等待上限为 5 秒。
- 同一接收操作现在携带一次 scope revision 和同次读取的偏好，解密后的类别检查不会再次读取成员范围。
- 架构检查从旧 workspace convergence 目录契约切换为 Spec 027 的目标入口、禁止符号和强制删除路径。
- 历史同步不是普通内容操作：可用成员和关系未确认/待决定/需升级的当前成员可进入成员历史核对；分叉、无效、效果未完成和已移除设备不能取得完整历史。受限决定仍走精确计划。
- 会话活动由 `SpaceFacade` 组合唯一成员 runtime 与搜索/接收活动；暂停任一后续步骤失败会恢复已经暂停的成员活动，避免锁定前半停状态。
- 跨 Space 完成必须同时替换 lineage、本机成员实例、本机活动门禁和新关系基线；只替换历史字节会被下一次验证判为损坏。
- Effect 最终启用是清除重新配对提示的唯一时点；安全启用或提示保存失败都会保留可恢复阶段，不提前宣布完成。

## Evidence Queue

- 建立规格验收条件到文件、测试和搜索证据的映射。
- 确认当前 `uc-application` 编译错误和非零测试清单。
- 盘点现有正式设备信任类型、V2 历史、加入持久接口和消费者依赖。
- 确认哪些工作树变化属于先前渐进迁移，哪些可安全替换。
