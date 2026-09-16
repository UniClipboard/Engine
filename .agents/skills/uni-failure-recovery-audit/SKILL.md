---
name: uni-failure-recovery-audit
description: "独立审查 UniClipboardEngine 的 Rust 错误转换、source chain、失败分类、事务边界、重试与重启恢复责任。用于定时可靠性审查、错误类型或持久化流程变更审查，以及排查 map_err slop；不用于实现修复，也不调用其他 skill。"
---

# Uni 失败与恢复审查

## 目的

确认失败从 Infra 穿过 Application 到稳定入口时仍可诊断，并且部分成功、重试、回滚和重启后的责任明确。`map_err`、`to_string()` 或错误枚举数量只用于发现候选项，不能单独构成 finding。

## 独立运行约束

- 一个 session 只运行本 skill，不调用或模拟其他审查 skill。
- 默认只读；除调用方明确指定的报告输出路径外，不修改仓库、Issue 或 PR。
- 开始前完整读取共享的 [finding contract](../uni-audit-report/references/finding-contract.md)。
- 只报告错误与恢复语义；其他维度写入 `handoff`，不代替相应 lane 审查。

## 事实来源

先读取根目录和目标目录最近的 `AGENTS.md`，再读取：

- `docs/design-docs/error-handling.md`
- `docs/RELIABILITY.md`
- `docs/design-docs/core-beliefs.md`
- `ARCHITECTURE.md`
- `docs/SECURITY.md` 中公开错误与敏感信息约束
- `docs/PLANS.md`，仅用于识别已声明技术债

## 工作流

1. 按共享契约确定审查范围，并记录工作树基线。
2. 定位错误枚举、`From`/`source` 实现、`map_err`、字符串化、事务、重试、补偿和恢复入口。
3. 对每个候选项追踪完整失败路径：底层失败 → Infra 映射 → Application 决策 → Engine/公开边界 → 调用方可采取的动作。
4. 检查：
   - 下层错误是否保留真实 source chain，而非只保存 `Display` 文本；
   - 转换是否增加稳定领域语义，还是制造无意义的逐层包装；
   - 公开错误是否稳定、可分类且已脱敏，同时内部诊断仍能到达根因；
   - 暂时性、永久性、取消、冲突和损坏是否被错误地混为一类；
   - 重试由谁触发、是否有次数/退避/幂等边界，重启后由谁恢复；
   - 多步写入是否存在部分提交，事务、回滚、补偿或可重放责任是否完整；
   - 错误是否被吞掉、降级为成功、只打印或从 detached task 中消失；
   - 测试是否验证错误类别、source 和关键恢复行为，而非只匹配展示字符串。
5. `map_err` 只有在证实 source 丢失、语义错分、敏感信息泄露或恢复动作受损时才报告。仓库允许的明确边界转换不应被标记。
6. 对已有明确计划和 owner 的同一债务标为 `existing_declared_debt`。
7. 按共享契约输出本 lane JSON；未指定路径时输出等价 Markdown。

## 建议检索

优先使用 `rg` 搜索 `map_err`、`to_string()`、`format!`、`source`、`#[source]`、`transparent`、事务、retry、recovery 和错误枚举。检索结果必须结合类型定义与调用路径人工验证。

## 完成条件

- 每条 finding 都能展示根因在哪一层丢失或恢复责任在哪个边界断裂。
- 没有把 `map_err` 或字符串化的存在本身当作问题。
- 公开脱敏与内部 source chain 被分别评价。
- 覆盖、验证命令和跳过项记录完整。
