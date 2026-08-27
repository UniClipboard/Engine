# Space 内部结构整理计划

## Goal

在不改变 Space 根出口、公开 Facade、deps 和运行行为的前提下，把平铺的内部模块归入生命周期、准入、成员关系和连接恢复四个责任区。

## Completion Criteria

- [x] `space` 根部只保留入口、组装、四个责任区和测试/维护文档。
- [x] 生命周期相关 case、当前 Space 和 session 协调统一归入 `lifecycle/`。
- [x] 成员事实、信任、历史、维护 runtime、签名和 re-pairing 统一归入 `membership/`。
- [x] 准入继续归 `admission/`，连接恢复统一归 `connectivity/recovery/`。
- [x] 每个责任区的 `mod.rs` 只向 Space 内其他责任区导出必要内容，不公开子模块。
- [x] 旧平铺路径物理删除，不保留别名或双路径。
- [x] Space 根出口、`uc_application::facade` 和 `uc_application::deps` 保持不变。
- [x] 更新 Space 维护地图、架构圣经和仓库结构规则。
- [x] application 全部测试、公开路径测试、格式、架构和差异检查完成；既有全仓阻塞单独记录。

## Phases

### Phase 1: Failing structure guard
**Status:** complete

- 固定目标目录和允许留在根部的文件。
- 先扩展架构规则并确认它因当前平铺结构失败。

### Phase 2: Move lifecycle and connectivity
**Status:** complete

- 移动生命周期、session、current_space 和网络恢复。
- 建立责任区内部出口并修复引用。

### Phase 3: Move membership
**Status:** complete

- 移动 ledger、trust、history、maintenance、signing 和 re-pairing。
- 建立成员责任区内部出口并修复跨区引用。

### Phase 4: Documentation and verification
**Status:** complete

- 更新维护地图和架构文档。
- 运行聚焦、完整和仓库级检查。

## Decisions

- `space/application.rs` 继续作为成员/准入组装点，`space/facade/` 继续作为公开入口实现。
- 不建立全局 `use_cases/`、`runtime/`、`ports/` 目录；辅助实现留在所属责任区。
- 本次只移动和收口内部路径，不改业务顺序、结果、错误、持久格式或外部调用。

## Errors Encountered

| Error | Attempt | Resolution |
| --- | ---: | --- |
| 生命周期首次编译剩余升级流程的花括号旧根路径 | 1 | 改为生命周期本地出口与同目录错误类型 |
| 成员首次编译发现根测试四个旧模块名和七个多余责任区导出 | 1 | 测试统一经过 membership 出口；删除仅责任区内部自用的导出 |
| 两个准入查询类型实际只被跨区测试使用，根测试另有 re-pairing 旧路径 | 1 | 两个类型改为仅测试出口；re-pairing 测试统一使用 membership 出口 |
| 架构检查仍读取旧 ledger 路径并直接报文件不存在 | 1 | 将检查指向 `membership/ledger/repository.rs`；其余旧文档路径留到文档阶段统一更新 |
| 准入父目录无法继续导出 invitation 的 `pub(super)` 协作对象 | 1 | 子模块保持私有；六个具体对象改为仅在 `crate::space` 范围可见 |
| 全仓编译仍被 Infra 当前未完成的旧接口适配阻塞 | 1 | 未改动其他层；application 编译、全部测试和公开路径测试通过，失败边界单独记录 |
| 全仓格式检查发现两个无关文件已有格式差异 | 1 | 未改动无关文件；application 格式检查通过 |
| 最终辅助路径搜索使用的 awk 表达式不兼容 macOS awk | 1 | 不重复使用该命令；以已通过的仓库责任区穿透检查作为正式证据 |
