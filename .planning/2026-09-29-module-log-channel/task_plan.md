# 任务计划：模块日志通道与错误链

目标：按 `library/engine-observability-framework-handoff.md`（SHA256 2db2f0db…）实施 P0–P3。基线 c7a821b4，无既有尝试产物。

## 用户决定（2026-09-29）
- A 未知外部错误层：opaque 占位。 B 标准级别：INFO（Detailed 窗口 DEBUG）。
- C 诊断导出：默认包含模块日志。 D：P1 审计前仅测试/开发构建启用。
- E 类型识别：增量登记 + lint（spike：context 层不可 downcast，仓库约 325 个错误类型）。

## 阶段
- [ ] P0 先写失败 E2E（日志文件→诊断导出）→ Sensitive / 登记宏 / 模块日志层 / 独立配额 / 固定原因拒绝错误
- [ ] P1 审计计数、包装敏感值、lint 规则；审计后再评估 release 启用
- [ ] P2 移除 LocalCompletionDetail 本地链路与 error.call_path 重复；迁移 instrument(err)
- [ ] P3 多 zip 时间线合并脚本；改写 observability.md / error-handling.md；exec plan 收尾
