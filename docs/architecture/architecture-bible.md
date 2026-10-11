# 架构文档维护记录

当前架构入口为 [ARCHITECTURE.md](../../ARCHITECTURE.md)，长期事实按 [设计文档索引](../design-docs/index.md) 分主题维护。本文件按任务提供的维护约束保留检查记录，不复制主题正文。

## 文档维护记录

- 2026-10-11：Issue #34 第一切片检查发现、连接与授权边界。发现目录、来源生命周期、陌生设备身份和生产 lookup 接入的唯一事实来源为[设备发现与候选目录](../design-docs/peer-discovery.md)，已完成的实施验证与跳过项保留于[执行计划](../exec-plans/completed/2026-10-11-peer-discovery-slice.md)。

- 2026-10-11：将纯 trace carrier helper 移到 network 共同模块，供现有协议与后续传输无关剪贴板交换使用；只改变 crate 内可见范围，线上格式和行为不变。

- 2026-10-11：机械提取原普通剪贴板 adapter 内的测试到相邻文件，模块层级与测试 token 不变；后续生产切换可单独审查实现与测试的迁移。

- 2026-10-11：修复真实剪贴板 trace 验收发现的系统写入与手动重发关联缺口；完整动作只延续不透明 ObservationContext，既有 Engine port decorator 仍负责计时与分类，稳定 facade 不变。
