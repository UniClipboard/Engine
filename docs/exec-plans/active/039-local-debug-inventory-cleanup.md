# 规格 039：历史本地调试记录逐项收口

## 状态

- **状态**：待实现
- **日期**：2026-09-05
- **前置规格**：[037 OpenTelemetry tracing 与结构化日志](../completed/037-opentelemetry-tracing-and-structured-logs.md)
- **完整负责人**：运行诊断合同 owner 维护唯一清单、分类规则和最终门禁；每个业务领域 owner 只处理自己的调用点
- **调用方唯一动作**：维护者运行同一清单生成与检查命令，不手工维护第二份列表
- **成功结果**：清单中的每个历史调用点都有可执行分类，允许输出的记录只含固定安全字段
- **失败结果**：未分类、含正文、原始错误或敏感字段的调用点继续被默认拒绝，不因迁移未完成扩大输出
- **重试与重启责任**：本计划只整理诊断记录，不改变业务恢复、重试或持久状态

# 1. Overview

037 将系统日志、JSONL 和远程输出改为默认拒绝普通模块记录，因此现有历史 debug/info/warn 不会自动进入这些输出。最终生成清单有 1,231 个本地调试调用点，其中不少带自由正文、原始错误或敏感字段标记。

这些调用点不是当前泄露路径，但长期留在未分类状态会让后续排障人员绕过统一合同。本规格逐项决定删除、改成类型化安全本地记录，或以明确理由保留为不输出的开发调用。

# 2. Goals

- 让生成清单中的每个 local debug 调用点都有唯一处置结果。
- 删除无诊断价值、重复或只复述控制流的记录。
- 需要保留的记录使用固定事件、固定分类和必要的非敏感计数。
- 原始错误只保留 source chain，不进入日志正文。
- 每个领域分片完成后重新生成清单并运行真实系统/JSONL 哨兵检查。

# 3. Non-Goals

- 不扩大 037 的远程字段或 target 白名单。
- 不为日志增加业务 port、Facade、结果字段或 Engine 步骤查询。
- 不一次性机械替换所有调用点。
- 不改变业务错误、重试、网络、存储或恢复行为。
- 不把“仅本地”当作可以记录内容、身份、地址、路径或密钥的理由。

# 4. Current Architecture Context

```text
Component: 可再生观测清单
Path: docs/generated/observability-inventory.md
Responsibility: 从生产 Rust 源列出 stable remote、local operational、local debug 和 product analytics。
Relationship: 本计划唯一工作队列，不手工复制调用点。
```

```text
Component: 隐私检查
Path: scripts/architecture/check-observability-privacy.mjs
Responsibility: 拒绝正文、原始错误、敏感字段、越权 target 和退役路径。
Relationship: 每个分片的固定退出门禁。
```

```text
Component: 进程观测运行时
Path: crates/uc-observability-runtime/
Responsibility: 只接收类型化诊断 owner 与固定健康记录。
Relationship: 普通历史调用点当前默认不输出，本计划不能放宽这一边界。
```

# 5. Proposed Design

## Components

每个领域建立一次短期工作队列，按以下顺序决定：

1. 没有明确排障问题可回答，删除。
2. 与已有稳定记录重复，删除。
3. 只需开发时查看且含自由信息，保留为默认不输出的 debug，并写明理由。
4. 需要进入受管本地输出，先在诊断合同增加固定类型，再由完整 capability owner 调用。

## Data Model

处置结果只有 `delete`、`keep_rejected_debug`、`typed_local` 三类。工作状态由生成清单和提交差异表达，不新增持久表或运行时配置。

## API / Interface

不新增业务 API。只有确有两个以上调用者需要同一固定事实时，才扩展现有诊断合同；任意字符串事件名、阶段名或字段注册表仍禁止。

## Workflow

1. 生成当前清单并冻结调用点总数。
2. 领域 owner 只领取自己的目录。
3. 每个调用点选择一种处置并补相邻测试。
4. 重新生成清单，确认没有越权输出。
5. 汇总 owner 检查跨领域重复和稳定文档后归档。

# 6. Implementation Plan

## Slice 0：分类规则与样例（串行）

**File**：隐私检查、生成清单和每类一个真实样例。

**Change**：固定三种处置及负向 fixture，证明普通调用点仍不能进入受管输出。

**Risk**：样例被误当通用日志 helper。只使用现有类型化合同，不新增万能入口。

## Slice 1：Application 领域（可多 Agent 按 Clipboard、Space、Search、Transfer 独占目录并行）

**File**：`crates/uc-application/src/<domain>/`。

**Change**：各 Agent 只处理一个业务领域，不修改共享诊断合同；需要共享扩展时只提交 handoff。

**Risk**：并行修改共享文件。共享合同由汇总 owner 串行处理。

## Slice 2：Infra 领域（可多 Agent 按 Network、Storage、Security 独占目录并行）

**File**：`crates/uc-infra/src/<domain>/` 与 compatibility 实现。

**Change**：移除地址、路径、错误正文和身份字段；认证失败只保留固定类别。

**Risk**：删掉唯一故障证据。每次删除前先写出该记录回答的问题和替代证据。

## Slice 3：Engine、Binding 与汇总（串行）

**File**：`crates/uc-engine/`、`bindings/`、稳定文档、清单与门禁。

**Change**：清理装配和语言边界记录，合并确有复用的固定合同，执行真实 sink 验收。

**Risk**：绑定打印公开输入。配置与错误 Debug 继续整体隐藏。

# 7. Edge Cases

```text
Scenario: 原始错误包含路径、地址或身份。
Expected behavior: source chain 保留给调用方，日志只写固定错误类别。
Implementation: 类型化映射加敏感哨兵测试。
```

```text
Scenario: Agent 想为一个调用点新增通用 recorder。
Expected behavior: 拒绝共享抽象；调用点继续默认不输出，直到有真实复用证据。
Implementation: 共享合同只由汇总 owner 串行修改。
```

```text
Scenario: 清单行号因相邻修改变化。
Expected behavior: 重新生成，不手工修行号或保存第二张表。
Implementation: 生成脚本是唯一来源。
```

# 8. Testing Strategy

## Unit Test

- 固定错误分类、计数边界和空值省略。
- 隐私检查对正文、原始错误、敏感字段和越权 target 的负例。

## Integration Test

- 每个领域至少一条真实成功和失败路径进入内存或本地 sink。
- 系统输出、JSONL 与原始远程数据使用同一组敏感哨兵扫描。

## Regression Test

- 业务结果、错误 source、重试和重启测试不变。
- workspace、架构、格式与生成清单检查通过。

# 9. Acceptance Criteria

* [ ] 生成清单中每个 local debug 调用点都有唯一处置结果。
* [ ] 受管输出没有自由正文、原始错误或敏感字段。
* [ ] 普通模块 target 仍默认拒绝，远程白名单没有扩大。
* [ ] 没有新增万能日志 helper、业务步骤接口或 Engine 状态查询。
* [ ] 每个并行分片有独占目录、非零测试和重新生成的清单证据。
* [ ] 所有自动检查通过；未执行设备明确标为“跳过”。

# 10. Risks and Trade-offs

- 删除历史日志会减少临时信息，但保留未经约束的正文会持续扩大隐私风险；以能够回答的具体故障问题作为保留门槛。
- 分领域处理需要多次生成清单，但能保持审查范围和业务语义，不做不可验证的批量替换。
- 固定类型增加少量合同维护成本，换来低基数、可搜索和跨平台一致性。

# 11. Open Questions

无开工前阻塞问题。各领域的实际保留项必须由对应调用者和故障案例证明，不能在计划阶段预先假定。
