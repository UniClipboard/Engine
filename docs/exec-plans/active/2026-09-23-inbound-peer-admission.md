# 入站对端身份与网络准入的唯一负责人

## 状态

- **状态**：实施中；S1–S5a 已实现并验证，全部切片后的收尾测试与生产路径检查待执行，S5b 待现场核实（见“实施记录”）
- **日期**：2026-09-23
- **来源问题**：t-0028 双 Desktop 现场。配对最终返回成功后，邀请方的在线确认被加入方拒绝 748 次（`peer_not_admitted`）；加入方主动连接 233 次全部握手超时；双方公开状态却都显示对端 `active`/`usable`。只读诊断记录见 Desktop 线程 `.herdr-project/uni-t-0028/report.md` 顶部。
- **已证实**：
  - 网络准入不是快照，每次入站都实时读取成员账本。
  - 拒绝方是加入方。
  - 现场证据排除了“账本不放行”分支和“连接开关关闭”分支。
- **未证实（本计划不以其为前提）**：邀请方成员历史中的身份和 `dev` 当前网络身份不一致。本计划的 S4、S5a 能在今后把这类不一致直接暴露出来，但不据此宣称已找到现场根因。
- **完整负责人**：
  - 入站身份解析与拒绝分类：Infra `crates/uc-infra/src/network/iroh/inbound_peer.rs`（新增），负责“远端公钥 → 设备 → 是否放行”，所有入站协议共用。
  - 准入规则：Application `MembershipLedger`，网络准入与公开“可用”从同一判定得出。
  - 配对时的身份一致性：Infra 加入方激活准备 `DefaultJoinerActivationPreparation`。
  - 本机身份自检：Application `QueryDeviceTrustUseCase`，经既有 `spaceDeviceUpdate` 公开（S5a）。恢复方式另由 S5b 决定，本计划不预设。
- **调用方唯一动作**：宿主不新增调用。宿主继续只读 `spaceDeviceUpdate`，只新增一个原因取值 `local_identity_mismatch`，不新增恢复取值。
- **成功结果**：
  - 每次入站拒绝，拒绝方本地恰好留下一条带固定原因的诊断。
  - 网络准入与公开“可用”不再各算一份。
  - 配对时发现邀请方身份不一致，直接终止为 `IdentityConflict`。
  - 本机身份与历史不一致，公开状态显示“需要处理”并给出原因，不再显示“稍后自动重试”；本机诊断在状态变化时留下一条记录。
- **失败结果**：
  - 诊断采集失败不改变业务结果。
  - 账本、成员投影或本机身份读取失败时一律拒绝，并保留 source chain（错误来源链）。
- **重试与重启责任**：
  - 不新增重试。
  - 身份不一致属于自动重试无法解决的状态。维护调度照常运行，但公开状态不再显示“稍后自动重试”；S5a 不自动修复，也不引导任何破坏性操作。
  - 所有判定都实时读取持久状态，重启后结果相同。

## 范围

### 目标

1. **可观测性**：新增运行诊断事件 `peer.inbound.rejected`，区分 7 类拒绝原因。
2. **架构**：
   - 9 处各自手写的“公钥 → 设备”解析收敛到一个 Infra 模块。
   - 准入规则从 Infra 的 `MlsPeerAdmissionAdapter` 移到 Application 的 `MembershipLedger`，与 `derive_current_scope` 共用同一谓词。
3. **设计不变量**：
   - 加入方激活前校验：成员历史中的邀请方指纹，必须等于其续连端点公钥的指纹。
   - 查询时校验：本机当前身份指纹，必须等于成员历史中的本机指纹。
4. **代码**：
   - 身份解析返回带类型的结果，区分找不到、有歧义、读取失败和指纹派生失败。
   - 删除各协议里的重复实现。

### 非目标

- 不改设备间协议。拨号侧仍只收到原有拒绝字节，`presence.check.completed` 的 `peer_not_admitted` 保持不变；原因只记录在拒绝方本地。
- 不改数据库 schema、成员历史格式或持久化格式版本。唯一例外是 S4 在准入记录的本地终止原因取值表中补充 `IdentityRejected`（编码 9）；该表的 3–8 同样由本分支新增、尚未随任何发布冻结，编码 9 属于补全同一未发布目标格式，不新增版本（见“实施记录”S4）。
- 不改变哪些协议需要账本准入。成员历史交换和分支恢复现在只校验身份、不查账本，保持不变。
- 不修复 t-0028 现场，不给出现场根因结论，也不提供“自动修正身份”的恢复动作。
- 不清理准入分支以外的既有调试日志（例如解码失败时打印的 `peer_device_id`）。
- Desktop、移动端对新取值的界面适配由各产品仓负责，本仓只提供稳定线值和文档。

