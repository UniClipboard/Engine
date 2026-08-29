# Spec 028 Cleanup Findings

## Current State

- `SpaceAdmissionAggregate` 已拥有类型化 admission 状态和取消转换。
- 旧取消、pending transition 与 current-join 投影已全部迁移，不再读取 membership ledger。
- `SqliteSpaceAdmissionState` 已加密保存新 Aggregate，并维护
  `current_local_join_id`。
- 现有 recovery load 会过滤无 recovery payload 的 Candidate，因此不能直接作为
  “读取当前加入”接口。
- `SponsorAdmissionSecurityDelivery` 原本只作为新安全 preparation 的传递载体；将等价但归属
  正确的 `PreparedMemberSecurityDelivery` 放入 Application 端口后，Core legacy 模块即可删除。
- 取消状态不能复用 recovery load：Candidate 没有 pending recovery，但仍是合法取消阶段。
- 新 `CurrentJoinAdmissionStatePort` 按 JoinId 读取并携带版本 token，Infra 在同一事务内
  校验 current pointer、record version 和密文替换；Facade 已不再读取旧 ledger。
- membership ledger 的 legacy 准入字段没有剩余生产消费者，字段、操作模块与旧 outbox
  已删除；空间重建继续只清理成员事实和效果。
- Core legacy `AdmissionRejectionReason` 的最后一个消费者是公开 current-join 投影；新协议
  枚举覆盖同一稳定原因集合，切换后不再阻止删除整个文件。

## Invariants

- `SpaceAdmissionProtocol` 是 start/cancel/handle/recover/complete 的唯一完整负责人。
- Application 只接收角色能力对象，不接收完整 Aggregate。
- 取消的读、版本校验、写和 current pointer 更新必须由同一个状态端口隐藏。
- 不改变持久化明文边界；业务负载继续经 MasterKey AEAD 加密。
- 错误分类必须保留 source chain。
