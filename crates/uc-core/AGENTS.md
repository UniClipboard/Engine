# `uc-core` 维护地图

完整设计规范见 [`docs/design-docs/layers/core.md`](../../docs/design-docs/layers/core.md)，跨层原则见
[`docs/design-docs/engineering-principles.md`](../../docs/design-docs/engineering-principles.md)。

## 范围

- 只放领域实体、值对象、规则、事件、策略，以及被 Core 代码直接需要的 port。
- 不放 UseCase、流程编排、数据库、文件系统、网络/密码实现、平台 API、序列化协议或启动接线。
- Rust 行内注释与 doc comment 使用中文；标识符使用英文。

## 硬约束

- 有持久生命周期的领域状态只通过单一事件转换入口推进，效果与副作用分离。
- 终态不可回退；重复、乱序和过期输入用稳定 outcome 表达。
- Port 文档只描述领域契约，不引用调用方、路由、具体协议或实现顺序。
- 新 Port 必须证明 Core 领域代码直接消费；UseCase 专属能力归 Application。
- 禁止引入数据库、网络、UI、异步运行时或具体密码实现依赖。

修改后运行 Core 定向测试、workspace check、fmt、架构检查和 `git diff --check`，并同步架构圣经。
