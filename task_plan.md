# Spec 028 Legacy Admission Cleanup

## Goal

按既定顺序把 Space admission 的剩余生产入口迁移到
`SpaceAdmissionProtocol` / `SpaceAdmissionAggregate`，随后删除旧 ledger 状态、
`SponsorAdmissionSecurityDelivery` 和整个 `space_join_record` 模型。

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

## Current Slice

取消、当前状态和待完成激活均已收口到 `SpaceAdmissionProtocol`；membership ledger 中的
`admission_records`、`admission_profile` 与旧 outbox 已删除。

## Next Step

全部切片与验收完成；旧模型没有生产代码引用，仓库门禁通过。

## Errors Encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
| 根目录计划仍描述更早的 WorkspaceConvergence 重构 | 1 | 按当前持续目标替换为 Spec 028 清理计划。 |
| 同一个 patch 同时删除并新增计划文件被拒绝 | 1 | 分成删除与新增两个 patch。 |