## 验收测试清单

测试分两类：

- **仓内（已编译）**：已放进仓库，可以编译；未完成的断言标了 `#[ignore = "inbound-peer-admission Sx: …"]`。
- **暂存（未编译）**：依赖本计划新增的 API，存放在 `2026-09-23-inbound-peer-admission/staged-tests/` 下。目录结构与落地位置一一对应，实施对应切片时原样移入仓库。
- **已落地**：暂存测试已移入仓库并通过，暂存副本已删除。

实施者规则：

- **不得修改任何断言语义。** 只有签名细节与本文冲突、且必须调整时才允许改动，调整原因须写进本文“实施记录”。
- 暂存测试在移入仓库之前，就是该切片的规格。

| 编号 | 位置 | 类型 | 证明什么 | 切片 |
| --- | --- | --- | --- | --- |
| A1 | `crates/uc-observability-contract/tests/inbound_peer_rejection_contract.rs` | 仓内 | 新事件的名称、级别、封闭取值、固定字段集；未知值和附加字段被拒 | S1 |
| A2 | `crates/uc-infra/tests/inbound_peer_rejection_diagnostics.rs` | 仓内 | 真实 iroh 下 presence 和剪贴板的 10 种拒绝，各自在导出文件中恰好一条记录；对端拒绝字节不变；身份失败不查账本；不泄露标识 | S1+S2 |
| A3 | `crates/uc-infra/tests/inbound_peer_single_owner.rs` | 仓内 | 9 个入站文件不再自带解析；拒绝分支不打印对端标识；Infra 准入副本已删除；激活准备调用了身份校验 | S2–S4 |
| A4 | `crates/uc-engine/tests/space_device_update_identity_contract.rs` | 仓内 | `local_identity_mismatch` 的稳定线值，且不带恢复动作；S5b 之前不接受 `re_pair`；既有取值不变 | S5a |
| A5 | `crates/uc-observability-contract/tests/local_identity_check_contract.rs` | 仓内 | 自检诊断 `space.local_identity.changed`：不一致为 WARN，恢复为 INFO，只有固定字段，拒绝指纹和设备标识 | S5a |
| B1 | `crates/uc-infra/src/network/iroh/inbound_peer/tests.rs` | 已落地 | 身份门的判定表：唯一、未知、歧义（与顺序无关）、读取失败、派生失败、拒绝、不可用；身份失败不访问账本 | S2 |
| B2 | `crates/uc-application/src/space/membership/ledger/peer_admission_tests.rs` | 已落地 | 准入与 scope 同源；本机失效时不放行任何对端；历史外设备不放行；没有版本来源时不走缓存；可用设备一定被放行；Engine 用的构造函数规则一致 | S3 |
| B3 | `crates/uc-infra/src/space/admission/joiner/sponsor_identity/tests.rs` | 已落地 | 续连端点与历史指纹一致才放行；不一致或无法解码都终止为 `IdentityConflict`，并保留类型化 source | S4 |
| B4 | `crates/uc-application/src/space/membership/query_device_trust/local_identity_tests.rs` | 已落地 | 本机身份被替换时显示需要处理、没有恢复动作，并且优先于可重试状态；尚无身份或本机未生效时不误报；读取失败保留 source；诊断只在状态变化时记录，不含指纹 | S5a |

当前基线（2026-09-23，本机实测）：A1–A5 可以编译，未标 ignore 的断言通过，`--ignored` 运行时全部因功能缺失而失败。B1–B4 未编译。

## 切片与步骤

切片按顺序进行，每个切片单独提交、单独通过门禁。每一步写明改哪个文件、改成什么，以及哪条验收由红转绿。

### S1 观测合同：`peer.inbound.rejected`

