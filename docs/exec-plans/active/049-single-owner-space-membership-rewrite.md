# 049 成员状态单一负责人重写

## 状态与完整责任

- **状态**：实施中；S0–S5 与 S3.a 已完成（见“实施记录”），下一步 S6。
- **日期**：2026-09-23。
- **依据**：[ADR-027](../../design-docs/decisions/027-single-owner-space-membership-state.md)；2026-09-23 双 Desktop
  profile 配对后移除，移除方设备不消失、被移除方永久“正在更新空间设备状态”的诊断（结论见 ADR-027 背景）。
- **完整负责人**：
  - 成员规则、对端状态、待办与展示计算：Core 成员账本聚合 `MembershipLedger`（`crates/uc-core/src/membership/ledger/`）。
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

1. 新增 Core 成员账本聚合 `MembershipLedger`，按 [Core 设计规范](../../design-docs/layers/core.md#5-状态机)提供唯一
   `apply`、`outstanding_work`、`present` 与 `snapshot`/`restore`。
2. 以 `MembershipOwner`、`MembershipWorker`、`PeerAccess` 替换 `MembershipLedger` 闭包提交、效果执行器、
   受限投递、历史反熵拆分、投影清理步骤与维护固定步骤链。
3. 准入正式提交与加入方激活经 Owner 输入，作为准入转换的 `BeforeCommit` 效果按事件幂等提交（成员记录在
   `control.sqlite`，准入记录在 profile 数据库，不共用事务）；删除 Infra 中全部成员记录构造与改写。
4. 新增唯一最终成员记录格式 `MembershipLedgerRecordV5`，从 V1–V4 一次性迁移。
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
| [入站对端身份与网络准入](../completed/2026-09-23-inbound-peer-admission.md) | 其 Infra `InboundPeerGate` 与拒绝分类保留为入口；身份与授权来源改为 `PeerAccess` 快照 | 该计划已于 2026-09-25 收尾，S5 可开始；不改其拒绝原因与诊断事件 |
| [单一空间工作负责人](2026-09-20-single-space-work-owner.md) | 配对与普通成员工作互斥的运行资格保留 | `MembershipWorker` 执行待办前沿用同一工作许可，不新增第二套模式事实 |
| [034 确定性虚拟 Peer Network](034-deterministic-virtual-peer-network-test-suite.md) | S3 的应用层多节点场景复用并扩展其虚拟网络 | 扩展只加节点数与待办驱动，不在网络中复制业务规则 |

## 目标结构

```text
crates/uc-core/src/membership/ledger/
  mod.rs          状态、输入、效果、不变量与终态的模块文档；只做声明和导出
  aggregate.rs    MembershipLedger、apply 的穷尽分发、规范化与不变量校验
  peer_link.rs    PeerLink（Member / Departing）、关系与同步退避
  input.rs        LedgerInput、outcome、效果与 LedgerTransition
  effect.rs       未完成成员效果
  error.rs        LedgerTransitionError 与稳定分类
  work.rs         outstanding_work 与 LedgerWork
  present.rs      present、可用范围与展示映射
  snapshot.rs     snapshot / restore
crates/uc-application/src/space/membership/
  owner.rs        MembershipOwner：唯一写入者
  worker.rs       MembershipWorker：执行 outstanding_work 中已到期的待办
  access.rs       PeerAccess：入站访问判定
  ports.rs        Owner/Worker 需要的存储、传输、安全能力
  queries/        设备信任、名单、就绪与诊断查询，只调用 present
  record.rs       MembershipRecord：成员账本快照与同存的交换、分叉资料（纯数据）
crates/uc-infra/src/space/membership_record.rs   成员记录仓储入口
crates/uc-infra/src/space/membership_record/
  store.rs        加密读写、解锁读取时迁移写回、条件提交
  codec.rs        版本分派
  codec/v5.rs     MembershipLedgerRecordV5
  codec/common.rs V4 与 V5 共用的冻结布局（分叉、交换、位置）
  codec/legacy.rs V1–V4 布局
  codec/migrate.rs V4 形状到成员记录的映射
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

- Infra 新增 `MembershipLedgerRecordV5` DTO 与编解码，不直接序列化 Core 或 Application 类型。
- 实现 V1–V4 → V5 迁移，映射规则见下表；迁移在解锁读取时进行，失败不改写原资料。
- **验证**：S0 固定向量全部迁移成功且结果符合下表；损坏与未知版本返回稳定错误；迁移后再次读取为 V5 且内容不变。

| 旧记录内容 | V5 结果 |
| --- | --- |
| 当前有效成员 | `PeerLink::Member`，沿用确认位置与兼容性 |
| 已移除、仍有待投递通知 | `PeerLink::Departing`，窗口从迁移时刻重新计时 |
| 已移除、无待投递通知 | 删除该对端记录 |
| 关系为分叉或无效 | 对应 `Diverged` / `Invalid` |
| 关系为“等待移除决定” | 本机确有待决定的移除时为 `AwaitingLocalDecision`，否则为 `Unconfirmed`（重新核对） |
| 待投递决定以本机为移除目标 | 删除 |
| 本机地位 | 不单独保存：由历史与未完成效果得出（S1），本机不在有效成员中且无影响本机的效果即为已移除 |
| 效果日志 | 只迁移未激活且位于当前历史路径上的效果，载荷按完整布局还原为类型化材料；已激活效果不再保存 |
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

**实施拆分**（开工前核实后确定）：旧 V4 账本的读写方分布在 Application 用例、Infra 准入与代际构建、Engine
装配共二十余处，而 V5 仓储在解锁读取时即改写整行，新旧读写方无法并存，因此 S3 一次切换全部读写方，
工作树在切换期间允许暂时不能编译，切片结束时恢复全部检查。顺序：

1. 固定旧类型编码的富状态 V4 向量（`membership_record/fixtures/rich_state.v4.bin`），Infra 测试不再依赖
   Application 旧类型。
2. Core 补充 Owner 接入所需的输入语义：一致但未确认位置的证据、非成员来源只贡献已验证历史、伴随资料
   变化只推进修订号。
3. Application：`ports.rs`（成员记录存储 port，提交携带成员读模型计划）、`owner.rs`、`worker.rs`、
   `access.rs`；各用例改为经 Owner 读取与提交；删除旧账本、效果执行器、受限投递、固定步骤链与投影清理步骤。
4. Infra：V5 仓储实现存储 port，并在同一事务中落实成员读模型；效果 port 改收类型化效果；邀请方激活
   只做安全激活并返回已验证历史，由 Application 交给 Owner（S4 在此基础上补齐准入 `BeforeCommit`
   与故障注入验证）；加入方与分叉换组写入暂存代际数据库的记录暂按 Core 规则生成 V5 记录，S4 再上移；
   删除 `SqliteMembershipLedger` 与旧编解码。
5. Engine 装配与观测装饰改接新 port；设备分组变化事件由 Owner 按聚合效果发出。
6. 测试：Application 测试改用内存成员记录存储；R1–R3 取消忽略；新增 W1、W2。

### S3.a 剩余失败诊断

S3 完成时仍有以下失败，均在 S3 之前的基线（`cd9537b6`）上同样出现：

- `space_switch::same_device_returns_to_a_previous_space_after_switch_and_restart`（启动返回 1216）
- `space_switch::suspend_during_space_switch_recovery_does_not_resurrect_the_network`（1103）
- `topology::handoff_four_device_removal_preview_matches_executed_choice`
- `admission::confirmed_pairing_survives_restart_removal_and_same_device_rejoin`
- `admission::pending_join_is_not_published_before_final_confirmation`（间歇，最终确认后查询返回 1211）
- `uc-application` 单元测试 `admission_recovery_scenarios::joiner_pairing_fixture_reaches_active_settled`

- 逐项判定是测试用例问题（断言、等待、前提与产品规则不符）还是产品逻辑问题，并以日志与代码路径为证据。
- 测试用例问题：修正测试，只改等待或前提，不放宽业务断言；修改前说明依据。
- 产品逻辑问题：记录根因与影响，另行决定修复方式与归属，不在本切片中顺手修改。
- **验证**：每项单独与全组运行结果；判定与证据记录在“实施记录”。

### S4 准入交接

- Sponsor 最终确认与 Joiner 激活改为提交 Owner 输入，作为准入转换的 `BeforeCommit` 效果：Owner 先提交成员
  事实，成功后才保存准入终态；同一准入事件的重复输入不改变状态（聚合结果 `Unchanged`）。
- 加入方的目标控制世代不带成员记录与读模型，提升后由 Owner 输入建立；分叉换组的目标世代由 Owner 预先形成
  成员记录、Infra 原样写入（两种交接的原因见 ADR-027“准入交接”“分叉恢复”）。
- 删除 `crates/uc-infra/src/space/admission/sponsor/complete.rs` 与
  `crates/uc-infra/src/security/space_control_generation/material.rs` 中的成员记录构造与改写；Infra 只保留
  安全材料、签名和存储能力。
- **验证**：R4 与现有六位码配对、跨 Space 加入、配对期限与终止测试全部通过；注入“成员已提交、准入记录
  保存失败”后重启，同一尝试完成且成员历史中只有一次加入；注入成员提交失败时准入记录不改变。

### S5 入站访问

- `InboundPeerGate` 的身份解析与授权改读 `PeerAccess` 快照；`PeerIdentityResolver` 不再依赖成员表。
  解析候选与成员读模型相同，拒绝分类不变。
- 正在离开的对端只接收精确移除通知：指出站方向，S3 已实现；入站方向保持查账本协议拒绝、只核对身份的
  协议照常识别（核实结论见“实施记录”S5）。
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
bash scripts/testing/run-test-group.sh membership-e2e
cargo fmt --all -- --check
node scripts/architecture/check-rust-style.mjs
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

   `cargo test -p uc-engine --locked` 不编译成员多设备场景（该文件只在 `dev-tools` 下编译），R1–R4、F0–F7 与其余
   多 Engine 场景由 `membership-e2e` 分组运行。
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

完成内容：`crates/uc-core/src/membership/ledger/` 的成员账本聚合 `MembershipLedger`，与同级模块一样以私有模块
加 `membership` 平铺导出，通用名称统一加 `Ledger` 或 `Peer` 前缀；尚无调用方。

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
| `cargo test -p uc-core --lib --locked membership::ledger` | 17 通过；含 24 个种子各 160 步的交错性质测试，逐步断言规范化不变量与“阻塞待办 ⇔ 更新中/可重试失败”，并在全部成功与跨过离开窗口后断言收敛 |
| `cargo test -p uc-core --locked` | 全部通过 |
| `cargo clippy -p uc-core --all-targets --locked`（仅新模块） | 无告警 |
| `cargo check --workspace --all-targets --locked`、`cargo fmt --all -- --check`、`check-rust-style.mjs`、`check-engine-repository.mjs`、`git diff --check` | 通过 |

### S2（2026-09-23，分支 `hp/uni/t-0010-android`）

完成内容：

- Infra 成员记录仓储 `crates/uc-infra/src/space/membership_record/`：唯一最终格式 `MembershipLedgerRecordV5`；
  V1–V4 旧布局改由 Infra 独立声明（`codec/legacy.rs`、`codec/common.rs`），不再依赖 Application 类型解码；
  `SqliteMembershipRecordStore` 在解锁读取时于同一事务内迁移并写回 V5，迁移失败原行不变；提交按修订号
  条件写入。仓储尚未接入组装，旧 `SqliteMembershipLedger` 仍在使用，S3 切换。
- Application 新增纯数据 `MembershipRecord`（`space/membership/record.rs`），为 S3 Owner 的存储 port 预留
  数据形状；`MembershipBranchRecoverySession` 增加 `restore` 与只读访问，供 Infra 独立 DTO 往返。
- Core 新增 `MembershipLedger::restore_normalized`：迁移用，按每次转换后的同一规范化规则整理后再校验；
  普通读取仍用拒绝修复的 `restore`。
- S0 固定向量移到 `crates/uc-infra/src/space/membership_record/fixtures/`，生成器与旧解码测试同步路径。

实现中确定、与设计概要相比需要说明的细节：

- V5 只直接嵌入标识值与带自身版本的已签名协议对象（事件、决定、回执、分页、换组、恢复包）；其余状态全部由
  Infra 结构声明。成员历史保存为 `encode_persisted_v2` 归档，读取时重新验签，因此仓储持有历史签名验证器；
  迁移时刻来自注入的时钟。
- 迁移本身是一次写入，修订号前进一步；没有当前 Space 的旧记录迁移为只含修订号的 V5 记录，保证下一个 Space
  的修订继续递增。旧记录有 Space 而加入门禁关闭时无法解释，按损坏处理。
- 旧效果载荷不带类型标记：按事件、本机发起的移除、决定三种布局完整长度严格解析，恰好一种成立才采用。
- 旧读取时对分叉展示资料与冲突记录的一致性检查（`matches_record`）未在 V5 读取中重复，S3 由 Owner 加载时
  保留该检查。
- S3 删除 Application 旧账本类型前，须把 `codec/tests.rs` 中以旧类型编码的富状态 V4 字节固定为向量文件。

验证结果：

| 检查 | 结果 |
| --- | --- |
| `cargo test -p uc-infra --lib --locked space::membership_record` | 8 通过：6 个 V4 固定向量按映射表迁移且 V5 往返不变；损坏、截断、尾随字节、未知版本、错误 generation、验签失败返回 `Corrupt`；解锁读取迁移一次并以 V5 重读不变（离开窗口起点保持首次迁移时刻）；迁移失败原行字节不变；V1–V4 无 Space 记录迁移；条件提交；以 Application 类型编码的含全部状态种类 V4 与 Infra 独立布局逐字节一致；三种效果载荷判别 |
| `cargo test -p uc-core --locked` | 全部通过（成员账本 18 项，新增规范化恢复用例） |
| `cargo test -p uc-infra --locked` | 全部通过 |
| `cargo test -p uc-application --lib space --locked -- --test-threads=1` | 361 通过，1 失败：基线问题 `admission_recovery_scenarios::joiner_pairing_fixture_reaches_active_settled`，未处理 |
| `cargo clippy -p uc-core -p uc-application -p uc-infra --all-targets --locked`（仅本次改动文件） | 无告警 |
| `cargo metadata --locked`、`cargo check --workspace --all-targets --locked`、`cargo fmt --all -- --check`、`check-rust-style.mjs`、`check-engine-repository.mjs`、`git diff --check` | 通过（`uc-ohos-napi` 测试既有未使用导入告警，非本次改动） |
| `cargo test -p uc-engine --locked` 与 R1–R4 真实场景 | 跳过（新仓储尚未接入运行时，Engine 路径无变化） |

### S3（2026-09-23，分支 `hp/uni/t-0010-android`，2026-09-24 完成）

完成内容：

- Application：`MembershipOwner` 是成员记录的唯一提交者，并在同一事务中落实成员读模型；
  `MembershipWorker` 执行 `outstanding_work` 与分叉恢复、组密钥投递；`PeerAccess` 判定入站对端。各用例改为经
  Owner 读取与提交；旧账本、效果执行器、受限投递、固定步骤链与投影清理步骤已删除。
- Core：Owner 接入所需的证据与伴随资料输入；采用对端历史止于激活基线。
- Infra：V5 仓储实现存储 port；邀请方激活返回已验证历史；暂存代际按 Core 规则生成 V5 记录（S4 上移）。
- Engine：装配与观测装饰改接新 port；设备分组变化事件由 Owner 按聚合效果发出。
- 测试：内存 Owner 测试台，W1、W2 多节点场景；R1–R3 取消忽略。
- 架构检查：`check-engine-repository.mjs` 新增“成员记录提交只由 Owner 构造”检查及其反例。

实现中确定、需要说明的设计变化（已由用户确认的部分同步写入
[ADR-027](../../design-docs/decisions/027-single-owner-space-membership-state.md#移除待决定期间的对端关系049-s3-补充已由用户确认)）：

- ADR-020 优先于原 F1 预期：本机发起的移除尚待决定时普通内容不越过该移除。Core 新增对端关系
  `AwaitingPeerDecision` 与同步结果 `PeerSyncResult::AwaitingPeerDecision`：对端确认的位置是本机位置的严格
  祖先，且本机历史中有本机发起、尚未决定的移除时才接受，否则暂缓。V5 编码在 `PeerRelationV5` 末尾追加该
  变体；V5 尚未发布，不新增版本。发起方把未决定的第三台设备显示为 `ConfirmationPending` /
  `PausedUnverifiable`，发送结果为 0/0/0（既不离线也不排队）。
- 两种待决定关系都不触发历史同步；本机决定未作出时，同步确认保留 `AwaitingLocalDecision`。
- `PeerAccess` 在网络层允许两种待决定关系的对端，使受限历史与决定能够交换；内容仍由成员范围控制
  （接收门禁检查 `usable_peer_device_ids`）。
- 空间工作许可改为读写锁（`crates/uc-application/src/space/admission/protocol/protocol.rs`）：普通成员工作
  共享，准入独占。修复两台设备互相同步时各等对方许可 10 秒的对称死锁。
- 执行器每轮顺序：本机效果 → 分叉恢复 → 组密钥更新 → 网络待办与历史同步；两遍共享已尝试集合，效果步骤
  由 `effect_step` 串行化并在执行前复核阶段。
- 为某对端开始新同步时清除其上次 `StableRejected`，重试期间显示更新中。
- 诊断 `pending_confirmation_count` 同时计入两种待决定关系的对端，使交叉移除造成的分裂可见。
- `RemoveMember` 在加入后 Space 会话仍在切换时返回可重试错误：`RemoveSpaceMemberError::Unavailable` 映射
  1392 且 `retryable=true`；`Locked` 仍不可重试（用户确认）。
- 测试语义调整（已向用户报告）：F1 在决定前断言内容被扣留，通知送达后移除方不再列出被移除设备（R1 要求，
  原断言为列为 `Removed`）；`pending_final_confirmation_survives_joiner_restart` 由“不得为 Completed”改为
  “若为 Completed 必须真正收敛”（待确认、效果、冲突均为 0 且组 epoch 相同）；F7 在读取基准组 epoch 前
  等待相关节点未完成效果清零（用户批准：历史等价只说明成员事实已提交，效果由执行器随后落实，旧实现同样
  如此，负载下窗口变大导致读到旧 epoch）；`membership_history_retryable_failure_exposes_deadline_and_recovers`
  改为等待带重试时间的 Retrying（用户批准：失败事件早于退避结果提交，旧式投影把更新中也显示为 Retrying）。
- `.config/nextest.toml` 为 `space_membership_auto_pairing_e2e` 增加测试组（并发 4）与 60 秒 × 10 的慢测试
  期限，default 与 ci 两个 profile 相同（用户批准）。该二进制须用 nextest 运行：`cargo test` 在一个进程中
  串行运行全部 51 项约 27 分钟，时序失真。
- 按调试期间新增的日志均作为正式、脱敏的业务或诊断日志保留。
- `offline_member_catches_multiple_removals_without_blocking_new_invitations` 负载下超时的根因（用户确认修复方式）：
  B 离线期间 A 发给它的组密钥更新投递失败并按持久退避延期（30 秒起翻倍，最长 1 小时）；B 回来并完成成员历史
  同步后，更新仍要等退避到期。`2026-09-20` 单一空间工作负责人计划有意删除了按上线事件绕过退避，因此不恢复
  `PeerOnline`，改为以已认证的历史交换为依据：Owner 的提交草稿记录新确认本机位置的对端（出站
  `HistorySyncFinished::Confirmed` 或入站 `PeerEvidenceReconciled::Confirmed`），提交后交给执行器；组密钥投递
  对这些对端使用 Infra 已有的 `online_peer` 能力忽略退避。只影响投递时机，不改变投递对象、持久格式与公开接口。
  验证：新增 Application 测试（退避中的更新在收件人确认历史后投递、其他收件人不受影响；Owner 只交出一次新确认的
  对端）；该场景单独运行由 58 秒降至 45 秒，日志显示投递早于退避到期；修复后 `membership-e2e` 全组 46/51，
  `offline_member` 通过，失败为 4 个基线失败与基线同样间歇失败的 `pending_join_is_not_published_before_final_confirmation`。
  `cargo nextest run -p uc-core -p uc-application -p uc-infra`：2557 项中 2553 通过；失败为基线
  `joiner_pairing_fixture_reaches_active_settled`，以及并行下借用端口被抢的
  `node_lifecycle::production_node_restarts_ten_times_with_stable_identity_and_released_port`（单独运行通过）；
  `history_exchange_splits_the_256_activation_receipt_boundary`（约 56 秒）与
  `actual_client_exchange_reports_reply_failures_and_preserves_trace_result`（约 31 秒）超过仓库默认 20 秒期限被
  终止，放宽期限后均通过，均与本次改动无关。

验证结果：

| 检查 | 结果 |
| --- | --- |
| `cargo test -p uc-core --locked`、`cargo test -p uc-infra --locked` | 全部通过 |
| `cargo test -p uc-application --lib space --locked -- --test-threads=1` | 324 通过，1 失败：基线问题 `admission_recovery_scenarios::joiner_pairing_fixture_reaches_active_settled`，未处理；1 项按设计忽略（独占日志捕获） |
| `cargo nextest run --no-fail-fast -p uc-engine --locked --features dev-tools --test space_membership_auto_pairing_e2e`（仓库配置） | 51 项中 44 通过、7 失败。R1–R4、W1/W2、F0–F7 通过 |
| 上述失败中基线（`cd9537b6`）同样失败的 | `same_device_returns_to_a_previous_space_after_switch_and_restart`（启动 1216）、`suspend_during_space_switch_recovery_does_not_resurrect_the_network`（1103）、`handoff_four_device_removal_preview_matches_executed_choice`、`confirmed_pairing_survives_restart_removal_and_same_device_rejoin`；`pending_join_is_not_published_before_final_confirmation` 在基线单独运行 3 次失败 1 次（同为最终确认后查询返回 1211），本分支单独运行也间歇失败 |
| 负载下间歇失败、单独运行通过 | `membership_history_retryable_failure_exposes_deadline_and_recovers`：开发事件在传输失败时即记录，早于 Owner 提交退避结果，而旧式健康投影把 Updating 也映射为 Retrying，测试可能读到无重试时间的 Retrying（测试侧竞态；经用户批准改为等待带重试时间的 Retrying，单独运行通过）；`offline_member_catches_multiple_removals_without_blocking_new_invitations`（组 epoch 等待超时，未深入）；`f0_partitioned_sponsors_create_isolated_sibling_branches` 与 `space_switch_cancels_in_flight_file_send_without_leaving_imports` 曾在高负载运行中失败 |
| `cargo clippy -p uc-core -p uc-application -p uc-infra -p uc-engine --all-targets --locked`（仅 S3 改动行） | 生产代码无告警（已修复 `sort_by_key`、受限投递枚举装箱与 `DeviceId` 多余 clone）；测试文件中的 `unwrap` 告警保留。`uc-application` 既有 `application/shutdown.rs` 的 `async_yields_async` 拒绝级 lint 非本次改动，检查时以 `-A clippy::async_yields_async` 放行 |
| `cargo metadata --locked`、`cargo check --workspace --all-targets --locked`、`cargo fmt --all -- --check`、`check-rust-style.mjs`、`check-engine-repository.mjs`、`git diff --check` | 通过（`uc-ohos-napi` 测试既有未使用导入告警，非本次改动） |
| `cargo test -p uc-engine --locked` 其余测试二进制与宿主契约 | 跳过（本次未运行） |
| 实体双 Desktop 复现场景 | 跳过（需另行授权） |

S3 已完成（2026-09-24 用户确认）；剩余失败的诊断转入 S3.a。S3 不单独合入主分支。

### S3.a（2026-09-24，分支 `hp/uni/t-0010-android`，诊断完成）

逐项判定（证据为单独运行日志与代码路径）：

| 失败项 | 判定 | 依据与处理 |
| --- | --- | --- |
| `joiner_pairing_fixture_reaches_active_settled` | 测试用例问题，已修复 | 最终确认门控后，本机激活按设计返回 `Processing`，公开 `Active` 由 Infra 依据“加入已结算”投影；夹具保存了最终确认前的状态再断言 `Active`。夹具改为记录激活结果与最终确认后的已结算事实，断言不变；`admission_recovery_scenarios` 4/4 通过 |
| `handoff_four_device_removal_preview_matches_executed_choice` | 测试用例问题，已修复 | 失败断言要求接受远端移除的 A 仍以 `Removed` 列出被移除设备。按 ADR-027，只有本机发起的移除保留离开收尾条目（规格 021 的 `AwaitingRemovalAcknowledgement` 只针对移除发起方），与已批准的 F1 调整同类。断言改为“目标不再列出”（更严格）；单独运行 2/2 通过 |
| `confirmed_pairing_survives_restart_removal_and_same_device_rejoin` | 产品逻辑问题（成员展示），已修复（用户确认） | 同一设备被移除后重新加入，邀请方配对确认 120 秒内一直为空。`history.member_for_device` 返回凭据表中第一个映射到该设备的实例，可能是已移除的旧实例，设备因而显示为已移除，按有效实例索引的配对确认查不到；S3 之前的设备信任查询使用同一函数。按设备找实例的能力仍被离开中的设备与重复移除需要，因此不在调用方加兜底，而是修正该函数定义：同一设备的实例中当前有效者优先（`max_by_key`）。移除用例去掉“有效实例或历史实例”的两段查找，只保留一次查找加“有效或已有移除事件”的判定；决定来源核对改为直接核对签署实例所属设备（`device_for_member`）。新增 Core 回归测试（修复前稳定失败）；该场景 12.5 秒通过，R1–R4 与旧重复成员场景通过 |
| `same_device_returns_to_a_previous_space_after_switch_and_restart` | 产品问题，不属本计划 | 跨 Space 加入后重启，`Engine::start` 在 profile 密钥恢复启动检查中返回 `Corrupt`（1216），早于任何成员日志；全新加入后重启正常。相关文件正由另一项未提交工作修改，入站准入计划已登记为既有问题 |
| `suspend_during_space_switch_recovery_does_not_resurrect_the_network` | 产品问题，不属本计划 | 恢复时先检查未完成的 Space 切换，此时 Space 仍锁定，读取加入方激活与待恢复准入状态返回 `locked`，被映射为不可重试的 1103，恢复失败。属 Engine 运行期恢复顺序 |
| `pending_join_is_not_published_before_final_confirmation`（间歇） | 产品问题（组密钥存储短暂锁冲突），待决定 | 新增两条固定分类的运行诊断（用户确认）：设备信任查询依赖失败时记录依赖名与原因类别；Infra 读取组密钥投递状态失败时记录阶段、原因、来源。失败运行显示 `dependency="security_update_status" cause="key_epoch_repository"`，Infra 为 `source="storage" reason="unknown"`，发生在邀请方最终确认激活写入安全状态期间；通过的运行也出现过同样记录，只是测试恰好未在该时刻查询。数据库连接已设置 5 秒忙等，而投递状态读取路径先读后写加密索引，推断为延迟事务锁升级时的立即 BUSY（原始错误正文按隐私规则不记录，无法直接证实）。修复方向：该事务改为立即获取写锁，或状态读取不再写索引 |

验证：`cargo nextest run -p uc-core -p uc-application -p uc-infra` 2559 项全部通过；`membership-e2e` 全组 49/51，失败为
`same_device_returns_to_a_previous_space_after_switch_and_restart` 与
`suspend_during_space_switch_recovery_does_not_resurrect_the_network`（均不属本计划）；交付检查通过。

### S3.a 修复（2026-09-24，分支 `hp/uni/t-0010-android`）

| 问题 | 根因 | 修复与验证 |
| --- | --- | --- |
| `pending_join_is_not_published_before_final_confirmation`（间歇） | 组密钥投递索引在延迟事务中先读后写；WAL 下读后升级写锁遇到其他连接已提交的写入会立即返回 BUSY，忙等不生效 | 两处索引事务改为立即获取写锁。新增并发读写测试，修复前稳定失败；该场景连续 5 次通过 |
| `suspend_during_space_switch_recovery_does_not_resurrect_the_network`（1103） | 跨 Space 激活以目标访问材料覆盖资料 KEK，资料 vault 根密钥仍由旧 KEK 包裹；挂起清空内存中的 vault 后无法重新打开，准入状态读取失败被误报为“已锁定” | 替换 KEK 的步骤前后按口令修改的两阶段协议重新包裹 vault 根密钥（尚无 vault 时不处理）；存储测试覆盖挂起、重启与幂等重试，修复前失败 |
| `same_device_returns_to_a_previous_space_after_switch_and_restart`（1216） | 同上，重启时资料密钥恢复无法打开 vault | 同上修复后不再返回 1216，随后暴露下一项 |
| 同一场景的加入拒绝 | 设备未经移除重新加入时，签名历史的有效成员同时保留新旧两个实例；成员账本按实例排除本机，旧实例被当成对端，其设备即本机，起始校验失败 | 成员账本的对端改为按设备排除本机（用户选择）。新增 Core 回归测试，修复前返回 `InvalidSnapshot`；场景通过 |

同批修正了掩盖上述根因的吞错与误分类（用户要求）：

- 准入密钥错误保留下层来源；安全存储报告数据损坏时按损坏分类，不再显示为暂不可用或已锁定。
- 资料 vault 自动打开失败时，Core 安全存储端口只能携带固定文本，改为在转换前记录固定分类诊断（阶段与原因类别）。
- 组密钥安全存储不再字符串化下层错误；事务内部返回的 `KeyEpochError` 保持原分类。
- 加入方准备阶段的不一致按安全材料、成员历史、成员关系标注，分别拒绝为 `SecurityMaterialInvalid`、
  `MembershipHistoryInvalid`、`RelationshipConflict`，并记录固定分类诊断；未标注的不一致拒绝为
  `ActivationStateInvalid`，不再一律视为关系冲突。

验证：`cargo nextest run -p uc-core -p uc-application -p uc-infra` 全部通过；`membership-e2e` 全组 51/51（此前一次全组运行中
`f1_remove_and_add_from_parent_head_preserve_branch_membership` 在负载下等待组密钥纪元超时，单独运行 3/3 通过，复跑全组通过）；
交付检查通过。其余同类吞错由错误来源保留计划统一清点。

### S4（2026-09-25，分支 `hp/uni/t-0010-android`）

交接方式（用户确认，写入 [ADR-027](../../design-docs/decisions/027-single-owner-space-membership-state.md#准入交接)）：

- **邀请方**：S3 已先提交成员事实再保存准入终态。S4 删除 Infra 激活中另写成员资料的一步
  （`apply_member_facts`），成员读模型只由 Owner 提交在同一事务中维护。
- **加入方**：目标控制世代只写安全材料与准入凭据，删除 `material.rs` 的成员记录构造与读模型写入，以及
  准入切换输入中不再使用的 `target_membership_history`。激活执行提升目标世代后，Application 让 Owner 重新
  加载并以 `join_space` 建立本机成员状态，成功后才保存准入记录；重放时识别同一加入不产生变化。
- 加入后历史需要本机激活回执（参与历史位置摘要），而进入激活中后准入记录不再保存 Applied 消息。加入方
  暂存目标新增 V3，在激活准备时写入回执；Core `accept_complete` 接收补全后的暂存目标。V2 自
  `v1.1.0-rc.16` 起已冻结，V3 是本分支对该格式唯一的新版本；旧版本准备、目标世代已带成员记录的 V2
  激活只核对一致。
- **分叉换组**：检查点保存在成员记录中，提升后再由 Owner 输入会让崩溃后的恢复读到旧检查点，因此改为
  Owner 以 `stage` 在当前状态上形成目标世代的记录（采用目标分支历史、检查点推进到 `TargetStaged`）与读模型
  计划，Infra 在同一事务中原样写入目标世代；删除 Infra 中的 `BranchRecovered` 推进、检查点补推与关系表
  改写。
- **Owner 重新加载**：Owner 此前在控制世代切换后仍使用旧世代的已发布状态，换组提升后的第一次提交因修订号
  冲突失败，下一轮才恢复。新增 `reload`，加入方激活与换组提升后都先重新加载。
- 架构检查：`StagedMembershipRecord` 只由 Owner 构造；Infra 在成员记录编解码之外不得调用
  `MembershipLedger::start`/`apply`、`LedgerInput` 或拼装 `SpaceMembershipRecord`；两项均有反例。

新增测试：加入方“成员已提交、准入记录保存失败”后重放只提交一次；成员提交失败时准入记录不变、重试后
完成；邀请方两种故障注入同样核对只有一次加入；暂存目标 V2 无回执、V3 保留 V2 字段与回执、未知或截断
格式无效。故障注入以同一持久记录上的重试模拟重启（Owner 测试台的记录跨调用保留）。

验证结果：

| 检查 | 结果 |
| --- | --- |
| `cargo nextest run -p uc-core -p uc-application -p uc-infra --locked` | 2591 项全部通过 |
| `cargo nextest run -p uc-engine --locked`（含 `public_contract`、宿主契约） | 358 项全部通过 |
| `bash scripts/testing/run-test-group.sh membership-e2e` | 第一次 50/51：`f2_concurrent_leaf_removals_resolve_to_selected_branch` 在负载下 73.7 秒未清除分叉待选（快照断言超时）；单独运行 3/3 通过，耗时 56.2/56.2/57.6 秒，与改动前保留产物中的 55.7/58.6 秒一致。第二次全组 51/51 |
| `cargo check -p uc-infra --features lan-compat --all-targets --locked` | 通过 |
| `cargo metadata --locked`、`cargo check --workspace --all-targets --locked`、`cargo fmt --all -- --check`、`check-rust-style.mjs`、`check-engine-repository.mjs`、`git diff --check` | 通过（`uc-ohos-napi` 测试既有未使用导入告警，非本次改动） |
| 实体双 Desktop 复现场景 | 跳过（需另行授权） |

### S5（2026-09-25，分支 `hp/uni/t-0010-android`）

开工前核实（用户确认）：

- 移除通知由移除方经成员历史协议主动出站投递（`RestrictedEventV3`，同一流上收 Ack），正在离开的对端
  不在普通成员范围内，只收到这一条通知；这部分 S3 已实现。入站方向无需新增放行规则：查账本的协议
  （在线确认、剪贴板、成员证明、拉取、传输进度）已拒绝它；只核对身份的成员历史交换与分支恢复照常
  识别它，历史分页一律 `Invalid`，被移除设备送来的决定仍只贡献已验证历史，不能收紧。
- 身份解析候选取与成员读模型相同的一组设备（方案 A），不扩大到历史中全部有准入事实的设备，入站准入计划
  的拒绝分类不变。

完成内容：

- Application：`PeerAccess` 读取 Owner 发布的状态，同时实现 `PeerAdmissionPort` 与新增的
  `PeerIdentityDirectoryPort`（候选由成员读模型计划得出）。网络入口先于 Space 应用组装，因此 Engine 先构造
  未绑定的 `PeerAccess`，`SpaceApplication` 建立 Owner 后绑定；未绑定时按暂不可用拒绝。此前 `PeerAccess`
  每次入站都从数据库加载、解码并校验整条记录。
- Owner 数据库代号：替换控制库的路径共五条（加入方激活、分叉换组、设备管理重置、取消加入改用临时库、
  恢复出厂改用临时库），S4 只为前两条显式重新加载。改为连接池每次替换数据库时推进代号，成员记录存储端口
  暴露代号，Owner 的已发布状态带代号、代号变化即重新加载并通知读取方；删除 S4 的 `reload`。否则入站判定
  改读 Owner 后，恢复出厂等路径会在重启前继续放行旧成员。
- Infra：`PeerIdentityResolver` 与九个入站入口、mDNS 连接提示改读身份目录，不再依赖 `MemberRepositoryPort`。
  测试以内存成员表充当身份目录（`member_table_directory`、`member_table_identity_directory!`），种子数据与断言
  不变。
- 测试：Owner 在数据库替换后重新加载并通知、同代号不重复加载；身份目录覆盖本机与当前成员、正在离开的
  对端在离开窗口结束后才移出；未绑定时拒绝；移除提交后入站判定立即拒绝而身份仍可识别（不依赖读模型）；
  连接池代号在替换与改用临时库时改变、替换失败时不变。A3 新增“入站身份不读取成员读模型”的结构检查。
- 入站准入测试的一处签名细节调整：B1 `resolution_failures_keep_their_typed_source` 的读取失败来源由
  `MembershipError` 变为身份目录的 `MembershipLedgerError`，原始 `MembershipError` 保留在其来源链中；断言改为
  沿来源链查找，语义不变。A3 的 `PeerAccess` 断言因类型沿用该名称无需修改。

验证结果：

| 检查 | 结果 |
| --- | --- |
| `cargo nextest run -p uc-core -p uc-application -p uc-infra -p uc-observability-contract -p uc-engine --locked` | 3025 项全部通过 |
| `cargo test -p uc-infra --locked --test inbound_peer_rejection_diagnostics --test inbound_peer_single_owner -- --include-ignored` | 1/1、5/5，无 ignored 项 |
| `bash scripts/testing/run-test-group.sh membership-e2e` | 51/51 |
| `cargo check -p uc-infra --features lan-compat --all-targets --locked` | 通过 |
| `cargo metadata --locked`、`cargo check --workspace --all-targets --locked`、`cargo fmt --all -- --check`、`check-rust-style.mjs`、`check-engine-repository.mjs`、`git diff --check` | 通过（`uc-ohos-napi` 测试既有未使用导入告警，非本次改动） |
| 实体双 Desktop 复现场景 | 跳过（需另行授权） |
