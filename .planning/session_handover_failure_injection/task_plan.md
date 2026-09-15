# Session Handover Failure Injection

## Goal

通过现有 Engine 操作和测试专用故障开关，验证 Space 会话交接失败后旧权限不复活、网络入口不双开，且故障解除后当前进程自动恢复。

## Next Step

继续规格 043 的其余故障边界与实体设备验收。

## Current Phase

Phase 5

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

## Decisions Made

| Decision | Rationale |
| --- | --- |
| 只通过公开 Engine 操作观察结果 | 测试不绑定私有实现，未来重构仍有效 |
| 测试开关只决定下一次会话准备是否失败 | 不改变生产协议、存储或公开接口 |

## Errors Encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
| 首次插入红测的尾部上下文名称与当前文件不一致 | 1 | 用实际相邻测试名称重新定位插入点 |
| 红测编译缺少故障动作与结果 | 1 | 这是预期红态；下一步实现 dev-only 一次性开关 |
| 加强后的测试在故障计数增加前观察到正常切换空窗 | 1 | 先等待注入消费计数为 1，再断言不可用与网络构造次数 |
