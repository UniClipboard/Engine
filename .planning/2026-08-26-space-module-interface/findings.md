# Space 模块出口发现

## 2026-08-26

- `lib.rs` 已限定外部 crate 只能使用 `facade` 与 `deps`，`space` 本身为 `pub(crate)`。
- `space/mod.rs` 当前公开 23 个 crate 内子模块，其他 application 模块可直接穿透内部结构。
- 选定范围内共有约 184 处深层引用，分布在 63 个文件；其中多数位于 `space` 内部，真正需要隔离的是 `space` 之外的引用。
- `facade/space_setup` 当前同时拥有 SpaceFacade 实现并直接重新导出多个 Space 子模块类型，因此实现需要归回 `space`，公开目录只保留清单。
- `deps.rs` 当前直接从多个 Space 子目录重新导出组装能力，应改为只从 `crate::space` 根出口取得。
- 新增结构规则后的首次运行按预期失败：识别 24 个公开子模块、8 个从其他 application 模块穿透 Space 的文件，以及 5 个 Space 内部反向依赖公开 Facade 的文件。
- 公开 Facade 目录共有 `commands.rs`、`deps.rs`、`errors.rs`、`facade.rs` 和 `mod.rs`；前四个应归入 Space，原 `mod.rs` 保留为公开清单。
- Facade 实现迁入 Space 后，公开 `facade/space_setup/mod.rs` 可以只保留从 Space 根出口取得的白名单，不需要兼容实现。
- 结构搜索已达到零深层穿透：Space 外部只使用 `crate::space::{...}`，Space 内部不再依赖 `crate::facade::space_setup`。
