# Task Plan: Admission Expiry S1

## Goal
实现 S1：一次 Join 从 Engine 接收操作起共享固定五分钟期限；尚未进入正式决定的 Joiner 尝试可在本机到期、取消或被新尝试取代；结果使用现有加密准入仓储原子保存并可在重启后恢复。

## Completion Standard
- 公开 Join/Cancel/Query 能观察 Pending 到 Terminated 的稳定结果。
- 299999ms 仍 Pending，300000ms 本机 Expired。
- 网络不可用不阻止取消或到期；A 结束后 B 可立即开始。
- 短邀请码只兑换一次，解析时间消耗原期限。
- 旧记录继续读取且不伪造期限；不新增数据库表或明文字段。
- 真实临时 userdata 重开后期限和终止结果保留。
- 相关测试、workspace check、格式与架构门禁通过；完成严格代码审查并提交。

## Test Seams
- Engine Join/Cancel/Query 公开结果。
- `SpaceAdmissionProtocol` 完整 Joiner 动作与恢复入口。
- 加密准入仓储关闭后重开。

## Phases

### Phase 1: Contract tracer
- [x] 为 Core 五分钟窗口、到期转换和旧记录读取写失败测试
- [x] 实现最小生命周期模型与版本化保存
- **Status:** complete

### Phase 2: Join and local termination
- [x] Engine 采样起点并传入 Application
- [x] Start 原子保存新尝试和旧终止结果
- [x] Cancel 对安全阶段直接本机结束
- **Status:** complete

### Phase 3: Recovery and deadline wake
- [x] 恢复在网络动作前执行到期判断
- [x] 后台按准确截止时间单次唤醒，启动/恢复时重建定时
- [x] 短邀请码解析前后遵守原期限且不重复兑换
- **Status:** complete

### Phase 4: Persistent end-to-end proof
- [x] 使用临时真实 userdata 验证 299999/300000、重开、A/B 替换和失败原子性
- [x] 验证旧尝试不会复活且普通数据不受影响
- **Status:** complete

### Phase 5: Documentation, review, verification, commit
- [x] 更新执行计划、ADR 状态与架构圣经
- [x] 严格审查完整 diff 并修正发现
- [x] 运行定向测试、完整测试和仓库交付检查
- [x] 创建本地原子提交
- **Status:** complete

## Decisions Made
| Decision | Rationale |
| --- | --- |
| 复用 `local_join_ordinal` 作为意图代次 | 已是持久单调序号，避免重复字段。 |
| 生命周期放在 admission 聚合根而非每个阶段 | 所有阶段共享同一期限，避免字段复制和遗漏转换。 |
| 新增独立记录格式并保留旧 decoder | 不向旧 postcard 布局直接加字段。 |
| 只为可本机安全结束的阶段暴露 S1 到期转换 | 不提前实现正式决定后的撤销。 |
| 准确截止由现有维护 runtime 持有单次定时器 | Runtime 只负责唤醒，业务判断仍由准入恢复负责人完成。 |

## Errors Encountered
| Error | Resolution |
| --- | --- |
| 首次从 `~/.codex/skills/tdd` 读取失败 | 使用实际可用的 `~/.agents/skills/tdd`。 |
