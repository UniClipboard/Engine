# Progress Log

## 2026-08-30

- 已删除无生产调用者的旧 Core 恢复模型、旧 outbox recovery use case 和旧 admission message handler。
- Core Aggregate 已能从 Candidate/Prepared 构造合法 `CancelRequested`，并覆盖阶段、序号、前驱证据和路由测试。
- 当前开始迁移 `CancelSpaceJoinUseCase`，随后依次迁移状态查询和 pending transition。
- 已完成取消入口迁移：`SpaceAdmissionProtocol` 负责 profile 串行、当前 JoinId 校验、
  Core 取消转换、条件提交和维护唤醒；旧 `CancelSpaceJoinUseCase` 及测试已删除。
- 新 Infra state port 继续使用加密 admission repository，并以 profile generation、
  admission id 和 record version 生成条件提交 token。
- 当前加入状态查询、pending transition 查询与完成已迁移到 `SpaceAdmissionProtocol`；
  最终激活返回类型化稳定结果，不再读取 legacy join record。
- membership ledger 已删除 `admission_records`、`admission_profile`、旧 join-record 操作和
  legacy admission outbox seam；成员账本定向测试 15 项通过。
- 安全投递改由 Application 新协议端口的 `PreparedMemberSecurityDelivery` 表达，公开拒绝
  状态改用 `SpaceAdmissionRejectionReason`；Core `space_join_record` 模块、codec、错误、导出
  和自有测试已整体删除。

## Verification

- 最近 Core admission tests：77 passed。
- 最近 Application library tests：685 passed（删除旧 handler 后）。
- Application 取消测试：2 passed。
- Core admission state tests：43 passed。
- `cargo check -p uc-application --all-targets --locked` 通过。
- `cargo check -p uc-infra -p uc-engine --all-targets --locked` 通过。
- ledger 清理后再次执行上述 Infra/Engine check，通过。
- ledger 定向测试：15 passed。
- 删除 Core legacy 模型后，`cargo check --workspace --all-targets --locked` 通过。
- 新 aggregate 77 项、Application Joiner 协议 12 项和三条 Infra Sponsor 安全路径测试通过。
- 最终 `cargo metadata`、workspace all-target check、fmt check、架构 preflight 与 diff check
  全部通过；架构检查器同步禁止 legacy handler/outbox/ledger/Core 模型回归。
- `git diff --check` 通过。
