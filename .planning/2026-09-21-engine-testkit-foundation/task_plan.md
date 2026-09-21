# Engine Testkit Foundation

## Goal

在当前 Engine 主线建立可复用、非侵入的测试 kit、nextest 分组和结构化报告基础，不迁移或修改 t-0010 场景，不改变产品行为。

## Completion Criteria

- 正式设计文档和 active exec plan 通过文档与链接自检。
- `uc-testkit` 最小骨架覆盖场景身份、预算、阶段计时、事件等待、目录/端口租约、失败分类、结构化工件和清理结果。
- 成功示范测试和故意失败示范均真实运行并产生可核验工件。
- nextest 分组与本地统一命令可用，现有 `cargo test` 保持可用。
- CI 增加非破坏性入口，不替换现有门禁。
- 相关测试和仓库交付检查通过，形成一个本地原子提交。

## Phases

- [x] Phase 0A：读取架构、文档规则、现有测试能力和 0040 调查报告。
- [x] Phase 0B：完成正式 spec、执行计划、索引与架构记录并自检。
- [ ] Phase 1A：建立 `uc-testkit` 最小完整切片和自测试。
- [ ] Phase 1B：接入 nextest 分组、本地命令和非破坏 CI。
- [ ] Phase 1C：运行成功/故意失败示范、相关回归与仓库门禁。
- [ ] Phase 1D：更新计划证据、报告并创建本地原子提交。

## Ownership

- 完整负责人：`uc-testkit` 的 `Scenario`。
- 调用方唯一动作：用稳定场景名、固定 seed、预算和复现命令创建场景，记录阶段/事件/资源，最后提交成功或失败结果。
- 成功结果：JSON 工件与人类摘要记录通过、阶段耗时、最后事件、资源清理和复现信息。
- 失败结果：稳定失败分类、未满足条件、最后事件、阶段耗时、工件位置和复现信息。
- 重试责任：testkit 不重试；nextest profile 统一决定是否重试。本轮默认不对产品型失败重试。
- 清理责任：资源租约由 RAII 回收，`Scenario::finish` 记录显式清理结果；进程级强制清理由后续 process driver 负责。

## Errors Encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
| 仓库与上级目录未找到 `CONTEXT.md` | 1 | 记录为输入缺口，使用根 `AGENTS.md`、`ARCHITECTURE.md` 和 docs 唯一事实来源继续。 |
| 本机未安装 `cargo-nextest` | 1 | 计划中固定 CI 安装和本地安装检查；实现后安装固定版本再实际运行。 |
