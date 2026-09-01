# 技术债跟踪

只记录已经有明确证据、负责人边界与退出条件的债务；模糊想法应先进入设计讨论。

| 项目 | 状态 | 依赖/退出条件 | 计划 |
| --- | --- | --- | --- |
| Legacy Space transition 退役 | 待实施 | maintenance-only 失败关闭、旧 executor 删除、V2 layout 私有化与架构防回归检查完成 | [032](active/032-admission-space-transition-internal-refactor.md) |
| 成员反熵实体设备矩阵 | 待验收 | 执行并如实记录设备与 Release bundle 项 | [029](active/029-durable-membership-history-anti-entropy.md) |
| 本地产物统一准备 | 待实现 | 脚本、清单和三目标验证完成 | [计划](active/local-artifacts-preparation.md) |
| 移动端文件日志层 | 待实现 | 宿主 cache、滚动与脱敏验证完成 | [计划](active/mobile-log-file-layer.md) |

关闭项目时记录验证证据，更新稳定文档，并将对应计划移入 `completed/`。
