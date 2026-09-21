# Progress

## 2026-09-21

- 完成阶段 0 输入阅读和现状盘点。
- 确认 `CONTEXT.md` 缺失、本机 `cargo-nextest` 未安装。
- 确定正式文档为 `docs/design-docs/testing-architecture.md` 和 `docs/exec-plans/completed/044-engine-testkit-foundation.md`。
- 确定实现边界为独立 `tests/uc-testkit` crate，不修改产品业务模块，不迁移 t-0010。
- 完成 `docs/design-docs/testing-architecture.md`、`docs/exec-plans/completed/044-engine-testkit-foundation.md`、索引和架构圣经记录。
- 先新增集成测试并确认缺少公开 API 的红灯，再实现独立 `uc-testkit` crate。
- 完成身份、预算、阶段、事件等待、资源租约、失败分类、JSON/摘要和清理结果。
- 固定并实跑 cargo-nextest `0.9.145`；fast 组、JUnit、成功和故意失败示范均通过。
- 新增非破坏 CI job，保留原 checks、coverage 和 connection-recovery 门禁。
- workspace all-target check、Engine repository preflight 和 `uc-engine/public_contract` 48 项测试通过。
- 真实网络、设备与远程 CI 跳过；未修改或迁移 t-0010 场景。
