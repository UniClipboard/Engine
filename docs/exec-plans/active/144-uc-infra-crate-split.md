# 144 `uc-infra` 拆成 7 个 crate

## 状态与完整责任

- **状态**：实施中。S0（完整基线实验）按用户指示跳过；S1、S2、S3、S4 已完成并合并；S5（`uc-infra-profile`，
  删除 `uc-infra`）已完成。S6 第一部分（门禁/CI/文档核对与 S2–S5 回归修复）已随 PR #152 合并（`7d4e496d`）；S6 第二部分
  （构建性能 A/B 实验）已在本机受控条件下完成，结果见下方。
- **日期**：2026-10-04。
- **跟踪**：[Issue #144](https://github.com/UniClipboard/Engine/issues/144)（设计全文、七个 crate 的职责/允许依赖表、
  六类依赖切断方案、Edge Cases、测试策略、构建性能实验方法、验收标准均在 issue 正文，本文件不复制，只跟踪切片状态）。
- **依据**：issue #144 第 5/6 节的 crate 职责表与 S0–S6 步骤表。
- **完整负责人**：拆分顺序、每个切片的范围与验收由本计划维护；每个切片落地后的架构事实（依赖方向、包清单）
  由 `scripts/architecture/check-engine-repository.mjs` 校验，不在本文件重复记录检查脚本本身的规则。
- **调用方唯一动作**：无新增调用方接口——这是内部实现重组，`uc-engine` 仍是唯一稳定入口；下游产品仓不感知。
- **成功结果**：七个目标 crate 全部落地，原 `uc-infra` 删除，`uc-engine` 之外无人直接依赖七个新 crate；
  issue 第 9 节 Acceptance Criteria 全部勾选，包括真实构建性能 A/B 实验的结论。
- **失败结果**：任何切片发现 issue 设计在实际代码里行不通，必须在本文件和对应 PR 里写明原因，不得为了硬套
  设计破坏正确性（例如本次已确认 `content_key_catalog.rs`/`mls_group.rs`/`history_signature.rs`/
  `key_epoch_aad.rs`/`space_admission_auth.rs` 都比 issue 原表预期更早进入 crypto，见下）。
- **重启与重试责任**：每个切片（S1–S5）是一套独立可工作的迁移，不留旧路径 re-export；下一个切片可以在任意
  会话重新开始，只需先确认当前 `main` 上七个 crate 的实际边界（见下表状态列），不能假设 issue 原文的耦合点
  分析仍然完全对应当前代码。

## 切片状态

| 步骤 | 范围（issue 原文） | 状态 | 结果/证据 |
| --- | --- | --- | --- |
| S0 | 精确逐文件迁移清单 + 真实 timings/RSS 基线 | **跳过**（用户 2026-10-03 明确指示直接动代码） | 无基线数据；后续构建性能验收缺这一环 |
| S1 | 提取 `uc-infra-local`、`uc-infra-crypto` | **完成** | [PR #145](https://github.com/UniClipboard/Engine/pull/145)，已 squash merge 到 `main`（`f0f0b5fb`，2026-10-04） |
| S2 | 原子提取 `uc-infra-security`（session/vault/事务代次一起搬，DB 耦合的生命周期部分留给 profile） | **完成** | [PR #147](https://github.com/UniClipboard/Engine/pull/147)，已 squash merge 到 `main`（`ec3301a3`） |
| S3 | 提取 `uc-infra-storage`、`uc-infra-content` | **完成** | [PR #148](https://github.com/UniClipboard/Engine/pull/148)（`bce30ec9`）；见下方"S3 范围" |
| S4 | 完成邀请 codec、错误分类、身份槽位切断，再提取整个 `uc-infra-p2p` | **完成** | [PR #150](https://github.com/UniClipboard/Engine/pull/150)（`d3af2e55`）；见下方"S4 范围" |
| S5 | 剩余升级/激活能力迁 `uc-infra-profile`，LAN 移 `uc-mobile-lan`，删除 `uc-infra` | **完成** | [PR #151](https://github.com/UniClipboard/Engine/pull/151)（`f8add578`）；见下方"S5 范围" |
| S6 | 更新架构门禁、CI、构建缓存、发布脚本、文档；完成公平性能对照 | 第一部分**完成**（PR #152，`7d4e496d`）；性能对照**完成**（本机受控增量构建，冷构建/release 跳过） | 见下方"S6 第一部分范围"与"S6 第二部分"两节 |

## S1 范围（2026-10-03，PR #145）

`uc-infra-local`（`fs/`、`blob/`、`time/`、`device/`、`settings/`、四个扁平文件状态仓储、`file_secure_storage`）、
`uc-infra-crypto`（当时只有 `crypto_model`、`secrets`、`v1_aead`、`hashing`、`identity_fingerprint`）。详见 PR #145。

## S2 范围与已知偏离（2026-10-04）

**`uc-infra-security`（新建）**：`access.rs`（`RuntimeSpaceAccessAdapter`/`MigrationSpaceAccessAdapter`）、
`active_space_security_session/`、`admission_key_manager.rs`、`admission_proof.rs`、`blob_cipher_adapter.rs`、
`content_protection/`（`mod.rs`/`context.rs`/`envelope.rs`/`inline.rs`，**不含** `blob_store.rs`，见下）、
`default_current_profile.rs`、`group_update_error.rs`、`key_material.rs`、`key_migration_adapter.rs`、
`key_slot_store.rs`（原 `uc-infra` 的 `fs/key_slot_store.rs`）、`membership_update.rs`、
`profile_content_key_vault/`、`profile_passphrase_recovery.rs`（新文件，见下）、`scope_identifier.rs`、
`secure_storage_access.rs`、`session.rs`（`InMemorySession`）、`session_rebind.rs`。

**比 issue 原表更早进入 `uc-infra-crypto` 的部分**（issue 把它们列为 crypto 候选，但本来估计要等 S2 才动；
实际在 S2 做 security 原子提取时一并验证了它们确实零依赖 session/DB，就跟着搬了）：
`content_key_catalog.rs`、`mls_group.rs`（OpenMLS 引擎）、`history_signature.rs`、`key_epoch_aad.rs`、
`space_admission_auth.rs`（OPAQUE 协议状态机）。

### 依赖切断（issue 第 5 节"必须明确处理的依赖切断"）

1. **`ProfilePassphraseRecoveryPort` 换边**（第 5 节第 1 条）：trait 定义和它的错误类型 `ProfileKeyRecoveryError`
   从 `uc-infra` 的 `profile_key_recovery.rs`（实现文件）搬到 `uc-infra-security` 新文件
   `profile_passphrase_recovery.rs`；`uc-infra` 的 `ProfileKeyRecoveryStore` 继续实现这个 trait，不新增同义 port。
2. **错误分类的数据库依赖切断**（第 5 节第 3 条）：`group_update_failure_detail`（现在在 `uc-infra-security`，
   被 p2p 的 `network/iroh/group_update_adapter.rs` 读取）原本直接 `downcast_ref::<diesel::result::Error>()`，
   这会让 security crate（进而 p2p）依赖 Diesel。改法：在真实错误转换处（`uc-infra` 的
   `db/repositories/space_security_store.rs::backend()`，本来就依赖 Diesel）把 diesel 错误翻译成新增的
   `uc_observability_contract::diagnostics::connectivity::ClassifiedGroupUpdateStorageFailure`（diesel-free，
   带 `GroupUpdateReason` + 原始 source），`group_update_failure_detail` 只认这个新类型，不再认识
   `diesel::result::Error`。原有的"real SQLite failure"单测按这条新边界拆成两个：一个在 `db/` 验证 Diesel→
   分类的真实转换，一个在 `uc-infra-security` 验证分类→诊断读取，覆盖面不变。

### 跨 crate 测试的处理方式

严格遵守"不新增只验证导入路径的测试，现有覆盖随模块移动"：

- `access.rs::admission_tests` 里唯一一条真实 SQLite 集成测试
  （`current_revocation_snapshot_survives_repository_restart`）迁到 `uc-infra` 新的
  `tests/space_access_adapter_restart.rs`——它要用仍留在 `uc-infra` 的真实 `DieselSpaceSecurityStore`，
  而 `RuntimeSpaceAccessAdapter` 已经在 `uc-infra-security`；这个方向（下游 `uc-infra` 测试上游
  `uc-infra-security` 的公开 API）不产生循环依赖。
- `content_protection::tests` 里两条用到 `ProfilePayloadAdapters`/`V3EncryptedBlobStore`（content/profile，
  未拆分前仍留在 `uc-infra`）的测试，迁到 `uc-infra` 新的 `security/content_protection/tests.rs`，调用
  `uc-infra-security` 的公开 `ContentProtection` API。
- `access.rs::admission_tests` 里 `retained_device_applies_admission_then_revocation_epoch_updates` 原本用
  `crate::clipboard::chunked_transfer::TransferCipherAdapter`（content，留在 `uc-infra`）验证 admission+
  revocation 后内容密钥代次正确轮转。这条测试依赖的 `prepare_group_join`/`admit_group_member`/
  `install_group_join` 是 `uc-infra-security` 内部 `#[cfg(test)]` 的测试脚手架方法（没有对应的生产级 public
  trait），不能作为真实产品契约对外暴露，所以这条测试**没有**移出 `uc-infra-security`；改用同 crate 内
  同样"绑定当前 session 代次"的 `BlobCipherAdapter` 代替 `TransferCipherAdapter` 完成同一组断言（见
  `access.rs` 测试里的注释）。这改变了被练习的具体 adapter（`BlobCipherPort` 而非 `TransferCipherPort`），
  但验证的不变量（代次轮转后谁能/不能解密）完全一致。
- 多处 `#[cfg(test)]` 的 `pub(crate)` 方法（`InMemorySession::{create_legacy_bootstrap_material,
  create_migrated_space_material,snapshot,restore}`、`RuntimeSpaceAccessAdapter::{prepare_group_join,
  admit_group_member,admit_group_member_with_replay,install_group_join}`、
  `ActiveSpaceSecuritySession::activate`）被 `uc-infra` 自己的测试/生产代码跨 crate 调用；`cfg(test)` 是
  crate-local 的，下游 crate 测试时不会为 `uc-infra-security` 打开它，所以统一改成
  `#[cfg(feature = "test-util")]` 并转 `pub`，`uc-infra` 的 `[dependencies]`/`[dev-dependencies]` 都加了
  `uc-infra-security`（后者带 `features = ["test-util"]`）让 Cargo 的 feature unification 在编译 `uc-infra`
  测试时一并打开。

### 架构脚本同步

- `check-engine-repository.mjs`：`EXPECTED_PACKAGES`/`INTERNAL_PACKAGES` 加 `uc-infra-security`；
  `checkInfraSpaceSecurityOwnership` 和几处硬编码的 `crates/uc-infra/src/space/security/*.rs`/
  `crates/uc-infra/src/security/{secure_storage_access,profile_content_key_vault/key_store}.rs` 路径读取
  改成指向 `uc-infra-security`/`uc-infra-crypto` 的新位置。
- `check-observability-privacy.mjs`：`SOURCE_ROOTS` 补了 `uc-infra-local/src`、`uc-infra-crypto/src`、
  `uc-infra-security/src`——**S1 当时漏了这一步**（只顾着改 `check-engine-repository.mjs` 的包清单），
  这次一并补上；回扫后合法 callsite 从检查口径变化前的 1153 升到 1273，说明 S1 遗留的两个新 crate 之前完全
  没被这项检查覆盖过，属于本次顺带发现并修复的缺口。

### 失败矩阵（issue 要求"先列失败矩阵再改动接口"的落实方式）

没有产出单独的失败矩阵文档；采用的替代验证方式是"零覆盖流失"而非"预先枚举输入/结果表"：每个被移动的
`#[cfg(test)]`/单测模块都保留或等价迁移（细节见上一节），不是靠访问不到私有实现就删测试或加 pub 绕过，
迁移过程中用编译器报错驱动——cargo 报"方法不存在/不可见"的每一处，都对应判断该方法是测试脚手架（转
`test-util` feature）还是真实生产 API（直接放宽可见性）。移动前后：`uc-infra --lib` 973 passed（S1 基线）
→ 迁移后 `uc-infra-security` 114 passed + `uc-infra-crypto` 新增的 30 个（41 passed 总计，含 S1 的 11 个）+
`uc-infra --lib` 840 passed（含本次新增的 1 个跨 crate DB 集成测试，不含移到 `tests/` 目录下的另一个），
`uc-engine --lib` 276 passed 不变。全部 0 failed。这满足了"不丢覆盖"的实质要求，但不是 issue 字面要求的
"先列失败矩阵"文档；如果后续复盘认为这不够，需要补一份显式的矩阵。

## S3 范围（2026-10-04，storage + content 两半）

**`uc-infra-storage`（新建）**：`db/`（全部 repository/mapper/model/schema，含其 Diesel `migrations/` 目录）、
`file_transfer/`、`search/`、`active_space_generation_manifest_store.rs`（及其
`EncryptionPassphraseChangeJournal`，仍只在 `uc-infra` 的 `space/encryption_passphrase_change.rs` 内部使用）。

**没有跟着这一半走的部分**（issue 原表把它们的 SQL 所有权分给 storage，但本次刻意不动）：

- `space/membership_record/`（issue 第 5 节第 5 条："成员记录 membership_record/ 移 storage"；后续已决定不改，见"已决定不改"一节）
- `space/admission/repository/` 及其 `display.rs` 配套读取（issue 同一条："准入记录 repository/、配套 display
  读取实现...移 storage，完整事务一起移动"）

这两组和 `space/admission/` 其余"preparation/activation"（profile 目标，留在 `uc-infra` 等 S5）物理上还在同一
目录树里，没有先做一次"只挪 SQL 那一半、把 preparation/activation 留在原地"的拆分就直接搬，风险和工作量都
不比这一刀本身小；留给专门的后续切片，不在 S3 这次顺带做。

**`uc-infra-content`（issue 原表 S3 的另一半，2026-10-04 完成）**：`clipboard/`、`config/`、
`security/encrypted_blob_store.rs`（现 `uc-infra-content/src/encrypted_blob_store.rs`）、
`content_protection/blob_store.rs`（现 `uc-infra-content/src/content_protection/blob_store.rs`）、
`profile_payload_adapters.rs`，以及 `decrypting_clipboard_event_repo`/`decrypting_representation_repo`/
`encrypting_clipboard_event_writer`/`encrypting_inbound_receive_commit` 四个加解密适配器，全部只依赖
`uc-infra-local`/`uc-infra-crypto`/`uc-infra-security`。`uc-infra/src/security/mod.rs` 保留
`pub use uc_infra_content::{...}` 转发，不留旧实现。`EncryptedBlobStore::open_bytes` 从 `pub(super)` 放宽到
`pub`，供 `uc-infra` 的 `profile_storage_upgrade::primary_payloads` 跨 crate 调用（升级路径需要先读原字节区分
介质失败与密文认证失败）。`uc-engine` 里 4 处 `uc_infra::clipboard::`/`uc_infra::config::` 引用改成
`uc_infra_content::clipboard::`/`uc_infra_content::config::`，`uc-infra`/`uc-engine` 的 `Cargo.toml` 都加了
`uc-infra-content` 依赖（`uc-infra` 的 `test-util` feature 一并转发 `uc-infra-content/test-util`）。验证：
`uc-infra-content` 自身 76 passed；`uc-infra --lib` 513 passed（含本次新增的可见性放宽覆盖）；`uc-engine --lib`
276 passed/3 ignored，与既有基线一致。

### 可复用的模式（延续 S2 的跨 crate 测试处理方式）

- **编译器驱动的可见性修正**：和 S2 一样，没有预先枚举——每个 `cargo check` 报的"方法不存在/不可见"都用来
  判断该放宽到 `pub`（真实跨 crate 调用方，主要是 `uc-infra` 的 `profile_storage_upgrade`/
  `space/membership_record`/`space/admission`）还是该转 `#[cfg(any(test, feature = "test-util"))]`
  （`test_relationship_store` 等纯测试脚手架）。
- **跨 crate 测试：原样迁移/新 integration test/小量复刻，三选一，不删测试**：这次只用到了"小量复刻"——
  `mobile_device_repo` 的并发写入契约测试（`verify_activity_contract`）原本在 `uc-infra` 的
  `mobile_sync::device_repo::tests` 里，同时被内存 fake（留在 `uc-infra`）和真实 SQLite repo（现在在
  `uc-infra-storage`）复用；由于依赖方向是 `uc-infra` → `uc-infra-storage`，storage 不能反过来调用 `uc-infra`
  的测试私有方法，所以在 `uc-infra-storage` 里复刻了一份同样内容的契约测试（纯测试断言，不是生产实现）。
- **已知的架构脚本缺口**：`check-engine-repository.mjs` 里有两处之前直接 `read()` 旧路径
  （`crates/uc-infra/src/db/repositories/{mod.rs,relationship_store.rs}`）的硬编码检查，这次一起改到了新路径；
  如果后续还发现类似遗漏，优先假设"检查脚本跟旧路径"而不是"代码本身有问题"。

## S4 范围（2026-10-04，`uc-infra-p2p`）

**新建 `uc-infra-p2p`**：`network/`（Iroh 节点/连接/发现/blob 网络传输/认证交换/网络诊断，含
`space_admission/` 协议帧处理）、`pairing/`（mDNS 发现、短码邀请解析）、`rendezvous/`（HTTP 短码目录客户端、
邀请签发/消费）。只依赖 `uc-core`/`uc-application`/`uc-infra-crypto`/`uc-infra-security`/
`uc-observability-contract`/`uc-sync-protocol` 加 iroh/reqwest 网络栈本身；验证过（`cargo metadata` 遍历生产
依赖闭包）不含 diesel/libsqlite3/image/zstd/搜索实现——issue 强调的验收标准在当前代码里本来就是干净的，
不需要额外清理，只是确认并固定下来。

**开工前先处理的真实双向耦合**（即表格里"完成邀请 codec、错误分类、身份槽位切断"指的内容，不是字面设计，是
实际发现的两类问题）：

1. **邀请票据 codec 反向依赖**：`pairing/invitation_resolver.rs`、`rendezvous/invitation_adapter.rs`（搬进
   `uc-infra-p2p`）原本依赖 `uc-infra::space::admission::full_invitation` 的
   `encode_full_invitation`/`decode_full_invitation`/`decode_invitation_entry`（纯 postcard/base64 编解码，
   不含网络 I/O）；而 `uc-infra` 自己的 admission 侧也要用同一份 codec。两边互相需要，任何一边单方面依赖
   另一边都会成环。解法：把这三个函数连同 `FullInvitationCodecError`/`DecodedFullInvitation` 整个迁到
   `uc-sync-protocol`（其定位正是"transport-independent wire codecs"，且两边已经都依赖它），新增 `base64`
   依赖（已同步加入 `check-engine-repository.mjs` 的 `SYNC_PROTOCOL_ALLOWED_DEPENDENCIES` 白名单）。原来两个
   纯编解码单元测试原样迁入 `uc-sync-protocol`；一个需要完整 DI（`DefaultJoinerInvitationPreparation`）的
   集成测试留在 `uc-infra`，迁到调用方 `invitation_start.rs` 自己的 `#[cfg(test)]` 模块。
2. **`SpaceAdmissionChannelCredentialError::diagnostic_failure` 的孤儿实现**：这个方法原来是在 `uc-infra` 的
   `space/admission/credentials.rs` 里给（物理上要搬进 `uc-infra-p2p` 的）`SpaceAdmissionChannelCredentialError`
   类型追加的 inherent impl，内部向下转型 `uc-infra` 自己的 `CredentialLoadError`/
   `SpaceAdmissionCredentialStoreError` 做更细的分类（Locked/RecoveryRequired）。一旦类型搬到 `uc-infra-p2p`，
   `uc-infra` 不能再给外部 crate 的类型补 inherent impl（Rust 孤儿规则）,而这个 impl 要转型的两个类型又是
   `uc-infra` 专属,不能反向依赖进 `uc-infra-p2p`。解法：在 `uc-infra-p2p` 里把这个方法简化成只返回
   `Unavailable`/`Rejected` 两个变体各自对应的保底分类,删掉向下转型的细化逻辑——确认过唯一的外部调用方
   （`uc-infra-p2p` 自己的 `space_admission/diagnostics.rs`）没有测试依赖那层细化，行为影响为零。

**其余机械性工作**：`uc-infra` 里 6 个仍调用 `crate::network::iroh::...` 的文件（`security/profile_key_recovery.rs`、
`security/profile_upgrade_backup/inventory.rs`、`space/encryption_passphrase_change.rs`、
`space/admission/credentials.rs`、`space/admission/joiner/sponsor_identity.rs`、
`space/adapters/membership_network_gate.rs`）改成 `uc_infra_p2p::network::iroh::...`，同时把
`decode_space_admission_continuation_endpoint`、`membership_history_exchange_adapter::request_purpose`、
`SponsorOpaqueMaterial::into_parts` 从 `pub(crate)`/`#[cfg(test)]` 放宽到 `pub`（均为真实生产跨 crate 调用，
由编译错误驱动确认）。`uc-engine` 里 11 个文件的 `uc_infra::network::iroh::` 引用改成 `uc_infra_p2p::network::iroh::`。
`uc-infra`/`uc-engine` 的 `Cargo.toml` 都加了 `uc-infra-p2p` 依赖；`uc-infra` 的 `test-util` feature 一并转发
`uc-infra-p2p/test-util`。`uc-infra` 自身不再需要 iroh 网络栈大部分依赖（`iroh-blobs`/`iroh-mdns-address-lookup`/
`iroh-relay`/`iroh-tickets`/`noq-proto`/`swarm-discovery`/`reqwest`/`rustls`/`if-addrs`/`futures-util`），全部移出
`uc-infra/Cargo.toml`（只留 `iroh` 本身，因为 `EndpointAddr` 类型仍经 `uc-infra-p2p` 的解码函数回传到几个调用点）；
`wiremock`/`mockall` 这两个 dev-dependency 同理只被 network/pairing/rendezvous 的测试用到，一并移出。

**跨 crate 集成测试搬迁**：`crates/uc-infra/tests/` 下纯 p2p 的 8 个文件（`node_lifecycle.rs`、
`lan_only_relay_mode.rs`、`iroh_blobs_probe.rs`、`iroh_clipboard_identity_probe.rs`、
`iroh_peer_reachability_probe.rs`、`peer_admission_identity_resolution.rs`、
`inbound_peer_rejection_diagnostics.rs`、`clipboard_receive_diagnostic_file.rs`）原样迁到
`crates/uc-infra-p2p/tests/`；`admission_diagnostic_file.rs`、`profile_storage_upgrade.rs` 两个混合依赖
（真实 SQLite admission/security 代码 + p2p 类型测试替身）留在 `uc-infra`，只改引用路径。

**验证**：`cargo metadata` 遍历 `uc-infra-p2p` 生产依赖闭包，确认不含 diesel/libsqlite3/image/zstd/搜索；
`uc-infra-p2p --all-targets` 全绿；`uc-infra --lib` 237 passed/3 ignored（较 S3 后的 511/5 减少的 274/2 恰好
对应搬进 `uc-infra-p2p` 的测试，零覆盖流失）；`uc-engine --lib` 276 passed/3 ignored，与既有基线一致。

### 可复用的模式（延续 S2/S3 的跨 crate 测试处理方式）

- **先处理双向耦合，再搬文件**：S4 和 S1–S3 的关键差异是这次真的发现了双向依赖（见上），必须先把共享的纯
  逻辑（codec）下沉到两边都能单向依赖的公共 crate，再做物理搬迁，不能先搬再补。
- **Rust 孤儿规则是真实的物理约束**：给搬走的类型追加 inherent impl 在拆分前能编译，拆分后立刻报错；这类
  "跨 crate 对同一类型的扩展"要在设计阶段就假设会出问题，而不是等编译器报错才发现。
- **编译器驱动的可见性修正**：延续 S2/S3，没有预先枚举——每个 `cargo check` 报的"方法不存在/不可见"都用来
  判断该放宽到 `pub` 还是转测试专属可见性。

## S5 范围（2026-10-03，`uc-infra-profile` + 删除 `uc-infra`）

**`mobile_sync/` 先搬到 `uc-mobile-lan`，不进 `uc-infra-profile`**：issue 原表写"LAN 移既有兼容
crate"，但实际检查 `uc-infra/src/mobile_sync/` 后发现它只实现 `uc-core::ports::mobile_sync` 的端口
（`credentials_minter`/`password_hasher`/`endpoint_info`/`file_staging`/`lan_probe`），没有任何
`crate::` 内部耦合——它从一开始就该和 `compatibility/uc-mobile-lan` 现有的 `facade`/`usecases`
（同样消费这组端口）放在一起，不是先进 `uc-infra-profile` 再搬第二次。`uc-engine` 的 `lan-compat`
feature 改为直接转发 `uc-infra-storage/lan-compat`（mobile 设备 SQLite 记录仍在 storage），不再经过
`uc-infra`/`uc-infra-profile`。`uc-infra` 因此也不再需要 `network-interface`/`image` 依赖，一并删除。

**`uc-infra` 剩余的 `config_migration/`、`security/`（`profile_backup_archive`、
`profile_key_recovery`、`profile_lifecycle`、`profile_reset`、`profile_runtime_layout`、
`profile_startup_storage`、`profile_storage_upgrade`、`profile_upgrade_backup`、
`space_control_generation`、`space_transition_activation`、`v3_*`）、`space/`（`adapters`、
`admission`、`membership_record`、`encryption_passphrase_change`、`membership_branch_transition`）
整体就是 issue 表里的 `uc-infra-profile` 全部范围，没有剩余——这次是整 crate 改名
（`git mv crates/uc-infra crates/uc-infra-profile` + 包名/描述/依赖方更新），不是部分抽取**。
顺手清理了 S3 遗留的死文件：`diesel.toml`/`Makefile` 仍指向早已不存在的 `src/db/`、`migrations/`
（S3 把它们搬进 `uc-infra-storage` 时没有删这两个配置文件），这次一起删除。

**机械性更新**：`uc_infra::` → `uc_infra_profile::`（`uc-engine`、`uc-infra-profile` 自己的
`tests/`/`benches/`、`uc-observability-runtime` 测试）；根 `Cargo.toml` workspace members、
`[profile.dev.package.*]`；`uc-engine`/`uc-application` 的依赖声明与注释；`.config/nextest.toml`
里四条按包名过滤的测试覆盖规则（两条已在 S4 随 `uc-infra-p2p` 搬迁但当时漏改：
`provider_dependency_evidence`/`node_lifecycle` 应为 `package(uc-infra-p2p)`；两条
`profile_storage_upgrade*` 改为 `package(uc-infra-profile)`）；`.github/workflows/*.yml`、
`.github/actions/rust-ci-setup/action.yml`、`scripts/performance/run.mjs` 里按包名过滤的 CI 步骤；
`scripts/architecture/check-engine-repository.mjs` 的 `EXPECTED_PACKAGES`/`INTERNAL_PACKAGES`/
大量硬编码 `crates/uc-infra/src/...` 路径；`scripts/architecture/check-observability-privacy.mjs`
的 `SOURCE_ROOTS`（新增 `crates/uc-infra-profile/src` 与 `compatibility/uc-mobile-lan/src`——后者
是因为 `mobile_sync` 带着它的 `uc_observability_contract` 调用点一起搬进了 uc-mobile-lan，之前这个
crate 从未被该检查脚本扫描过）；`crates/uc-engine/tests/dependency_firewall.rs` 的
`engine_default_dependency_contract_excludes_lan_compat_dependencies` 整段重写（原来断言
`uc-infra` 的 `network-interface` optional + `uc-infra/lan-compat` 转发，现在断言
`uc-mobile-lan` 的 `network-interface` 是普通依赖 + `uc-infra-storage/lan-compat` 转发）。

**验证**（更正：下列只覆盖 `--lib` 与少数集成测试，没有跑 `uc-infra-profile`、`uc-observability-runtime`
的全部集成测试，PR #151 合入时 CI 的 Engine tests 实际失败，见"S6 第一部分范围"）：`cargo check --workspace --all-targets --locked` 全绿（零错误，一次性通过，没有像 S1-S4
那样需要反复用编译错误驱动修可见性——因为这次是整 crate 改名，内部可见性关系不变）；
`check-engine-repository.mjs` 全绿（含全部负向夹具）；`check-observability-privacy.mjs`
1324 个记录点（较 S4 后的 1261 新增 63，对应 `uc-mobile-lan` 首次被纳入扫描）；`uc-infra-profile --lib
--features test-util` 237 passed/3 ignored（与 S4 后的基线完全一致，mobile_sync 搬离不影响这个
crate 的测试数——它原本就是 `#[cfg(feature = "lan-compat")]` 才编译，默认 `--lib` 跑的测试本来就不含它）；
`uc-mobile-lan --lib` 238 passed（含 mobile_sync 自带的 15 个单测，随文件原样迁入，零覆盖流失）；
`uc-engine --lib` 276 passed/3 ignored，`dependency_firewall` 34 passed，均与既有基线一致；
`uc-infra-p2p` 生产依赖闭包重新用 `cargo metadata` 遍历，631 个依赖，diesel/libsqlite3-sys/image/
zstd/tantivy/rusqlite 零命中——确认整 crate 改名不影响 S4 已验证过的验收标准。

### 可复用的模式

- **"issue 设计表的职责归属"要先核对实际代码耦合，不能直接照抄**：`mobile_sync/` 在 issue 原表只写了
  "LAN 移既有兼容 crate"，字面上可能被理解成先进 `uc-infra-profile` 再搬，但读代码后发现它和
  `uc-infra-profile` 的其余模块没有任何耦合，是独立的端口实现集合，直接搬进真正的消费方更省一步、也
  更准确。
- **整 crate 改名用 `git mv` + 包名/路径机械替换，不要当成部分抽取来做**：当"剩下的全部内容都属于同一个
  新 crate"时，不需要像 S1-S4 那样逐文件判断去留，`cargo check` 也因此一次性全绿，没有可见性修正的
  反复迭代。
- **架构脚本的"按包名过滤"规则（nextest 覆盖组、CI workflow 步骤）要和"按路径硬编码"规则一起检查**：
  这次发现 `.config/nextest.toml` 里两条规则在 S4 搬 `uc-infra-p2p` 时已经漏改，说明这类按
  `package(...)` 过滤的配置也需要在每次切片收尾时全仓搜索包名字符串，不能只检查
  `check-engine-repository.mjs` 一个脚本。

## S6 第一部分范围（2026-10-04，门禁/CI/文档核对与回归修复）

**S2–S5 合入时 CI 的 Engine tests 都是失败的**，各切片只在本地跑了 `--lib` 与少数集成测试，失败被当作
既有偶发失败一并忽略。逐次对比 `main` 上 pr-check 的失败清单后确认，下列失败由拆分引入（`uc-engine-uniffi`
的 `lifecycle_targets`/`scheduled_flush` 等时序测试在拆分前的 `main` 上已偶发失败，不属于本列）：

| 引入切片 | 失败 | 原因 | 修复 |
| --- | --- | --- | --- |
| S1 起 | `uc-observability-runtime` 的 `module_log_channel`、`host_composition`；S3 起 `peer_address_read_diagnostics`；S5 起 `space_admission_state::sponsor` | **生产回归**：`module_log::is_engine_source` 只认精确的 `uc_infra`/`uc_infra::` 前缀，所有 `uc_infra_*` crate 的记录被当作第三方来源，进不了模块日志通道与 Engine 运行期 | 前缀表改为七个能力 crate 的精确名称，不放宽匹配规则 |
| S2/S4 | `uc-infra-profile::inbound_peer_single_owner` 五项 | 结构验收按 `crates/uc-infra/src/...` 读源码，文件不存在即 panic | 路径改到 `uc-infra-p2p`/`uc-infra-profile` 的实际位置 |
| S4 | `admission_diagnostic_file`（`error.reason` 由 `record_missing` 退化为 `storage_recovery_required`） | **诊断回归**：S4 为避开孤儿规则把 `SpaceAdmissionChannelCredentialError::diagnostic_failure` 简化为按变体兜底，丢掉了按来源细分的凭据分类；当时判断"没有测试依赖"不成立 | 错误变体增加 `failure: CredentialFailure` 字段，由构造错误的凭据负责人（`uc-infra-profile`）按来源类型写入，网络侧只读取，不跨 crate 向下转型 |

| S3 | Linux 上 `peer_address_read_diagnostics` 的栈断言（路由修复后才暴露） | `sanitized_backtrace` 先用完整行过滤，再把每帧截到 160 字符。内联组合子帧（`map<…>`）只在泛型参数里带模块路径，拆分前 `uc_infra::…::relationship_store` 恰好在第 160 字符结束；改名为 `uc_infra_storage::` 后标记被截掉 | 每帧上限改为 256，断言不变；Docker `rust:1.95-bookworm` 复现：拆分前通过、修复前失败、修复后连续 3 次通过 |

修复后 `uc-infra-profile` + `uc-infra-p2p` 全部测试 635 通过，`uc-observability-runtime` 75 通过；
本机按 CI 口径复跑：`evidence` 18 项、`persistence-provider` 65 项全部通过；`workspace` 4087 项中
4084 通过，失败/超时的 `offline_lifecycle::crash::interrupted_file_transfer_recovers_after_receiver_process_restart`、
`host_contract space_leave::repeated_leaves_do_not_accumulate_tasks_or_descriptors` 与 uniffi
`lifecycle_targets` 在拆分前的 `main`（`f8add578`）独立 worktree 上同样失败（前者 6 次中 3 次失败，
后者稳定超时），不是本次引入；`process` 组 `key_loss` 的超时只在高并发下出现，单独运行全部通过。

**issue 第 6 节"必须同步的已知入口"逐项核对**：

- 根 `Cargo.toml`：**`uc-infra-p2p` 缺 dev `opt-level = 3`**（S4 漏加，七个 crate 里唯一一个），已补；
  `crates/uc-engine/tests/dependency_firewall.rs` 用 `toml` crate 解析根清单，要求每个 `uc-infra-*`
  workspace 成员都保留该设置（含从原 mjs 检查迁移的删除与降级两条负面用例）。
- `.github/actions/rust-ci-setup/action.yml` 的 `fast-compile-infra`：S5 后只把 `uc-infra-profile`
  降回 opt-level 0，原语义是整个 `uc-infra`，已扩展为七个 crate。
- `scripts/testing/run-test-group.sh`（`evidence`/`persistence-provider`/`process`）与
  `run-connection-recovery-e2e.sh` 仍用 `-p uc-infra`/`package(uc-infra)`，已按测试实际所在 crate 改写；
  `check-engine-repository.mjs` 新增 `package selectors` 检查（含负面用例）：`.github/`、`scripts/`、
  `.config/nextest.toml` 中的 `package(...)` 与 `-p/--package uc-*` 必须是 workspace 成员。
- `check-observability-privacy.mjs` 已覆盖七个新根（S1–S5 随切片同步）；release 脚本只引用
  `crates/uc-infra-storage/migrations`（S3 已改）；LAN 兼容线源码包是整个提交的 `git archive`，自然包含新闭包。
- 文档：`docs/design-docs/layers/infrastructure.md` 按七个 crate 重写分层、依赖方向与 `test-util` 约定；
  其余 active 计划、设计文档、ADR 中指向当前代码位置的路径与命令，`docs/generated/` 的来源指针，
  以及代码注释/测试里的复跑命令全部改到实际 crate。completed 计划、`.planning/` 与 ADR 的历史叙述保持原样。
  ADR-032 中"`uc-infra` 与 `network/iroh` 双向依赖"的未决事项已标为由 S4 解决。

## 已决定不改：`membership_record` 与准入仓储留在 `uc-infra-profile`（2026-10-04）

issue 第 5 节第 5 条把 `space/membership_record/` 和 `space/admission/repository/` 的 SQL 归给 storage。
本计划决定**不搬迁**。边界定为：领域记录格式和对应查询由 `uc-infra-profile` 拥有；表结构、migrations、
连接池、执行器与事务原语由 `uc-infra-storage` 提供；依赖方向是 profile → storage，没有环。

**源码核查**（2026-10-04，以 `7d4e496d` 为准，只读）：

- **规模**：
  - `membership_record/`：9 个文件、约 2560 行，其中 2 个文件直接写 SQL，只碰 `membership_ledger_state` 一张表。
  - `admission/repository/`：8 个文件、约 2950 行，其中 5 个文件直接写 SQL，碰 `admission_repository_state`、`admission_repository_record`、`admission_recovery_summary` 三张表。
  - 其余代码是加密记录格式、旧格式迁移和恢复索引。
- **profile 之外的调用方**：
  - 对外公开的只有 `SqliteMembershipRecordStore`、`SqliteSpaceAdmissionState` 和 `AdmissionRepositoryBenchmark`。
  - 生产代码里只有 Engine 组装层（`crates/uc-engine/src/assembly/deps.rs`、`assembly/wire/mod.rs`）构造它们，作为 Application port（如 `MembershipRecordStorePort`）注入。Engine 不读取记录内容。
  - `uc-engine/tests/host_contract/{key_loss.rs,startup/admission_recovery.rs}` 直接用 SQL 写入或破坏准入表，用来构造故障现场，属于测试，不是生产调用方。
  - 除 migrations 外，storage 和其他 `uc-infra-*` 的源码都不引用这四张表。
- **跨表原子事务**：
  - 成员记录提交（`membership_record/store.rs` 的 `commit_record` 和另一处提交路径）在同一个 `immediate_transaction` 里先写 `membership_ledger_state`，再调用 storage 的 `MembershipProjectionWriter::apply(conn, plan)`（`uc-infra-storage/src/db/repositories/relationship_store/projection.rs`），写入 `encrypted_relationship` 的 member、trusted_peer、peer_address 行。
  - 这条跨 crate 的原子提交现在就能成立：storage 提供在调用方事务中执行的写入器，profile 负责组合，测试 `the_read_model_is_written_in_the_record_transaction` 覆盖了它。
  - 准入仓储的"事务内读写"接口（`load_state_in_transaction_on`、`save_state_on` 等）只被 profile 自己的准入模块调用（`credentials.rs`、`joiner/activation_state.rs`、`joiner/cancellation.rs`、`display.rs`），没有被边界挡住的跨 crate 事务。

**保留现状的理由**：没有 profile 之外的真实调用方需要这些记录；已有的跨表原子事务没有被边界阻断；
整块搬进 storage 会让 storage 承担准入和成员领域格式，只拆 SQL 又要切开持久化格式代码；留在 profile
时，改动这些记录只会让 profile 和 Engine 重新编译。

**重新考虑的条件**：
- 出现 profile 之外需要读写这些记录的生产调用方。
- 出现需要和这些表同事务提交、但无法由 profile 组合 storage 写入器完成的 storage 侧写入。

**核查边界**：
- 只做了源码静态核查，没有运行时追踪。
- 没有逐一核查 Application 层是否在一次业务动作中跨多个 port 期望原子性。那属于流程负责人的设计，不由本边界决定。

## S6 第二部分：构建性能对照协议（2026-10-04）

按 issue §8 执行。S0 被跳过，没有预先登记的基线，因此以拆分前的 `54ffafb3` 为 A 组、以 S6 第一部分合并后的
main `7d4e496d` 为 B 组，在同一台机器上成对测量。

- **固定条件**：
  - toolchain 1.95.0；两组 `.cargo/config.toml` 逐字相同（`jobs = 2`，rustflags 为 `--cfg tokio_unstable`）。
  - dev profile；A 组的 `uc-infra` 与 B 组的七个 `uc-infra-*` 都是 dev `opt-level = 3`。
  - 共享 sccache 不清空，按 issue §8 第 1 条记录每次构建前后的命中/未命中差值。
  - 两组各用独立 worktree，构建目录由 wrapper 分配。
  - 每组的锁文件取各自提交的版本，使用 `--locked`。
- **构建目标**：`cargo build --locked -p uc-engine-uniffi --lib`，产出 rlib、staticlib 和 cdylib，覆盖代表性宿主的最终链接。`cargo check` 用同一目标单列，不当作代码生成性能。
- **修改探针**：在四个实际实现函数的函数体开头插入一行 `std::hint::black_box(<常量>);`。语义等价，但会改变代码生成。
  - P2P：`network/iroh/clipboard_dispatch_adapter.rs::network_span`
  - SQL：`db/repositories/relationship_store.rs::is_relationship_diagnostic_frame`
  - 缩略图：`clipboard/thumbnail_generator.rs::calculate_target_size`
  - Profile：`security/profile_storage_upgrade/journal.rs::record_primary_warnings`

  每次测量都换一个新常量，按"轮次 × 文件 × 阶段"编码，并记录文件 SHA256，避免命中 sccache。每次测量前只有一个文件发生变化；跨阶段改动由不计入结果的同步构建吸收。
- **轮次**：3 轮，A/B 在每个测量点交替执行，奇数轮先 A、偶数轮先 B。每轮依次做：
  1. 同步构建（不计时）
  2. 无修改 warm build
  3. 四个文件各改一次后 build
  4. 同步 check（不计时）
  5. 无修改 warm check
  6. 四个文件各改一次后 check
- **采集**：
  - wall time
  - Cargo JSON 中 `compiler-artifact.fresh` 的 fresh/dirty 包列表
  - `--timings` 原始 HTML
  - sccache 统计差值
  - 每 250 毫秒对 rustc、链接器、C 编译器和归档工具进程采样一次，记录单进程最大 RSS 和同一时刻的总 RSS。只在同一采样时刻内求和，采样间隔内可能漏掉峰值。
- **不测的项目**（记为"跳过"）：
  - 冷构建：要求不清空共享缓存，又没有独立的缓存命名空间。
  - 完整 workspace 构建、release 构建、其他宿主或设备。
- **判定**：按 issue §9 的建议预算，分别报告四类修改在三轮中的中位耗时。预算未达成就不宣称完成提速目标。

## S6 第二部分：构建性能对照结果（2026-10-04）

按上面的协议执行，3 轮共 60 个测量点，全部成功。原始数据、`--timings` HTML、Cargo JSON、驱动脚本、汇总脚本和 `SHA256SUMS`（`rounds-*` 共 190 个文件，`prime/` 4 个文件）在
`/Volumes/ExternalSSD/cargo-targets/workspaces/engine/6686a9a07cb796b0/test-artifacts/issue-144-s6b-build-perf/`
（`prime/` 和 `rounds-20261004T105410Z/`）。复跑命令：`python3 perf_ab.py <输出目录> 3 A=<54ffafb3 worktree> B=<7d4e496d worktree>`，再运行 `perf_summary.py <输出目录>`。

**三轮中位 wall time**（A = 拆分前 `54ffafb3`，B = `7d4e496d`；目标为 `-p uc-engine-uniffi --lib`，含 staticlib/cdylib 链接）：

| 模式 | 改动 | A 中位 | B 中位 | B 相对 A | A 单进程 RSS 中位 | B 单进程 RSS 中位 |
| --- | --- | --- | --- | --- | --- | --- |
| build | P2P | 85.9 s | 64.1 s | −25.4% | 2834 MB | 1566 MB |
| build | SQL | 87.5 s | 66.2 s | −24.3% | 2866 MB | 1452 MB |
| build | 缩略图 | 84.8 s | 54.0 s | −36.3% | 2923 MB | 1584 MB |
| build | Profile | 85.6 s | 52.4 s | −38.9% | 2887 MB | 1517 MB |
| check | P2P | 33.3 s | 25.5 s | −23.2% | 1476 MB | 854 MB |
| check | SQL | 34.1 s | 22.6 s | −33.6% | 1453 MB | 854 MB |
| check | 缩略图 | 34.05 s | 18.0 s | −47.1% | 1457 MB | 855 MB |
| check | Profile | 33.6 s | 17.5 s | −48.1% | 1454 MB | 809 MB |

无修改的 warm build/check 两组都在 0.4–1.2 秒之间，均无重编单元。

**失效集合**（来自 Cargo JSON 的 `fresh` 字段，三轮一致）：
- A 组：四类修改都会重编 `uc_infra`、`uc_engine`、`uc_engine_uniffi`。
- B 组：
  - 改 P2P 只重编 `uc_infra_p2p`、`uc_infra_profile`、`uc_engine`、`uc_engine_uniffi`。
  - 改 SQL 只重编 storage、profile、Engine。
  - 改缩略图只重编 content、profile、Engine。
  - 改 Profile 只重编 profile、Engine。
- 兄弟能力 crate 都保持 fresh，例如改 SQL 时 p2p、content、security 不重编。
- 两组每次都会重编 `uc_observability_runtime`，原因未调查。它对两组相同，不影响对比。

**按 issue §9 的预算判定**（只在下列受控条件内成立）：
- 四类实际修改的 build 中位耗时降幅都超过 20%，**满足**"至少三个场景降低 20%、其余不超过 5% 回退"。
- 单进程最大 RSS 下降约 45–50%，**满足**"单进程降低 20%"。
- 同一时刻总 RSS 的 B 组中位值也低于 A 组，**满足**"总峰值不回退超过 10%"。

**噪声与污染边界**：
- 第 3 轮从 build 缩略图开始，负载均值升到 25–40，期间有 18–21 个其他会话的 cargo/rustc 进程。sccache 差值里也混入了其他会话的编译（例如 B 组第 3 轮改 SQL 时，命中 15 次、未命中 577 次）。60 个测量点中有 12 个记录到外部 cargo 进程。
- 只用干净的第 1–2 轮复算，build 的降幅为 −24.6%、−29.2%、−37.6%、−40.0%，check 的降幅为 −23.7% 到 −48.0%，结论不变。
- 本机常驻负载（Android 模拟器、远程桌面、浏览器）没有关闭，每个点都记录了负载均值。

**RSS 测量边界**：
- 每 0.25 秒用 `ps` 采样一次，只统计 rustc、ld、clang、cc、ar 等工具进程；同一时刻总量只在同一次采样内求和。
- 可能漏掉持续时间短于采样间隔的峰值。
- 没有统计 sccache 服务进程本身。

**未测，记为"跳过"**：
- 冷构建：不清空共享缓存，也没有独立的缓存命名空间。
- 完整 workspace build 和 release/LTO 构建。
- 其他宿主（Android、HarmonyOS、桌面）、其他机器和 CI 环境。

**结论范围**：本结论只适用于 macOS（Apple M4、10 核）、`jobs = 2`、dev profile、预热好的共享 sccache、以 `uc-engine-uniffi` 为目标的增量构建。不能推广到冷构建、release 构建或下游产品仓的 workspace 根配置。

## main coverage 失败：uniffi 生命周期测试的根因与修复（2026-10-04）

**现象**：main `7d4e496d` 的 Rust coverage job（run 37196002696）中，
`uc-engine-uniffi::public_contract lifecycle_targets::a_pause_reaches_the_engine_while_the_profile_vault_key_is_waiting`
失败，位置在 `lifecycle_targets.rs:93`：`engine.lifecycle_state()` 返回 `RuntimeUnavailable`。第 39、40 行的 panic 是它连带出来的。

**是否由拆分引入**：在本机用干净的拆分前 worktree（`54ffafb3`）和当前代码交替各跑 8 轮。这个测试在两边都是 **3/8 失败**，
失败位置和报错完全相同。所以它在拆分前就已存在，不是拆分引入的。

**根因**（用 macOS `sample` 在卡住期间抓到的线程栈证实）：
- 移动绑定在 `uc-engine-uniffi` 线程上用**单线程** tokio 运行期驱动 Engine。
- 恢复时，`RecoverSpaceSessionUseCase` 经 `CurrentSpaceResolver` 调用 `ActiveSpaceGenerationManifestStore::load_runtime`。这是一个 async 函数，但它在运行期线程上**同步**执行了 `AdmissionKeyManager::open_profile_payload`，进而同步读取宿主安全存储里的 profile key。
- 测试的读取闸门让这次读取阻塞，于是运行期唯一的线程被占住，同一运行期上的生命周期 worker 无法处理状态查询，10 秒后返回 `RuntimeUnavailable`。
- 另一路 `kek` 读取经 `spawn_blocking` 执行，走的是正确的路径。两路读取谁先撞上闸门是随机的，所以测试时好时坏。
- 在真实设备上，这意味着恢复时只要宿主安全存储的读取较慢，就会让同一运行期上的暂停和状态查询停滞。

**修复**：`load_runtime` 仍在运行期上读取文件，把解密这一步（会读安全存储）放进 `spawn_blocking`，并沿用 storage 仓储已有的 `Span::current().in_scope` 写法；解码部分抽成不碰存储的 `decode_runtime_plaintext`。同步版本 `load_runtime_sync` 的行为不变。没有新增抽象，也没有新增测试。

**验证**：用现有的端到端测试验证：修复后两个 `lifecycle_targets` 测试各跑 16 次，第 93 行的失败为 **0/32**（修复前 3/8）。之后又连续抓栈运行 30 次，全部通过。

**仍未解决**：
- 修复后的 32 次运行里有 1 次在 `lifecycle_targets.rs:73` 失败：`create_space` 之后 `suspend()` 超出 10 秒期限（`DeadlineExceeded`）。修复前在本机高负载时也出现过。
- 之后连续 30 次抓栈运行都没能再触发它，根因没有证据，所以没有做推测性修改。
- 同一个 store 里 `promote`、`persist_manifest` 等 async 路径也会在运行期线程上同步调用 `seal_profile_payload`，属于同类风险，但这次没有被任何失败证实，暂不修改。

工件在 `/Volumes/ExternalSSD/cargo-targets/workspaces/engine/6686a9a07cb796b0/test-artifacts/issue-144-uniffi-lifecycle-20261004T121315Z/`：
- `results.txt`、`runs/`：修复前的对照
- `sample/`：卡住时的线程栈
- `fixed/`：修复后的 32 次运行和 `fix.diff`
- `sample73/`：第 73 行问题的抓栈尝试
- `verify/`：交付检查

## 遗留风险 / 下一步必须处理的事项

1. **构建性能 A/B 实验已完成**（S6 第二部分，结果见上）。冷构建、完整 workspace、release 和其他宿主仍未测。
2. **main `7d4e496d` 的 Rust coverage 失败**：根因已找到并修复，见"main coverage 失败"一节。修复后的 CI 结果
   以本 PR 为准。`lifecycle_targets.rs:73` 偶发的暂停超时根因仍未找到。
3. **S2 没有产出 issue 字面要求的失败矩阵文档**，S3/S4/S5 同样没有补；只做了等价的"零覆盖流失"验证。
4. **切片收尾必须以 CI 的 Engine tests 为准**：本地只跑 `--lib` 会漏掉集成测试与跨 crate 观测测试；
   S2–S5 的回归都是 CI 已报告但被当作偶发失败合入的。偶发失败要逐条与拆分前 `main` 的失败清单对比后才能忽略。
5. **`infrastructure.md` 中与拆分无关的过时内容未处理**：§13.3.1 引用的 `pairing/session.rs` recv-pump
   与 `spawn_supervised` 已不存在，§14.1 提到不存在的 `uc-platform`；拆分前就已过时，不在本计划范围。
6. **`uc-infra-security`/`uc-infra-storage`/`uc-infra-content`/`uc-infra-p2p`/`uc-infra-profile` 的
   `test-util` feature** 各自放宽了若干 `#[cfg(test)]`/`#[cfg(any(test, feature = "test-util"))]`
   方法；继续拆分时如果还有类似的跨 crate 测试脚手架需求，复用同一个 feature 名字，不要新增第二个
   同义 feature。
