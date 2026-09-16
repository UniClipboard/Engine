# 设备关系选择交接：复现与根因诊断

本文记录诊断基线。后继已授权修复及当前验证状态见[修复记录](2026-09-07-device-group-choice-fix.md)；下文旧版复现命令的 ignore 开关已在后继实现中取消。

## 状态与边界

- 日期：2026-09-07。
- 任务：编写测试、复现交接中的问题、诊断根因；本轮不修改生产行为。
- 检查来源：交接包 `uniclip-engine-handoff-KhMYmN`，解压完整性通过；五份 JSONL 共 441 条，另有截图与交接文档。
- Engine 基线：`a82f566a68f07a2f7d80c6ab4a23cd65f8b7212d`，1.1.0-rc.6。
- 全部测试使用合成身份或独立临时配置，不操作现有 a/b/c/d 数据、选择或进程；保留 Desktop 原有未提交升级适配。
- 完成标准：真实路径产生明确失败、对照测试排除相邻原因、保存可重复命令、说明现场归因的证据限制。

## 结论

### 1. 接受移除的预览错误

完整链路：Application 查询当前可用关系，Engine 的
[`device_trust_snapshot`](../../../crates/uc-engine/src/operations/device/member.rs)
把同一份 `current_impact` 同时填入 `apply_impact` 与 `keep_current_impact`。
这里没有计算选择后的影响。Desktop 的
`src/components/device/device-trust-model.ts::getPendingDecisionView` 又把这个“当前可用”列表
当作“继续同步”，把移除目标独立当作“停止同步”，所以 c 同时出现在两侧。
`DeviceTrustDecisionContent.tsx` 对 apply/keep 优先使用这一条路径。

最小复现输入：本机 d，a/c/d 当前可用，b 等待本机决定，b 提议移除 c。
真实 Engine 映射返回 `apply_impact.usable_device_ids = [a,c,d]`。
另一个测试只把移除目标换成本机 d，预览仍返回 `Active`，而不是 `Removed`。

四个真实 Engine 实例通过公开操作建立同组，b 移除 c：a 接受后实际有 3 个成员，d 保留后实际有 4 个成员，
双方待移除事项已处理，实际目标资格分别为 Removed/Active。全部实例关闭后，事前预览与实际结果对照仍失败。
该测试使用真实存储、密码与 Iroh 链路，耗时 60.29 秒；不是对旧截图响应的重放。

另有公开 `DeviceGroupChoiceOptionSummary.member_device_ids`，但它表达候选成员而非已经可同步的设备。
该路径与旧 impact 不应继续作为两个独立事实来源。远端未知成员使用 `members_complete=false`，不能解释成已知空组。
本轮没有用 UI 删除重复名字来遮盖错误，也没有把“在同一组”与“已经恢复同步”混为一谈。

### 2. 拒绝移除后又出现一次分支选择

[`DecideDeviceTrustChangeUseCase`](../../../crates/uc-application/src/space/membership/decide_device_trust_change/use_case.rs)
保存本机拒绝决定、把提议方标成 Diverged，返回 KeptCurrentDeviceGroup；原 pending change 消失。
但它没有把这个选择与后续分歧事项的完成状态联系起来。

随后同一次移除的原始证据进入
证据处理入口（现位于 [`ReconcileMembershipEvidenceUseCase`](../../../crates/uc-application/src/space/membership/reconcile_history_evidence/use_case.rs)），
该入口通过分支规则识别分歧并创建 Unresolved 记录，没有识别“用户刚刚已经明确保留本机组”。
所以两个阶段分别成功，仍会要求用户第二次选择。

测试通过真实拒绝用例和真实证据接收入口验证：pending 从有到无，冲突从 0 到 1；期间没有新增成员操作。
这是 pending-change 到 branch-conflict 的两个不同事项，不是原 pending ID 被重新打开。
是否需要对未来真实新增分歧再次询问仍由独立新事实决定，不能一概屏蔽后续事项。

### 3. 已完成冲突因补入旧证据而生成新事项

[`MembershipConflictPolicy::branch_id`](../../../crates/uc-core/src/membership/membership_conflict_policy.rs)
同时使用当前 head、depth 和 `history_digest`。
[`current_position`](../../../crates/uc-core/src/membership/versioned_membership_history/history.rs)
的 digest 覆盖整个已知历史，包括没有应用到当前分支的 sibling 事件、其他决定和回执。
因此相同已应用分支可以因为“知道得更多”而获得另一个 branch ID，继而获得另一个 conflict ID。

`exchange_conflict_evidence` 只按精确 conflict ID 查找已存事项；新 ID 被插成 Unresolved。
旧记录仍然是 Completed，用户的选择并没有被删掉，但也无法阻止新事项出现。
这违反规格 030 的[同一组 heads 应得到同一冲突编号](030-membership-conflict-resolution-and-chaos-validation.md)约束。

最小序列：四成员基线；本地 a 移除 c，远端 b 移除 d；a 收证据后选择本机分支，事项清零；
b 仅补入 a 已知的那条移除 c 记录，b 的 head 和有效成员保持不变；再次送达 b 的证据，a 重新出现 1 个待处理事项。
测试同时检查旧事项仍 Completed、本机分支标识不变、远端分支标识变化。

四个对照均通过：完全相同证据重复三次不重现、另一合法证据来源转发同一分支不重现、重建 ledger/处理对象不重现、
远端真正新增一次移除则产生独立新事项。Application 定向用例复用现有内存仓储和接受签名的测试替身，
覆盖真实历史规则、用例、条件提交和查询，不验证密码算法。重建测试复用内存中保存的事实，不能冒充磁盘崩溃恢复或实体设备重启验收。

