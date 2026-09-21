# 045 确定性成员恢复高价值场景

## 状态

- 状态：实施中
- 日期：2026-09-21
- 完整负责人：既有 `SpaceAdmissionProtocol`、`MaintainSpaceMembershipUseCase`、成员历史证据与设备信任用例
- 唯一调用：测试场景提交一个业务动作或一次明确维护触发，再读取公开 Application 投影判断终态
- 成功结果：五个固定 seed 场景在显式预算内得到预期公开终态，并写出 testkit JSON 与人类摘要
- 失败结果：稳定失败分类、未满足条件、最后事件、阶段耗时、复现命令和工件位置
- 重试责任：产品重试仍由既有 Application 负责人决定；场景只推进可控时间、受控消息和明确维护轮次
- 重启责任：场景销毁并重建 Application 负责人，复用同一持久测试仓储；不得复制恢复决策
- 长期边界：[Engine 测试架构](../../design-docs/testing-architecture.md)与[规格 034](034-deterministic-virtual-peer-network-test-suite.md)

## 1. 问题与目标

044 已建立 runner、testkit 与报告基础，但尚未证明它能承载成员准入和恢复中的高价值业务场景。现有责任层测试分别验证协议步骤，分钟级真实网络测试验证交付链路；两者之间缺少可快速重复、能跨负责人读取最终公开结果的确定性场景。

本阶段只覆盖五个已明确的风险：

1. 最终确认首次失败后重试，且普通成员维护不得越过准入恢复。
2. 旧候选按已完成、可证未完成、证据不足三类收敛。
3. 三设备在最终确认前不可见，确认后只出现一个有效成员。
4. 设备状态更新区分可重试失败、需处理和恢复后的公开状态。
5. 停止并重建 Application 后从持久状态继续，而不是从测试内存继续。

## 2. 非目标与边界

- 不读取、修改、迁移、复制或重写 t-0010 工作区及其测试。
- 不删除、改名或降低既有责任层测试的权威地位；新场景先与旧测试双轨。
- 不接入真实 Iroh、外网、固定 `sleep`、环境变量、机器绝对路径或设备。
- 不实现规格 034 的完整多协议虚拟网络、拓扑 DSL、随机故障或虚拟多节点产品运行期。
- 不增加生产公开接口、持久格式或业务行为；必要 seam 仅位于 `cfg(test)` 且保持 crate-private。
- 不让 testkit 或场景解释 admission、membership、history 或 trust 状态机。

## 3. 架构边界与目录职责

```text
crates/uc-application/src/space/testing/
  scenario.rs       使用 uc-testkit 统一身份、预算、事件与工件
  admission.rs      受控协议交换和可重建持久 fixture
  membership.rs     维护轮次、公开投影与固定 seed fixture
  scenarios.rs      五个端到端业务场景，不保存业务状态
```

实际实现可在不扩大职责的前提下合并小文件。测试模块只组合既有负责人：

- `SpaceAdmissionProtocol` 拥有最终确认、持久检查点和恢复。
- `MaintainSpaceMembershipUseCase` 固定准入恢复优先级；场景不得逐步骤调用后续维护。
- 成员历史负责人解释候选证据；场景只提供合法输入并读取结果。
- `QueryDeviceTrustUseCase` 或成员名单查询提供最终公开设备投影。
- 重建场景复用仓储对象，重新构造负责人；不得复制 persisted record 到第二套模型。

`uc-application` 仅增加 `uc-testkit` 的 dev-dependency。生产依赖图与发布产物不包含 testkit。

## 4. 场景合同

| 场景 | 真实负责人 | 最终公开断言 | 单场景预算 | 旧测试对照 |
| --- | --- | --- | --- | --- |
| final-confirmation-retry | admission recovery + maintenance | 首次失败后仍待确认；重试后 settled；失败轮没有普通维护调用 | 1 秒 | `activation_is_retried_from_the_saved_plan_after_commit_conflict`、`session_transition_stops_the_current_maintenance_round_after_admission` |
| legacy-candidate-convergence | history evidence owner | completed 不重开、provably incomplete 进入待处理、insufficient evidence 保持不可提升 | 1 秒 | `handoff_reconstructed_owner_preserves_completed_choice`、`verified_candidates_explain_changes_and_keep_remote_sync_pending`、`rejected_evidence_does_not_create_display_facts` |
| three-device-confirmation-visibility | admission + public trust/roster projection | CompleteAck 前新成员不进入可用清单；确认后恰好一个对应成员可见 | 2 秒 | `complete_ack_is_saved_before_the_sponsor_returns_settled` 与现有 trust/roster 过滤测试 |
| device-state-retry-attention-recovery | device trust decision owner | 暂时失败保持可重试；稳定问题显示需处理；恢复后无待处理且投影可用 | 2 秒 | `decide_device_trust_change` 现有失败与恢复测试 |
| restart-from-persistent-state | admission recovery owner | 重建前保存的阶段由新负责人继续到 settled；旧负责人内存不是成功条件 | 2 秒 | `settled_is_saved_and_finishes_joiner_recovery` 与重建持久记录测试 |

