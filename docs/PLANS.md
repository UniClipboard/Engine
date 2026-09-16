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
