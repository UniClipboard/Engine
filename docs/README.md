# UniClipboardEngine 文档记录系统

`docs/` 是仓库知识的记录系统。根 [`AGENTS.md`](../AGENTS.md) 只提供维护地图；设计原因、当前事实、
产品需求、执行状态和参考资料分别由下列唯一入口管理。

## 主题入口

- [架构](../ARCHITECTURE.md)：系统边界、依赖方向与主要模块。
- [设计](DESIGN.md)：长期设计、稳定契约与 ADR。
- [计划](PLANS.md)：进行中、已关闭计划与技术债。
- [产品理念](PRODUCT_SENSE.md)：愿景、产品原则与平台关系。
- [产品/宿主边界](FRONTEND.md)：Engine 与前端、移动/桌面宿主的责任边界。
- [安全](SECURITY.md)：密文持久化、日志隐私与发布完整性。
- [可靠性](RELIABILITY.md)：恢复、后台任务与验证层次。
- [质量记分卡](QUALITY_SCORE.md)：评审维度与证据入口。

## 记录分类

| 目录 | 保存内容 | 不保存 |
| --- | --- | --- |
| [`design-docs/`](design-docs/) | 长期设计、稳定契约、分层规范、ADR | 实施进度 |
| [`exec-plans/active/`](exec-plans/active/) | 设计中、实施中、待实现、阻塞计划 | 已完成工作 |
| [`exec-plans/completed/`](exec-plans/completed/) | 完成或被取代的实施历史 | 当前架构事实 |
| [`generated/`](generated/) | 可从源码、迁移或工具再生的资料 | 手工权威规则 |
| [`product-specs/`](product-specs/) | 产品问题、范围、需求与验收 | 内部算法和步骤 |
| [`references/`](references/) | 领域词表、迁移映射等查询资料 | 新决策 |

文档的创建、状态、移动与写作规则见 [文档记录系统](design-docs/documentation-system.md)。
