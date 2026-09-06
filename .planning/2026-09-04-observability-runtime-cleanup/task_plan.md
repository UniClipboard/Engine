# Task Plan: Observability Runtime Cleanup

## Goal

在保持宿主公开用法不变的前提下，修正本地日志收尾、丢失统计和超时语义，并按完整负责人整理 `uc-observability-runtime`。

## Next Step

向用户汇报已完成的整理、验证结果与并行改动边界。

## Current Phase

Phase 5

## Phases

### Phase 1: Current-State Audit

- [x] 读取重叠文件的未提交差异
- [x] 区分用户/其他 Agent 改动与本任务缺口
- [x] 固定文件所有权与删除清单
- **Status:** complete

### Phase 2: Red Tests

- [x] 为本地日志真实 flush/stop 写失败测试
- [x] 为完整丢弃统计写失败测试
- [x] 确认 deadline 是“调用方有界等待、后台尽力完成”，不增加取消语义
- **Status:** complete

### Phase 3: Minimal Behavior Fixes

- [x] 让本地日志成为完整 owner
- [x] 修正 flush/shutdown 与 health 语义
- [x] 保持外部接口兼容
- **Status:** complete

### Phase 4: Internal Restructure

- [x] 按进程生命周期、远程管道、输出装配拆分内部实现
- [x] 删除测试支持与生产装配的混杂
- [x] 更新架构维护记录
- **Status:** complete

### Phase 5: Verification And Commit

- [x] 运行 crate 测试、格式、架构门禁与 diff 检查
- [x] 审查最终差异并确认存在同文件并行改动
- [x] 不创建提交，避免把无法安全拆分的并行改动一起纳入
- **Status:** complete

## Decisions Made

| Decision | Rationale |
|---|---|
| 保持宿主公开接口不变 | 当前混乱位于内部所有权，不应转嫁给调用方 |
| 不增加通用 port/trait | 每类输出目前只有一个真实实现，额外抽象没有收益 |
| 先修行为再搬文件 | 避免把错误语义原样分散到更多文件 |

## Errors Encountered

| Error | Attempt | Resolution |
|---|---:|---|
| 默认 `target` 路径不可用 | 1 | 后续验证使用独立 `CARGO_TARGET_DIR` |
| 并行改动使 `Cargo.lock --locked` 暂时不一致 | 1 | 先审查并行差异，验证时使用与当前工作区一致的安全方式 |
| 单个补丁同时删除并重建 `runtime.rs` 被工具拒绝 | 1 | 改为新增文件、替换入口、接线三个原子步骤 |
| 格式化后的语句导致多文件补丁上下文不匹配 | 1 | 按当前文本拆成更小补丁，失败尝试未落盘 |
| Android 交叉编译缺少 NDK clang | 1 | 标记未执行；没有把环境失败算作代码失败 |
| HarmonyOS 交叉编译缺少系统头文件 | 1 | 标记未执行；没有把环境失败算作代码失败 |
| 安全策略拒绝递归删除临时 target | 1 | 验证精确目录后使用逐项删除并确认不存在 |
| 计划自检脚本没有执行权限 | 1 | 使用 `sh` 显式运行同一脚本 |
