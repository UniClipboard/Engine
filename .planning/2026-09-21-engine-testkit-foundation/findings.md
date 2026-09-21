# Findings

- 仓库没有 `CONTEXT.md`；已读取 `AGENTS.md`、`ARCHITECTURE.md`、`docs/PLANS.md`、文档系统、工程原则、Rust 规范、架构圣经、规格 034 和 0040 调查报告。
- `.cargo/config.toml` 当前把 libtest 默认并发设为 1，以避免真实网络拓扑叠加；nextest 分组不能直接全局放开并发，必须按资源类型渐进。
- 本机当前没有 `cargo-nextest`。
- 规格 034 已定义后续确定性虚拟成员网络，但本轮边界不实施该生产领域相关测试架构，也不迁移 t-0010。
- 最小长期边界应是独立 workspace crate `tests/uc-testkit`：只依赖通用库，不依赖 `uc-engine`、`uc-infra`、`uc-application` 或 `uc-core`。
- nextest 负责执行、分组、超时、重试和 JUnit；testkit 只负责场景内证据、资源和稳定失败分类。
