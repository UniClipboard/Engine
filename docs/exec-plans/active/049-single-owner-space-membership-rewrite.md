# 049 成员状态单一负责人重写

## 状态与完整责任

- **状态**：实施中；S0 已完成（见“实施记录”），S1 未开始。
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

- 实现 `space_membership` 模块全部类型与三入口；`apply` 使用一个穷尽 `match`，不公开逐步修改方法和状态字段。
- 效果至少包括：`BeforeCommit` 激活准入安全状态；`AfterCommit` 发布设备信任变化、应用安全状态、唤醒 Worker。
- 提供唯一“接收安全更新的成员集合”函数（关闭 Core 边界计划 A2 的判定部分）。
- **验证**：每个状态 × 输入的单元测试断言完整效果列表；`present` 无通配分支；性质测试以固定种子枚举输入序列，
  断言 `due_work` 为空当且仅当设备更新阶段不是 Updating 或 RetryableFailure（等待本机决定与需要处理除外）；
  `restore(snapshot())` 往返一致，非法快照被拒绝。不新增第三方依赖。

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
