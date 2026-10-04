# 144 `uc-infra` 拆成 7 个 crate

## 状态与完整责任

- **状态**：实施中。S0（完整基线实验）按用户指示跳过；S1、S2 已完成（S1 已合并，S2 本次提交）。S3–S6 未开始。
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
| S2 | 原子提取 `uc-infra-security`（session/vault/事务代次一起搬，DB 耦合的生命周期部分留给 profile） | **完成** | 本次提交；见下方"S2 范围" |
| S3 | 提取 `uc-infra-storage`、`uc-infra-content` | 未开始 | — |
| S4 | 完成邀请 codec、错误分类、身份槽位切断，再提取整个 `uc-infra-p2p` | 未开始 | — |
| S5 | 剩余升级/激活能力迁 `uc-infra-profile`，LAN 移 `uc-mobile-lan`，删除 `uc-infra` | 未开始 | — |
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

## 遗留风险 / 下一步必须处理的事项

1. **构建性能 A/B 实验（issue §8/§9）完全没有做**：S1/S2 分出的三个 crate 里，`uc-infra-security` 依赖
   OpenMLS/opaque-ke 等重依赖，`uc-infra` 仍是编译耗时的主体。必须等 S3（storage/content）或更晚再测，
   否则"提速 20%"之类的验收标准无法验证。
2. **S2 没有产出 issue 字面要求的失败矩阵文档**（见上一节），只做了等价的"零覆盖流失"验证。
3. **S3 开工前的分支状态**：本次 S2 提交若按 squash merge 流程合并，继续 S4 需要先从新 `main` 切干净分支。
4. **`uc-infra-security` 新增的 `test-util` feature** 目前只转发 `uc-infra-crypto/test-util` 并放宽了若干
   `#[cfg(test)]` 方法；S3/S4 继续拆分时如果还有类似的跨 crate 测试脚手架需求，复用同一个 feature，不要
   新增第二个同义 feature。