1. 新建 `crates/uc-observability-contract/src/diagnostics/connectivity/inbound_peer.rs`：
   - `pub enum InboundPeerProtocol { Presence, MembershipHistory, MembershipAttestation, MembershipBranchRecovery, Clipboard, ActiveClipboard, ActiveClipboardPull, TransferProgress }`，`#[serde(rename_all = "snake_case")]`，派生 `Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize`。
   - `pub enum InboundPeerRejectionReason { IdentityUnresolved, IdentityAmbiguous, MemberReadFailed, FingerprintUnavailable, LedgerDenied, LedgerUnavailable, NotAccepting }`，派生同上。
   - `fn phase(self) -> &'static str`：前四个原因返回 `identity`，后三个返回 `admission`。
   - `pub fn record_inbound_peer_rejection(protocol: InboundPeerProtocol, reason: InboundPeerRejectionReason)`：使用 `ObservationContext::capture()` 调用 `emit_local`。
2. `record.rs`：
   - `LocalEvent` 增加 `InboundPeerRejected { protocol: InboundPeerProtocol, reason: InboundPeerRejectionReason }`，序列化后 `kind` 为 `inbound_peer_rejected`。
   - `name()` 返回 `"peer.inbound.rejected"`；`level()` 返回 `"WARN"`。
   - `fields()` 只输出 `event.name`、`direction=inbound`、`protocol`、`uc.outcome=rejected`、`error.phase`、`error.reason` 六个字段。
3. `connectivity.rs`：声明 `mod inbound_peer;`，并 `pub use` 两个枚举和记录函数。
4. 在 `docs/design-docs/observability.md` 的运行诊断部分补一行事件说明：用途、字段、不含标识。
5. **验收**：去掉 A1 的全部 `#[ignore]`，`cargo test -p uc-observability-contract --test inbound_peer_rejection_contract --locked` 全部通过。

### S2 Infra 统一入站身份门（依赖 S1）

1. 新建 `crates/uc-infra/src/network/iroh/inbound_peer.rs`，在 `network/iroh/mod.rs` 中以 `pub(crate) mod inbound_peer;` 声明（不对外导出）。内容：
   - `pub(crate) enum InboundPeerRejection { IdentityUnresolved, IdentityAmbiguous, MemberReadFailed, FingerprintUnavailable, LedgerDenied, LedgerUnavailable, NotAccepting }`，派生 `Debug, Clone, Copy, PartialEq, Eq`。
     - `pub(crate) fn is_identity_failure(self) -> bool`
     - `fn diagnostic(self) -> InboundPeerRejectionReason`
   - `pub(crate) struct PeerIdentityResolver { member_repo, fingerprint_factory }`：
     - `new(member_repo, fingerprint_factory)`
     - `async fn identify(&self, remote_public_key: &[u8; 32]) -> Result<DeviceId, InboundPeerRejection>`：指纹派生失败返回 `FingerprintUnavailable`；`list()` 失败返回 `MemberReadFailed`；0 个匹配返回 `IdentityUnresolved`；匹配到 ≥2 个不同 `device_id` 返回 `IdentityAmbiguous`（与列表顺序无关）；恰好 1 个返回 `Ok`。
     - `fn fingerprint_matches(&self, remote_public_key: &[u8; 32], fingerprint: &IdentityFingerprint) -> bool`：供成员历史交换的“新成员自带准入信息”回退使用，以替代它手写的 `==` 比较。
   - `pub(crate) struct InboundPeerGate { protocol: InboundPeerProtocol, identity: PeerIdentityResolver, peer_admission }`：
     - `new(protocol, member_repo, peer_admission, fingerprint_factory)`
     - `identify`：委托给 resolver。
     - `async fn authorize(&self, device: &DeviceId) -> Result<(), InboundPeerRejection>`：账本返回 `Ok(false)` 映射为 `LedgerDenied`，返回 `Err` 映射为 `LedgerUnavailable`。
     - `async fn admit(&self, key) -> Result<DeviceId, InboundPeerRejection>`：先 identify 再 authorize；身份失败时不访问账本。
     - `fn record_rejection(&self, rejection)`：调用 S1 的记录函数。
   - `pub(crate) fn record_inbound_rejection(protocol, rejection)`：供只用 resolver、不走账本的协议使用。
   - **记录规则**：只在真正回写拒绝字节、拒绝回执或主动关闭连接的那一处记录，每次拒绝恰好一次。先记录，再回写。门的方法本身不记录；成员历史交换的回退成功时也不能产生记录。
