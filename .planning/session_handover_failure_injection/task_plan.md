# Session Handover Failure Injection

## Goal

通过现有 Engine 操作和测试专用故障开关，验证 Space 会话交接失败后旧权限不复活、网络入口不双开，且故障解除后当前进程自动恢复。

## Next Step

后续在实体设备上验收真实远端大文件切换；本轮本机故障测试已完成。

## Current Phase

Phase 12

## Phases

### Phase 1: Discovery
- [x] 盘点现有 dev-tools 探针和完整交接路径
- [x] 选定第一个公开行为测试 seam
- **Status:** complete

### Phase 2: Red Test
- [x] 写一个会话准备失败后自动恢复的真实双端红测
- [x] 证明红测因缺少故障恢复能力而失败
- **Status:** complete

### Phase 3: Minimal Implementation
- [x] 增加最小测试专用故障注入能力
- [x] 修复红测暴露的生产恢复缺口
- **Status:** complete

### Phase 4: Failure Matrix
- [x] 覆盖故障期间旧权限关闭和单网络入口
- [x] 覆盖连续恢复失败、故障解除后成功
- **Status:** complete

### Phase 5: Verification and Commit
- [x] 更新规格与架构圣经
- [x] 运行定向、性能和完整交付检查
- [x] 审查差异并提交
- **Status:** complete

### Phase 6: Remaining Failure Discovery
- [x] 确认持久提交失败、发布失败和切换前失败的可注入边界
- [x] 明确每个边界的恢复结果和公开观察方式
- **Status:** complete

### Phase 7: Remaining Failure TDD
- [x] 逐个完成红测、最小实现和绿色验证
- [x] 覆盖旧权限、单网络和恢复结果
- **Status:** complete

### Phase 8: Regression and Commit
- [x] 复测三秒目标并运行完整交付检查
- [x] 更新规格与架构记录
- [x] 审查并提交独立变更
- **Status:** complete

### Phase 9: Cancellation and In-flight Discovery
- [x] 确认切换取消的真实触发方式和期望终态
- [x] 选择一个可控、真实的在途业务请求
- **Status:** complete

### Phase 10: Cancellation TDD
- [x] 先写公开行为测试并确认当前行为
- [x] 确认无需生产修复即可安全恢复
- **Status:** complete

### Phase 11: In-flight Request TDD
- [x] 先覆盖一个真实在途请求的切换结果
- [x] 验证旧请求终止、新会话可用且网络不重建
- **Status:** complete

### Phase 12: Regression and Commit
- [x] 复测三秒目标并运行完整交付检查
- [x] 更新规格和架构维护记录
- [x] 审查并提交独立变更
- **Status:** complete

## Decisions Made

| Decision | Rationale |
| --- | --- |
| 只通过公开 Engine 操作观察结果 | 测试不绑定私有实现，未来重构仍有效 |
| 测试开关只决定下一次会话准备是否失败 | 不改变生产协议、存储或公开接口 |
| 后续仍使用公开 Engine 操作作为唯一测试 seam | 延续已确认的端到端边界，不直接测试私有方法 |
| 本轮沿用已确认的公开 Engine 操作 seam | 用户要求继续既有故障测试；测试专用入口只负责制造时机和读取匿名计数 |

## Errors Encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
| 首次插入红测的尾部上下文名称与当前文件不一致 | 1 | 用实际相邻测试名称重新定位插入点 |
| 红测编译缺少故障动作与结果 | 1 | 这是预期红态；下一步实现 dev-only 一次性开关 |
| 加强后的测试在故障计数增加前观察到正常切换空窗 | 1 | 先等待注入消费计数为 1，再断言不可用与网络构造次数 |
| 新故障矩阵因缺少统一故障点类型、动作和计数而无法编译 | 1 | 预期红态；下一步实现统一测试专用故障控制 |
| 持久切换完成失败被分类为不可重试，后台 watcher 停止恢复 | 1 | 已主动停止长等待；检查真实错误类型和 Engine 转换规则 |
| 读取完成错误文件时使用了已被目录化替代的旧路径 | 1 | 改读 `complete_pending_space_transition/error.rs` |
| 新会话发布失败后第一次清理完成，但后台恢复超过预期且网络中继超时 | 1 | 已主动停止长等待；检查准备会话的关闭所有权是否误伤长期网络或共享状态 |
| 追加发现时 patch 上下文与当前文件不完全一致 | 1 | 读取当前内容后使用完整行重新更新 |
| 单次发布失败仍无法恢复，进程采样未保留可读异步调用栈 | 1 | 已停止测试；改用仅开发测试启用的阶段计数定位等待位置 |
| 注册表拒绝测试引用了未导入的错误类型 | 1 | 按编译提示从同一私有模块集中导入并使用短名称 |
| 在途文件发送红测返回成功而不是取消 | 1 | 取消与操作结果同时就绪时选择无优先级；先让会话取消确定性优先，再继续验证临时资源收尾 |
| 增加取消优先级后红测仍返回成功 | 1 | 三秒读取在 Space transition 真正开始前已结束；把读取窗口增至七秒，确保覆盖配对交换与两段排空等待 |