五场景一次完整执行目标少于 10 秒。每个场景使用静态名称、固定 seed、显式 wall-clock 保护预算和逻辑时间；消息只通过受控 transport/port 交付，不用真实网络或轮询睡眠。

## 5. 报告与复现

每个场景使用 044 的 `uc-testkit::Scenario`：

- 工件根固定为仓库相对目录 `target/test-artifacts/membership-recovery`；运行标识由场景名、seed 和显式轮次组成，不读环境变量。
- `result.json` 保存结果、失败分类、最后事件、阶段耗时、清理状态和复现命令。
- `summary.txt` 提供相同证据的人类摘要。
- 事件只使用稳定类别，不记录设备名、标识、邀请、地址、payload、密钥或真实路径。
- 复现命令固定为精确测试过滤；压力轮次由测试参数或独立精确测试入口表达，不依赖环境变量。

## 6. 实施顺序

### Phase 0：规格和失败清单

- [x] 映射五个场景到既有 Application 负责人和旧测试。
- [x] 明确产品失败、环境失败、fixture/driver/framework 失败边界。
- [x] 确认本阶段不需要生产公开接口或第二套业务状态机。

出口：本文、索引和架构维护记录通过 `git diff --check`；形成独立文档提交。

### Phase 1：最小端到端场景

- [ ] 先实现 `final-confirmation-retry`，测试先于任何 test-only seam 修改。
- [ ] 用受控消息与维护入口证明失败、重试和不插队。
- [ ] 读取生成的 JSON/摘要并核对失败工件字段。

回退：删除新测试模块和 dev-dependency；生产代码不需回退。

### Phase 2：其余四场景

- [ ] 逐一加入候选收敛、三设备可见性、设备状态恢复和持久重建。
- [ ] 每次只增加当前场景所需的最窄 `cfg(test)` fixture 能力。
- [ ] 每个场景独立复现且总耗时小于 10 秒。

### Phase 3：双轨与稳定性

- [ ] 五个新场景与对应旧测试各连续运行至少 20 次，比较最终业务结果。
- [ ] 五场景整体连续运行 100 轮，无随机失败。
- [ ] 旧测试源码和默认 `cargo test` 入口保持不变。

### Phase 4：完整验证与提交

- [ ] 运行 `uc-testkit` 自测、新场景、相关旧测试和旧 cargo test 入口。
- [ ] 运行 workspace all-target check、fmt、Rust style、Engine repository、隐私和 `git diff --check`。
- [ ] 记录命令、次数、耗时、工件和未达标项。
- [ ] 按 branch-name-guard 检查后创建范围清晰的本地原子提交；不推送、不创建 PR。

## 7. 失败分类

实现前固定以下失败方式：

- 产品不变量：确认前可见、确认后重复、维护越序、重建丢失持久进度或错误提升证据。
- 产品超时：受控消息和明确维护轮次已经推进，公开终态仍未达成。
- fixture 无效：构造出的领域记录不能通过现有 constructor/codec/verification。
- driver protocol：漏投消息、错误顺序、等待不存在的稳定事件。
- framework artifact：JSON 或摘要无法创建、拒绝覆盖策略冲突。
- 环境/资源：共享 target 不可用或磁盘不足；不得改用 `/tmp` 绕过。

所有错误文本和工件必须脱敏；产品失败不得被后续工件失败覆盖。

## 8. 兼容、风险与回退

- 测试支撑若必须暴露业务内部阶段，停止实现并重新收窄到完整负责人动作或公开投影。
- 若某场景只能依赖真实网络，本阶段标为未达标，不把真实 Iroh 引入 fast lane。
- 若 100 轮因工件目录冲突失败，修正运行标识或显式清理测试工件；不得关闭拒绝覆盖保护。
- 若五场景超过 10 秒，先定位阶段耗时，不删除业务断言或扩大并发掩盖问题。
- 文档提交、场景实现和必要 testkit 增量分别可独立回退；任何回退都不修改旧测试。

## 9. 验收标准

- [ ] 五个场景都调用既有 Application 真实负责人并断言最终公开状态。
- [ ] 使用固定 seed、可控时间、可控消息，无固定 sleep、环境变量、机器路径和真实网络。
- [ ] 每个场景生成 JSON 与摘要，失败包含复现命令和工件位置。
- [ ] 对应旧测试双轨至少 20 次，最终业务结果一致。
- [ ] 五场景总耗时小于 10 秒，连续 100 轮无随机失败。
- [ ] cargo test 与 nextest 双轨可运行，旧测试权威地位不变。
- [ ] 静态、架构与隐私检查通过；真实网络、远程 CI 和设备明确记为跳过。

## 10. 进度记录

| 日期 | 阶段 | 结果 |
| --- | --- | --- |
| 2026-09-21 | Phase 0 | 完成场景到既有负责人的映射；确认只需 test-only 支撑，不改变生产行为。 |
