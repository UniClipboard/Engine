# 最终确认前不提交正式成员

## Goal

加入设备在最终确认前只保留为加密持久的待确认状态，不进入正式成员历史、安全通信集合或其他设备的活动成员视图；最终确认形成唯一正式提交点，并在丢包、重复消息、重启和混合版本下得到稳定、幂等且安全的结果。

## Ownership

- 完整负责人：Application `SpaceAdmissionProtocol`，内部由 Sponsor 负责候选与最终提交，Joiner 负责本机激活与最终确认，Recovery 负责丢包、重启和重复消息收敛。
- 调用方唯一动作：开始加入、处理认证消息、查询当前加入/成员状态；调用方不编排协议阶段。
- 成功：Sponsor 验证最终确认后一次提交正式 Add，随后安全效果与成员历史同步；Joiner 与 Sponsor 都保存稳定完成结果。
- 失败：正式提交前的协议/版本冲突保存稳定拒绝；正式提交后的网络中断保留可恢复完成状态，不回滚或自动删除。
- 重试与重启：由加密准入记录与既有恢复入口继续同一尝试；重复消息返回同一结果，不创建第二个成员。

## Test Seams

- Engine 完整 E2E：三台独立实例只通过公开加入、成员查询、重启和内容传送观察确认前后可见性。
- `SpaceAdmissionProtocol` 完整入口：使用现有持久仓库与消息入口注入最终请求/回复丢失、重复消息、双方重启和版本不兼容。
- Core aggregate：只覆盖唯一提交点所需的纯状态不变量，不直接测试私有辅助函数。

## Phases

- [completed] 盘点当前协议状态、持久格式、正式 Add 写入点、消息版本与现有脏改动。
- [completed] 建立三设备“确认前不可见、确认后可见”的红色完整回归。
- [completed] 逐项建立确认请求丢失、确认回复丢失、双方重启与重复消息回归。
- [completed] 实施唯一正式提交点与加密待确认状态的最小完整协议修复。
- [completed] 补同设备连续申请、旧异常历史、安全移除与混合版本行为。
- [completed] 验证普通加入、切换、移除再加入、内容双向传输和相关完整测试。
- [completed] 更新长期设计与架构圣经，清理临时探针。
- [completed] 执行严格只读审查；修复发现后再次审查。
- [completed] 运行全仓交付检查，整理并创建当前分支本地提交。
- [completed] 更新交付报告；真实设备验收保持未执行。

## Required Verification

- 每个切片先红后绿，记录实际失败点。
- 三设备确认前第三方不可见，确认后才可见且内容双向可用。
- 请求/回复丢失、Sponsor/Joiner 分别重启、重复消息均收敛。
- 同设备不同凭据不产生重复正式身份；旧异常成员只走明确安全移除。
- 混合版本在正式 Add 前稳定拒绝，或有证据证明安全兼容。
- 受影响 crate 完整相关测试与 Engine 多实例 E2E。
- `cargo metadata --locked --format-version 1`
- `cargo check --workspace --all-targets --locked`
- `cargo fmt --all -- --check`
- `node scripts/architecture/check-rust-style.mjs`
- `node scripts/architecture/check-engine-repository.mjs`
- `git diff --check`
- 临时诊断标记搜索无残留。

## Constraints

- 保留工作区全部既有修改；先查原始 diff，再编辑重叠文件。
- 不操作真实成员、手机资料或 Linux 空间。
- 不降低身份确认和邀请安全，不静默改写或删除旧历史。
- 不推送、不创建 PR、不发布；最终只创建本地提交。
- Cargo 串行使用共享 target 和现有 sccache。

## Errors Encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
