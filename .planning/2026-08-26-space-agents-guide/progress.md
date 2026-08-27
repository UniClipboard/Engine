# Space AGENTS.md 编写进度

## 2026-08-26

- 读取 `codebase-design`、`diagram-design` 和 `planning-with-files` 指令。
- 枚举 `space` 源码文件、use case 结构和模块导出。
- 确认文档采用大写 `AGENTS.md`，图直接嵌入 Markdown。
- 建立完成标准和分阶段盘点计划。
- 读取生命周期、重建、查询、移除和决定 case，记录入口、顺序和失败语义。
- 读取全部 admission case、历史发送和历史接收实现，记录原子点、幂等和网络门禁。
- 读取维护 runtime、ledger、效果恢复、受限投递、session activity、re-pairing 和 network recovery。
- 追踪 `AppFacade -> SpaceFacade -> SpaceApplication -> cases/endpoints/runtime -> MembershipLedger` 调用关系和公开结果类型。
- 新增 `space/AGENTS.md`：包含 code map、5 张关系图、30 个 case、网络恢复 workflow、ledger 设计和修改检查表。
- 修正 `space/mod.rs` 仍指向旧 convergence 结构的模块说明。
- 更新架构圣经的文档维护记录。
- 从源码与文档分别提取 case 名称进行比对，30 个名称完全一致；文档内仓库路径全部存在。
- 检查 Markdown 围栏：5 个 Mermaid 图和 1 个命令块均完整闭合。
- `cargo metadata --locked --format-version 1` 通过。
- `cargo check -p uc-application --all-targets --locked` 通过。
- Space 测试清单非零，共 113 项；串行执行结果 113 通过、0 失败。
- `cargo fmt -p uc-application -- --check` 通过。
- `node scripts/architecture/check-engine-repository.mjs` 通过。
- `git diff --check` 通过。
- `cargo check --workspace --all-targets --locked` 未通过：当前未提交的 Infra 适配仍引用 application 已删除的旧接口，并有既有 delivery error 构造不匹配；本次文档改动未涉及这些文件。
- `cargo fmt --all -- --check` 未通过：仅报告 `crates/uc-engine/src/assembly/sync_engine.rs` 与 `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs` 的既有格式差异；本次涉及的 application 格式检查通过。
