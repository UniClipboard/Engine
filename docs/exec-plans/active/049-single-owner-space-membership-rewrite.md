# 049 成员状态单一负责人重写

## 状态与完整责任

- **状态**：实施中；S0、S1 已完成（见“实施记录”），S2 未开始。
- **日期**：2026-09-23。
- **依据**：[ADR-027](../../design-docs/decisions/027-single-owner-space-membership-state.md)；2026-09-23 双 Desktop
  profile 配对后移除，移除方设备不消失、被移除方永久“正在更新空间设备状态”的诊断（结论见 ADR-027 背景）。
- **完整负责人**：
  - 成员规则、对端状态、待办与展示计算：Core `space_membership` 聚合。
  - 成员事实的唯一写入、提交后效果履行、快照发布与唤醒：Application `MembershipOwner`。
  - 持久待办执行：Application `MembershipWorker`，只把结果交回 Owner。
  - 入站对端访问判定：Application `PeerAccess`，读取 Owner 发布的快照。
  - 成员记录密文格式与旧格式迁移：Infra 成员记录仓储。
  - 本计划整体顺序与验收：本计划。
- **调用方唯一动作**：宿主与绑定不新增、不修改任何调用。Engine 公开操作、结果、事件与错误码不变。
- **成功结果**：配对、移除、被移除后接受或拒绝、离线、重启与分叉场景在双端都收敛到规格 021 的稳定结果；
  不再有永远无法结束的收尾工作；成员事实只有一个写入者。
- **失败结果**：成员记录无法验证或迁移失败时，保留原资料不改写，Space 进入需要处理状态并保留 source chain；
  暂时失败保留持久待办和下次重试时间。
- **重启与重试责任**：持久待办由聚合状态计算，重启后由 Owner 加载同一状态重新得出，不另存 outbox；
  Worker 与维护运行期只负责触发、并发、暂停和关闭。

## 范围

### 目标

