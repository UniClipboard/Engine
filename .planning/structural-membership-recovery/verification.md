# 设备组恢复修复验证

日期：2026-09-07。Engine 基准为 fe55e543，以下修复尚未提交。使用同级 Desktop 的专用四/五配置，没有操作个人 a/b/c/d。

## 修复范围

1. 当前已验签历史是成员资料维护的唯一依据。Application 生成完整维护计划，Infra 在一次事务内检查历史版本并维护成员、可信身份和地址；删除旧的历史移除清理模块。
2. 历史效果日志与当前可执行效果分离。换组时由完整目标账本构造负责保留审计事实、丢弃旧传输进度与确认，并限定当前分支的执行资格。
3. V4 增量交换携带发送方归档证明范围；验证发送方的真实历史，不要求接收方删除合法旁支。V2 签名历史及完整证据字节不变，旧账本迁移保留历史和决定，仅退役旧在途交换。
4. 资料不足与真正无效分开处理。当前有效成员可以通过完整验签重新核对；正文同步不能在验证前放行。旧回复不能覆盖换组后的关系，新传输与迟到旧页由同一入站状态转换管理。

所有参与端需要使用新的 membership-history/4 协议。此次没有验证 Windows、移动端或新旧协议混合运行，不承诺旧版对端可继续交换成员历史。

## 先红后绿

下列测试均实际观察到断言失败后修复为通过；编译失败及零测试运行不计红测。

| 测试 | 原失败与最终保障 |
| --- | --- |
| `selected_branch_keeps_active_members_despite_old_completed_removal` | 当前组有效成员被旧分支移除记录删除；真实 SQLite 验证成员保留，并覆盖缺失资料重建与过期计划不得删除 |
| `old_branch_incomplete_effect_does_not_pause_a_current_member` | 旧分支未完成任务暂停当前成员；任务资格限定到当前历史 |
| `suffix_preserves_verified_receiver_side_branches_and_local_decisions` | 合法旁支导致 InvalidPersistedHistory；保留接收方事实并验证发送方归档 |
| `encrypted_legacy_rows_upgrade_to_v4_on_commit` | 写入仍为版本 3；验证旧版本读取及版本 4 密文写回 |
| `invalid_current_peer_recovers_only_after_complete_verified_evidence` | 错误 Invalid 被永久跳过；完整证据核对可恢复，损坏证据仍不放行 |
| `suffix_cannot_apply_records_outside_the_sender_proof` | 重算传输摘要后可以夹带证明范围外记录；现在整批拒绝且无部分变更 |
| `old_history_reply_cannot_clear_a_new_branch_divergence` | 迟到确认清除新分支分歧；旧历史回复不再修改新状态 |
| `changed_transfer_replaces_staged_pages_without_quarantining_the_member` | 新首帧遇旧暂存被判无效；新请求可取代旧请求，旧后续页不破坏新请求 |
| `deferred_projection_is_revisited_by_periodic_maintenance` | 投影暂时失败后定期维护不再执行它；改为每轮本地核对，不依赖重启或新网络事件 |

真实 Engine F2 增加远端选择后重启与完整成员查询，但它在修复前也通过，因此只作为回归，不冒充根因红测。

## 已通过的定向检查

- Core 成员历史集成：41 项。
- Application 成员相关：103 项；新增分页替换测试另有 1 项定向通过。扩大 Application 单元测试：757 项。
- Ledger 单元组：22 项。
- V3 分支切换每阶段崩溃重放、成员投影契约、旧账本迁移分别实际执行通过。
- `cargo check --workspace --all-targets --locked` 通过。
- `node scripts/architecture/check-engine-repository.mjs` 通过，包括隐私检查与负例。
- Desktop 后台与真实 E2E GUI 从当前 Engine 构建通过。
- `cargo test --workspace --exclude uc-engine --lib --tests --locked --quiet` 完整通过：2792 通过、4 项原有忽略；日志 `.cache/membership-recovery-checks/workspace-without-engine-tests-final.log`。
- Engine F2 单独通过：1 项、163.97 秒。测试按指定设备的公开候选选择，不再把第一个远端候选误认为目标组；保留完整成员、影响、重启与隔离断言。
- 最后补充定期投影恢复后，Application 758 项完整通过，其中维护与调度 9 项；Desktop 前端 180 文件、1142 项通过，后台及 GUI 已重新构建，最终窗口矩阵重新运行。

## 实际窗口与旧资料恢复

路径以下均相对同级 Desktop 根目录，每轮包含实际程序校验值、选择记录、截图与结果。

| 场景 | 完整通过证据 |
| --- | --- |
| 四配置不同移除，选远端组，实际收发与重启查询 | `.cache/device-group-e2e/run-mtr7ink4/` |
| 四配置加入全新第五台，逐项选择，A→E 收发及排除关系验证 | `.cache/device-group-e2e/run-mtr7spcm/` |
| 四配置旧失败现场克隆恢复，实际收发 | `.cache/device-group-e2e/run-mtr8kd1j/` |
| 五配置旧失败现场克隆恢复，逐项选择与实际收发 | `.cache/device-group-e2e/run-mtr97qhm/` |

旧现场只复制到新测试配置，复制前后校验原资料。五配置早一轮业务通过但测试清理非零，不计完整通过；清理工具已有先红后绿回归，并在上表最后一轮以退出码 0 复验。

## 扩大验证边界

全仓测试存在一条已证明的原版本失败：`testing::host_adapter_contract::engine_clipboard_inbound_preserves_success_duplicate_and_shutdown_behavior`。关闭同步后重发的错误分类为 NoEligibleTargets，而旧断言要求 SynchronizationDisabled。

已用干净 fe55e543 源码和 dev-tools 真实执行 1 项，复现同样失败；临时源码树已移除，共享外置构建缓存保留。原测试没有本轮残留修改，不修改无关重发规则，也不将跳过该项称为全仓通过。

第一次扩大多实例运行有失败且被中断，不计全仓通过。单独重跑 F2 时确认候选顺序假设错误，修正后通过。

其余五项采用串行复验：F0/F1/F4/F5 通过；F7 首次失败在把 `total_pending=1` 当作发送失败。既有发送合同在有界等待后将未完成目标交给后台，F1 已覆盖同一语义，因此改为同组允许 accepted 或 pending 后仍必须收到原文，跨组 accepted 和 pending 总和必须为 0，不将 pending 单独计作通过。

F7 单独再次通过，1 项、417.06 秒，覆盖十实例所有不同设备对（90 对），同组实际接收，跨组无接受或待发送。日志 `.cache/membership-recovery-checks/f7-final.log`；F0/F1/F4/F5 的串行通过记录在 `.cache/membership-recovery-checks/topology-serial.log`，该文件同时保留 F7 修正前失败，不改写成全绿。

Desktop 全场景与系统键盘验收以其 `.planning/device-group-choice-gui/verification.md` 为准。
