# Spec 030 成员分叉选择与复杂拓扑验证

## Goal

按 `docs/specs/030-membership-conflict-resolution-and-chaos-validation.md` 分阶段实现成员冲突识别、加密持久选择、可恢复 generation 切换、Engine/三端 contract，以及确定性复杂拓扑验证。

## Test seams

- Core：`MembershipConflictPolicy` 的纯领域输入输出。
- Application：`MembershipLedger` 原子状态与唯一 `resolve_membership_conflict` 动作。
- Infrastructure：恢复包验证和 generation transition 端口的真实持久化边界。
- Engine/Bindings：公开 operation/result/error 的稳定映射。
- Desktop E2E：只从 CLI/daemon 观察分支、安全状态和正文通信矩阵。

## Phases

- [x] Phase 1：Core 稳定 conflict/branch id、选择资格与转换矩阵。
- [x] Phase 2：Ledger 加密 conflict record 与 Diverged 同 commit。
- [x] Phase 3：Application 唯一 resolve use case、幂等选择与恢复调度。
- [x] Phase 4：同 lineage branch generation transition 与恢复包 adapter。
- [x] Phase 5：Engine、iOS、Android、HarmonyOS 统一设备组选择 contract，并清理旧四入口。
- [ ] Phase 6：Desktop F0-F13、20 个固定 chaos seed 与 Spec 029 回归。
- [ ] Phase 7：架构文档、代码审查、全量门禁与原子提交。

## Current Slice

Phase 6：Desktop F0-F13、20 个固定 chaos seed 与 Spec 029 回归（Phase 5 合并入口后的重新验证）。

## Next Step

用已验证的 Partition/Heal 动作编写 F0 红测：A-B-C 共同基线分区后分别准入 D/E，断言 sibling branch 隔离、分支内正文成功、跨分支正文失败，并在 Heal 后只暴露单一设备组选择。

## Constraints

- 持久 conflict、intent、transition 和恢复材料均须在 MasterKey AEAD 边界内。
- 不自动选赢家，不合并 sibling，不因 P2P 失败回退 LAN。
- 所有依赖失败保留 source chain，日志不含敏感标识或负载。
- 保留用户当前未提交修改；重叠前先核对。
- 每次仓库修改都更新架构圣经维护记录。

## Errors Encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
| 根目录计划仍属于 Spec 029 | 1 | 为 Spec 030 建立独立 `.planning` 目录，不覆盖原计划。 |
| Ledger 原子测试把 `compare_and_commit` 返回值误当成 `VerifiedMembershipLedger` | 1 | 返回值实际是 `LoadedMembershipLedger`，断言直接读取字段。 |
| resolve use case 无法从 membership 聚合导入 conflict status | 1 | 将 ledger conflict record/status 加入 membership 聚合公开导出。 |
| 收窄 coordinator outcome re-export 后模块内测试找不到类型 | 1 | 仅在 `cfg(test)` 下重新导出 outcome，避免生产 unused import。 |
| workspace 全量测试的 3 个既有 clipboard/search host-adapter 用例失败 | 1 | 成员套件与 all-target check 均通过；单独重跑首项仍返回 QueryHistory unavailable 1243，记录为本切片外既有失败，不误报全量通过。 |
| `VerifiableGroupInfo` 无法从 OpenMLS prelude 解析 | 1 | 使用公开的 `openmls::messages::group_info::VerifiableGroupInfo` 显式导入。 |
| 新增两阶段端口后测试替身缺少 GroupInfo 方法和 external commit | 1 | 补齐显式 begin/complete 输入与所有 passive/test adapter，实现阶段边界。 |
| generation executor 首次真实测试缺少 legacy bootstrap repository | 1 | 复用同一 SQLite security store 同时实现 revocation 与 legacy bootstrap repository，建立真实 sponsor MLS。 |
| Engine contract 红测启用 `dev-tools` 时同时编译到既有退役 membership removal / SpaceJoined 测试 | 1 | 新 contract 后续使用默认 feature 定向测试；既有 dev-tools 漂移作为独立问题记录，不误归因于 Phase 5。 |
| workspace check 发现移动 probe host 对新 `OperationResult` 未穷举 | 1 | probe host 同步增加查询/选择命令和完整结果映射，使真实移动验收入口覆盖 Phase 5 contract。 |
