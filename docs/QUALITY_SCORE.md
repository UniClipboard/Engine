# 文档与架构质量记分卡

本记分卡用于评审，不替代自动化门禁。每项按 0–2 分记录：0 表示缺失，1 表示部分满足，2 表示有明确证据。

| 维度 | 2 分标准 | 证据入口 |
| --- | --- | --- |
| 单一事实来源 | 没有并行规范或兼容实现 | `ARCHITECTURE.md`、ADR、删除检查 |
| 模块深度 | 调用方只见完整入口与稳定结果 | `design-docs/engineering-principles.md` |
| 安全持久化 | 新负载默认密文且迁移/失败路径覆盖 | `SECURITY.md`、明文探针 |
| 错误可诊断性 | 稳定分类保留完整 source chain | `design-docs/error-handling.md` |
| 可靠恢复 | 正式提交、重试、重启和关闭责任明确 | `RELIABILITY.md` |
| 契约测试 | Core/Application/Infra/Engine 边界均有证据 | 对应设计文档与测试目标 |
| 文档可导航性 | AGENTS 只作地图，索引无孤儿/断链 | `README.md`、各目录 index |
| 发布可追溯性 | 版本、提交、校验和设备矩阵一致 | `SECURITY.md`、release manifest |

变更交付时只更新实际受影响维度，并链接可复查证据。没有执行证据时不得给满分。
