# Engine 构建目录修复发现

## Requirements

- 修复失效的 Engine 默认构建目录。
- 清理近期 Codex 会话产生且已不再使用的临时构建结果。
- 阻止多 Agent 为每项任务在内置盘创建独立构建目录。
- 保留现有源码、用户改动、日志、个人文件和虚拟机。

## Research Findings

- 仓库 `target` 是指向 `/Volumes/ExternalSSD/cargo-targets/active/uni-engine` 的符号链接，目标目录缺失。
- 最近 11 个 Codex 会话显式指定了 35 个临时构建目录；现存可再生临时结果超过 120 GiB。
- 近期命令经常显式清空 `RUSTC_WRAPPER`，使外置 `sccache` 无法跨目录复用。
- 规格 037 的主会话和并行子 Agent 是当前最大来源。
- `/Users/mark/bin/cleanup-uniclip-storage.sh` 会拒绝清理活动目录、符号链接、源码、日志和个人数据；它不会选择外置 `active/uni-engine` 根目录。

## Technical Decisions

| Decision | Rationale |
|----------|-----------|
| 在 `scripts/architecture/` 增加构建目录检查 | 让默认路径失效和内置盘回退成为可重复检查的失败 |
| 根级 `AGENTS.md` 规定 Cargo 验证所有权 | 问题来源是 Agent 执行策略，需要在入口约束 |
| 恢复现有链接目标，不改仓库链接 | 现有链接位置正确，只缺目标目录 |
| 使用既有保护脚本清理 | 它已有路径白名单、活动进程和打开文件检查 |

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| 工作区已有大量未提交修改 | 只修改根级规则、新检查和架构维护记录，不碰其余文件 |
| 另一个 037 会话曾持续编译 | 清理前重新检查进程；有活动构建就不执行删除 |
| 全仓格式检查失败 | 失败只涉及另一项未完成工作的四个 Rust 文件，本轮不修改 |
| 仓库总检查失败 | 新构建目录门禁通过；后续被另一项未完成工作的隐私自测失败拦截 |
