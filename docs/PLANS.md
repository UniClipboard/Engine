# 执行计划

执行计划记录尚需推进的具体工作和已经关闭的实施历史。它们不是当前架构事实；计划完成后，
稳定结论必须回写设计文档或 ADR，然后移入 `completed/`。

## 生命周期

```text
提议/实施/阻塞 → exec-plans/active/
                 ↓ 完成或明确被取代
                exec-plans/completed/
```

- [进行中的计划](exec-plans/active/)
- [已完成或被取代的计划](exec-plans/completed/)
- [技术债跟踪](exec-plans/tech-debt-tracker.md)

每份 active plan 必须写明状态、完整负责人、唯一调用、成功/失败结果、恢复责任和验收条件。
未执行的设备或发布矩阵项只能标为“跳过”。

## 当前专项计划

- Issue #34 第三切片：[组更新收发与主动恢复执行计划](exec-plans/completed/2026-10-11-group-update-connectivity-plan.md)，对应[实现规格](exec-plans/completed/2026-10-11-group-update-connectivity-spec.md)。状态为完成；完整 Uni 收发、统一应用与 /4 主动逐页恢复落地，最终 workspace、成员、真实 SQLite/MLS 强杀断点、独立宿主和固定 main 双方向互通通过；物理设备与跨网络矩阵跳过，不新增批量协议，不关闭整个 #34。
- Issue #34 第二切片：[普通剪贴板连接边界执行计划](exec-plans/completed/2026-10-11-clipboard-connectivity-plan.md)，对应[实现规格](exec-plans/completed/2026-10-11-clipboard-connectivity-spec.md)。状态为完成；切片验收及根门禁通过，完整 workspace 的成员 G2 一项失败与隔离通过均保留记录，原因待查；平台与历史发布矩阵明确跳过。
