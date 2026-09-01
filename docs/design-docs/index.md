# 设计文档索引

这里记录长期有效的架构、边界、协议与重要技术取舍。当前仓库事实从根目录
[`ARCHITECTURE.md`](../../ARCHITECTURE.md) 进入；准备实施的工作不放在这里，而放入
[`docs/exec-plans/`](../exec-plans/)。

## 先读

- [核心信念](core-beliefs.md)：设计判断的共同前提。
- [工程原则](engineering-principles.md)：深模块、单一负责人、依赖方向与删除检查。
- [错误处理](error-handling.md)：稳定分类、source chain 与安全上下文。
- [运行期观测](observability.md)：Engine 组装边界的 port decorator 范式。
- [文档记录系统](documentation-system.md)：文档类型、生命周期和写作规则。

## 架构与契约

- [架构圣经](../architecture/architecture-bible.md)：当前实现的详细事实。
- [uc-engine 跨平台核心接口](uc-engine-interface.md)：唯一稳定 Rust 入口。
- [Port 设计](ports.md)：Core/Application 能力边界。
- [Engine 仓库检查](engine-repository-checks.md)：所有权、依赖和发布门禁。
- [当前成员运行范围](current-member-runtime-scope.md)：成员资格与普通能力的统一范围。
- [Space Application](space-application.md)：Space 领域的入口、负责人、恢复路径与测试地图。

## 分层规范

- [`uc-core`](layers/core.md)
- [`uc-application`](layers/application.md)
- [`uc-infra`](layers/infrastructure.md)

## 功能设计

- [本地加密搜索](features/001-local-encrypted-search.md)
- [主动刷新共享设备](features/012-automatic-shared-device-refresh.md)
- [按设备公开等待状态](features/019-device-specific-convergence-waiting-status.md)

## 决策记录

所有 ADR 按原编号保存在 [`decisions/`](decisions/)。ADR 一经被取代也不删除，只在状态中指向替代决策。
