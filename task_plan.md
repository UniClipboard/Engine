# Spec 028 Legacy Admission Cleanup

## Goal

按既定顺序把 Space admission 的剩余生产入口迁移到
`SpaceAdmissionProtocol` / `SpaceAdmissionAggregate`，随后删除旧 ledger 状态、
`SponsorAdmissionSecurityDelivery` 和整个 `space_join_record` 模型。

在实现切换完成后，继续建立 Spec 028 的自动化、真实基础设施、设备和发布交付证据；
未执行的设备项目必须明确记录为“跳过”。

## Completion Criteria

- cancel、current status、pending transition 均由 `SpaceAdmissionProtocol` 完整负责。
- 生产代码不再读写 legacy `admission_records`。
- `SponsorAdmissionSecurityDelivery` 被类型化新协议结果替代。
- `space_join_record` 及其旧 codec、错误和 re-export 删除。
- 架构圣经同步更新；workspace、格式、架构和 diff 检查通过。

## Phases

- [x] 删除无生产调用者的旧 Core 恢复模型、旧恢复用例和旧消息处理器。
- [x] 迁移 cancel/status/pending transition 到新 Aggregate。
- [x] 删除 ledger `admission_records`。
- [x] 替换 `SponsorAdmissionSecurityDelivery` 并删除 `space_join_record`。
- [x] 更新文档并执行全量验证。
- [x] 对照 Spec 028 建立验收证据矩阵并修正文档状态。
- [x] 运行完整 workspace 测试与 Engine/绑定 contract tests。
- [x] 运行真实 SQLite、Iroh loopback、Engine 双实例与明文探针。
- [x] 记录实体设备矩阵和 Release bundle 的通过或跳过状态。
- [x] 完成交付审计并同步架构圣经。
- [x] 删除旧 pairing ALPN、session/event port 与 production Router 装配，同时保留邀请 discovery。
- [x] 扩展架构检查并更新 Spec 028、架构圣经与验收状态。
- [x] 运行定向测试、完整 workspace 测试和最终门禁并提交清理切片。

## Current Slice

执行 Spec 028 clean-cutover：把邀请 discovery 从旧 pairing session adapter 中拆出，删除
`/uniclipboard/pairing/2`、旧 session/event port、wire 和生产 Router handler。

## Next Step

- clean-cutover 清理已完成；下一项仅剩实体设备/三设备验收，需在具备设备的环境执行。

## Errors Encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
| 根目录计划仍描述更早的 WorkspaceConvergence 重构 | 1 | 按当前持续目标替换为 Spec 028 清理计划。 |
| 同一个 patch 同时删除并新增计划文件被拒绝 | 1 | 分成删除与新增两个 patch。 |
| workspace 测试中 `config_migration_round_trip_e2e` 两项导出失败，报告必需配置文件缺失或不可读 | 1 | 测试夹具遗漏现行 `.current-space-id-v1` 且仍断言退休 `.setup_status`；修正后 2 项通过。 |
| Engine 双实例 E2E 默认运行 0 项；启用 `dev-tools` 后两个 legacy DevOperation 调用已删除 AppFacade 方法而编译失败 | 1 | 正在审计 DevOperation 是否应删除或映射到新产品入口；不得恢复旧 convergence facade。 |
| 新 Engine E2E 中 fresh join 稳定停在公开 Pending 并于 60 秒超时 | 1 | 已确认 Application/Infra 后台能推进准入，但运行中的 Engine session 没有消费 pending Space transition；本切片由 SessionSupervisor 收口。 |
| 架构脚本在清理后读取已删除的 `pairing/session.rs` | 1 | dual-invitation 日志检查改为读取职责正确的 `pairing/invitation_resolver.rs`。 |
| 架构脚本把工作区残留的空目录判为旧源码 | 1 | 回归门禁改为逐个禁止可被 Git 跟踪的旧模块文件。 |
