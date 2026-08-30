# Spec 030 Findings

## Initial state

- 当前分支领先远端 18 个提交，并有 7 个代码文件及架构圣经未提交修改；主要属于 Spec 029/Admission 重启诊断，Phase 4/5 可能重叠，必须保留。
- `VersionedMembershipHistory` 已是签名单父历史、关系与成员事实的事实来源；新 policy 应建立在验证后的 history 上，不复制验证逻辑。
- `MembershipLedger` 已是加密原子边界；conflict record 应成为 ledger 模型的一部分而非独立明文 repository。
- 规格的 Open Questions 不阻止核心实现：本地选择语义、短期恢复包时长和 CI 分层可以采用保守内部默认，不改变公开 contract。

## Invariants

- conflict id 与传输 peer、到达顺序无关。
- branch id 只绑定 lineage 与完整目标 head。
- Removed 不能恢复旧成员实例；Absent 不是可选目标。
- 用户选择 intent 一经持久化，后台不得改写。

## Phase 1

- `current_position().history_digest` 包含完整持久历史与决定，branch id 必须同时绑定它和 head；只使用 event head 会把同一移除的 Accept/Reject 分支误判为相同。
- 共同祖先使用事件链证明，不使用 branch-specific history digest；conflict id 对排序后的两个 branch id 做摘要，因此观察顺序不影响结果。
- 本机实例在目标 `active_members` 中才可恢复；保留历史凭据但已不在 `effective_members` 中表示 Removed，需要重新配对；尚未激活或完全缺席均不可选择。

## Phase 2

- SQLite `membership_ledger_state.encrypted_payload` 已对整个 ledger 使用 profile MasterKey AEAD；conflict record 追加到 `LoadedMembershipLedger` 即进入既有密文与 transaction/CAS 边界。
- conflict record 的 `Debug` 只输出选择分类、阶段、revision 和计数，conflict/branch/peer/transition 标识全部脱敏。
- 多个 peer 的同一分支证据使用集合保存，不把 transport 来源纳入 conflict id。