2. 把 B1 移到 `crates/uc-infra/src/network/iroh/inbound_peer/tests.rs`，并在 `inbound_peer.rs` 末尾加 `#[cfg(test)] mod tests;`。**验收**：`cargo test -p uc-infra --lib inbound_peer --locked` 通过。
3. 逐个迁移以下协议，每个协议一次提交。构造函数签名一律不变，内部改为持有 `InboundPeerGate` 或 `PeerIdentityResolver`，并删除该文件中的 `resolve_device`、`is_admitted` 包装、`fingerprints_equal` 以及拒绝分支里带 `remote = %remote`、`peer = %device_id.as_str()` 的日志：
   1. `peer_reachability_adapter.rs`（`Presence`）：
      - 认不出身份：记录对应原因，再 `reject_admission`。
      - 账本不放行或不可用：记录 `LedgerDenied` 或 `LedgerUnavailable`（保留现有 `KnownPeerContact` 发送），再 `reject_admission`。
      - 代次变化、尝试已被撤销、`accepting=false`：记录 `NotAccepting`，再 `reject_admission`。
      - 写回 ACCEPTED 之后的二次复核失败：不记录。那里不回写拒绝字节，属于撤销竞态。
      - 拨号侧 `dial_and_track`、`ensure_reachable` 改用 `gate.authorize` 或保留端口调用，行为不变。
   2. `clipboard_receiver_adapter.rs`（`Clipboard`）：认不出身份和账本拒绝两处在 `emit_ack(Rejected)` 之前记录。
   3. `active_clipboard/receiver_adapter.rs`（`ActiveClipboard`）。
   4. `active_clipboard/pull_serve_adapter.rs`（`ActiveClipboardPull`）。
   5. `transfer_progress_adapter.rs`（`TransferProgress`）：原先 `list().await.ok()?` 会把读取失败吞成“未知”，改为保留类型。
   6. `membership_attestation_adapter.rs`（`MembershipAttestation`）：两条入站路径和第 747 行的“解析设备与声明设备不一致或未放行”分支。设备不一致时记录 `IdentityUnresolved`。
   7. `membership_branch_recovery_adapter.rs`（`MembershipBranchRecovery`）：只用 resolver，不增加账本检查。
   8. `membership_history_exchange_adapter.rs`（`MembershipHistory`）：只用 resolver。resolver 返回 `IdentityUnresolved` 时，再用 `fingerprint_matches` 走原来的回退；回退也失败，才记录并 `reject`。其他身份失败不走回退，直接记录并拒绝。
   9. `node.rs` 的 mDNS 连接提示：改用 `PeerIdentityResolver::identify`。`IdentityUnresolved` 和 `IdentityAmbiguous` 返回 `None`；读取失败和派生失败继续产出 `Err`。**不记录**拒绝事件：这不是入站连接。
4. 未跟踪文件 `crates/uc-infra/tests/peer_admission_identity_resolution.rs` 的 `duplicate-identity-stale-first` 阶段断言的是旧行为（会查询旧记录的账本）。该文件的其余覆盖面已被 A2 和 B1 取代。和该文件的作者确认后，删除它，或把该阶段改为断言“拒绝且不访问账本”。不要让两个测试对同一输入给出相反的期望。
5. **验收**：
   - 去掉 A2 的 `#[ignore]` 和 A3 前两个测试的 `#[ignore]`。
   - `cargo test -p uc-infra --test inbound_peer_rejection_diagnostics --test inbound_peer_single_owner --locked` 通过（A3 的 S3、S4 两项仍保持 ignore）。
   - 各协议原有测试全部保持通过。

### S3 准入规则归 Application 账本（可与 S2 并行，Cargo 验证串行）

1. 在 `ledger/repository.rs` 中提取唯一谓词 `VerifiedMembershipLedger::admits_peer(&self, device: &DeviceId) -> bool`。以下条件全部满足时返回真：
   - 存在已验证历史；
   - `local_member_active` 为真，与 `derive_current_scope` 中的同名计算完全一致，建议抽成一个私有函数供两处共用；
   - `device` 是当前生效成员（`effective_member_for_device` 存在，且准入信息中的设备与之一致）；
   - `peer_reconciliation[device].relationship == Consistent`。
