---
name: to-spec
description: Create an implementation-ready software design specification for the current requirement.
---

你是一名资深软件架构师，请为当前需求编写一份可直接用于 Coding Agent 执行的软件设计 Spec。

目标：
这份 Spec 不是面向产品经理的需求文档，而是面向 AI Coding Agent 和工程师的实现文档。阅读 Spec 后，Agent 应该能够理解上下文、明确修改范围、设计方案、实现步骤和验收标准。

请严格按照以下结构输出：

# 1. Overview

说明：
- 这个需求解决什么问题
- 当前存在的问题是什么
- 为什么需要这个改动

要求：
- 不描述空泛目标
- 结合当前代码架构和实际场景


# 2. Goals

明确本次实现必须达到的目标。

使用列表：

- Goal 1
- Goal 2

每个目标需要可验证。


# 3. Non-Goals

明确本次不处理的内容。

例如：

- 不重构已有模块
- 不修改公共 API
- 不改变已有行为

防止 Agent 扩大修改范围。


# 4. Current Architecture Context

分析当前系统：

包含：

- 相关模块
- 数据流
- 核心组件职责
- 当前实现方式
- 涉及代码路径


格式：

```

Component:
Path:
Responsibility:
Relationship:

```


# 5. Proposed Design

详细描述实现方案。


包含：

## Components

新增或修改哪些组件。

对于每个组件说明：

- 职责
- 输入
- 输出
- 与其他模块关系


## Data Model

如果涉及数据：

定义：

- 数据结构
- 字段含义
- 生命周期


## API / Interface

定义：

- 方法名
- 参数
- 返回值
- 错误处理


## Workflow

使用步骤描述运行流程：

例如：

1. 用户触发 xxx
2. Module A 调用 Module B
3. Module B 处理 xxx
4. 返回结果


# 6. Implementation Plan

拆解实现步骤。

要求：

每一步：

- 明确修改位置
- 修改内容
- 风险


例如：

```

Step 1:
File:
Change:

Step 2:
File:
Change:

```


# 7. Edge Cases

列出异常情况。

至少考虑：

- 空数据
- 并发
- 网络失败
- 数据损坏
- 兼容旧版本
- 极端输入


每个 case 给出：

```

Scenario:
Expected behavior:
Implementation:

```


# 8. Testing Strategy

设计测试方案。


包括：

## Unit Test

测试核心逻辑。


## Integration Test

测试模块协作。


## Regression Test

确保已有功能不受影响。


每个测试说明：

- 输入
- 操作
- 预期结果


# 9. Acceptance Criteria

提供最终验收 checklist。

格式：

```

* [ ] xxx
* [ ] xxx

```

要求每一项可以明确判断成功或失败。


# 10. Risks and Trade-offs

说明：

- 技术风险
- 性能影响
- 维护成本
- 替代方案


# 11. Open Questions

列出目前无法确定的问题。

不要自行假设。


---

额外要求：

1. 不要直接写代码。
2. 不要省略架构分析。
3. 不要使用“优化”“增强体验”等无法验证的描述。
4. 所有设计决策必须说明原因。
5. 如果当前信息不足，请明确指出缺失信息，并列出需要补充的问题。
6. 优先保持现有架构一致性，而不是引入新的复杂方案。
7. Spec 应该让另一个没有参与项目的人，仅通过阅读文档即可完成实现。
```

---

对于你现在这种 **UniClipboard / critic engine 这种长期维护项目**，我还建议额外加一段：

在设计过程中，请优先识别：

- 哪些逻辑属于业务规则
- 哪些逻辑属于 heuristic
- 哪些逻辑属于基础设施
- 哪些模块容易产生上下文负担

如果存在复杂 heuristic，请要求：

- 给出输入案例
- 给出判断规则
- 给出失败案例
- 给出未来扩展方式

不要把复杂判断隐藏在代码实现中。
