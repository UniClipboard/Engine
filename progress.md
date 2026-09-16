# Progress Log

## 2026-08-30 — Spec 029 durable anti-entropy

- 新增 `DeliverPendingGroupUpdatesUseCase`：每轮最多处理 8 条持久密钥欠账，只在 Iroh 认证 Accepted ACK 后删除；临时失败保留并持久轮转，避免多设备时队尾饿饿。
- Engine 已在 Router spawn 前安装 group-update ALPN，并将 dispatch/store 注入同一成员维护 runtime。
- Desktop C1 真实三进程验收通过：A/B/C 收敛三成员，A→C 与 C→A exact transfer 均通过（65.71s）。所有 `[DEBUG-029-*]` 临时根因日志已删除。
- 复盘 C1 密钥失败后确认：B 已持久生成面向 A 的 epoch 3 group update，但生产没有 Application 消费者，Engine 也未安装已存在的 Iroh group-update handler。
- Sponsor 激活保留认证 ACK 水位，不再为旧成员伪造新 head 确认。
- 周期维护改由逐 peer 水位欠账驱动；加密 ledger 保存 retry、退避和公平游标。
- 出站单轮限制八个 peer，游标在网络调用前持久化；入站合并同事务建立 fan-out 并在提交后唤醒维护。
- 成员历史传输切换到 v3 summary/request/ACK 握手与 v3 Iroh ALPN。
- Core、Application receiver 与 scheduler 定向测试正在通过；workspace 和 Desktop 验收待执行。

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

- 本切片最终 `cargo metadata`、workspace all-target check、fmt、架构检查、diff check 和完整 workspace tests 全部通过；Infra 718 passed/4 ignored，Application 688 passed，Engine 118 passed。
- 清理诊断后重新构建 Desktop CLI/daemon，C1 再次通过（66.15s）。
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

## 2026-08-30 交付验收

- 已确认代码级 Engine 接入和 legacy 删除完成，但 Spec 028 的实体设备、完整 E2E、明文探针
  与发布证据尚未全部建立。
- 开始按自动化、真实基础设施、实体设备和 Release bundle 四个证据边界逐项验收。
- `cargo test --workspace --all-targets --locked --no-run` 完成，全部 workspace、Engine、Infra、
  UniFFI、HarmonyOS、兼容线和宿主测试目标均成功构建；耗时约 2 分 53 秒。
- 首次完整 workspace 测试运行至 Engine：Core/Application 与 Engine lib tests 通过；随后
  `config_migration_round_trip_e2e` 的 2 项测试均在 export 阶段因必需配置文件缺失失败，
  完整测试因此尚未通过。
- 已修复配置迁移 E2E 夹具漂移：改为携带现行 `.current-space-id-v1`，删除无生产消费者的
  `.setup_status` 断言；同一测试目标 2 项均通过。
- 真实 SQLite admission state 23 项、认证 9 项和 OPAQUE RFC 向量 1 项通过。
- Engine 双实例 E2E 默认因 `dev-tools` 门禁运行 0 项；按正确 feature 启用后发现两个旧
  DevOperation 仍调用已删除的 convergence/removal facade，测试目标无法编译。
## 2026-08-30 Phase 5 交付验收：Engine E2E 迁移

- `dev-tools` 编译失败不是新准入运行时回归，而是旧 `QueryWorkspaceConvergence` / `DecideMembershipRemoval` 操作仍引用已删除的 facade。
- 按 Spec 028 的 clean-cutover 要求退役旧收敛测试契约，将 Engine E2E 收敛为 2 个稳定入口场景：新设备加入/已有设备切换，以及完成准入后重启并传输正文。
- `--list` 已确认 2 个测试非零且可编译。
- 首次实跑因测试 `EngineConfig` 使用非语义版本字符串而在 P2P assembly 前失败；已改为 `1.1.0`，待重跑。
- 已将 Sponsor 侧也切换到正式 `Operation::IssueInvitation`，并把 fresh join、Space switch、重启传输拆成独立反馈环。
- fresh join 可将 admission aggregate 推进到非拒绝 terminal，但运行中的 Engine session 不会消费异步激活并重建，公开 setup 状态持续未完成，60 秒稳定超时。
- 针对 admission current 指针的试验性保护不能修复公开状态，已撤销；下一切片修复 `SessionSupervisor` 的异步 transition 触发，而不是改变持久化槽位语义。
- 已增加由全局 `TaskRegistry` 管理的 Engine transition watcher；它只观察稳定 pending transition，所有改变由 `SessionSupervisor` lifecycle lock 内完成。
- 已移除普通 admission maintenance 对 `recover_activation()` 的抢占；activation 只由显式 `complete_pending_space_transition()` 完成。
## 2026-08-30（继续 Phase 5）

- 已增加 Sponsor 单一激活端口：在 Complete 回复提交前幂等安装安全状态、AddDevice 成员事实和正式历史；错误保留 source，并把暂时失败归为 recovery required。
- Joiner generation 提升前现在写入 MasterKey AEAD 加密的正式 membership ledger，本机成员门禁与 peer reconciliation 从已验证历史构造。
- 定向 Infra/Engine all-target check 通过；重启 E2E 已越过 roster/scope 错误，但仍因重启后 relationship store locked 导致内容传输超时。
- 修复冷启动 session 安装不尝试 keyring 恢复的问题；普通启动恢复失败仍可保持锁定，准入 transition 后继续严格要求解锁。
- 修复跨 Space 目标数据库已有 source ledger 时错误假设 revision 为 0；现在读取实际 revision 与历史摘要后原子替换。
- Engine 三条稳定准入 E2E 连续两轮通过；workspace all-target check、metadata、fmt、架构 preflight 与 diff check 通过。
- Joiner 目标 generation 安装测试 9 项通过；新增准入表 migration 后，修正两个按末尾序号回滚的旧迁移测试，定向 5 项通过。
- 完整 `cargo test --workspace --all-targets --locked` 通过；Infra 749 项通过、4 项明确 ignored，Engine、绑定、宿主、兼容线与明文探针测试均通过。
- 当前环境未发现 Android/HarmonyOS 调试工具或实体设备，也没有 Release bundle；iOS、Android、HarmonyOS 使用仓库 skipped matrix 明确记为“跳过”，Release 核验同样跳过。