2. 重写 `derive_current_scope`：对端进入 usable 的条件是 `admits_peer && !effect_pending`，从而保证不变量“可用一定已放行”。暂停原因的优先级与现在保持一致。
3. 新建 `ledger/peer_admission.rs`，实现 `impl PeerAdmissionPort for MembershipLedger`：
   - **每次都重新 `loader.load()`，不使用 `verified_snapshot_cache` 来判断记录是否最新。** 生产环境给网络准入用的是原始 `SqliteMembershipLedger`，它的 `current_revision()` 为 `None`，走缓存会让准入永远停在旧状态。历史字节不变时，可以复用已解码的历史，以免重复验签（仿照 `validate_loaded` 的 `cached_snapshot` 参数）。
   - 错误映射：
     - 没有当前空间 → `Ok(false)`；
     - `Locked`、`Unavailable`、`Conflict` → `PeerAdmissionError::Unavailable`；
     - `Corrupt`、`RecoveryRequired` → `PeerAdmissionError::InvalidState`。
4. 在 `uc_application::deps` 导出 `pub fn build_membership_peer_admission(loader: Arc<dyn LoadMembershipLedgerPort>, verifier: Arc<dyn HistoricalMembershipSignatureVerifier>) -> Arc<dyn PeerAdmissionPort>`。内部构造一个只读 `MembershipLedger`，committer 调用一律返回 `MembershipLedgerError::Unavailable`，永远不会被调用。
5. 把 B2 移到 `crates/uc-application/src/space/membership/ledger/peer_admission_tests.rs`，并在 `ledger/mod.rs` 声明。若 `validate_loaded` 把 B2 中“本机不在当前成员里”的夹具判为损坏，允许改用移除事件构造同一语义，并在本文“实施记录”中说明。
6. Engine `assembly/wire/infra.rs` 的 `build_peer_admission_port` 改为调用 `build_membership_peer_admission(membership_ledger, Arc::new(OpenMlsHistoricalSignatureVerifier))`。
7. 删除 `crates/uc-infra/src/space/security/peer_admission.rs` 及 `security/mod.rs`、`space/mod.rs` 中的导出。迁移 Infra 中引用它的测试；它们应改用内存假实现，不再依赖账本。
8. **验收**：
   - `cargo test -p uc-application --lib peer_admission_tests --locked` 通过。
   - 去掉 A3 中 S3 那一项的 `#[ignore]`，并通过。
   - `cargo test -p uc-application --lib space::membership --locked` 整体保持通过。

### S4 配对时校验邀请方身份（依赖 S2 中的指纹工具，不依赖 S3）

1. `network/iroh/space_admission/route.rs` 新增 `pub(crate) fn decode_space_admission_continuation_endpoint(route: &[u8]) -> Result<EndpointAddr, SpaceAdmissionTransportError>`（`decode_route(route, false)`），并在 `space_admission.rs` 中 `pub(crate) use`。
2. 新建 `crates/uc-infra/src/space/admission/joiner/sponsor_identity.rs`：
   - `pub(super) enum SponsorRouteIdentityError { RouteUndecodable, FingerprintUnavailable, Mismatch }`，派生 `Debug, Clone, Copy, PartialEq, Eq, thiserror::Error`。
   - `pub(super) fn verify_sponsor_route_identity(route: &AdmissionContinuationRoute, sponsor_fingerprint: &IdentityFingerprint, fingerprints: &dyn IdentityFingerprintFactoryPort) -> Result<(), SponsorRouteIdentityError>`
   - `pub(super) fn sponsor_identity_rejection(error: SponsorRouteIdentityError) -> PrepareJoinerActivationError`：返回 `invalid_for(IdentityConflict, error)`，保留类型化 source。
3. 把 B3 移到 `joiner/sponsor_identity/tests.rs`。
4. `DefaultJoinerActivationPreparation::new` 增加参数 `fingerprints: Arc<dyn IdentityFingerprintFactoryPort>`。在 `prepare()` 中，校验完成信息并解码历史之后，取 `history.admission_facts_for(commit.exact_candidate().candidate_event().author_member_instance_id)`，与 `candidate.continuation_route()` 一起调用 `verify_sponsor_route_identity`，失败时返回 `sponsor_identity_rejection`。Engine `sync_engine.rs` 传入 `Arc::clone(&space_setup.fingerprint)`。
5. 前提已核实：Engine 的续连路由由主端点编码（`sync_engine.rs:274`），本机身份指纹也由同一端点派生（`sync_engine.rs:280`）。因此健康设备上两者必然一致。
6. 确认 `PrepareJoinerActivationError::Invalid` 在加入流程中终止为拒绝、不会进入重试。现有测试若没有覆盖 `IdentityConflict`，在 Application 加入方测试中补一条。
7. **验收**：
   - B3 通过；去掉 A3 中 S4 那一项的 `#[ignore]`，并通过。
   - 现有配对端到端测试保持通过：`cargo test -p uc-infra --lib space::admission --locked`；`cargo test -p uc-engine --test space_membership_auto_pairing_e2e --locked`。

