# 工程与模块设计原则

## 依赖与职责

固定依赖方向是 `uc-engine → uc-infra → uc-application → uc-core`。`uc-engine` 只负责组装、
生命周期和稳定契约；`uc-infra` 可以实现 Core 或 Application 定义的 port，但不能决定业务；
`uc-application` 负责完整流程；`uc-core` 只表达纯业务规则。

跨层功能开工前必须回答：

1. 谁对完整结果负责？
2. 调用方唯一需要执行什么？
3. 成功和失败分别返回什么稳定结果？
4. 重启恢复或重试由谁负责？

回答不清楚时，先设计模块边界，不进入实现。

## 防止复杂度外泄

- 不把判断、流程推进、通信、持久化、失败恢复、后台重试和启动接线同时暴露给调用方。
- 不为每个内部步骤建立一一对应的公共接口。
- 一个行为即使跨多个 crate，也必须能从唯一负责人入口与测试读懂。
- Runtime 只负责触发、并发、暂停、恢复和关闭，不掌握业务步骤。
- Facade 只表达完整意图和稳定结果，不读取仓储或处理协议阶段。
- Composition root 是唯一同时知道 port 与具体 adapter 的位置。

## Port 所有权

Port 归需要能力的层与业务模块所有，不按最终实现位置归类。Core 只有在领域代码直接消费能力时
才定义 port；use-case 专属能力放在相应 Application 业务模块附近。Port 描述业务意图或稳定能力，
不得泄露 SQL、Iroh、HTTP、平台 SDK 或调用方的步骤顺序。

## 删除检查

评审模块时设想删除它：

- 若复杂度会散回多个调用方，说明模块确实隐藏了知识，应保留。
- 若删除后几乎没有变化，说明它只是转发层，应合并或重划职责。

文件数量不是问题；同一行为的知识散落才是问题。

## 相关设计

- [Core 设计规范](layers/core.md)
- [Application 设计规范](layers/application.md)
- [Infra 设计规范](layers/infrastructure.md)
- [Port 定义](ports.md)
- [Space Application](space-application.md)
