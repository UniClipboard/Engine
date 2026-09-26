# 异常成员移除后的安全收尾恢复

## Goal

在不放宽邀请安全门槛、不删除资料、不按在线状态自动处理成员的前提下，使已经同步到本机的重复成员历史经用户正常移除后能够完成安全收尾，并恢复新邀请；若确实无法完成，公开状态必须明确反映待处理或失败。

## Ownership

- 完整负责人：Application 的成员移除与持久效果恢复流程。
- 调用方唯一动作：按明确目标请求正常移除，并查询设备状态或生成邀请。
- 成功：成员历史、成员展示和安全资料一致，重复执行与重启保持一致，邀请恢复。
- 失败：保留原始原因和可恢复状态，展示不得宣称已经完全移除；邀请继续安全阻止。
- 重试与重启：由持久成员效果恢复负责人继续，不由 Desktop 或用户重复业务操作驱动。

## Test Seam

使用现有 `uc-engine` 完整成员 E2E 公共动作，构造异常历史同步、正常移除、维护/重启和生成邀请；Core/Application/Infra 只补充首个失败点所需的最小规则或真实适配测试。

## Phases

- [completed] 读取约束、现有改动和相邻设计，建立红色完整回归场景。
- [completed] 最小化场景并定位第一个失败点，形成 3–5 个可证伪假设。
- [completed] 实施最小安全修复，使完整场景转绿。
- [completed] 扩展重启、重复处理、多个旧身份、普通移除与普通邀请覆盖。
- [completed] 更新架构圣经，清理临时探针并运行相关完整检查。
- [completed] 更新交付报告。
- [in_progress] 真实设备验收保持未执行，等待用户明确确认目标，不标记原任务完成。

## Required Verification

- 红测命令可稳定重现完整用户症状，修复后同一命令转绿。
- 受影响 crate 的聚焦测试与完整相关测试通过。
- `cargo metadata --locked --format-version 1`
- `cargo check --workspace --all-targets --locked`
- `cargo fmt --all -- --check`
- `node scripts/architecture/check-rust-style.mjs`
- `node scripts/architecture/check-engine-repository.mjs`
- `git diff --check`
- 搜索并确认没有本轮临时诊断标记。

## Constraints

- 保留现有所有修改，不改真实成员、手机资料或 Linux 空间。
- 不直接去重、不清空资料、不按离线状态自动删除。
- 不提交、不推送、不创建 PR、不发布。
- Cargo 串行使用仓库共享 target 与现有编译缓存。

## Errors Encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
| 根目录已有计划属于更早的 Spec 029，当前 `.active_plan` 也指向无关任务 | 1 | 新建独立计划目录并切换当前工作树的活动计划，不覆盖历史计划内容。 |
| 完整回归修复前正常移除后仍残留 2 个旧身份 | 1 | 保留为红测证据；正式移除改为一次撤销目标设备的全部旧身份，同一场景已转绿。 |
