# Docs 维护地图

本目录是仓库知识记录系统。修改文档前先读 [`README.md`](README.md) 和
[`design-docs/documentation-system.md`](design-docs/documentation-system.md)。

## 目录

- `design-docs/`：长期设计、稳定契约、分层规范和 ADR。
- `exec-plans/active/`：设计中、实施中、待实现或阻塞的计划。
- `exec-plans/completed/`：完成或明确被取代的实施历史。
- `generated/`：可从代码、迁移或工具重新生成的资料。
- `product-specs/`：产品问题、需求、范围和验收标准。
- `references/`：领域词表、来源映射等查询资料。
- `DESIGN.md` 等顶层文件：主题入口，不复制深层正文。

## 修改规则

- 项目文档使用中文；代码标识符保持英文。
- 仓库路径使用相对路径，链接指向唯一事实来源。
- ADR 被取代后保留原文件并更新状态，不删除或重新编号。
- Active plan 完成后回写稳定结论，再移入 `completed/`。
- Generated 内容必须标明来源与时效，不手工宣称它比代码更新。
- 不确定的历史原因必须明确标成推断，不能编造。
- 修改仓库内容时同步更新 `architecture/architecture-bible.md` 的正文或“文档维护记录”。

交付前检查 Markdown 相对链接、孤儿文档、旧路径残留和 `git diff --check`。
