# 生成资料

此目录保存可从代码、迁移或生成工具重建的资料。生成物帮助理解，但不取代源码和迁移。

- [Engine 操作跨层调用链对照](engine-operation-map.md)：逐操作列出契约枚举、路由、适配函数与应用入口，由 `scripts/architecture/render-engine-operation-map.mjs` 生成。
- [数据库 Schema 快照](db-schema.md)：从 Diesel schema 提取的当前表清单。
- [数据库关系图](database-er-model.html)：2026-08-12 的可视化快照，可能落后于最新迁移。
- [ADR-020 成员核对说明图](adr-020-membership-reconciliation-explained.html)：决策说明图。

更新生成物时必须同步记录来源与生成日期；无法确认新鲜度时明确标为快照。
