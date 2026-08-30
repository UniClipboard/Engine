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