## 2026-08-30 Spec 028 clean-cutover

- 开始清理旧 pairing transport；已确认新 admission endpoint 已独立存在，但 Engine 仍生产装配 `/uniclipboard/pairing/2`。
- 清理边界确定为：保留短码、完整邀请和地址 discovery，删除旧 session/event/wire/ALPN handler 与兼容探测。
- 已删除 Infra pairing session/wire、Core session/event ports 与旧消息模型；workspace all-target check 通过。
- 架构脚本首次运行因仍读取已删除的 session 文件失败，已迁移检查目标到 invitation resolver。
- Node 定向测试 31 项通过、1 项环境型 ignored；Engine 三条 admission E2E 通过。
- 架构检查新增 retired pairing transport 负向夹具并通过；Spec 028 clean-cutover 条目与关联规格状态已同步。
- 完整 `cargo test --workspace --all-targets --locked` 通过；删除旧 Core/Infra pairing transport 后 Infra tests 从 753 项收敛为 721 项，删除的是旧协议专属测试。
- metadata、workspace all-target check、fmt、架构与 diff 门禁全部通过；clean-cutover 以 `08db918` 提交。

## 2026-08-30 Desktop CLI E2E

- 修复短码路由格式校验、`ResolvedInvitation → Initiated` 持久化恢复与维护唤醒；Application 回归测试覆盖跨维护轮次推进。
- Desktop Tokio E2E 改为多线程 runtime，避免同步 CLI 子进程阻塞本地 rendezvous mock；daemon 使用 `e2e-rendezvous` feature 构建。
- 真实 Desktop `fresh_join_recovers_the_desktop_session` 与 `join_commands_report_none_then_real_active_join` 已通过；Engine metadata、workspace all-target check、fmt、架构检查和 diff 门禁通过。
- 三节点 `c1_online_sponsor_chain_converges_and_syncs_directly` 仍失败：A 只保留 A/B 两名成员，未收敛 C，因而尚未执行 exact transfer；这属于下一处成员历史传播缺口，不能把当前切片标记为完整 Desktop 交付。
- Desktop workspace all-target check 跳过：Tauri 构建要求缺失的 `binaries/uniclipd-aarch64-unknown-linux-gnu` sidecar；CLI/daemon 两个目标已单独成功构建。

## 2026-08-30 Spec 029

- 完成 `docs/exec-plans/active/029-durable-membership-history-anti-entropy.md`，固定逐 peer ACK 水位、加密持久欠账、摘要与缺失 suffix、入站 fan-out 和有界公平调度。
- 验收覆盖链式、树型、交错在线、预算耗尽、ACK 丢失、重启和合法分叉；本轮仅完成设计，生产语义尚未切换。
- 开始按 Spec 029 实施；确认 TDD seam 为 Core planner、Application 反熵入口、Infra typed transport 和 Desktop E2E。

- 已确认 `desktop` 仓库通过 Cargo patch 使用当前本地 Engine，而非固定远端 revision。
- 已找到真实 CLI/daemon E2E harness；下一步构建二进制并执行新准入、重启与互传场景。
- 修复 Desktop 嵌套 checkout 下 E2E crate 错认 Engine workspace 的边界，并将旧 unpair 返回值迁移到 `DeviceTrust`；Desktop CLI/daemon 构建通过。
- `cli_engine_workflows` 5 项真实进程测试均运行但失败；三 profile exact-transfer 场景在第二节点 session locked 超时，冷重启仍为 423，尚未进入正文互传。

- 复核 Core 终态与 view：当前诊断输出对应 `RecoveryRequired`，纠正了此前对 Active 终态的怀疑。
- 保留 focused E2E 红测作为复现入口，下一步定位 recovery category 与触发分支。
- focused E2E 进一步确认首次 `Initial` 建链返回 `ProtocolRejected`，尚未收到任何准入回复；正在区分服务端 protocol/application 分支。
- 修正路由双重编码、Sponsor continuation 凭证读取和已有 Sponsor admission 的阶段解析后，focused E2E 已能完成 `Candidate → Commit → Complete → Settled`。
- 当前最后红点：`complete_pending_space_transition` 后旧 facade 查询到正确目标 Space，但 `install_new_session` 构建出的 facade 回到 source Space 且 `re_pairing_required=true`；根因范围已收敛到 session factory 重建仍复用启动时依赖图/旧 profile 视图。
- 通过区分 legacy current-space identity 与 active-generation manifest，修复现代准入目标被错误执行 profile-isolation rebuild；fresh join E2E 已通过。
- 删除 `JoinSpace` 返回后立即完成 transition 的错误 Engine 分支；已有设备换 Space 已能完成，测试不再声称稳定查询未暴露的迁移计数。
- 重启传输红测进一步定位为成员激活缺口：Sponsor peer roster 为 0，Joiner membership scope 不一致；协议终态虽然完成，Sponsor activated security/history 尚未进入正式 membership ledger 激活流程。
