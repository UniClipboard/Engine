# Task Plan: Architecture Deepening Spec

## Goal
在现有架构契约基础上，为五项已识别的结构深化工作编写一份可由 Coding Agent 分阶段实施和验收的 active exec-plan。

## Next Step
任务完成；后续由用户审阅规格 036 并决定是否启动 Slice 1。

## Current Phase
Phase 5

## Phases

### Phase 1: Requirements & Discovery
- [x] 确认用户要求把五项架构建议写成正式 spec
- [x] 对齐仓库文档约束和现有计划格式
- [x] 复核五项候选的源码证据、所有权与依赖关系
- **Status:** complete

### Phase 2: Spec Design
- [x] 确定计划编号、状态、完整负责人和阶段边界
- [x] 定义接口方向、失败结果、恢复责任与非目标
- **Status:** complete

### Phase 3: Documentation
- [x] 编写 active exec-plan
- [x] 更新 active index 和 architecture bible 维护记录
- **Status:** complete

### Phase 4: Verification
- [x] 核对 spec 结构、相对路径和架构契约
- [x] 运行仓库规定的非行为改动检查
- **Status:** complete

### Phase 5: Delivery
- [x] 检查工作区只包含预期修改并保留用户原有改动
- [x] 向用户交付 spec 路径、核心结构和检查结果
- **Status:** complete

## Decisions Made
| Decision | Rationale |
|----------|-----------|
| 使用一个总计划、五个有依赖顺序的工作流 | 这些工作共享“深化模块、减少调用者复杂度”的目标，但不能作为一次原子大改实施 |
| 计划放入 `docs/exec-plans/active/` | 内容是尚未实施的具体工作，不是当前架构事实 |

## Errors Encountered
| Error | Resolution |
|-------|------------|
| 一次 `apply_patch` 因目标行上下文顺序错误未应用 | 读取当前文件后改用精确上下文补丁 |
| 两次组合补丁因跨文件后续 hunk 上下文不匹配只应用了前段 | 重新读取落盘状态，后续将规划状态更新独立成单文件补丁 |