1. 新增 Core `SpaceMembership` 聚合，按 [Core 设计规范](../../design-docs/layers/core.md#5-状态机)提供唯一
   `apply`、`due_work`、`present` 与 `snapshot`/`restore`。
2. 以 `MembershipOwner`、`MembershipWorker`、`PeerAccess` 替换 `MembershipLedger` 闭包提交、效果执行器、
   受限投递、历史反熵拆分、投影清理步骤与维护固定步骤链。
3. 准入正式提交与加入方激活经 Owner 输入，作为准入转换的 `BeforeCommit` 效果按事件幂等提交（成员记录在
   `control.sqlite`，准入记录在 profile 数据库，不共用事务）；删除 Infra 中全部成员记录构造与改写。
4. 新增唯一最终成员记录格式 `SpaceMembershipRecordV5`，从 V1–V4 一次性迁移。
5. 采纳 ADR-027 的三项协议语义澄清：被移除方接受后不回复决定；正在离开的对端以送达或 5 分钟到期结束；
   本机已移除为终态。

### 非目标

- 不改 Engine 公开接口、绑定、宿主行为与事件。
- 不改准入消息、成员历史分页、受限投递和组密钥更新的设备间格式。
- 不重写准入状态机内部阶段；不重写分叉恢复（`recover_conflict`）内部阶段，只将其挂到 Owner 下。
- 不改签名历史规范编码、OpenMLS 安全模型、MasterKey AEAD 与日志脱敏规则。
- 不新增公开诊断或观测接口；观测只装饰既有完整能力。
- 不修改或清理真实设备资料；实体设备验收需另行授权。

## 与其他进行中计划的关系

| 计划 | 关系 | 处理 |
| --- | --- | --- |
| [Core 边界收口](2026-09-23-core-boundary-remediation.md) | A1、A3、A7 由本计划的聚合与 Owner 直接消除；A4 的判定式改由 Core 聚合提供 | 本计划完成时在该计划中标注这四项已由 049 关闭，其余项不在本计划范围 |
| [入站对端身份与网络准入](2026-09-23-inbound-peer-admission.md) | 其 Infra `InboundPeerGate` 与拒绝分类保留为入口；身份与授权来源改为 `PeerAccess` 快照 | 等该计划收尾测试完成后再做 S5，不改其拒绝原因与诊断事件 |
| [单一空间工作负责人](2026-09-20-single-space-work-owner.md) | 配对与普通成员工作互斥的运行资格保留 | `MembershipWorker` 执行待办前沿用同一工作许可，不新增第二套模式事实 |
| [034 确定性虚拟 Peer Network](034-deterministic-virtual-peer-network-test-suite.md) | S3 的应用层多节点场景复用并扩展其虚拟网络 | 扩展只加节点数与待办驱动，不在网络中复制业务规则 |

## 目标结构

```text
crates/uc-core/src/membership/space_membership/
  mod.rs          状态、输入、效果、不变量与终态的模块文档；只做声明和导出
  aggregate.rs    SpaceMembership、apply 的穷尽分发、snapshot/restore
  local.rs        LocalStanding
  peer_link.rs    PeerLink 与各状态的结束条件
  input.rs        MembershipInput
  effect.rs       BeforeCommit / AfterCommit 效果
  work.rs         due_work 与 Work
  present.rs      present 与展示映射表
crates/uc-application/src/space/membership/
  owner.rs        MembershipOwner：唯一写入者
  worker.rs       MembershipWorker：执行 due_work
  access.rs       PeerAccess：入站访问判定
  ports.rs        Owner/Worker 需要的存储、传输、安全能力
  queries/        设备信任、名单、就绪与诊断查询，只调用 present
crates/uc-infra/src/space/membership_record/
  codec.rs        V5 DTO 与 V1–V4 迁移
```

`mod.rs` 只保留模块声明和必要导出；拆分文件不扩大公开接口。

## 切片

所有切片在同一功能分支实施。按 ADR-025 的约束，中间切片不得独立合入主分支；只有 S6 完成后整体合并。

### S0 固定行为（先写，预期当前实现失败的用例以 `#[ignore]` 标注）

- **Engine 真实多设备场景**（`crates/uc-engine/tests/space_membership_auto_pairing_e2e.rs` 同目录新增
  `removal_convergence.rs`）：
  - R1 两台配对后 A 移除 B：A 的 `QueryDeviceGroupChoices` 在通知送达后不再列出 B，设备更新为 Completed。
  - R2 B 在 A 撤销其身份之后才接受移除（本次诊断场景）：B 的本机成员为 Removed、本机同步关系为
    `RemovedLocalDevice`，设备更新稳定为 Completed；A 不再列出 B。
  - R3 B 拒绝移除：B 把 A 标为分叉暂停，设备更新稳定为 NeedsAttention；A 不再列出 B，设备更新为 Completed。
  - R4 B 被移除后重新加入：以新成员实例恢复可用。
- **旧格式固定向量**：用当前真实用例生成 V4 成员记录样本（含本次诊断的两种残留形态），存为 S2 迁移测试输入。
  V1–V3 已由 Infra 现有旧行升级测试覆盖到 V4，S2 在其后接 V4 → V5。
- 现有 F1–F7 拓扑场景、`public_contract.rs` 与宿主契约测试保持不改。
- **验证**：新用例编译通过；未标注 `#[ignore]` 的现有测试全部通过；`#[ignore]` 原因写明对应切片。

### S1 Core 聚合

S1 只新增 Core 模块及其测试，不接入任何调用方；S3 才切换 Application。

**状态**（字段全部私有，只读访问）：

| 部分 | 内容 | 规则 |
| --- | --- | --- |
| 历史 | `VersionedMembershipHistory` | 资格唯一来源；改变历史的输入携带已由 Application 经历史 API 与验签器产生的新历史，聚合核对沿革与事件后采用 |
| 本机 | 设备、成员实例、加入门禁 | 本机地位由门禁与历史得出：门禁开且在有效成员中为有效；在有效成员中但门禁关、或有未完成效果影响本机为激活中；否则为已移除 |
| 对端 | `PeerLink::Member` / `PeerLink::Departing` | 规范化：每个有效对端恰有一个 `Member`；`Departing` 只属于已不在有效成员中的设备；其余记录删除；本机为移除目标时不保留待投递决定 |
| 成员效果 | 未激活效果：事件、类型、阶段（已准备、成员资料已应用、安全已应用）、受影响设备、类型化材料 | 激活即删除；只保留当前分支路径上的事件 |
| 同步游标 | 上次轮转到的对端 | 只影响待办顺序 |

`Member` 携带关系（未确认、一致、需升级、等待本机决定、分叉、无效）、已确认位置、同步退避（起始修订、
重试次数、下次时间、最近结果）与待投递决定；`Departing` 携带移除事件与窗口起点（提交时刻）。

**输入**（按外部事实命名）：本机移除已签名、本机决定已签名、准入正式提交（邀请方）、对端历史证据已核对、
历史同步已选定对端、历史同步结束、投递结束（通知或决定）、离开窗口到期、成员效果阶段完成、分支已恢复。
加入方激活与新建 Space 用构造器 `start_*`，已保存状态用 `restore`。

**结果与效果**：重复、过期、位置已变化用 outcome 表达；效果只有 `AfterCommit`：发布设备信任变化、唤醒
执行器。准入正式提交的 `BeforeCommit` 由准入聚合声明，不在本聚合重复。

**待办**：`outstanding_work(observations)` 返回全部未完成待办及最早执行时间——成员效果推进（按因果深度）、
移除通知投递、离开到期、决定投递、历史同步（本机有效时，对允许核对且未确认当前位置的对端，按退避与游标）。
每项标明是否阻塞设备更新；移除通知与离开到期不阻塞。

**展示**：`present(observations)` 给出本机成员状态、每台设备的成员状态、关系、同步状态与暂停原因、
可用对端范围，以及成员部分的设备更新阶段；组密钥投递状态作为观察输入叠加。本机身份不一致的覆盖仍由
Application 查询负责。与现有推导相比只有三处有意变化：`Departing` 送达或到期后消失；本机已移除时设备
更新为完成；本机为移除目标时不产生决定投递。

- **验证**：每个状态 × 输入的单元测试断言新状态、outcome 与完整效果；`present` 与规范化无通配分支；
  固定种子枚举输入序列的性质测试断言：规范化不变量始终成立，阻塞待办非空当且仅当成员部分为更新中或
  可重试失败（需要处理除外）；`restore(snapshot())` 往返一致，非法快照被拒绝。不新增第三方依赖。

### S2 持久格式与迁移

- Infra 新增 `SpaceMembershipRecordV5` DTO 与编解码，不直接序列化 Core 或 Application 类型。
- 实现 V1–V4 → V5 迁移，映射规则见下表；迁移在解锁读取时进行，失败不改写原资料。
- **验证**：S0 固定向量全部迁移成功且结果符合下表；损坏与未知版本返回稳定错误；迁移后再次读取为 V5 且内容不变。

| 旧记录内容 | V5 结果 |
| --- | --- |
| 当前有效成员 | `PeerLink::Member`，沿用确认位置与兼容性 |
| 已移除、仍有待投递通知 | `PeerLink::Departing`，窗口从迁移后首次处理重新计时 |
| 已移除、无待投递通知 | 删除该对端记录 |
| 关系为分叉或无效 | 对应 `Diverged` / `Invalid` |
| 本机不在有效成员中且无待决定移除 | `LocalStanding::Removed` |
| 存在待本机决定的移除 | `LocalStanding::AwaitingDecision` |
| 效果日志 | 安全游标 = 最早未激活事件之前的位置；已激活效果不再保存 |
| 分叉、分支恢复会话与换组记录 | 原样搬入分叉记录，由 Owner 继续推进 |
| 历史分页暂存、完成 ACK | 原样保留 |

### S3 Application 切换

- **应用层确定性多节点场景**（原列于 S0，改在本切片针对 Owner 编写，避免针对即将删除的内部入口写测试；
  扩展 `membership/testing/virtual_membership_network.rs`，注入测试时钟）：
  - W1 被移除方离线：移除方 5 分钟后删除该对端，第 299999 毫秒仍存在，第 300000 毫秒后消失。
  - W2 三节点固定种子交错：配对、移除、延迟决定、离线、重启任意交错后，断言最终收敛（见“验收”第 4 项）。

- 实现 `MembershipOwner`、`MembershipWorker`、`PeerAccess`；Owner 在同一事务中提交成员记录与成员读模型。
- 设备信任、名单、就绪、诊断查询只调用 `present`。
- 维护运行期只保留触发、并发、暂停、关闭与工作许可；删除固定步骤链与 Corrupt 中断逻辑。
- `recover_conflict` 作为一项 Worker 待办接入，内部阶段与 Infra 换组能力不改。
- **验证**：S0 中 R1–R3、W1–W2 取消 `#[ignore]` 并通过；`uc-application` Space 测试全部通过；
  全仓库只有 Owner 调用成员记录提交能力。

### S4 准入交接

- Sponsor 最终确认与 Joiner 激活改为提交 Owner 输入，作为准入转换的 `BeforeCommit` 效果：Owner 先提交成员
  事实，成功后才保存准入终态；同一准入事件的重复输入返回 `Duplicate`。
- 删除 `crates/uc-infra/src/space/admission/sponsor/complete.rs` 与
  `crates/uc-infra/src/security/space_control_generation/material.rs` 中的成员记录构造与改写；Infra 只保留
  安全材料、签名和存储能力。
- **验证**：R4 与现有六位码配对、跨 Space 加入、配对期限与终止测试全部通过；注入“成员已提交、准入记录
  保存失败”后重启，同一尝试完成且成员历史中只有一次加入；注入成员提交失败时准入记录不改变。

### S5 入站访问

- `InboundPeerGate` 的身份解析与授权改读 `PeerAccess` 快照；`PeerIdentityResolver` 不再依赖成员表。
- 正在离开的对端只允许接收精确移除通知。
- **验证**：入站准入计划的全部测试不改断言通过；新增用例证明移除提交后、投影更新前，入站判定已与账本一致。

### S6 删除与文档

- 删除 ADR-027 “删除”一节列出的全部旧代码；逐项核实并删除只剩遗留引用的 Core 成员模块。
- 更新 `docs/design-docs/space-application.md`、`docs/design-docs/pairing-lifecycle.md`、
  `docs/design-docs/membership-history-ownership.md`；在 Core 边界计划中标注 A1、A3、A4、A7 已关闭。
- **验证**：见下节全部检查。

## 验收

1. `crates/uc-engine/tests/public_contract.rs` 与宿主契约测试不修改即通过；绑定目录无改动。
2. 全仓库只有 `MembershipOwner` 能提交成员记录；Infra 中不存在成员记录构造或改写（以架构检查脚本固定）。
3. `present` 对全部本机地位与对端状态穷尽匹配，无通配分支。
4. 多节点收敛断言：移除方不残留设备；被移除方进入已移除终态；任何一方不永久停留在 Updating；
   资格从不在缺少签名事件时扩大。
5. R2（被移除方晚于身份撤销才决定）作为固定回归通过。
6. V1–V4 固定向量迁移全部通过，迁移失败不改写原资料。
7. 交付前检查全部通过：

```bash
cargo metadata --locked --format-version 1
cargo check --workspace --all-targets --locked
cargo test -p uc-core --locked
cargo test -p uc-application --lib space --locked -- --test-threads=1
cargo test -p uc-infra --locked
cargo test -p uc-engine --locked
cargo fmt --all -- --check
node scripts/architecture/check-rust-style.mjs
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

8. 实体双 Desktop 复现场景（本次诊断的 profile a/b 流程）需另行授权；未执行时记为“跳过”。

## 风险

| 风险 | 应对 |
| --- | --- |
| 旧资料迁移映射错误导致成员资格变化 | S0 先固定向量；迁移只缩小或保持资格，从不从旧关系补造资格 |
| 分叉恢复挂接后行为变化 | F2–F7 拓扑场景不改即须通过 |
| 与入站准入计划并行修改同一入口 | S5 等该计划收尾完成后开始，只替换数据来源 |
| 改动面大、分支周期长 | 每个切片结束运行完整检查并在“实施记录”留下证据；主分支变化定期合入功能分支 |

## 实施记录

（按切片记录完成内容、验证命令与结果；未执行项记为“跳过”。）

### S0（2026-09-23，分支 `hp/uni/t-0010-android`）

完成内容：

- Engine 真实多设备场景 `crates/uc-engine/tests/space_membership_auto_pairing_e2e/removal_convergence.rs`：
  R1–R4。R1–R3 以 `#[ignore = "049 S3：…"]` 标注，R4 常规运行。
- V4 固定向量 6 个，位于 `crates/uc-infra/src/space/membership_ledger/fixtures/`：`two_member_active`、
  `sponsor_removal_notice_pending`、`sponsor_removal_notice_delivered`、`local_removal_pending_decision`、
  `local_removal_accepted`、`local_removal_rejected`。由 Application 真实移除、决定、受限投递和效果恢复用例
  推进生成（生成器：`membership/testing/legacy_ledger_fixtures.rs` 与两处用例测试，设置
  `UC_WRITE_LEGACY_LEDGER_FIXTURES=1` 才覆盖写入）；Infra 测试
  `legacy_v4_fixtures_decode_with_recorded_residual_states` 证明它们是当前 V4 并锁定残留形态。样本只含合成标识，
  签名为测试替身。
- 核实并更正 ADR-027 与本计划：成员记录与成员读模型在 `control.sqlite`，准入记录在 profile 数据库，不能共用
  事务；准入交接改为 `BeforeCommit` 效果加事件幂等。

验证结果：

| 检查 | 结果 |
| --- | --- |
| R1–R4 含 ignored 运行（`cargo test -p uc-engine --features dev-tools --test space_membership_auto_pairing_e2e --locked -- removal_convergence --include-ignored --test-threads=1`） | R4 通过；R1、R3 失败于“A 仍列出被移除设备：Removed / AwaitingRemovalAcknowledgement / RemovedPeerDevice”；R2 失败于“B 设备更新停在 RetryableFailure”，与诊断一致 |
| 固定向量生成测试与 Infra 解码测试 | 通过 |
| `cargo metadata --locked`、`cargo check --workspace --all-targets --locked` | 通过（`uc-ohos-napi` 测试中既有未使用导入告警，非本次改动） |
| `cargo fmt --all -- --check`、`check-rust-style.mjs`、`check-engine-repository.mjs`、`git diff --check` | 通过 |
| `cargo test -p uc-application --lib space --locked -- --test-threads=1` | 361 通过，1 失败：`admission_recovery_scenarios::joiner_pairing_fixture_reaches_active_settled`（`joiner-pairing-was-not-active`）。本次对 `uc-application`、`uc-core` 只有测试代码增量，该失败连续三次稳定复现，属本次之前已存在的问题，未处理 |
| 现有 F0–F7 拓扑场景与其余 Engine 真实场景 | 跳过（本次未改动其代码路径，完整套件耗时长） |

### S1（2026-09-23，分支 `hp/uni/t-0010-android`）

完成内容：`crates/uc-core/src/membership/space_membership/`（以公开子模块 `membership::space_membership`
暴露，避免通用名称与现有扁平导出冲突），尚无调用方。

实现中确定、与设计概要相比需要说明的细节：

- 聚合不再保存加入门禁：现有账本只在没有当前 Space 时关闭门禁，而那时聚合不存在。`start` 接收调用方给出的
  修订号，保证同一 profile 内设备信任修订单调递增。
- 离开窗口从移除提交时刻起算（原实现从首次处理起算）；迁移时以迁移时刻起算。
- 对端证据确认一致时同时清零同步退避；否则旧的“暂缓”结果在确认后仍会让设备更新停在可重试失败。
- 同步“暂缓”只在该对端仍需同步时计入可重试失败；分叉后不再显示永远不会执行的重试。稳定拒绝仍无条件
  报告需要处理，保持原问题分类。
- 决定投递被对端明确拒绝时删除该投递：本机已无法推进它，保留只会永久显示更新中。
- 被本机移除后又重新加入的设备在规范化时从 `Departing` 转回 `Member`。
- 采用对端历史时为新增和移除的成员登记效果的规则（原在 `handle_history_message`）移入聚合。

验证结果：

| 检查 | 结果 |
| --- | --- |
| `cargo test -p uc-core --lib --locked space_membership` | 17 通过；含 24 个种子各 160 步的交错性质测试，逐步断言规范化不变量与“阻塞待办 ⇔ 更新中/可重试失败”，并在全部成功与跨过离开窗口后断言收敛 |
| `cargo test -p uc-core --locked` | 全部通过 |
| `cargo clippy -p uc-core --all-targets --locked`（仅新模块） | 无告警 |
| `cargo check --workspace --all-targets --locked`、`cargo fmt --all -- --check`、`check-rust-style.mjs`、`check-engine-repository.mjs`、`git diff --check` | 通过 |
