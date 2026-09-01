---
name: uni-module-depth-audit
description: "独立审查 UniClipboardEngine 的调用者复杂度、模块深度、所有权边界、port 与依赖装配。用于定时架构审查、公开接口或依赖面变更审查，以及确认复杂度是否泄露给调用方；不用于实现修复，也不调用其他 skill。"
---

# Uni 模块深度审查

## 目的

从调用者视角判断接口是否隐藏实现复杂度，并确认跨层流程具有唯一完整负责人。只报告有完整调用链证据的问题，不因接口数量、参数数量或名称本身下结论。

## 独立运行约束

- 一个 session 只运行本 skill，不调用或模拟其他审查 skill。
- 默认只读；除调用方明确指定的报告输出路径外，不修改仓库、Issue 或 PR。
- 开始前完整读取共享的 [finding contract](../uni-audit-report/references/finding-contract.md)，所有输出严格遵循该契约。
- 只报告本 lane 的问题；遇到错误、并发、观测或安全问题，仅在报告的 `handoff` 中标记建议 lane，不替其他 lane 作结论。

## 事实来源

先读取根目录和目标目录最近的 `AGENTS.md`，再读取：

- `ARCHITECTURE.md`
- `docs/design-docs/core-beliefs.md`
- `docs/design-docs/engineering-principles.md`
- `docs/design-docs/ports.md`
- `docs/design-docs/uc-engine-interface.md`
- `docs/architecture/architecture-bible.md`
- `docs/PLANS.md`，仅用于识别已声明技术债

文档与代码不一致时，以当前代码行为作为实现证据，以长期设计文档作为规则依据，并明确记录差异。执行计划不是当前事实。

## 工作流

1. 按 finding contract 确定 `diff`、`targeted` 或 `full` 范围。读取 `git status --short`，保护既有修改。
2. 建立范围内公开入口、调用方唯一动作、领域 owner、port、Infra 实现和 Engine 装配的映射。
3. 对每个候选问题至少追踪一条代表性调用链，直到实际副作用或稳定边界；不要只审阅 trait 或构造器声明。
4. 从调用者视角检查：
   - 调用方是否必须理解策略选择、操作顺序、重试、加密、状态机或底层表示；
   - 接口面积是否与隐藏的实现能力相称，是否只是把内部步骤重新暴露；
   - 参数对象、依赖集合和 port 是否表达稳定能力，而非组织内部零件；
   - Core、Application、Infra、Engine 是否各守职责，跨层流程是否有唯一完整负责人；
   - Engine 是否只做稳定入口和装配，绑定是否保持薄且只依赖 `uc-engine`；
   - 跨层持续计时和结果分类是否留在 Engine 组装的领域 port decorator；
   - 测试替身是否围绕稳定 seam，而不是复制大量内部装配知识。
5. 用调用点、实现点、规则文档和影响路径共同验证 finding。数量、正则命中或个人风格偏好只能形成候选项。
6. 将已在 active plan 或技术债追踪器中有明确 owner 的同一问题标为 `existing_declared_debt`，不要重复制造新问题。
7. 按共享契约输出本 lane JSON；未指定输出路径时，在最终回复给出同等字段的 Markdown。

## 建议检索

优先用 `rg` 定位 `pub` API、trait、port、`*Deps`、builder、Engine 装配和绑定依赖，再顺着调用关系阅读。可以运行 `cargo metadata --locked --format-version 1` 和架构检查脚本验证依赖事实；命令未执行必须记为 `skipped`。

## 完成条件

- 范围内关键入口均映射到 owner 和实际副作用。
- 每条 finding 有调用者负担、完整证据链和被违反的仓库规则。
- 没有把接口规模、依赖数量或 `pub` 关键字单独当作缺陷。
- 报告只包含本 lane 结论，并如实记录覆盖范围和跳过项。
