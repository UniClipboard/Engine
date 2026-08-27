# Space 模块出口收口计划

## Goal

先建立稳定的 `space` 模块出口，使后续内部目录整理只修改 `space` 内部，不再要求其他 application 模块跟随深层路径变化。

## Completion Criteria

- [x] `space/mod.rs` 不公开任何子模块，只逐项导出允许离开 `space` 的类型和能力。
- [x] `SpaceFacade` 的实现归 `space` 所有，既有 `uc_application::facade` 入口保持不变。
- [x] `space` 之外不存在 `crate::space::<child>::...` 深层引用。
- [x] case、runtime、session、ledger 实现不能被 `space` 外部直接访问。
- [x] 仓库规则能持续阻止公开子模块和深层引用回归。
- [x] 更新 `space/AGENTS.md` 与架构圣经。
- [x] application、公开契约、格式、架构和差异检查完成；工作区既有失败单独记录。

## Phases

### Phase 1: Inventory and failing guard
**Status:** complete

- 盘点 `space` 外部当前深层引用和现有公开出口。
- 先新增结构规则并确认它因当前结构准确失败。

### Phase 2: Establish the interface
**Status:** complete

- 将 SpaceFacade 实现迁入 `space`。
- 将子模块改为私有，并在 `space/mod.rs` 建立明确出口。
- 把其他 application 模块改为只依赖根出口。

### Phase 3: Documentation and verification
**Status:** complete

- 更新维护地图和架构维护记录。
- 运行聚焦测试、公开契约和仓库规定检查。

## Decisions

- 本次只建立出口，不整理 `lifecycle`、`membership` 等内部目录。
- 既有 `facade` 和 `deps` 是允许的公开清单，不保留旧的内部深层路径。
- 不改变产品操作、输入输出、错误、事件或持久数据。

## Errors Encountered

| Error | Attempt | Resolution |
| --- | ---: | --- |
| application 首次编译缺少 `SpaceActivityError` 根出口，并有三个测试引用警告 | 1 | 补入根出口；测试专用名称改为仅在测试构建时引入 |
| 全仓编译仍被 Infra 当前未完成的旧接口适配阻塞 | 1 | 未改动其他层；application 全编译、全部测试和外部公开路径测试通过，失败边界单独记录 |
| 全仓格式检查发现两个无关文件已有格式差异 | 1 | 未改动无关文件；application 格式检查通过 |
