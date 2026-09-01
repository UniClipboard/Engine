# `uc-application` 维护地图

完整规范见 [`docs/design-docs/layers/application.md`](../../docs/design-docs/layers/application.md)，Port 规则见
[`docs/design-docs/ports.md`](../../docs/design-docs/ports.md)，错误规则见
[`docs/design-docs/error-handling.md`](../../docs/design-docs/error-handling.md)。

## 范围

- Application 负责用户或系统动作的完整流程、顺序、稳定失败分类与恢复责任。
- 按业务领域组织，不建立公共 ports/usecases/errors 技术目录。
- UseCase 专属 port 与消费它的业务模块放在一起，由 Infra 实现、Engine 组装。

## 硬约束

- 依赖只能指向 Core，不引用 Infra 具体类型或自行创建 adapter。
- 一个动作只有一个完整负责人；Facade 和调用方不得编排内部步骤。
- 业务顺序能从负责模块的主要入口读懂，持久恢复不散落到 Runtime。
- 下层失败必须保留 source chain，禁止字符串化或吞错。
- 业务计时和阶段观测由 Engine decorator 完成，不在调用点散布 tracing。

Space 领域修改还需阅读 [`src/space/AGENTS.md`](src/space/AGENTS.md)。交付前运行相关测试、workspace check、fmt、架构检查与 diff check，并同步架构圣经。