### S5a 本机身份自检：只检测、只公开原因（独立）

S5a 只回答“是不是这个问题”，不回答“怎么修”。它不新增恢复动作，也不要求产品仓增加引导。

1. Application 模型：
   - `SpaceDeviceUpdateProblem` 增加 `LocalIdentityMismatch`；
   - `SpaceDeviceUpdateStatus` 增加构造函数 `needs_attention_without_recovery(reason)`：`phase=NeedsAttention`，`recovery=None`，`next_retry_at_ms=None`；
   - **不新增** `SpaceDeviceUpdateRecovery` 取值。
2. `QueryDeviceTrustUseCase::new` 增加最后一个参数 `local_identity: Arc<dyn LocalIdentityPort>`；`new_for_tests` 内部传入返回 `Ok(None)` 的测试实现，这样现有测试不受影响。
3. 在 `execute()` 中，仅当本机成员生效时，读取 `get_current_fingerprint()`（**不得调用 `ensure()` 或 `create()`**）：
   - `Some(fp)` 且与 `history.admission_facts_for(local_member_instance)` 的指纹不同：`space_device_update` 设为 `needs_attention_without_recovery(LocalIdentityMismatch)`，优先于其他一切状态；
   - `None`：不作判断；
   - `Err`：返回 `QueryDeviceTrustError::Dependency { source }`。
4. 运行诊断（依赖 S1 的合同结构，可与 S1 同时实现）：
   - 在 `uc-observability-contract` 的 connectivity 合同中新增 `LocalEvent::LocalIdentityChanged { state }`：`kind` 为 `local_identity_changed`，`state` 取值为 `mismatch` 或 `consistent`；
   - 事件名 `space.local_identity.changed`；`mismatch` 为 WARN，并输出 `uc.outcome=needs_attention`；`consistent` 为 INFO，并输出 `uc.outcome=ok`；
   - 只输出 `event.name`、`state`、`uc.outcome` 三个字段；
   - 导出记录函数 `record_local_identity_changed(state)`。
5. `QueryDeviceTrustUseCase` 在进程内记住上一次的检查结果（`Mutex<Option<bool>>`，不持久化）。记录规则：
   - 从“未检查”或“一致”变为“不一致”时，记录一次 `mismatch`；
   - 从“不一致”变为“一致”时，记录一次 `consistent`；
   - 其他情况（包括启动后第一次检查即为一致、连续多次不一致）都不记录。
   - 这不是新的业务动作，只是运行诊断，符合观测文档中“业务动作与运行诊断分开”的要求。
6. 在 `space/application.rs` 构造处传入既有 `local_identity`。
7. 把 B4 移到 `query_device_trust/local_identity_tests.rs`，并在 `query_device_trust/mod.rs` 中声明。
8. Engine：
   - `SpaceDeviceUpdateProblemSummary` 增加 `LocalIdentityMismatch`，线值为 `local_identity_mismatch`；
   - `operations/device/space_device_update.rs` 补齐映射；新原因的 `recovery` 为空；
   - 旧版 `MembershipMaintenanceHealthSummary` 对新原因保持 `phase=NeedsAttention`，`reason`、`recovery` 为 `None`。
9. 检查 `bindings/` 下是否存在上述枚举的镜像。如有，同步更新；如没有，在本文注明“绑定通过 JSON 透传”。
10. **宿主兼容性**：宿主遇到新原因时，只需按已有的“需要处理”通用展示即可。发布前须确认 Desktop 和移动端的解码不会因未知的 `reason` 取值而失败；若会失败，先由产品仓放宽解码，本仓再发布。
11. 更新公开契约文档：`docs/design-docs/` 中描述 `spaceDeviceUpdate` 的位置，写明新原因的含义：本机当前网络身份与成员历史记录不一致，自动重试无法解决，恢复方式待定。**不写**任何建议的用户操作。
12. **验收**：
    - B4 通过；去掉 A4、A5 的 `#[ignore]`，并通过；
    - `cargo test -p uc-application --lib query_device_trust --locked`、`cargo test -p uc-engine --test public_contract --locked` 保持通过。

