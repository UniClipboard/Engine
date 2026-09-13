# Findings: Admission Expiry S1

## Current implementation
- `JoinerStartStatePort::commit` 已在一个 SQLite immediate transaction 内保存旧 replacement、新 record、current/latest 指针和本机 ordinal；S1 应扩展这条事务，不新增仓储。
- 物理仓储已经是逐 record 密文保存的 V3 表结构；Core admission record payload 仍是 V1。S1 只需增加独立 Core record V2 decoder/encoder，不需要数据库 migration。
- `current_local_join_id` 在 terminal commit 后清空，`latest_local_join_id` 保留最后结果，正好支持终止后不占槽位但仍可查询。
- 当前 `cancel_before_authentication` 把本机取消保存成 Rejected(Cancelled)；S1 要改为 Terminated(Cancelled)。
- 当前 `SpaceAdmissionSupersededState` 已保存按阶段所需的最小证据；公开查询尚未把它映射为 Terminated(Superseded)。
- 短邀请码已有 Ready -> Started -> Resolved 持久转换，重启遇到 Started 会拒绝而不再次兑换；S1 在这些转换前后加入同一期限判断。
- 恢复流程在 Startup/Resume/Periodic/StateChanged/PeerOnline 运行，但 Periodic 固定 30 秒，不能独自满足精确五分钟。
- 维护 runtime 已集中拥有暂停、恢复、关闭和定时任务，适合持有单个 admission 截止定时器；它只能发触发信号，不能判断到期业务。

## Safety boundaries
- S1 不到期终止 Prepared 及以后阶段，不追加正式 Remove。
- 查询保持只读；必须先持久化 terminal，再展示 Terminated。
- 旧 V1 record 解码为 LegacyNoDeadline，不猜测开始或截止时间。
- 日志和调试输出不得出现 admission id、join id、邀请、时间组合或私密材料。
