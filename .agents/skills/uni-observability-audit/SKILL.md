---
name: uni-observability-audit
description: "独立审查 UniClipboardEngine 的日志、tracing、指标、关联上下文、关键阶段与结果分类是否足以安全调试。用于定时可观测性审查、关键流程或观测装配变更审查，以及排查日志或 trace 缺口；不用于实现修复，也不调用其他 skill。"
---

# Uni 可观测性审查

## 目的

判断出现用户可见失败、延迟或数据不一致时，现有信号能否定位到负责阶段和结果类别，同时不泄露敏感内容。缺少 `tracing` 宏或日志数量少本身不是 finding。

## 独立运行约束

- 一个 session 只运行本 skill，不调用或模拟其他审查 skill。
- 默认只读；除调用方明确指定的报告输出路径外，不修改仓库、Issue 或 PR。
- 开始前完整读取共享的 [finding contract](../uni-audit-report/references/finding-contract.md)。
- 只报告观测覆盖与信号安全性；错误、并发或架构问题仅写入 `handoff`。

## 事实来源

先读取根目录和目标目录最近的 `AGENTS.md`，再读取：

- `docs/design-docs/observability.md`
- `docs/design-docs/error-handling.md`
- `docs/SECURITY.md`
- `docs/design-docs/core-beliefs.md`
- `ARCHITECTURE.md`
- `docs/PLANS.md`，仅用于识别已声明技术债

## 工作流

1. 按共享契约确定范围并记录基线。
2. 列出范围内用户可见流程的关键阶段、成功结果、预期拒绝、内部失败、取消和超时。
3. 从 Engine 入口追踪关联上下文、领域 port decorator、Application 流程和 Infra 副作用，建立“阶段 → 现有信号 → 可回答问题”的映射。
4. 检查：
   - 失败发生后是否能定位到流程、阶段、稳定结果类别和负责 owner；
   - trace/span 上下文是否跨异步边界和后台 task 正确传播；
   - 跨层持续计时和结果分类是否只由 Engine 装配的领域 port decorator 完成；
   - Application 是否被迫直接依赖 tracing 或墙钟/单调时钟细节；
   - 结构化字段是否稳定、低基数且能关联一次操作，避免自由文本成为唯一诊断入口；
   - 指标是否有明确分母、单位和结果分类，避免只统计成功或产生无界 label；
   - 同一失败是否被多层重复记录并制造噪声，或在任何边界都没有记录；
   - 日志、错误和事件是否避免剪贴板内容、凭据、令牌、邀请、设备名、地址、文件名和路径；
   - 关键观测契约是否有测试或装配检查。
5. “缺失” finding 必须说明具体故障场景、现有信号无法回答的问题和最小应观测边界；不要提出“多加日志”式结论。
6. “泄露” finding 只记录字段来源、汇点和代码位置，不复制运行时敏感值或原始样例。
7. 对已有明确计划和 owner 的同一问题标为 `existing_declared_debt`。
8. 按共享契约输出本 lane JSON；未指定路径时输出等价 Markdown。

## 建议检索

使用 `rg` 搜索 `tracing`、`instrument`、`span`、`event`、`metric`、`histogram`、`counter`、analytics、decorator 和 Engine 装配。随后追踪调用链与字段来源；宏数量和 import 缺失只能作为候选信号。

## 完成条件

- 每条缺口都绑定到具体故障场景和不可回答的诊断问题。
- 每条泄露风险只描述数据流，不回显敏感数据。
- 明确区分未覆盖、重复噪声、错误分类和高基数字段。
- 覆盖范围、测试证据与跳过项记录完整。