### S5b 恢复方式（待定，不在本计划实施范围）

S5b 的前提是先在现场核实问题是否真实存在。进入条件：

1. 用只读方式核实 t-0028 现场：`a` 的 `currentJoin.sponsorIdentityFingerprint` 与 `dev` 当前身份指纹是否不同。或者在 S5a 发布后，任一现场出现 `space.local_identity.changed state=mismatch`。
2. 查明身份被替换的原因，例如钥匙串条目丢失后启动时重新生成、资料升级或迁移、恢复出厂。原因决定了应当“防止替换”还是“接受替换”。

两个候选方向，届时另写计划并选择其一：

- **重新配对引导**：新增恢复取值，产品仓提供“移除本机后重新加入”的引导。代价是破坏性操作，误报时伤害大。
- **身份更新事件**：设备用仍然有效的成员凭据签发“更新本机网络身份”的成员事件，其他设备验证后更新准入信息，用户无需操作。代价是新增协议能力和持久化格式版本。

在 S5b 决定之前，A4 中 `no_re_pair_recovery_is_published_before_s5b` 必须保持通过。

## 统一门禁（每个切片提交前）

```bash
cargo metadata --locked --format-version 1
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-rust-style.mjs
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

全部切片完成后另跑：

```bash
cargo test -p uc-observability-contract -p uc-infra -p uc-application -p uc-engine --locked
cargo test -p uc-infra --locked --test inbound_peer_rejection_diagnostics --test inbound_peer_single_owner -- --include-ignored
```

第二条命令必须没有任何 ignored 项。完成时，A1–A5 中所有 `#[ignore]` 都应已删除。

Cargo 验证由一个负责人通过共享 `target` 串行执行。

## 风险与待决

- **生产路径的订阅者**：`IrohNodeBuilder::spawn` 在 `NoSubscriber` 下启动 Router（`node.rs:1567`）。按注释，协议处理仍使用进程订阅者，但 A2 使用的是 iroh 原生 Router，不经过 `IrohNodeBuilder`。S2 完成后，须在一次本机双实例手动配对中确认 Engine 文件里能看到 `peer.inbound.rejected`（例如让一端 `disconnect_all` 后被拨入）。未执行的记为“跳过”。
- **记录量**：现场 2 小时内出现 747 次拒绝。本计划不做限流。如果后续导出体积成为问题，在合同层增加计数型汇总，不得丢弃首条记录。
- **S5a 的宿主影响**：只新增一个原因取值，不带恢复动作。宿主按通用“需要处理”展示即可，但须先确认宿主解码能容忍新取值（见 S5a 第 10 步）。
- **S4 对存量设备无效**：它只拦截今后的配对。已经处于不一致状态的设备要靠 S5a 暴露；S5a 会在身份被替换的那一端报出（按现场推断是邀请方）。

## 实施记录

