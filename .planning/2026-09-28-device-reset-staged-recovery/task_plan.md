# 任务计划：单设备 Space 重建的锁竞争与 Staged 续做修复

负责人：
- `RebuildSpaceUseCase`：流程顺序、结果、重试责任，以及在重建期间排除成员维护。
- `V3DeviceManagementReset`：日志阶段，以及已改写 Staged 目标的丢弃与重新快照。
- `SpaceControlGeneration`：凭据重绑的等待与 Busy 分类。
- 成员维护负责人：提供排除维护的 port。

调用方的唯一动作不变：`recover_space_session` / `unlock_space` / `reset_space`。

## 阶段

1. [x] 读取上游报告，核对最新 main，梳理调用链，完成静态审查（见 findings.md）
2. [x] 0.19.4 真实发布资料快照：用官方 CLI 在 `win` 的专属目录中生成，已入库，并附来源说明与校验
3. [x] 矩阵：0.19.4 快照锚点、两个 D1 单元（正常升级 / 提交中断），宿主失败时交回安全存储
4. [x] 先写失败测试：Infra A–F、Application H（未编译）
5. [ ] **等待构建主机**（排在 t-0089 之后，由协调者安排）
6. [ ] 红：在未修改的产品代码上运行步骤 4 的测试与两个矩阵单元，保存日志
7. [ ] 只加入维护互斥接口（`ExcludeMembershipMaintenancePort`，由 `MaintainSpaceMembershipUseCase` 的
   `execution_lock` 实现），重建流程暂不使用；补写两个测试并确认它们在行为上失败：
   - 持有互斥期间，维护轮次不运行，释放后运行；
   - `RebuildSpaceUseCase` 在 stage…promote 期间持有互斥（替身通过 `try_lock` 检查）。
8. [ ] 实现（绿）：
   - Staged 续做：切回来源 → 删除目标（在控制代租约下；目标不存在视为已删）→ 日志改回 Allocated → 重新快照；
   - `already_committed` 路径先 promote 再 finalize；
   - rebind 使用 busy timeout，SQLITE_BUSY/LOCKED → `SpaceControlGenerationError::Busy`；
   - 带类型的分类链：暂时不可用 → `1103 Unavailable retryable=true`；
   - 重建期间排除成员维护。
9. [ ] 全部测试、workspace check、fmt、架构检查、diff check
10. [ ] Windows 验证：在 VM 与 `win` 上用 Windows 原生构建的连接宿主与矩阵测试二进制，对 0.19.4 快照多轮运行
    `d1-l0194-head`（不注入，观察自然竞争）与中断单元，比较基线与修复版本。
    VM 使用专属目录与端口，不接触 vmclean1 及其他 thread 的资料。
    Desktop `uniclipd` 联调需要跨仓库本地覆盖，先交协调者登记。
11. [ ] 报告
