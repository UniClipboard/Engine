# 发现：单设备 Space 重建的锁竞争与 Staged 续做

来源：上游调查报告（t-0086）。基线 `origin/main` = `ad40c041`，已包含 rc.19 `348a69eb`。
`348a69eb..ad40c041` 只改动 iroh 准入诊断，本故障路径的代码没有变化。

## 调用链

`LocalSessionReadiness::prepare_data` → `UpgradeSpaceUseCase::execute` →
`RebuildSpaceUseCase::execute`：`prepare` → `stage` → `rebuild` → `commit(promote)` → `finalize`。
`ResetSpaceUseCase` 复用同一个 `RebuildSpaceUseCase`。

- `prepare`（`V3DeviceManagementReset`）：日志阶段 Allocated → Prepared（生成目标 control generation 快照并记录摘要）。
  当前 Staged 分支直接返回 `Ok(())`。
- `stage`：Prepared → 重新校验摘要，`control_pool.replace_database(target)`，日志改为 Staged。
  Staged 分支只切换连接池。**目标目录缺失时，`replace_database` 会新建一个只有表结构的空库。**
- `rebuild`：会话重绑到目标 Space；`MembershipOwner::reset`（`clear_space`，唤醒成员维护）；
  清除关系、删除远端成员、保存本机成员；`InitializeSpaceMembershipUseCase` → `bootstrap_legacy_space`
  （begin/stage/activate 写目标控制库；`install_current_material` 把新组的内容密钥合并进 profile vault 文件）。
- `commit`：`finalize_device_reset_target` → `rebind_registration_to_control_generation`
  （裸连接，没有 busy_timeout，所有错误都被 `.map_err(inconsistent)` 处理）→ checkpoint → 回读安全材料 → 摘要 →
  `activate_device_reset`（持 activation 租约，`reopen_prepared` 校验摘要，提升 manifest）。

## 写入去向（子代理逐项核实，未编译）

| 步骤 | 写入位置 |
| --- | --- |
| 会话重绑 | 仅内存 |
| 账本清空 / 重建（`membership_ledger_state` 与读模型） | 目标控制库（经被切换的 `control_db_executor`） |
| 关系、成员 | 目标控制库 |
| bootstrap begin/stage/activate、epoch 材料 | 目标控制库 |
| `install_current_material` | **profile vault 文件** `profile-content-key-vault-v1.json`（在目标之外） |
| 进度目标、re-pairing、版本 | 分别在 prepare、finalize、升级结束时写入；rebuild 期间不写 |
| 成员维护轮次 | 准入状态写 profile 库；组更新写控制库；可能发网络消息 |

## Staged 目标可安全重建的依据

1. 来源 control generation 仍由 manifest 引用，并且没有被修改（stage 之后连接池已离开来源库）。
   profile 数据库、blob、MasterKey、keyslot 都不在重置的依赖图中。
2. 目标控制库 = 来源快照 + 本次重建的派生结果。丢弃后重新快照，得到的是同一逻辑起点。
3. vault 以 protection_group_id（bootstrap id）为键，**只追加，没有任何删除入口**；content key id 随机生成。
   重做 bootstrap 会得到新组，与上次尝试的组并存。上次尝试期间若有内容用那个被放弃的 epoch 密钥加密，
   仍可按 content key id 解出，历史不会丢失。
   代价：每次放弃的尝试占用一个组位（上限 128）。已受影响用户只多占一个组位。
4. 只有在确认“manifest 仍指向来源、且日志属于当前来源”（`journal_matches`）之后，才允许丢弃；
   manifest 已指向目标时，prepare 和 stage 会提前返回，绝不触碰目标。
5. 不依据“epoch 是否存在”判断完成与否；legacy bootstrap 的全局语义保持不变。

## 崩溃边界

| 边界 | 现状 | 目标行为 |
| --- | --- | --- |
| Staged，目标已改写（本故障） | 重做 rebuild → `space already has key epoch material`，永久失败 | 切回来源 → 删除目标 → 日志回到 Allocated → 重新快照 |
| 已删除目标、日志仍为 Staged | stage 打开空库，**可能提升一个缺失凭据与成员的库** | 同上：目标不存在即视为已删除，继续重新快照 |
| 日志已改回 Allocated、快照未发布 | Allocated 分支重新快照（原有行为） | 不变；必须保证“先删目录、后改日志” |
| manifest 已提升、日志仍为 Staged | `already_committed` 路径直接 finalize → “not cleanup pending”，永久失败 | Application 先 promote（契约保证幂等）再 finalize |
| Promoted / CleanupPending | 原有的前向恢复 | 不变 |

## 并发

- 成员维护在 `prepare_data` 之前就已启动；升级路径不像 ResetSpace 路径那样先停止运行期。
- 只加 busy_timeout 不够：checkpoint 遇到活动读写会返回 Busy；摘要计算之后的写入会破坏 `reopen_prepared` 的提升证明。
- 现有机制：`MaintainSpaceMembershipUseCase::execution_lock` 覆盖整轮维护。
  （会话级的 `pause/resume` 是布尔状态，与锁定流程的暂停相互干扰，不采用。）
  重建流程负责人在 prepare…promote 期间持有这把锁，对应的 port 由维护负责人提供。

## 重试边界

- `operation_error_with_code(1103, "recover local session")` 固定为 Internal、不可重试。
- 需要一条带类型的分类链：`LocalSessionReadiness` 区分暂时不可用与失败 → `RecoverSpaceSessionError` 增加
  Unavailable → supervisor 映射为 `1103 Unavailable retryable=true`。
  commit 阶段的 Busy 和激活租约冲突属于暂时不可用；Inconsistent/RecoveryRequired 仍不可重试。

## 待红测试（尚未编译或运行）

| 测试 | 旧行为下的预期失败 |
| --- | --- |
| Infra `restart_rebuilds_a_staged_target_from_the_unchanged_source` | 目标仍保留第一次的材料 |
| Infra `retry_in_the_same_process_rebuilds_the_staged_target_from_the_source` | 同上 |
| Infra `a_missing_staged_target_is_rebuilt_from_the_source_instead_of_opened_empty` | stage 出错，或得到缺少来源材料的空库 |
| Infra `an_activated_target_is_finished_instead_of_discarded` | 预期旧行为下也通过（保护性回归） |
| Infra `commit_waits_for_a_short_concurrent_writer` | rebind 立即得到 locked → Inconsistent |
| Infra `a_held_writer_lock_is_reported_as_retryable_and_the_commit_can_be_retried` | 返回 Inconsistent |
| Application `a_committed_target_is_promoted_again_before_finalization` | 调用序列缺少 promote |
| 矩阵 `d1-l0194-head` | 预期通过（正常升级不回退） |
| 矩阵 `d1-l0194-head-commit-interrupted` | 恢复阶段 `host-start-failed`；事实中 retryable=false |
| （待写）维护互斥 | 需要新 port，旧代码上以编译失败为红 |