- 2026-09-23 S1：合同新增封闭协议/原因与 `peer.inbound.rejected` 的六字段本地记录。A1 4/4 通过；统一门禁通过。未执行设备或发布检查（本切片不涉及）。提交号见对应切片提交。
- 2026-09-23 S2：九处入站身份解析收敛到 `PeerIdentityResolver`，需账本的协议使用 `InboundPeerGate`，拒绝只在实际回写/关闭处记录；mDNS 提示只解析、不记入站拒绝。B1 9/9、A2 1/1、A3 的 S2 两项、既有 `peer_admission_identity_resolution` 1/1 通过；`cargo check --workspace --all-targets --locked`、metadata、fmt、Rust 风格、仓库架构及 diff 检查通过。A3 的 S3/S4 两项仍按计划 ignore。旧 E2E 的 `duplicate-identity-stale-first` 原断言要求先选 stale 记录并查询账本，与本计划“歧义时拒绝且不查账本”冲突；仅将其改为拒绝且查询列表为空，保留其余 E2E 语义。未执行本机双实例手动配对的生产 Engine 文件订阅检查，记为跳过；其他协议原有完整测试尚未执行。提交号见对应切片提交。
- 2026-09-23 S3：账本统一网络准入和公开 scope 的谓词；每次入站重新加载记录，历史字节未变时只复用验签结果。移除 Infra 准入副本，Engine 装配 Application 只读账本，架构检查随所有权调整。B2 10/10、Application 成员测试 165 通过、1 项既有忽略、A3 的 S3 项通过；统一门禁通过。A3 的 S4 项仍按计划忽略；未执行设备检查。提交号见对应切片提交。
- 2026-09-23 S4：加入方激活前比较已验证邀请方历史指纹与续连端点指纹，三类失败以类型化来源终止为 `IdentityConflict`。原 `reject_activation(IdentityConflict)` 返回 `InvalidTransition`，本计划 S4 规格遗漏了这一点；因此 Core 新增本地终止原因 `IdentityRejected`，在准入记录 V2 的本地终止原因取值表中编码为 9，重启后公开拒绝原因仍为 `IdentityConflict`。**格式决定**（经用户确认）：该取值表的 3–8 由本分支 `8df9d5e1` 新增，main 与 `v1.1.0-rc.18` 只有 0–2，尚未越过冻结边界；编码 9 补全同一未发布目标格式，不新增格式版本，是本计划唯一的持久化取值新增；只有本分支的早期构建读取编码 9 会得到 `InvalidState`，不涉及受支持发布。测试：Core `activation_rejection_categories_round_trip_through_persistence` 补 `IdentityConflict` 往返；Application 恢复既有 `RelationshipConflict` 断言，另增 `sponsor_identity_conflict_is_saved_as_a_terminal_rejection_without_retry`（激活夹具改为可指定拒绝原因，默认仍为 `RelationshipConflict`）。验证：B3 5/5；A2 1/1、A3 4/4，`--include-ignored` 无忽略项；Core space admission 108/108、Core 持久化 14/14；Infra `space::admission` 与 `network::iroh` 314 通过、4 项既有忽略；Application `space::admission` 93 通过、1 项失败 `joiner_pairing_fixture_reaches_active_settled`，该项在计划提交 `6cc76b45` 已失败，与本计划无关。Engine `space_membership_auto_pairing_e2e`（`dev-tools`）完整运行 40 通过、7 失败、11 忽略。对这 7 项分别在 S3 提交 `d5980541` 与当前工作树以同一组合各运行一次，结果一致：`completed_admission_survives_restart_and_allows_transfer`、`handoff_four_device_removal_preview_matches_executed_choice`、`offline_member_catches_multiple_removals_without_blocking_new_invitations` 两边都通过，完整运行中的失败属并发负载下的不稳定；`f2_concurrent_leaf_removals_resolve_to_selected_branch`、`f6_deep_chain_recovers_selected_branch_without_online_sponsors`、`same_device_returns_to_a_previous_space_after_switch_and_restart`（重启 `Engine::start` 返回 1216）、`suspend_during_space_switch_recovery_does_not_resurrect_the_network`（恢复返回 1103）两边都失败，属 S4 之前的既有问题，另行跟踪。完整运行日志中没有任何 `IdentityConflict` 拒绝。统一门禁通过，仅有既存 OHOS 测试未使用导入警告。未执行设备检查。
- 2026-09-23 S2 补正：mDNS 连接提示改用 `PeerIdentityResolver::resolve`，成员读取与指纹派生失败经新增的 `PeerIdentityError` 保留原始 source，入站路径仍使用 `Copy` 的 `InboundPeerRejection`；B1 增加来源链测试，10/10。删除 `peer_reachability_adapter` 残留的 `is_admitted` 包装，三个调用点直接使用 `gate.authorize(..).is_ok()`。已落地的 B1–B3 暂存副本已删除，B4 随 S5a 提交删除（暂存目录随之清空）。遗留：S4 的 `SponsorRouteIdentityError` 按 B3 规格为 `Copy`，指纹派生失败的 `anyhow` 来源没有保留；路由解码器本身也不带来源。
- 2026-09-23 S5a：本机查询只读取当前身份，比较已验证历史；不一致优先公开 `needs_attention/local_identity_mismatch`，不提供恢复动作。绑定未镜像该原因枚举，通过 Engine JSON 透传。B4 6/6、A4 3/3、A5 3/3、Application `query_device_trust` 21/21、设备组查询 1/1、Engine `public_contract` 50/50；统一门禁通过，仅有既存 OHOS 测试未使用导入警告。当前 Desktop 与 Mobile 源码未找到 `spaceDeviceUpdate` 或新原因的显式消费点；实际宿主版本对未知原因线值的解码兼容性仍须在发布前分别验证。
