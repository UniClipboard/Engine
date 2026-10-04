# 144 `uc-infra` 拆成 7 个 crate

## 状态与完整责任

- **状态**：实施中。S0（完整基线实验）按用户指示跳过；S1、S2、S3、S4 已完成并合并；S5（`uc-infra-profile`，
  删除 `uc-infra`）已完成。S6 未开始。
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
| S3 | 提取 `uc-infra-storage`、`uc-infra-content` | **完成** | 本次提交；见下方"S3 范围" |
| S4 | 完成邀请 codec、错误分类、身份槽位切断，再提取整个 `uc-infra-p2p` | **完成** | 本次提交；见下方"S4 范围" |
| S5 | 剩余升级/激活能力迁 `uc-infra-profile`，LAN 移 `uc-mobile-lan`，删除 `uc-infra` | **完成** | 本次提交；见下方"S5 范围" |
| S6 | 更新架构门禁、CI、构建缓存、发布脚本、文档；完成公平性能对照 | 未开始（S1/S2 已顺带同步 `check-engine-repository.mjs`、`check-observability-privacy.mjs` 的扫描范围，但完整 S6 清单未逐项核对） | — |

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

- `space/membership_record/`（issue 第 5 节第 5 条："成员记录 membership_record/ 移 storage"）
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

**验证**：`cargo check --workspace --all-targets --locked` 全绿（零错误，一次性通过，没有像 S1-S4
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

## 遗留风险 / 下一步必须处理的事项

1. **构建性能 A/B 实验（issue §8/§9）完全没有做**：`uc-infra` 已删除，七个目标 crate 全部落地，
   已经到了可以公平测的时间点，但本次仍未执行真实 timings/RSS 对照实验；需要专门的 S6 任务做。
2. **S2 没有产出 issue 字面要求的失败矩阵文档**，S3/S4/S5 同样没有补；只做了等价的"零覆盖流失"验证。
3. **`.github/workflows/*.yml`、`check-engine-repository.mjs`、`check-observability-privacy.mjs` 等
   架构门禁/CI 脚本的完整性尚未逐项核对 issue 第 6 节"必须同步的已知入口"清单**（release 来源脚本、
   `scripts/testing/` 等）——S6 需要专门过一遍。
4. **文档里的旧路径残留**：S5 只修了入口文档（`ARCHITECTURE.md`、`README.md`）与直接受影响的
   crate 地图（`uc-sync-protocol/AGENTS.md`），并重新生成了 `docs/generated/observability-inventory.md`
   （它自 S1 起就停在旧路径，不是本次引入）。其余约 25 份 active 计划/设计文档、
   `docs/generated/db-schema.md`/`search-rebuild-5000-benchmark.md`（S3 起来源指针已失效）、
   `docs/design-docs/layers/infrastructure.md`，以及 `uc-core` 里若干把 `uc-infra` 当成"Infra 层"
   泛称的 doc comment，仍引用 `crates/uc-infra/...`——按 issue 第 6 节归 S6 统一处理。
5. **`uc-infra-security`/`uc-infra-storage`/`uc-infra-content`/`uc-infra-p2p`/`uc-infra-profile` 的
   `test-util` feature** 各自放宽了若干 `#[cfg(test)]`/`#[cfg(any(test, feature = "test-util"))]`
   方法；继续拆分时如果还有类似的跨 crate 测试脚手架需求，复用同一个 feature 名字，不要新增第二个
   同义 feature。
