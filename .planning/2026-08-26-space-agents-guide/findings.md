# Space AGENTS.md 盘点发现

## Current Facts

- `space/mod.rs` 当前包含生命周期、准入、设备信任、成员历史、维护、统一 ledger、会话活动和网络恢复模块。
- `SpaceApplication` 是 application 内部组装点，`SpaceFacade` 是公开 Space 业务入口。
- `MembershipLedger` 是成员历史、加入记录、关系、分页传输、效果、受限投递和修订号的统一事实模块。
- 普通成员消费者只读 `CurrentSpaceMemberScopePort`；网络历史和准入各有一个单消息入口。

## Inventory Queue

- 生命周期 cases：initialize, unlock, lock, recover session, rebuild, reset, upgrade, setup/access queries。
- admission cases：issue/cancel/query invitation, join/cancel join, query/complete transition, inbound admission, recovery。
- membership cases：query trust, remove, decide, sync outbound, handle inbound, maintain runtime effects。
- support：application assembly, ledger, current identity/signing, session activity, re-pairing, network recovery。

## Lifecycle Cases

- `InitializeSpaceUseCase`：新 profile 创建加密 Space，保存设备名和本机成员，初始化单成员历史与安全组，最后激活当前 Space。
- `UnlockSpaceUseCase`：取得当前 Space、解锁、执行会话就绪收尾；不创建 Space。
- `LockSpaceSessionUseCase`：先暂停后台活动，再锁定；锁定失败必须恢复活动。
- `RecoverSpaceSessionUseCase`：从安全存储恢复现有会话，执行就绪收尾并恢复活动；无当前 Space 或无可恢复会话返回明确未恢复。
- `RebuildSpaceUseCase`：准备唯一目标、暂存、重绑会话、清理成员资料、建立单成员基线、提升并收尾；重启继续同一目标。
- `ResetSpaceUseCase`：取消内存邀请并调用 rebuild；`QueryCommittedDeviceManagementResetUseCase` 单独查询重置提交状态。
- `UpgradeSpaceUseCase`：比较版本里程碑，必要时调用 rebuild，然后记录当前版本。
- `QuerySpaceAccessStateUseCase` 和 `QuerySpaceSetupStateUseCase` 是两类只读查询：前者面向加密会话，后者面向设置流程/UI。

## Membership Cases

- `QueryDeviceTrustUseCase` 从单次 ledger 快照生成设备信任、当前加入、待决定变化和观察资料。
- `RemoveSpaceMemberUseCase` 签名移除并原子保存历史、关系、受限投递、Prepared 效果和 revision；提交后尝试效果并唤醒维护。
- `DecideDeviceTrustChangeUseCase` 处理接受、拒绝、本机确认、重复和并发变化；一次冲突重试，正式决定不回滚。

## Admission Cases

- 邀请分为签发、指定地址签发、地址查询、取消四个短 case；`PairingInvitationIssuer` 统一做成员准入门禁、观测和内存邀请登记。
- `JoinSpaceUseCase` 只让 preparation port 生成密码学/协议材料，随后先保存加入记录再唤醒后台；port 不得持久化、拨号或发送。
- `CancelSpaceJoinUseCase` 只允许提交边界前取消；原子保存 Rejected(Cancelled) 与取消 outbox，提交边界后返回既有状态。
- `HandleSpaceAdmissionMessageUseCase` 是一条认证准入消息的唯一入口：先验证邀请和 generation，调用无副作用 preparation，再原子保存 history/record/relationship/effect，保存后才消费内存邀请并返回 reply。
- `RecoverSpaceAdmissionsUseCase` 扫描持久 outbox，送达后原子结清；每条消息重载最新 record version，稳定拒绝和邀请消费结果也必须结清。
- `CompletePendingSpaceTransitionUseCase` 循环推进同一持久 transition；Finished 时原子切换目标历史、lineage、本机成员实例和活动门禁。
- `QueryPendingSpaceTransitionUseCase` 只检查是否存在已完成准入但尚未完成 Space transition 的加入记录。

## Membership History Cases

- `HandleMembershipHistoryMessageUseCase`：认证当前成员、校验页/大小/transfer、每页先保存再 ACK；完整后验证历史并原子保存关系、历史、效果和最终 ACK。
- `SynchronizeMembershipHistoryUseCase`：全量使用固定 10 秒总预算，单 peer 使用独立锁；严格分页 ACK，最终保存 Consistent/Diverged/Invalid。已移除、分叉和无效设备不得收到完整历史。

## Maintenance And Support

- `MaintainSpaceMembershipUseCase` 固定顺序：admission recovery -> effect recovery -> restricted delivery -> conditional history sync -> legacy cleanup。Corrupt 停止后续可能扩大权限的步骤。
- `SpaceMembershipRuntime` 只拥有触发和生命周期：Startup、Resume、Periodic、StateChanged、PeerOnline；同一时刻一轮，触发去重排队，暂停先停网络再等当前提交，关闭总预算 5 秒。
- `RecoverMembershipEffectsUseCase` 按 Prepared -> MemberFactsApplied -> SecurityApplied -> Activated 单向推进；AddDevice 激活成功后清除 re-pairing 提示。
- `DeliverRestrictedMembershipUseCase` 只发送 ledger 中精确保存的 event/decision；成功后原子移除对应计划，不开放普通 scope。
- `InitializeSpaceMembershipUseCase` 建立单成员 V2 根；`SpaceMembershipRebuilder` 负责清关系、远端成员、保存本机成员并调用初始化。
- `SpaceSessionActivity` 组合成员 runtime、接收和搜索的 pause/resume；后续暂停失败会恢复已暂停部分。
- `NetworkRecoveryFacade` 独立处理网络 session rebuild、共享进行中请求、退避和网络变化窗口；不读写成员资格。

## Ledger Invariants

- `LoadMembershipLedgerPort::load()` 加载完整解密记录；`CommitMembershipLedgerPort::compare_and_commit()` 必须在一个加密事务中同时比较 revision 和 history digest。
- ledger 包含 lineage、V2 history、本机身份/激活、peer reconciliation、inbound transfer、completed ACK、pending effects、admission records 和 profile metadata。
- `CurrentSpaceMemberScope` 按顺序派生：V2 当前成员 -> 本机激活 -> pending effect -> relationship；地址、在线状态和偏好不授予资格。
- revision 只用 checked_add；冲突不覆盖新状态；Corrupt/RecoveryRequired 不回退旧表。
