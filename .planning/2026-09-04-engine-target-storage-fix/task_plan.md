# Engine 构建目录修复计划

## Goal

恢复 Engine 的外置构建目录，清理未使用的临时构建结果，并阻止后续 Agent 再把任务级构建目录散落到内置盘。

## Next Step

向用户汇报修复结果与现有工作区验证边界。

## Current Phase

Phase 3

## Phases

### Phase 1: 失败基线

- [x] 证明默认构建路径当前失效
- [x] 证明仓库尚未禁止任务级内置盘构建目录
- **Status:** complete

### Phase 2: 最小修复

- [x] 恢复外置构建目录
- [x] 增加构建路径检查
- [x] 更新 Agent 规则和架构维护记录
- **Status:** complete

### Phase 3: 清理与验证

- [x] 确认无构建进程后清理临时结果
- [x] 运行真实 Cargo 检查并确认写入外置盘
- [x] 运行仓库交付检查并复核磁盘空间
- **Status:** complete

## Decisions Made

| Decision | Rationale |
|----------|-----------|
| 不修改现成清理脚本 | 脚本不会选择或删除外置盘 `active/uni-engine` 根目录 |
| 不使用任务级 `/tmp` 构建目录 | 这是近期重复占用的直接来源 |
| 并行 Agent 共享默认目录并串行执行 Cargo | 避免每个 Agent 保存一套完整结果 |

## Errors Encountered

| Error | Attempt | Resolution |
|-------|---------|------------|
| 首次读取 TDD skill 使用了错误的 `.system` 路径 | 1 | 按 skill 清单改读 `/Users/mark/.codex/skills/test-driven-development/SKILL.md` |
| 更新计划时使用了已经改变的旧文本 | 1 | 重新读取计划并按当前内容更新 |
| 更新进度时补丁上下文匹配失败两次 | 1 | 拆成小补丁并按当前行精确更新 |
| 负向验证使用 zsh 只读变量 `status` | 1 | 改用 `exit_code` 后通过 |