### 4. Desktop 按钮持续加载

`src/contexts/DeviceTrustContext.tsx` 中 choose 的完成查询与通知刷新共用 `refreshSequenceRef`。
通知刷新递增序号后，choose 在自己的查询完成时直接 return，跳过 `choice_finished`。
`finally` 仅清理 `decisionBusyRef`，没有清理渲染状态中的 `decisionBusy`；刷新完成也不会清理它。

真实 React Provider 测试控制两次查询的返回顺序。通知查询先返回、后返回两种情况都稳定失败：
提交已完成、查询全部结束、事项为 0、普通 loading=false，但 decisionBusy=true。
无并发通知的对照正常。该测试是 Provider 集成测试，不宣称已重新录制真实产品弹窗。

## 现场证据能支持到哪里

- a 的两次 Completed 后，revision 39 的两次查询均为 0 个事项，revision 40 又为 1；原日志与交接叙述相符。
- 原现场没有 issueId/choiceId 或相应分支证据，不能确认第一次是 pending 还是 conflict，也不能确认最后一条就是同源冲突。
  本轮确认的是当前实现中两条足以导致该现象的确定性路径，不把合成输入冒充现场完整历史。
- c/d 的旧记录中证据交换成功，同时 presence 因成员资格不被认可而拒绝；不能归因于网络不可达。
  这段是升级前的状态，不能据此声称新版通知仍缺失。本轮没有重放旧二进制或改动 c/d。

## 测试入口与实际结果

新增 12 个用例：7 个在期望行为处失败，5 个对照正常。失败用例保留真实期望，未反转断言来宣称通过。
Rust 已知失败用例用 `ignore` 标记，Desktop 用显式环境变量启用；默认运行的跳过不是通过。
修复时应移除对应开关并将用例纳入正常回归。

| 位置 | 覆盖 | 实际结果 |
| --- | --- | --- |
| [Engine 映射测试](../../../crates/uc-engine/src/operations/device/member.rs) | 移除对端、本机的预览 | 2 个预期行为断言失败，均已复现 |
| [拒绝后的再次选择](../../../crates/uc-application/src/space/membership/decide_device_trust_change/tests.rs) | 拒绝后收到同一次移除证据 | 1 个预期行为断言失败，已复现 |
| [冲突生命周期测试](../../../crates/uc-application/src/space/membership/handle_history_message/tests/handoff_reproduction.rs) | 迟到证据、重发、第三方转发、重建、新移除 | 1 个预期行为断言失败，4 个对照通过 |
| [四实例完整操作](../../../crates/uc-engine/tests/space_membership_auto_pairing_e2e.rs) | 真实查询、接受、保留、结果核对和关闭 | 预览断言失败，操作后真实成员核对通过 |
| Desktop `src/contexts/__tests__/DeviceTrustContext.handoff.test.tsx` | 真实 Provider，两种返回顺序及无并发对照 | 2 个预期行为断言失败，1 个对照通过 |

Engine 根目录执行，三个复现命令在当前版本应退出 101：

```bash
cargo test -p uc-engine --lib handoff_apply_preview --locked -- --ignored --nocapture
cargo test -p uc-application --lib handoff_ --locked -- --include-ignored --nocapture
cargo test -p uc-engine --features dev-tools --test space_membership_auto_pairing_e2e handoff_four_device --locked -- --ignored --nocapture
```

Desktop 根目录执行，启用两个失败用例时应退出 1：

```bash
UNICLIP_HANDOFF_REPRO=1 npx --yes --package=node@24 node node_modules/vitest/vitest.mjs run src/contexts/__tests__/DeviceTrustContext.handoff.test.tsx
```

Node 22 的现有环境在启动 jsdom worker 时遇到 ESM 加载错误，尚未运行测试；使用 Node 24 后实际执行成功并得到上述失败。
未修改产品依赖或锁文件。关联的旧 Provider 5 项测试均通过。

## 交付验证

- Application 成员相关测试：91 passed，2 ignored；这 2 项另以显式命令执行并确认失败。
- Engine 全部库测试：146 passed，2 ignored；这 2 项另以显式命令执行并确认失败。
- Desktop 默认执行旧 Provider 与新增文件：6 passed，2 skipped；显式复现执行为 6 passed、2 failed。
- 全工作区全目标检查、格式、架构与隐私检查、锁定依赖、两仓改动检查均通过；保留既有警告。
- 新增 Desktop 测试格式及静态检查通过；诊断文档 10 个相对链接均存在。
- 实体设备、产品页面全流程、生产观测导出：跳过；不扩大本轮测试结论。

## 后续修复责任

- Application 负责唯一完整的选择预览与执行语义，Engine 只映射，Desktop 统一消费；分别表达候选成员、可同步关系和本机移除。
- Core 的稳定分支身份应与“已知证据集合的增长”区分；Application 将本机已有明确选择与后续同源事项关联，保留真正新冲突的选择权。
  不得仅按成员集合去重，也不得让旧恢复包因放宽身份核对而获得新的授权。
- Desktop 保证一次选择请求的结束必定清理自己的忙碌状态，过期查询的丢弃不能跳过请求结束处理。
- 当前归因缺口不能通过记录明文设备、冲突、分支 ID 来补齐；任何新增观测须另行遵守既有隐私合同。
- 本轮只交付测试与诊断，以上生产修复尚未实施。
