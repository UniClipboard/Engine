# `uc-core` 维护地图

完整设计规范见 [`docs/design-docs/layers/core.md`](../../docs/design-docs/layers/core.md)，跨层原则见
[`docs/design-docs/engineering-principles.md`](../../docs/design-docs/engineering-principles.md)。
当前已知违规与收口顺序见 [Core 边界收口计划](../../docs/exec-plans/active/2026-09-23-core-boundary-remediation.md)。

## 范围

- 只放标识、值对象、聚合与状态机、不变量、策略、领域证明，以及 Core 领域代码直接调用的 port。
- 不放 UseCase、流程顺序、本地存储格式与编解码、线上编码、数据库/文件系统/网络/密码实现、平台 API、运行时或启动接线。
- Rust 行内注释与 doc comment 使用中文；标识符使用英文。

## 硬约束

- 有生命周期的对象只有一个公开推进入口 `apply`，状态字段不公开；转换纯函数，不读时钟、不取随机数。
- 转换返回有约束力的效果义务，每个效果声明在保存新状态之前还是之后完成。
- 跨记录判定写在 Core；Infra 只保证原子性。
- 终态不可回退；重复、乱序和过期输入用稳定 outcome 表达。
- 存储格式与版本归 Infra；只有签名或摘要覆盖的规范编码可留在 Core，并须有字节稳定测试。
- Port 文档只描述领域契约，不引用调用方、路由、具体协议或实现顺序。
- 禁止 `tokio`、`anyhow`、格式/传输类依赖；生产代码禁止 `panic!`、`unwrap()`、`expect()`、`unreachable!()`。

修改后运行 Core 定向测试、workspace check、fmt、架构检查和 `git diff --check`；架构事实变化时同步对应主题文档。
