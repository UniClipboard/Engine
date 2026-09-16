# Progress: Admission Record V2 Convergence

## 2026-09-14
- 确认工作区起始状态干净，分支起点为 `255e1a68`。
- 读取根目录、Core、Infra、Docs 约束和 Rust 编写规范。
- 建立独立收敛计划；完成标准是不再存在 admission aggregate V3 及以上兼容负担。
- 当前工作：对照 V1 与当前 V2-V6 的结构和测试。
- 已把确认、终止、撤销、邀请方到期和本机空间退出统一编码进 V2，删除 Core aggregate V3–V6 定义和读写分支。
- Core admission 101 项、公开持久化 12 项和 Infra 真实加密 SQLite admission 28 项通过。
- Core 完整 290 项单元测试、全部集成测试和 19 项文档测试通过。
- metadata、全 workspace/all-targets check、格式、Rust 规则、Engine 架构与隐私门禁、diff check 通过；保留既有 HarmonyOS 测试未使用导入警告。
- 最终差异审查确认 Core aggregate 只剩 V1/V2；Infra 原有外层加密仓储版本未被误删。本片准备作为独立本地提交。
