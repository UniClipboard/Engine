# Space AGENTS.md 编写计划

## Goal

为 `crates/uc-application/src/space/AGENTS.md` 编写一份以当前源码为准的维护地图，覆盖每个完整 case、支撑模块、职责、流程、关系、设计图和重点检查项。

## Completion Criteria

- [x] 当前源码中的每个完整 Space case 均有条目，名称和路径可验证。
- [x] 明确区分 facade、composition、case、deep module、port/adapter seam 和 runtime。
- [x] 提供总览、用户动作、网络入口和后台恢复的文档内关系图。
- [x] 提供可从目录快速导航的 code map，并标明事实读写和调用关系。
- [x] 写清敏感持久化、最终成员范围、原子提交、幂等、重启和日志规则。
- [x] 更新 `docs/architecture/architecture-bible.md` 的维护记录。
- [x] 路径、符号、Markdown、架构检查和差异检查通过。

## Phases

| Phase | Status | Description |
| --- | --- | --- |
| 1. 目录与 case 盘点 | complete | 枚举源码、入口、输入输出和可见性 |
| 2. 调用与事实关系 | complete | 追踪 facade、application、ledger、runtime 和 endpoint |
| 3. 编写 AGENTS.md | complete | 写 code map、图、case 手册和维护规则 |
| 4. 文档与验证 | complete | 更新架构维护记录并运行检查 |

## Decisions

- 文件使用仓库约定的 `AGENTS.md` 大写命名。
- 设计图直接使用 Mermaid 嵌入 Markdown，避免另建需要同步的图文件。
- “case”只指完整业务动作或网络动作；ledger、runtime、session、identity ports 等单列为支撑模块。
- 只描述当前 application 实现；不把尚未适配的 Infra/Engine 当作已完成接线。

## Errors Encountered

| Error | Attempt | Resolution |
| --- | ---: | --- |
| case 清单比对命令中的反引号被 shell 误解析 | 1 | 改用进程替换和单引号表达式；重新比对后无差异 |
| 本机未安装 Mermaid CLI | 1 | 已检查图块数量、闭合和结构；不引入仅供文档验证的新依赖 |
| 全仓检查被当前工作区其他层尚未适配的旧接口阻塞 | 1 | 保留原改动；应用层检查和 113 项 Space 测试单独通过，失败边界记录到进度 |
| 全仓格式检查发现两个无关文件已有格式差异 | 1 | 未改动无关文件；应用层格式检查单独通过 |
