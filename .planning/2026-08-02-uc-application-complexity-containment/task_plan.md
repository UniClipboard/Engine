# uc-application 复杂度收口总计划

## Goal

让 `uc-application` 对剪贴板入站、文件传输、移动上传、搜索、历史维护和成员恢复分别负责完整结果，并最终关闭 `AppFacade` 的内部绕过路径。

## Completion Criteria

- `spec.md` 中每项功能都有唯一负责人和明确结果。
- `uc-engine` 不再编排应用内部步骤、失败补偿和后台重试。
- 生产应用对象不存在半装配状态。
- 每次长流程都有唯一终态和明确关闭行为。
- 稳定 Engine operation、结果和错误码保持兼容。
- 旧入口和旧测试被删除，不保留两套实现。
- 自动化检查通过；未执行的设备项目明确记为跳过。

## Dependency

前置计划 `.planning/2026-08-02-space-setup-deps-design/` 已完成 Phase 0 至 Phase 8，并通过稳定契约、应用层、真实 Engine 场景和全仓交付检查。本计划的前置条件已经满足。

本计划已完成 Phase 1 至 Phase 5，并保持每个完整功能独立提交；后续继续按当前阶段推进，不提前合并剩余工作。

## Current Phase

Phase 0 至 Phase 7 全部完成

## Phases

### Phase 0：完成空间收口前置计划

- [x] 完成空间计划 Phase 0 至 Phase 8
- [x] 确认搜索和成员活动已归空间会话统一负责
- [x] 完成稳定契约、应用层、真实 Engine 场景和全仓交付检查
- **Status:** complete

### Phase 1：固定剩余功能行为

- [x] 固定剪贴板入站行为
- [x] 固定移动上传和文件传输终态
- [x] 固定历史维护顺序和失败策略
- **Status:** complete

### Phase 2：收口搜索运行期

- [x] 删除半装配和运行中补装
- [x] 让搜索随空间会话自动恢复和暂停
- **Status:** complete

### Phase 3：收口剪贴板入站模式和运行期

- [x] 建立明确完整的入站模式
- [x] 将接收、应用、确认、事件和关闭移入应用层
- [x] 删除引擎层入站循环
- **Status:** complete

### Phase 4：收口文件传输和移动上传

- [x] 建立唯一终态的文件传输会话
- [x] 将活动上传和全部失败清理移入应用层
- [x] 删除引擎层上传状态管理
- **Status:** complete

### Phase 5：收口历史维护

- [x] 将顺序、间隔和失败策略移入历史功能
- [x] 删除引擎层维护循环
- **Status:** complete

### Phase 6：收紧应用总入口

- [x] 隐藏内部对象
- [x] 删除重复入口和半就绪构造
- [x] 保持每个 Engine operation 只有一条应用调用路径
- **Status:** complete

### Phase 7：删除与整体验收

- [x] 删除旧实现和旧测试
- [x] 增加防止复杂度重新外溢的架构检查
- [x] 完成自动化和设备矩阵记录
- **Status:** complete

## Errors

| Error | Attempt | Resolution |
|---|---:|---|
| None | 0 | - |
| 追加 Phase 1 较广验证记录时补丁上下文不匹配 | 1 | 重新读取 `progress.md` 尾部并按准确位置追加，首次尝试未产生文件改动 |
