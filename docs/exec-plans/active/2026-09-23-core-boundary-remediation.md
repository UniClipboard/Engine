# Core 边界收口

## 状态与完整责任

- **状态**：提议。违规清单已完成第一轮盘点，尚未开始修复。
- **日期**：2026-09-23。
- **依据**：[Core 设计规范](../../design-docs/layers/core.md)已重写。旧版本自身有冲突：§9.1 称“配对状态机不属于 core”，§4.5 又要求有生命周期的状态机在 Core 用 `apply` 建模；§2.2、§6.3 禁止序列化格式进入 Core，§4.5.3 又要求 Core 保证序列化布局稳定。本计划列出现有代码与新规范之间的全部已知差距。
- **完整负责人**：每一项由表中“目标负责人”列出的模块负责；整体顺序与验收由本计划负责。
- **调用方唯一动作**：不新增对外调用。各项修复不得改变 Engine 公开接口与宿主行为。
- **成功结果**：清单每一项被修复，或者被明确判定为规范例外，并在规范中写明例外；下文“自动检查”在 CI 中阻止同类问题回归。
- **失败结果**：某项修复需要改变持久化格式或设备间协议时，先按[持久化格式演进](../../design-docs/engineering-principles.md#持久化格式演进)单独立项，不在本计划里夹带格式变更。
- **重启与重试责任**：本计划只做结构调整，不新增持久事实。涉及已落盘记录的项必须保持现有格式可读。

## 盘点方法与覆盖范围

- `uc-core`：全量扫描生产代码（去掉 `#[cfg(test)]` 之后的部分，约 3.4 万行）。检查依赖、IO、时钟与随机数、panic 类调用、port 被消费的情况、状态字段可见性、编解码与格式常量、注释语言。
- `uc-infra`、`uc-application`：针对 Core 类型做定向扫描，包括直接写入 Core 状态字段、在成员历史上自行做判定、在上层维护持久生命周期等。**这部分不是全量审计**，只覆盖成员、准入与安全相关代码；剪贴板、搜索、传输等领域的上层规则外泄尚未扫描。
- 行号以 2026-09-23 的工作区为准，修改后可能漂移。

## 违规清单

优先级：**P0** 规则放在 Core 之外或与声明不一致，可能导致多设备行为错误；**P1** 结构违规，使规则读不全或可以被绕过；**P2** 纪律问题。

### A. 规则外泄到 Infra / Application（P0）

| # | 违规 | 证据 | 目标负责人 |
| --- | --- | --- | --- |
| A1 | 新成员历史 head 产生后，旧 ACK 失效、重建逐设备传播义务的规则写在 Infra | [`sponsor/complete.rs`](../../../crates/uc-infra/src/space/admission/sponsor/complete.rs) `activate_inner` 重建 `peer_reconciliation`：清空 `confirmed_position`，登记 `pending_since_revision` | Core membership 给出新 head 下每个设备的传播义务；Application ledger 保存 |
| A2 | “哪些成员接收安全更新”的规则在 Infra 实现了至少三份 | [`sponsor/candidate.rs:159`](../../../crates/uc-infra/src/space/admission/sponsor/candidate.rs)、[`space_control_generation/mod.rs:960`](../../../crates/uc-infra/src/security/space_control_generation/mod.rs)、[`space/security/access.rs:3358`](../../../crates/uc-infra/src/space/security/access.rs)，各自从 `active_members()` 排除本机、重新加入设备或本机签名密钥 | Core 提供唯一的接收者集合函数 |
| A3 | 由成员历史投影出 `SpaceMember`、`TrustedPeer` 的规则在 Infra | [`space_control_generation/mod.rs:940`](../../../crates/uc-infra/src/security/space_control_generation/mod.rs)、[`membership_member_facts.rs:79`](../../../crates/uc-infra/src/space/adapters/membership_member_facts.rs) | Core 提供“历史 → 当前成员与可信设备集合”的投影 |
| A4 | 网络准入判定写在 Infra | [`space/security/peer_admission.rs:32`](../../../crates/uc-infra/src/space/security/peer_admission.rs)：`local_join_active`、有 lineage、关系为 `Consistent` 才放行 | 已由[入站对端准入计划](../completed/2026-09-23-inbound-peer-admission.md)迁往 Application ledger。按新规范，判定式本身应由 Core 提供，Application 只负责读取；实施该计划时一并处理 |
| A5 | 空间切换的下一阶段由 Infra 选择，并直接写入 Core 的 `phase` 字段 | [`v3_admission_space_transition.rs:333`](../../../crates/uc-infra/src/security/v3_admission_space_transition.rs) 的 `next.phase = phase`，以及 `match transition.phase` 选择下一步；Core 只提供 `can_advance_to` 守卫 | Core `cross_space_transition` 用 `apply` 决定下一阶段；Infra 只执行对应能力 |
| A6 | 邀请只能被领取一次的判定写在 Infra | [`sponsor/state.rs`](../../../crates/uc-infra/src/space/admission/sponsor/state.rs) 的 `claimed_invitations`；Core 的 `PairingInvitation::consume` 在生产代码中没有调用方 | Core 给出跨记录判定；Infra 在事务中执行 |
| A7 | 持久生命周期定义在 Application，且可被直接改写 | [`ledger/model.rs`](../../../crates/uc-application/src/space/membership/ledger/model.rs) 中的 `MembershipConflictStatus`、`MembershipEffectPhase`、`PeerHistorySyncState`、`MembershipBranchRecoverySessionState` 都是公开字段，被 7 个文件直接改写（如 `ledger/effects.rs:88`、`recover_conflict/use_case.rs`） | 状态与转换迁入 Core；Application 保存并推进 |

### B. 效果义务没有约束力（P0）

| # | 违规 | 证据 | 目标负责人 |
| --- | --- | --- | --- |
| B1 | Sponsor 端生产代码不读取 `AdmissionEffect`；只有 Joiner 的两处做了断言 | [`joiner/activation_state.rs:55`](../../../crates/uc-infra/src/space/admission/joiner/activation_state.rs)、[`joiner/cancellation.rs:77`](../../../crates/uc-infra/src/space/admission/joiner/cancellation.rs)；Sponsor 路径没有任何读取 | Application `SpaceAdmissionProtocol` 按效果履行 |
| B2 | 声明与实际顺序不一致：`settle_complete_ack` 声明 `ActivateSecurity`，实际激活发生在该转换**之前** | [`handle_complete_ack/execute.rs:53-65`](../../../crates/uc-application/src/space/admission/protocol/sponsor/handle_complete_ack/execute.rs) 先 `activate`，再转换，最后提交 | Core 把效果标为 `BeforeCommit`，或改成由转换驱动 |
| B3 | 效果没有声明与保存的先后关系 | [`state/aggregate.rs`](../../../crates/uc-core/src/membership/space_admission/state/aggregate.rs) `AdmissionEffect` 为扁平枚举 | Core |

### C. 状态机没有单一入口，或状态可被绕过（P1）

| # | 违规 | 证据 |
| --- | --- | --- |
| C1 | 准入聚合有 44 个 `pub(crate)` 转换方法，以及角色包装上的 65 个公开方法 | [`state/transition/`](../../../crates/uc-core/src/membership/space_admission/state/transition/)、[`state/capability.rs`](../../../crates/uc-core/src/membership/space_admission/state/capability.rs) |
| C2 | 撤销记录通过 `mark_migrating`、`mark_ready`、`transition_to`、`acknowledge_*`、`settle_obsolete_recipients` 等多个入口推进 | [`membership/revocation.rs`](../../../crates/uc-core/src/membership/revocation.rs) |
| C3 | 空间切换类型公开 `phase` 字段，可被任意改写 | [`cross_space_transition.rs`](../../../crates/uc-core/src/membership/cross_space_transition.rs) 中 6 个 `pub phase` |
| C4 | `SpaceMembershipState` 已有 `apply`，但 `phase` 等字段仍然公开，可以绕过 | [`workspace_convergence.rs:130`](../../../crates/uc-core/src/membership/workspace_convergence.rs) |
| C5 | `MembershipBranchTransitionV1::advance(phase)` 接受任意目标阶段 | [`membership_branch_transition.rs:98`](../../../crates/uc-core/src/membership/membership_branch_transition.rs) |
| C6 | 投递状态公开 `status` 字段 | [`clipboard/delivery.rs:71`](../../../crates/uc-core/src/clipboard/delivery.rs) |
| C7 | `PendingMembershipBatch::mark_retry` 在 `apply` 之外修改状态 | [`gossip.rs:412`](../../../crates/uc-core/src/membership/gossip.rs) |
| C8 | `PairingInvitation` 的生命周期在生产流程中被绕过：只用作数据容器，`consume`、`try_expire` 没有调用方 | [`pairing/invitation/invitation.rs`](../../../crates/uc-core/src/pairing/invitation/invitation.rs)、[`invitation/holder.rs`](../../../crates/uc-application/src/space/admission/invitation/holder.rs) |

### D. 存储格式与编码在 Core（P1）

| # | 内容 | 判定 |
| --- | --- | --- |
| D1 | 准入记录的持久化编解码，约 3200 行，另有 `AdmissionRecordPersistence` trait（`encode_persisted`、`decode_persisted`） | [`space_admission/state/persistence/`](../../../crates/uc-core/src/membership/space_admission/state/persistence/)，迁往 Infra；Core 改为快照与 `restore` |
| D2 | 空间切换、成员分支转换与恢复包、生成清单、内容密钥目录的格式常量与编解码 | [`cross_space_transition.rs`](../../../crates/uc-core/src/membership/cross_space_transition.rs)、[`membership_branch_transition.rs`](../../../crates/uc-core/src/membership/membership_branch_transition.rs)、[`membership_branch_recovery.rs`](../../../crates/uc-core/src/membership/membership_branch_recovery.rs)、[`active_space_generation_manifest.rs`](../../../crates/uc-core/src/membership/active_space_generation_manifest.rs)、[`admission_content_key_catalog.rs`](../../../crates/uc-core/src/membership/admission_content_key_catalog.rs)。逐项判定：只在本机存储的迁往 Infra；在设备间传递且受签名或摘要覆盖的留在 Core，并补字节稳定测试 |
| D3 | 成员历史的持久化与交换编码 | [`versioned_membership_history/archive.rs`](../../../crates/uc-core/src/membership/versioned_membership_history/archive.rs)、[`exchange/`](../../../crates/uc-core/src/membership/versioned_membership_history/exchange/)。规范签名材料留在 Core；`encode_persisted_v2` 需判定是规范编码还是存储格式 |
| D4 | 剪贴板线上协议 | [`network/protocol/clipboard_payload_v3.rs`](../../../crates/uc-core/src/network/protocol/clipboard_payload_v3.rs)，迁往 Infra |
| D5 | 文件展示元数据用 JSON 编解码 | [`clipboard/file_display_metadata.rs:15`](../../../crates/uc-core/src/clipboard/file_display_metadata.rs)，迁往 Infra |
| D6 | 设置模型承担旧设置文件的兼容解析 | [`settings/`](../../../crates/uc-core/src/settings/)，把文件格式兼容迁往 Infra，Core 只保留设置的语义 |

D1 与 D3 同时接手[错误来源保留](../completed/2026-09-24-error-source-preservation.md)移交的 90 处 `map_err(|_| ..)`
（准入记录、成员历史与内容密钥目录的持久化编解码，含全部 postcard 解码点）。迁往 Infra 时保留下层来源
（Infra 自有错误可直接使用 `anyhow`），不再按例外丢弃；完成后按文件模式复扫 `check-rust-style.mjs` 确认清零。

### E. 不属于 Core 的模块或依赖（P1）

| # | 内容 | 证据 | 目标 |
| --- | --- | --- | --- |
| E1 | 运行时任务管理 | [`task_registry.rs`](../../../crates/uc-core/src/task_registry.rs)，依赖 tokio `JoinSet`、`select!`、`Instant::now` | Application 或 Engine |
| E2 | 应用目录与路径布局 | [`app_dirs/mod.rs`](../../../crates/uc-core/src/app_dirs/mod.rs)（`PathBuf`、`device_id.txt` 等文件名） | Infra |
| E3 | 配置 DTO（`PathBuf`、端口号、TOML 映射） | [`config/mod.rs`](../../../crates/uc-core/src/config/mod.rs) | Infra 或 Engine |
| E4 | 移动同步兼容线的实现细节：SyncClipboard 路由、Argon2、LAN 探测 | [`mobile_sync/`](../../../crates/uc-core/src/mobile_sync/)、[`ports/mobile_sync.rs`](../../../crates/uc-core/src/ports/mobile_sync.rs) | 对应兼容负责人；Core 只保留主产品规则需要的概念 |
| E5 | 文件 IO | [`clipboard/system.rs:264`](../../../crates/uc-core/src/clipboard/system.rs) `std::fs::File::open` 流式计算哈希 | 由调用方提供哈希，或迁往 Infra |
| E6 | Core port 使用 tokio 通道 | [`ports/peer_reachability.rs`](../../../crates/uc-core/src/ports/peer_reachability.rs)、[`ports/search/search_index.rs`](../../../crates/uc-core/src/ports/search/search_index.rs)、[`ports/clipboard/sync_receiver.rs`](../../../crates/uc-core/src/ports/clipboard/sync_receiver.rs)、[`ports/clipboard/active_clipboard/receiver.rs`](../../../crates/uc-core/src/ports/clipboard/active_clipboard/receiver.rs) | 随 F1 迁往 Application 后解除 |
| E7 | 在 Core 内随机生成标识 | [`ids/id_macro.rs:8`](../../../crates/uc-core/src/ids/id_macro.rs)、[`membership/revocation.rs`](../../../crates/uc-core/src/membership/revocation.rs) 4 处、[`membership/bootstrap.rs:26`](../../../crates/uc-core/src/membership/bootstrap.rs)、[`mobile_sync/staged_file.rs:66`](../../../crates/uc-core/src/mobile_sync/staged_file.rs) 调用 `Uuid::new_v4()` | 由调用方传入 |
| E8 | 被禁止的依赖 | [`Cargo.toml`](../../../crates/uc-core/Cargo.toml)：`tokio`、`tokio-util`、`anyhow`、`serde_json`、`toml`、`url`、`bytes`、`uuid`（带 `v4`） | 随上面各项迁出后删除 |

### F. Port 放置（P1）

| # | 违规 | 证据 |
| --- | --- | --- |
| F1 | Core 定义了 171 个 trait。按“Core 领域代码直接调用”的标准，只有 `HistoricalMembershipSignatureVerifier` 明确合格；其余 port 只被 Application、Infra 使用，或者只在 Core 注释中被提及 | 以 `crates/uc-core/src/ports/`、[`membership/ports.rs`](../../../crates/uc-core/src/membership/ports.rs) 为主。逐个迁往使用它的 Application 业务模块，按模块分批进行 |
| F2 | 策略被建模成 port 并由 Core 自己实现 | [`clipboard/policy/v1.rs:245`](../../../crates/uc-core/src/clipboard/policy/v1.rs) `impl SelectRepresentationPolicyPort`，应改为普通策略函数或类型 |
| F3 | Port 注释写了调用方、协议或实现 | [`ports/mobile_sync.rs`](../../../crates/uc-core/src/ports/mobile_sync.rs)（HTTP 路由、SyncClipboard、Argon2、OsRng）、[`ports/connection_channel.rs`](../../../crates/uc-core/src/ports/connection_channel.rs)（iroh、Infra 文件路径）、[`ports/local_identity.rs`](../../../crates/uc-core/src/ports/local_identity.rs)（具体 UseCase 名）、[`ports/pairing_invitation.rs:193`](../../../crates/uc-core/src/ports/pairing_invitation.rs)、[`ports/peer_address.rs`](../../../crates/uc-core/src/ports/peer_address.rs)、[`ports/first_sync_state.rs`](../../../crates/uc-core/src/ports/first_sync_state.rs)、[`ports/blob/transfer.rs`](../../../crates/uc-core/src/ports/blob/transfer.rs) 等 |

### G. 纪律（P2）

| # | 违规 | 证据 |
| --- | --- | --- |
| G1 | 生产代码中的 panic 类调用 | [`ids/device_id.rs:38`](../../../crates/uc-core/src/ids/device_id.rs)：`DeviceId::new` 超长时 panic，而设备 ID 可能来自外部输入；[`clipboard/hash.rs:33-68`](../../../crates/uc-core/src/clipboard/hash.rs)：解析 panic 和 `expect`；[`clipboard/system.rs:197`](../../../crates/uc-core/src/clipboard/system.rs)、`:250`；[`membership/revocation.rs:699`](../../../crates/uc-core/src/membership/revocation.rs)、[`state/capability.rs:224`](../../../crates/uc-core/src/membership/space_admission/state/capability.rs) 的 `unreachable!` |
| G2 | 死代码 | `TrustedPeerEvent`、`TrustAbortReason` 在 Core 之外没有使用者；`InvitationEvent` 在 Core 之外只出现在一个 UseCase 中 |
| G3 | 注释语言 | 133 个生产文件中有约 1400 行纯英文注释 |
| G4 | 公开错误文本含设备 ID | `DeviceId::new` 的 panic 文本包含原始输入，需要确认是否符合[安全架构](../../SECURITY.md)的日志规则 |

## 执行顺序

1. **自动检查先行**（见下文“自动检查”）：先让检查以“只报告”模式运行，冻结违规数量，禁止新增。
2. **P0**：B3 → B1、B2（效果带先后关系，并由 Application 履行）；A1、A2、A3（集中成员历史的派生规则）；A5、A6；A4 随入站准入计划推进；A7 单独立项。
3. **P1**：C1 与 D1 一起做（准入聚合改为 `apply` 加快照，编解码迁往 Infra），格式保持可读，不新增版本；随后依次处理 C2–C8、D2–D6、E、F。
4. **P2**：随所在模块的改动顺手清理；G3 可以单独批量处理。

每一项修复都必须同时删除旧入口，不保留新旧两套实现。

## 验收

- 清单每一项在本文中标为“已修复”或“规范例外（链接到规范条款）”。
- 准入、撤销、空间切换、成员候选各有转换矩阵测试，断言完整效果列表及先后关系。
- Application 有测试证明：每种 `BeforeCommit` 效果失败时，不会保存新状态。
- 现有持久化记录能被读取，有旧格式夹具测试。
- “自动检查”在 CI 中以阻断模式运行。

## 自动检查（待实现）

在 `scripts/architecture/` 中新增 Core 检查，覆盖：

- `uc-core` 的依赖白名单（对应规范 §3）；
- 生产代码中禁止 `std::fs`、`std::env`、`Utc::now`、`Instant::now`、`Uuid::new_v4`、`tokio::`、`panic!`、`unwrap()`、`expect(`、`unreachable!`；
- 状态类型不得有 `pub phase`、`pub status`、`pub state` 字段；
- Core 中的 `*Port` trait 必须在 Core 非 port 代码中被引用，否则需要登记为例外；
- Infra、Application 不得对 Core 状态类型的字段赋值。
