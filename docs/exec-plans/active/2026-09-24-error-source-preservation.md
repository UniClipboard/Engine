# 错误来源保留：清除字符串化与丢弃来源

## 状态与完整责任

- **状态**：实施中。E0–E4 已完成，E5 进行中。
- **日期**：2026-09-24。
- **依据**：[错误处理与转换](../../design-docs/error-handling.md)要求保留完整 source chain；[运行期观测](../../design-docs/observability.md#错误来源与日志字段)要求日志只记录从 source chain 提取的固定分类。
- **完整负责人**：每处转换由目标错误类型所在模块负责（与错误处理规范的“转换所有权”一致）；整体顺序、清单复核与验收由本计划负责。
- **调用方唯一动作**：不新增对外调用。各项修复不改变 Engine 公开接口、公开错误码和宿主可见文本。
- **成功结果**：清单中的 F 类位置全部保留来源；E/R 类位置逐项判定为允许例外并按规范注释，或改为保留来源；新增代码中的同类写法由自动检查拒绝。
- **失败结果**：某项修复需要改变持久化格式、设备间协议或公开错误分类时，暂停该项，单独立项，不在本计划中夹带。
- **重启与重试责任**：本计划只改变错误值的内部结构，不新增持久事实，不改变任何重试、回执或恢复顺序。

## 问题

仓库中大量错误转换把下层错误变成字符串，或者直接丢弃。这样做有三个后果：

1. **诊断分类失效。** 观测规范要求日志只记录固定分类，这些分类要从 source chain 中逐层 `downcast` 得到，例如
   `io_error_kind`、SQLite 错误码。来源被字符串化后，分类器拿不到具体类型，只能记为 `unknown`。
   049 S3.a 的实际案例：组密钥投递状态读取失败时，Infra 记录为 `source="storage" reason="unknown"`；
   [`space_security_store/revocation.rs`](../../../crates/uc-infra/src/db/repositories/space_security_store/revocation.rs)
   与 [`legacy_bootstrap.rs`](../../../crates/uc-infra/src/db/repositories/space_security_store/legacy_bootstrap.rs)
   中的 `anyhow!(error.to_string())` 让 SQLite `BUSY` 无法被识别，结论只能停在推断
   （见 [049 计划](049-single-owner-space-membership-rewrite.md)失败诊断表）。
2. **流程判断失效。** 生产代码依据 source chain 做分支，例如
   [`is_cancel_error`](../../../crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs) 查找
   `BlobTransferError::Cancelled`，[`session_supervisor/lifecycle.rs`](../../../crates/uc-engine/src/runtime/session_supervisor/lifecycle.rs)
   查找 `TaskShutdownReport` 与 `EngineError`。中间任何一层字符串化，取消就会被当作普通失败，超时会被报成内部错误。
3. **隐私边界被穿透。** 把错误拼进文本时，常一并拼入路径、标签值或内容片段（清单 S3 中的 `path.display()`、`{parent:?}`，
   S2 中的 `parse quota baseline {:?}`）。这些文本随后可能经 `%error` 进入日志。

## 反模式与替换写法

完整逐行位置见[修改点清单](2026-09-24-error-source-preservation-inventory.md)。

| 类别 | 反模式 | 替换写法 | 生产代码数量 |
| --- | --- | --- | ---: |
| S1 | `.map_err(\|e\| anyhow!(e.to_string()))`、`anyhow::Error::msg(e)` | 直接 `?`，或 `.map_err(anyhow::Error::new)` | 91 |
| S2 | `.map_err(\|e\| anyhow!("动作: {e}"))` | `.context("固定动作")`；需要先转换时 `anyhow::Error::new(e).context("固定动作")` | 82 |
| S3 | `Error::Variant(e.to_string())`、`reason: e.to_string()`、`Variant(format!("..{e}"))` | 变体改为 `#[source]` 具体错误或 `anyhow::Error`，优先 `From` + `?` | 524 |
| S4 | `.map_err(\|_\| Error::Variant)` | 变体携带 `#[source]`；属于允许例外的写明理由（见下） | 914 |
| L1 | 日志字段 `error = %error`、`error = ?error` | 从 source chain 提取固定分类字段，不输出正文 | 329 |

数量取自提交 `48a2e95c` 的扫描快照，只计生产代码；测试代码计数见清单末尾。

### S4 的允许例外

以下来源不含可用诊断信息，或不能作为 source 保存，可以丢弃，但必须在同一行或前一行用中文注释写明理由：

- 锁中毒 `PoisonError<Guard>`：持有 guard，不能跨线程保存；
- `TryFromIntError`、`TryFromSliceError`：只表示长度或范围不符，目标分类已完整表达；
- `tokio::time::error::Elapsed`：超时本身就是分类；
- 通道 `SendError<T>`、`TrySendError<T>`：携带待发负载，保存会延长负载生命周期，且可能含剪贴板内容；
- `uc-core` 内部纯校验结果改分类，且下层同样是 Core 纯校验、没有外部失败时。

UTF-8、文本解析和系统时间（清单中的 R 类）需逐项判断：纯输入校验按例外处理；读取持久数据或对端输入时保留来源。

### 与其他计划的约束

- [Core 边界收口](2026-09-23-core-boundary-remediation.md) E8 计划从 `uc-core` 移除 `anyhow`。Core 错误类型保留来源时
  使用具体错误类型，不新增 `anyhow::Error` 字段。
- `compatibility/` 是独立版本的 LAN 兼容线，其修改点单独成切片，随兼容线自己的版本发布。
- 049 成员重写会话已在修复以下位置，本计划不重复排期，合入后复核并从清单中移除：
  [`admission_key_manager.rs`](../../../crates/uc-infra/src/security/admission_key_manager.rs) 的全部 `map_err(|_| AdmissionKeyError::X)`；
  [`profile_key_recovery.rs`](../../../crates/uc-infra/src/security/profile_key_recovery.rs) 的 `activate_from_backing_if_available`；
  [`space_security_store/`](../../../crates/uc-infra/src/db/repositories/space_security_store/) 中的 S1/S3 与存储失败分类器；
  Joiner 激活准备中 `AdmissionSpaceTransitionError::Inconsistent` 的分类。
  该会话明确未处理的 `SpaceAdmissionStateStoreError`、`MembershipLedgerError`、`CurrentSpaceIdentityError`、
  `RePairingStateError`、`ActiveSpaceGenerationManifestStoreError::Corrupt` 等单元或 `Copy` 错误类型仍属本计划。

## 切片

每个切片开工前重新生成清单，按当时行号执行；一个切片内只改一个 crate 或一个业务模块，便于按错误类型整体调整。

| 切片 | 内容 | 范围 | 前置 |
| --- | --- | --- | --- |
| E0 | 规则与清单：更新错误处理与观测规范，生成本清单 | 文档 | 无（本次完成） |
| E1 | 自动检查：`check-rust-style.mjs` 拒绝新增的 S1/S2/S3 写法与无注释的 S4 写法，只检查新增行 | 脚本 | E0 |
| E2 | S1：`anyhow!(error.to_string())` 全部替换 | `uc-infra` 数据库仓储、`uc-engine` 宿主适配（049 处理中的 `space_security_store/` 除外） | E0 |
| E3 | S2：`anyhow!("..{e}")` 改为 `context`，移除文本中的路径、标签值与内容 | `uc-application` 剪贴板、`uc-infra` 数据库与文件系统、`uc-engine` 对账 | E0 |
| E4 | S3 中含路径或标识的文本（隐私优先） | 清单 S3 中 `path.display()`、`{parent:?}` 等 | E0 |
| E5 | S3 其余：字符串错误变体改为携带 source | 按错误类型逐个处理：`uc-infra` 网络/传输/搜索、`uc-application` 门面、`uc-engine` 装配 | E4 |
| E6 | S4 F 类：Infra 能力来源 | `uc-infra`（安全存储、MLS、编解码、文件系统、网络） | E0 |
| E7 | S4 F 类：Application、Engine 与绑定 | `uc-application`、`uc-engine`、`bindings/` | E6 |
| E8 | S4 `uc-core`：按纯业务判断规则逐项判定，保留来源时只用具体错误类型 | `uc-core` | 与 Core 边界收口协调 |
| E9 | S4 E/R 类：逐项确认例外并补注释，或改为保留来源 | 全仓 | E6–E8 |
| E10 | L1：日志改为固定分类字段 | 全仓，按模块推进 | E2–E7 对应模块完成后 |
| E11 | 兼容线 S3/S4 | `compatibility/` | 兼容线自己的发布节奏 |

## 验收

- 每个切片按[错误处理与转换](../../design-docs/error-handling.md#测试)补测试：对外稳定分类不变，`source()` 非空，
  适用时能从 source chain 找到原始下层错误；不以显示文本作为唯一断言。
- 依赖 source chain 做分支的路径（取消、超时、关闭报告、SQLite 忙）各有一条测试证明分支仍然成立。
- 重新扫描后，F 类数量为零；E/R 类每一处都有例外注释或已改为保留来源。
- 公开错误与日志中没有新增正文、路径、标识或内容。
- 交付前检查按仓库 AGENTS 列表执行；未执行的设备矩阵项记为“跳过”。

## 扫描规则

清单由文本规则生成，覆盖 `crates/`、`bindings/`、`compatibility/`、`tests/` 下的 `.rs` 文件：

- S1：`anyhow!(<错误变量>.to_string())`，`Error::msg(<错误变量>)`；
- S2：`anyhow!` 格式串内插错误变量（`{e}`、`{err}`、`{error}`，或位置参数为错误变量），且位于 `map_err` 闭包或 `Err(e)` 分支中；
- S3：`Type::Variant(<错误变量>.to_string())`、`Type::Variant(format!(..<错误变量>..))`、
  `message|detail|reason|..: <错误变量>.to_string()`，含跨行形式；
- S4：`map_err(|_| ..)` 与 `map_err(|_xxx| ..)`，来源类别按调用链关键词推断；
- L1：日志宏中的 `error = %<错误变量>`、`error = ?<错误变量>`。

测试文件、`testing/` 目录及 `#[cfg(test)] mod` 之后的内容计为测试代码。规则会漏掉经中间变量转手的写法，也可能把少量
非错误的 `to_string()` 计入；E1 的自动检查落地时以同一规则为准，并补充漏报样例。

## 实施记录

### E1 自动检查（2026-09-24）

`scripts/architecture/check-rust-style.mjs` 对新增的非测试行执行 S1/S2/S3 检查，并拒绝同一行与前一行都没有中文注释的
`map_err(|_| ..)`；跨行写法与前一行拼接后判断。在当前全仓按文件模式试跑，命中 S1 33、S2 78、S3 520、S4 891 处，
与清单口径一致（S1 差额来自 049 已修复的 `space_security_store/`）。S3 额外覆盖 `map_err(|e| e.to_string())`。
L1 日志字段不在本切片检查范围，待 E10 确定固定分类字段的替换写法后再加入。

### E2 S1 替换（2026-09-24）

- `uc-infra` 数据库仓储（`blob_reference_repo`、`entry_receive_attempt_repo`、`migration_repo`、`mobile_device_repo`、
  `relationship_store`）与 `uc-engine` 宿主适配 `assembly/host.rs`：改为直接 `?` 或 `.map_err(anyhow::Error::new)`。
  这些仓储外层仍有 S3 的 `Storage(e.to_string())`，source chain 要到 E5 才能完整到达 Application。
- `rendezvous/invitation_adapter.rs`：mDNS 发布链路从 `Result<_, String>` 改为 `anyhow::Result`，
  `InvitationError::LocalPublicationFailed` 的来源保留 `MdnsPublisherError`。
- 测试：宿主剪贴板读取失败时可从错误链取回 `HostCapabilityError` 及其分类；本地发布失败可取回 `MdnsPublisherError`。
- 转入 E5：`network/iroh/membership_branch_recovery_adapter.rs` 中的 `anyhow::Error::msg(source)`，上游
  `connect_with_staggered_retry` 把多次拨号失败汇总为 `String`，需要随网络错误类型一起重新设计。
- 不处理：`space_security_store/revocation.rs` 剩余 1 处，属于 049 处理范围。

### E3 S2 替换（2026-09-24）

- 78 处 `anyhow!("动作: {error}")` 全部改为 `.context("固定动作")`、`.with_context(|| format!(..))`（只含固定字段标签）
  或 `anyhow::Error::new(error).context(..)`；涉及 `uc-application` 剪贴板、`uc-infra` 数据库/文件系统/搜索/安全与
  `uc-engine` 宿主适配、启动对账。
- 同时移除文本中的非固定值：配额基线原文（`cleanup.rs`）、表示 ID（`durable_spool_queue.rs`）。
- `db/pool.rs` 迁移失败来源是 `Box<dyn Error + Send + Sync>`，用 `anyhow!(error)` 保留原对象后再加 context。
- 测试：payload 字段截断时，可从错误链取回 `io::ErrorKind::UnexpectedEof`。

### E4 含路径或标识的文本（2026-09-24）

E1 的检查会拒绝只删标识、仍保留 `{error}` 的改法，所以 E4 按错误类型整体改为携带 source。

- 已完成：
  - `uc-core` 端口错误 `AppVersionStateError`、`FirstSyncStateError`、`EngineVersionStateError`：元组变体改为
    `#[source] Box<dyn Error + Send + Sync>`（沿用 `PeerReachabilityError` 的写法；Core 无法命名 Infra 类型，也不新增
    `anyhow`），显示文本不再含文件路径。空文件、schema 不识别等没有下层错误的损坏情形使用固定文本。
  - `uc-application` 的 `DispatchSyncError`、`ClipboardSyncError`、`ResendEntryError`、`GetEntryDeliveryViewError`：
    字符串变体改为 `#[source]`（`TransferCipherError` 或带固定动作 context 的 `anyhow::Error`），去掉
    `current peer scope: {error:?}` 的 Debug 文本。Engine 的公开错误码与分类不变。
  - 测试：游标读取失败可取回 `io::Error`、解析失败可取回 `serde_json::Error` 且显示文本不含路径；投递视图与分发目标
    查询失败可取回 `CurrentSpaceMemberScopeError`、`PeerAddressError`。
- `SearchError::Internal` 改为 `#[source] Box<dyn Error + Send + Sync>`；Infra 搜索适配器统一经
  `search/error.rs` 的 `internal("固定动作")` 转换，去掉解码失败文本中的条目 ID。`SearchFacadeError::Internal` 改为携带
  `SearchError`，为此去掉 `Clone`/`PartialEq`/`Eq` 派生，两处测试改为模式匹配。Engine 搜索错误码不变。
  测试：`internal` 转换后可沿错误链取回原始 `io::Error`。
- 转入 E11：`mobile_sync/file_staging.rs` 文本中的路径与 URI。其端口 `MobileFileStagingPort` 只服务兼容线，
  而兼容线 `get_file.rs` 自己也把 URI 与错误文本写入日志和 `Staging(String)`；只改 Infra 端堵不住泄露。
- 遗留：`OutboundPayloadError::Internal(String)` 仍是字符串变体，`ResendEntryError` 暂以 `anyhow!(message)` 承接，
  由 E5 修复（已在 E5 Application 批次修复）。

### E5 字符串错误变体改为携带 source（进行中）

约定：`uc-core` 端口错误用 `#[source] Box<dyn Error + Send + Sync>`（同 E4）；Application 与 Infra 自有类型用具体错误
或带固定动作 context 的 `anyhow::Error`。只改被扫描到的字符串化变体；携带业务文本（如拒绝原因、查询错误说明）的变体保留。

- 网络与传输（已完成）：
  - Core：`BlobError`、`ClipboardDispatchError`、`ActiveClipboardPullClientError`、`ActiveClipboardDispatchError`、
    `TransferCipherError::Internal`、`PublishError::Io`（改为 thiserror，去掉 `Clone`/`PartialEq`/`Eq`，原先按种类描述
    `io::Error` 的文本改为直接保存 `io::Error`）、`LocalIdentityError::Storage`、`FileTransferPrivacyMaintenanceError`、
    `DirectoryStagingCleanupError`、`ReceiveArtifactLogError::{Backend, EncryptionUnavailable}`。
  - Application：`FetchBlobError`、`PublishBlobError`。
  - Infra：`ChunkedTransferError`（加密与压缩失败携带来源）、`IrohNodeError::{Bind, BlobStoreInit}`、`ReporterError`。
  - `connect_with_staggered_retry` 改为返回 `StaggeredDialError`：保存最能说明原因的一次尝试
    （优先非超时失败，类型为 `ConnectWithOptsError`、`ConnectingError`、`Elapsed` 或 `JoinError`）及 `DialFailure` 分类；
    剪贴板单飞拨号以 `Arc` 共享给跟随方。E2 转入的 `membership_branch_recovery_adapter.rs` 随之改为保留来源。
  - 隐私修复：`IrohNodeError::InvalidRelayUrl` 原先在显示文本中带出用户配置的 relay URL（可能含凭据），改为固定分类
    `RelayUrlProblem` 作为来源，不再保存原值；已有测试确认 `Display`/`Debug` 不含主机名与密码。
  - 测试：拨号错误优先保留非超时尝试；`PublishError` 保留 `io::Error` 且显示文本不含路径；relay URL 错误不含原值。
- 数据库仓储（已完成）：Core 的 `BlobMigrationRepoError`、`MobileDeviceError::Storage`、`ClipboardRepositoryError::Storage`、
  `TrustedPeerError::Repository`、`MembershipError::Repository`、`PeerAddressError::Internal`、`BlobReferenceError::Repository`，
  Infra 的 `RelationshipStoreError::Storage`。至此 E2 中仓储外层的字符串化已消除，diesel 错误可从端口错误沿链取回（有测试）。
  - `MobileDeviceError` 也被兼容线使用。兼容线 5 个用例错误的 `PersistenceFailed(String)` 一并改为携带来源；Engine 对它们只取
    错误码、不用文本，对外行为不变。
  - 发现并修正：`anyhow::Error` 不带 context 直接 `.into()` 成盒装来源时，根错误无法 `downcast`。用临时类型让编译器列出
    全部此类位置（19 处，均在 `uc-infra`），逐个补上固定动作 context；规则已写入错误处理规范。
- Application 门面与用例（已完成）：`DiagnosticsFacadeError`、`ApplyInboundError`、`RosterError`、`ClipboardHistoryError`、
  `ResourceFacadeError`、`CancelEntryReceiveError`、`ClipboardOutboundError`、`OutboundPayloadError`、`ClipboardCaptureFacadeError`、
  `ClipboardLiveIndexError`、`SettingsFacadeError::{Load, Save}`、`RelayConfigurationError::{Load, Save}`、`StorageFacadeError`、
  升级检测与确认错误、`IssuePairingInvitationError::Internal`、`JoinSpaceError::Settings`、各 Space 生命周期用例错误、
  `SpaceActivityError::Receive`、`BlobTransferError`、`InboundPulledContentStoreError`、`ListProjectionsError`、
  `ToggleFavoriteError`、`BuildSnapshotError::PasteRepBlobFetchFailed`；Core 的 `ActiveClipboardPullServeError::Internal`；
  Engine 的 `WiringError` 初始化变体。
  - 有具体类型时直接保存：`RosterError` 保存 `MembershipError`、`LocalIdentityError`、`SpaceProtectionError`；
    `CancelEntryReceiveError` 保存 `PublishLogError`、`DirectoryStagingCleanupError`；版本号解析失败保存 `semver::Error`；
    `IssuePairingInvitationError::Internal` 保存整个 `InvitationError`（Core 的 `InvitationError::Internal(String)` 留待 Core 批次）。
  - `CancelEntryReceiveError::Transfer` 原先把多个取消失败的文本用 `; ` 拼接，改为保存第一个来源并记录失败数量。
  - `LocalSessionReadiness` 原先以 `Result<(), String>` 返回，改为 `anyhow::Result`；两个只转调同一私有方法的入口合并为 `prepare_data`。
  - 隐私修复：出站目录成员未发布时的错误原先带出成员路径，改为固定文本；拉取服务的 Infra 适配器不再把内部错误文本写入日志字段；
    主动拉取解码失败不再把原因文本拼进存储错误。
  - `ClipboardHistoryError` 去掉 `Clone`/`PartialEq`/`Eq`；`ApplyInboundError::WriteCoordinator(String)` 无构造点，删除。
  - 宿主可见行为不变：Engine 对上述错误只映射固定错误码与分类。
  - 测试：拉取服务加密失败可沿链取回 `TransferCipherError`；取消接收时两个传输取消失败保留首个来源（可取回 `io::Error`）
    与失败数，显示文本不含下层文本。
  - 剩余 S3 集中在 Infra（`SpaceAccessError`、legacy bootstrap、`EntryFileSetError` 等）与兼容线，以及 Core 的
    `InvitationError::Internal`、`FileTransferProjectionError::Backend`、`MembershipSecurityUpdateError::Repository`。
- Infra 安全与仓储（已完成）：Core 的 `SpaceAccessError::Internal`、`SpaceProtectionError::Repository`、
  `PublishLogError::{Backend, EncryptionUnavailable}`、`EntryFileSetError::Storage`、`AttemptError::Backend`、
  `FileTransferProjectionError::Backend`、`ProvisionalReceiveError::Backend`、`InboundReceiveCommitError::Backend`、
  `RelationshipStateResetError::Repository`、`MembershipSecurityUpdateError::Repository`、`LegacyMigrationRecoveryError::Internal`、
  `LanInterfaceProbeError::Probe`、`PasswordHasherError::{InvalidPhc, Internal}`、`KeyMigrationError::Internal`；
  Application 的 `CurrentMemberSignatureError::Repository`；Infra 的 `MdnsPublisherError`、`MdnsResolverError`、
  `RenderEncodeError::SerializeJson`；观测契约的 `ResetIdentityError::Storage`。
  - `v1_aead::derive_kek_argon2id` 原先返回 `String`，改为 `KdfError`（不支持的算法、参数、哈希、KEK 构造），
    不再把 KDF 算法名写进错误文本；`SpaceAccessError` 的 KDF 与本地密钥物料失败随之保存具体来源。
  - 兼容线：`AuthenticateBasicAuthError::Internal`、`ListLanInterfacesError::ProbeFailed`、
    `RegisterMobileShortcutDeviceError::{PasswordHashFailed, LanInterfaceProbeFailed}`、`UpdateMobileDeviceError::PasswordHashFailed`
    改为保存 Core 端口错误本身；Engine 只映射错误码，对外行为不变。
  - `SpaceProtectionError`、`RelationshipStateResetError`、`MembershipSecurityUpdateError`、`CurrentMemberSignatureError`
    去掉 `Clone`/`PartialEq`/`Eq`。
  - 隐私修复：文件集未知成员类型与未知排除原因不再把库中取值拼进错误；未知剪贴板回执字节只保留固定分类文本。
  - 补漏：`mobile_device_repo.rs` 只在 `lan-compat` 的测试配置下编译，上一批改动后该配置无法编译，且 7 处
    `anyhow` 未加 context 直接装箱。已修复；并用临时的“仅接受 std 错误”恒等函数包裹全部 `Error::X(e.into())`，
    在默认与 `lan-compat` 配置下编译，确认全仓已无同类位置。
  - 测试：KDF 参数错误保留 `argon2::Error`；PHC 解析失败保留 `password_hash::Error`，显示文本不含输入。
  - `SecureStorageError::Other` 暂缓：其另一处构造在 049 未提交的 `profile_key_recovery.rs` 中。
- 成员安全存储与邀请（已完成；049 暂停期间处理 `space_security_store/`）：Core 的 `BootstrapError::Repository`、
  `SpaceSecurityStateResetError::Repository`（去掉 `Clone`/`PartialEq`/`Eq`）、`InvitationError::Internal`、
  `ConsumeInvitationError::Internal`；Application 的 `QueryPairingInvitationAddressesError::Internal` 保存整个 `InvitationError`。
  - `revocation.rs` 中最后一处 S1（`anyhow!(error.to_string())`）改为 `anyhow::Error::new`。至此 S1 为零。
  - 邀请适配器：设置读取、Sponsor 地址解码、准入路由与完整邀请编码失败改为携带来源（原先后三处直接丢弃来源）；
    消费邀请收到意外状态或响应解析失败时保存 `RendezvousHttpError`，不再把状态码与服务端 slug 拼进文本。
  - 用临时 `audit_std` 恒等函数审查本批盒装来源，9 处 `anyhow` 来源补上固定动作 context。
- 兼容线与移动端文件暂存（E11 的 S3 部分，已完成）：Core 的 `MobileFileStagingError::Io`、`LatestClipboardSnapshotError::Resolution`；
  兼容线 `GetMobileSyncFileError::Staging`（保存整个 `MobileFileStagingError`）、`ApplyIncomingMobileClipError::{EncodeFailed, Internal}`、
  设置读写失败、二维码渲染失败（保存 `ConnectUriError` 等具体错误）。
  - 隐私修复：`file_staging.rs` 的错误文本不再包含暂存路径、URI 与句柄；`apply_incoming.rs` 不再把镜像文件名拼进错误；
    `get_file.rs` 不再把下层错误文本写入日志字段。
  - 兼容线只把 `anyhow` 作为开发依赖，改为正式依赖会改动 `Cargo.lock`；因此兼容线用 `Box<dyn Error>` 与私有的
    `ActionFailed`（固定动作 + 来源）承接，来自 Application 的 `anyhow` 错误先加 context 再装箱。
  - 测试：URI 解析失败可沿链取回 `url::ParseError`，显示与调试文本都不含 URI；二维码构造失败可取回 `ConnectUriError`。
  - 转入 E10：`file_staging.rs` 与 `get_file.rs` 的日志字段仍记录路径与 URI。
- UniFFI 宿主文本（已决策并完成，按 relay 的做法）：`uc-mobile-proto` 新增 `PayloadDecodeDetail`，显示文本与原先逐字一致，
  同时保留 `serde_json`/`base64` 来源；`ConnectUriError` 去掉 `PartialEq`/`Eq`，测试改为 `matches!`。
  `uc-mobile` 的 FFI 错误只能携带字符串，唯一文本化位置为 `ffi_reason`（`ConnectUriError::PayloadDecodeFailed` 与
  `SyncError::Network`），属于已确认的例外。
- 剩余 S3（均需单独决定）：
  - `SecureStorageError::Other`：等 049 提交 `profile_key_recovery.rs` 后处理。
- `RelayProbeError`（已决策并完成）：宿主诊断文本保持原文透传。Infra 新增 `RelayProbeDetail`，其显示文本与原先交给
  宿主的文本逐字一致，同时以 source 保留下层错误；Application 的 `RelayProbeError` 与 `SettingsFacadeError::RelayProbe*`
  改为携带不透明 source，外层显示文本只给分类。唯一的文本化位于 Engine 契约层 `relay_probe_host_message`，
  用于生成 `RelayProbeOutcome { message }`，属于已确认的例外；隐私风险维持现状。测试确认细节文本与原格式一致且 source 可取回。
- 后续事项：投递失败时 `reason_detail` 把 `ClipboardDispatchError` 的来源文本写入 `EntryDeliveryRecord` 持久化字段，
  行为保持不变；持久字段是否应保存错误文本需要按持久化与隐私规则单独评估。



### E6 S4 F 类：Infra 能力来源（已完成，除等待 049 的项）

约定（已确认）：同一错误类型既有纯校验失败、又有下层失败时，变体改为 `Variant { source: Option<..> }`，并提供
`variant()`（无来源）与 `variant_from(source)`（有来源）两个构造函数；模式匹配写 `Variant { .. }`。Infra 与 Application
自有类型用 `Option<anyhow::Error>`；Core 类型用 `Option<Box<dyn Error + Send + Sync>>`，其 `_from` 只接受 std 错误，
`anyhow` 来源必须先加 context 才能传入。本身不含信息的密码学错误（如 `aead::Error`）同样作为来源保留，不新增例外类别。
为此去掉的 `Copy`/`Clone`/`PartialEq`/`Eq`，测试改为 `matches!`，借用处改为传引用。

- 第一批（已完成）：`MlsGroupError`（openmls `SignerError` 未实现 `Error`，以私有 `SignerFailure` 包装保留原值）、
  `AdmissionSecurityTransitionError::InvalidState`、`SpaceAdmissionTransportError`（`connection_decision` 与
  `exchange_failure` 改为借用）、`SpaceAdmissionStateStoreError`（含 diesel `From` 与执行器错误还原）、
  Core 的 `MembershipHistoryExchangeError::Transport`。成员历史交换的服务端处理路径改用 `ServerExchangeFailure`
  同时携带诊断分类与来源。连接超时（`Elapsed`）仍按例外丢弃并注释。
- 第二批（已完成）：`ChunkedTransferError`（读流失败与切片转换携带来源；`DecryptFailed` 保存 AEAD 错误；`InvalidHeader`
  增加可选来源，不再把 `EncryptionError` 文本写入 `reason`）；Core 的 `ConfigMigrationError::{Io, Internal, IncompatibleBundle}`
  增加可选来源，`config_migration/` 下 `BundleError`、`ArchiveError`、`StagingError`、`PendingImportError` 改为可选来源，映射函数把它们
  整体作为来源上传（口令错误与数据损坏仍刻意不可区分，不上传细节）；Core 的 `MembershipAttestationError`、`GroupUpdateDispatchError`、
  `MembershipGossipTransportError`；`ActiveSpaceGenerationManifestStoreError::Corrupt`、`MembershipLedgerError::{Corrupt, Unavailable}`、
  `CurrentSpaceIdentityError`、`CurrentMemberSignatureError`、`RePairingStateError`、`TransferPersistenceCipherError`、
  `AeadError::{InvalidKey, EncryptFailed}`、`WireError::{InvalidHeader, InvalidPayload}`、`HandlerError::{Protocol, Authentication, Application}`。
  锁中毒与 `Elapsed` 超时按例外丢弃并注释。
  - 测试：损坏的 MLS 客户端状态可取回 `serde_json::Error`；截断的分块头可取回 `UnexpectedEof`。
  - 修正脚本缺陷：按类型名前缀匹配时曾把 `MobileFileStagingError::Io` 误当作 `StagingError::Io` 改动文档注释，已还原并加词边界；
    文档注释中被改成构造函数调用的链接已还原为变体名。
- 第三批（已完成）：仓储密码适配器 `ReceiveArtifactCipherError`、`PublishLogCipherError`、`ActiveRegisterCipherError`、`FileSetCipherError`；
  Core 的 `SpaceAccessError::CorruptedKeyMaterial`、`CurrentMembershipIdentityError`、`BootstrapError::{CryptographicState, InvalidBootstrapId}`、
  `LegacyMigrationRecoveryError::RecoveryRequired`、`KeyEpochError::{DecryptionFailed, PersistedStateIntegrityFailed}`、`SpaceProtectionError`；
  以及 `RelationshipStoreError::InvalidCiphertext`、`RecoveryMaterialError`、`ProfileLifecycleRepositoryError`、`FullInvitationCodecError`、
  `SpaceRebuildProgressError`、`SponsorRouteIdentityError`、`RenderDecodeError`、`JoinerStartMaterialError::InvalidInvitation`。
  - `hkdf::InvalidLength` 未实现 `Error`，且只表示常量输出长度超限，按长度类例外注释。
  - 测试 `invalid_full_invitation_is_rejected_without_a_dependency_error` 原先断言无来源；现在断言来源是邀请解码错误，
    仍能区分“无效输入”与“依赖故障”。
- 第四批（已完成，Infra 中未被阻塞的 S4 至此清零）：`ProfileFactoryResetCapabilityError` 改为带可选来源的结构体，
  出厂重置的 20 处底层失败携带来源；成员认证服务端处理函数由 `&'static str` 改为 `HandlerRejection`（固定原因加可选来源，
  日志仍只记原因）；可达性探测的准入确认改为携带 `anyhow` 来源；Core 的 `PublishLogError::InvalidCiphertext`、
  `BlobError::InvalidTicket`、`MembershipHistoryExchangeError::Offline`、`KeyMigrationError::InvalidCiphertext`、
  `MembershipSecurityUpdateError::Unavailable`，以及 `RenderEncodeError::EncryptFailed`、`PrepareJoinerInvitationError::Invalid`、
  `PullWireError::SnapshotHashNotUtf8`、`SessionProtocolUnavailable`；入站接收提交的加密失败携带来源。
  - 按例外注释：锁中毒（内存事件库、节点运行锁等）、整数与切片长度转换、`Vec` 转定长数组（错误值是明文密钥字节）、
    `SelectionPolicyVersion` 的 `String` 解析错误（同时去掉错误文本中的持久值与条目标识）、索引引用长度校验改归“索引未就绪”。
- 暂缓：`EncryptionError` 与 `SecureStorageError` 以及 `AeadError::DecryptFailed` 的变体被 049 未提交的 `profile_key_recovery.rs` 模式匹配，等 049 提交后处理。


### E7 S4 F 类：Application、Engine 与绑定（进行中）

- Application（已完成，S4 清零）：`RosterError::MembershipReconciliationUnavailable`、`ProfileFactoryResetError::{WipeKeys, ClearState}`、
  `RelayCredentialsError::Corrupt`、`JoinSpaceError::{PreviousJoinCannotBeSuperseded, InvalidStartMaterial}`、
  `DecideDeviceTrustChangeError`、`HandleMembershipHistoryMessageError`、Core 的 `MembershipInitializationError`、
  `CurrentSpaceMemberScopeError::RecoveryRequired`（原先 `Copy`；测试替身改为按分类重建）、`QueryDeviceTrustError`、
  `RemoveSpaceMemberError`、`MembershipEffectExecutionError::Deferred`；成员历史同步的导出失败新增 `ExchangeFailure::HistoryExport`。
  `MembershipLedgerError` 到 `QueryDeviceTrustError`/`CurrentSpaceMemberScopeError` 的 `From` 转换保留来源。
  - 按例外注释：整数转换、`Url::from_file_path` 的 `()` 错误、超时、锁中毒、通道发送与 oneshot 接收、用户输入的 relay URL 校验、
    文件集展开失败落为排除行（业务结果）。
- 观测运行时（已完成）：宿主配置输入校验、全局状态已安装、panic 载荷按例外注释；导出器与本地文件初始化失败只降级为
  `Unavailable`，此时日志通道尚未建立，按例外注释。
- 兼容线（已完成）：`MobileFileUploadError::UploadFailed` 改为可选来源；二维码输入校验、ISO 时间文本格式校验、mpsc 接收、
  `()` 错误与整数转换按例外注释。
- 公开契约边界（已决策，待实施）：`EngineError` / `BindingError` 保持现有公开形态；边界处的 `map_err(|_| code)` 改为经由命名的
  映射函数，先按观测规范从 source chain 提取固定分类记录诊断，再产出错误码；规范中把“公开契约边界映射”列为允许例外。
