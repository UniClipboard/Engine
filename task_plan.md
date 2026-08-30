# Spec 029 Durable Membership History Anti-Entropy

## Goal

按 `docs/specs/029-durable-membership-history-anti-entropy.md` 一次性实现逐 peer 认证水位、
MasterKey AEAD 持久欠账、摘要/suffix 交换、有界公平重试和入站多跳 fan-out，并恢复
Desktop 多节点复杂拓扑收敛。

## Completion Criteria

- `MembershipHistoryAntiEntropy` 是 Application 唯一完整负责人。
- 历史、effects、peer 水位与 fan-out 欠账通过同一 ledger CAS 原子提交。
- 只有认证 ACK 推进对应 peer 水位；失败、预算耗尽和重启不丢欠账。
- 链式、树型、离线恢复和公平调度测试通过。
- Desktop 三节点及复杂拓扑 E2E 通过；未执行实体设备项目明确标为跳过。
- Spec、架构圣经、workspace、格式、架构和 diff 门禁全部通过。

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
- [x] 使用本地 Desktop 的公开 CLI/daemon 跑通 fresh join、状态查询与 daemon 重启恢复 E2E。
- [x] 跑通三 profile Active 收敛及双向 exact transfer，并同步最终交付结论。
- [x] 完成 Spec 029，固定复杂拓扑下的持久成员历史反熵设计与验收矩阵。
- [ ] Core 摘要、历史关系和 ACK 水位规则。
- [ ] Ledger 持久同步状态、原子 mutation 与迁移。
- [ ] Application 单一反熵负责人、重试和公平调度。
- [ ] Infra typed summary/suffix wire clean cutover。
- [ ] 所有历史写入点与入站 fan-out 闭环。
- [ ] 多节点拓扑、真实 SQLite/Iroh/Desktop E2E。
- [ ] 审查、全量门禁和原子提交。

## Current Slice

Group Epoch 持久投递负责人、Desktop C1 验收、最终审查、全量门禁与 Engine 原子提交已完成。

## Next Step

- Engine 切片已完成并原子提交；下一步仅处理 Desktop 独立仓库的 CLI/E2E 变更分割与提交。

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
