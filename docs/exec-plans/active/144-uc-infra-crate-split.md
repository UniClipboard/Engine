# 144 `uc-infra` 拆成 7 个 crate

## 状态与完整责任

- **状态**：实施中，收尾阶段。S0（完整基线实验）按用户指示跳过；S1–S5 已完成并合并，`uc-infra` 已删除；
  S6 第一部分已随 PR #152 合并（`7d4e496d`）；S6 第二部分构建性能对照已在本机受控条件下完成。已完成部分的实施记录
  已归档到 [S1–S6 实施记录](../completed/144-uc-infra-crate-split-record.md)。剩余未闭环事项见下方"未闭环事项"。
- **日期**：2026-10-04。
- **跟踪**：[Issue #144](https://github.com/UniClipboard/Engine/issues/144)（设计全文、七个 crate 的职责/允许依赖表、
  六类依赖切断方案、Edge Cases、测试策略、构建性能实验方法、验收标准均在 issue 正文，本文件不复制，只跟踪切片状态与未闭环事项）。
- **依据**：issue #144 第 5/6 节的 crate 职责表与 S0–S6 步骤表。
- **完整负责人**：拆分顺序、每个切片的范围与验收由本计划维护；每个切片落地后的架构事实（依赖方向、包清单）
  由 `scripts/architecture/check-engine-repository.mjs` 校验，不在本文件重复记录检查脚本本身的规则。
- **调用方唯一动作**：无新增调用方接口——这是内部实现重组，`uc-engine` 仍是唯一稳定入口；下游产品仓不感知。
- **成功结果**：七个目标 crate 全部落地，原 `uc-infra` 删除，`uc-engine` 之外无人直接依赖七个新 crate；
  issue 第 9 节 Acceptance Criteria 全部勾选，包括真实构建性能 A/B 实验的结论。
- **失败结果**：任何切片发现 issue 设计在实际代码里行不通，必须在本文件和对应 PR 里写明原因，不得为了硬套
  设计破坏正确性（例如本次已确认 `content_key_catalog.rs`/`mls_group.rs`/`history_signature.rs`/
  `key_epoch_aad.rs`/`space_admission_auth.rs` 都比 issue 原表预期更早进入 crypto，见实施记录）。
- **重启与重试责任**：每个切片（S1–S5）是一套独立可工作的迁移，不留旧路径 re-export；下一个切片可以在任意
  会话重新开始，只需先确认当前 `main` 上七个 crate 的实际边界（见下表状态列），不能假设 issue 原文的耦合点
  分析仍然完全对应当前代码。

## 切片状态

| 步骤 | 范围（issue 原文） | 状态 | 结果/证据 |
| --- | --- | --- | --- |
| S0 | 精确逐文件迁移清单 + 真实 timings/RSS 基线 | **跳过**（用户 2026-10-03 明确指示直接动代码） | 无基线数据；后续构建性能验收缺这一环 |
| S1 | 提取 `uc-infra-local`、`uc-infra-crypto` | **完成** | [PR #145](https://github.com/UniClipboard/Engine/pull/145)，已 squash merge 到 `main`（`f0f0b5fb`，2026-10-04） |
| S2 | 原子提取 `uc-infra-security`（session/vault/事务代次一起搬，DB 耦合的生命周期部分留给 profile） | **完成** | [PR #147](https://github.com/UniClipboard/Engine/pull/147)，已 squash merge 到 `main`（`ec3301a3`） |
| S3 | 提取 `uc-infra-storage`、`uc-infra-content` | **完成** | [PR #148](https://github.com/UniClipboard/Engine/pull/148)（`bce30ec9`）；见[实施记录](../completed/144-uc-infra-crate-split-record.md) |
| S4 | 完成邀请 codec、错误分类、身份槽位切断，再提取整个 `uc-infra-p2p` | **完成** | [PR #150](https://github.com/UniClipboard/Engine/pull/150)（`d3af2e55`）；见[实施记录](../completed/144-uc-infra-crate-split-record.md) |
| S5 | 剩余升级/激活能力迁 `uc-infra-profile`，LAN 移 `uc-mobile-lan`，删除 `uc-infra` | **完成** | [PR #151](https://github.com/UniClipboard/Engine/pull/151)（`f8add578`）；见[实施记录](../completed/144-uc-infra-crate-split-record.md) |
| S6 | 更新架构门禁、CI、构建缓存、发布脚本、文档；完成公平性能对照 | 第一部分**完成**（PR #152，`7d4e496d`）；性能对照**完成**（本机受控增量构建，冷构建/release 跳过） | 见[实施记录](../completed/144-uc-infra-crate-split-record.md) |

## issue §9 验收逐项对照（2026-10-04，以 main `1dd4cbad` 为准）

状态只用三种："满足"必须有证据；"部分满足"写明缺口；"未测/跳过"不记为通过。

| # | 验收项（issue §9） | 状态 | 证据与缺口 |
| --- | --- | --- | --- |
| 1 | 七个实现 crate 均有单一明确能力归属，无原 `uc-infra` 包、聚合壳或复制代码 | 满足（有记录在案的偏离） | 七个 crate 已落地，`uc-infra` 已删除，`check-engine-repository.mjs` 校验包清单。偏离：`membership_record` 与准入仓储留在 profile，见实施记录"已决定不改"一节；`mobile_sync` 迁到 `uc-mobile-lan`。复制：只在测试中复刻过一段契约断言（`mobile_device_repo`），没有复制生产代码 |
| 2 | metadata 证明生产依赖有向无环；p2p 无 storage/profile/image/Diesel 闭包；默认 Engine 不引入 LAN | 满足 | Cargo 本身拒绝循环依赖；S4/S5 用 `cargo metadata` 遍历 p2p 生产闭包，631 个依赖中 diesel、libsqlite3-sys、image、zstd、tantivy、rusqlite 零命中；`dependency_firewall` 的 lan-compat 合同测试通过 |
| 3 | 修改 SQL 或内容实现的探针证明兄弟能力保持 fresh；Core/Application 修改不纳入窄失效承诺 | 满足（本机受控条件） | S6b 的 Cargo JSON `fresh` 字段：改 SQL 只重编 storage/profile/Engine，改缩略图只重编 content/profile/Engine，三轮一致。没测 Core/Application 修改，符合"不纳入" |
| 4 | 原有事务与安全操作仍由一个模块负责；Engine 不新增步骤编排、原始密钥或观测阶段查询 | 部分满足 | 现有架构门禁（所有权、观测接口、Engine 编排相关检查）每个切片都通过；Engine 源码没有引用任何密钥类结构。缺口：没有逐项人工审计；在 main `1dd4cbad` 上，两处原始密钥字节字段放宽为 `pub`，与 issue §5 不一致（另外，没有发现真实输出）。修复已随 [PR #154](https://github.com/UniClipboard/Engine/pull/154) 于 2026-10-04T16:08:39Z 合并为 main `cdf272ee`；该合并的 main CI 结果需要另行确认。见下方"密钥字段可见性审计" |
| 5 | 原存储/协议 golden fixtures 与旧 Profile E2E 可读，持久化无新版本 | 满足 | 拆分前后 migrations 逐个相同（61 个）；格式/版本常量集合完全相同；`crates/uc-sync-protocol/tests/golden_vectors.rs` 与 Upgrade compatibility smoke 在 CI 上通过 |
| 6 | 稳定入口 E2E 与失败恢复验证通过，提供可复跑、可校验工件 | 部分满足 | Engine tests（含 `host_contract`、membership smoke）、Connection recovery 在 main CI 上通过。缺口：没有按 issue §8 格式产出 E2E 工件（场景、revision、双方终态、脱敏日志）；本机有 `lifecycle_targets.rs:73` 偶发暂停超时、crash E2E 偶发超时（拆分前就有）、`host_contract space_leave::repeated_leaves…` 稳定超时（在干净 main 上也超时）。main 上的 Rust coverage 是否转绿见下方"未闭环事项" |
| 7 | 默认与 lan-compat、代表性直接 Rust 宿主和绑定检查通过；未运行设备平台明记跳过 | 部分满足 | 默认与 `lan-compat,dev-tools` 的 workspace check、uniffi/ohos `workspace_contract`、uniffi `public_contract` 通过。iOS/Android/HarmonyOS 实机和模拟器：**跳过** |
| 8 | 四类实现修改三轮中位耗时分别报告；建议至少三个场景降低 20%，其余不超过 5% 回退 | 满足（限定条件） | build 降 24.3%–38.9%，check 降 23.2%–48.1%，四类都超过 20%。仅限 Apple M4、`jobs = 2`、dev、预热 sccache 的增量构建；第 3 轮受其他会话干扰，只用前两轮复算结论不变 |
| 9 | 分别报告最大 rustc RSS 与同时刻总 RSS；单进程降低 20%、总峰值不回退超过 10% | 满足（限定条件） | 单进程最大 RSS 中位数下降约 45%–50%，同时刻总 RSS 不高于拆分前。0.25 秒采样，可能漏掉短峰值；没有提高并发 |
| 10 | 若只证明增量构建收益，明确限定结论；冷构建与 release 未测不记通过 | 满足 | 结论已限定为增量构建；冷构建、完整 workspace、release/LTO、其他宿主：**未测，不记通过** |
| 11 | 运行根 AGENTS 要求的交付检查；Cargo 验证串行复用 target | 满足 | 每个 PR 前都跑了 metadata、workspace all-targets check、fmt、Rust style、repository check、diff check，Cargo 验证串行执行（见实施记录） |

另有两项 issue §6、§7 的过程要求**未按字面完成**：S0 精确基线按用户指示跳过；S2–S5 都没有产出"先列失败矩阵再改接口"的事前失败矩阵文档。收尾时补写了[事后失败矩阵](../completed/144-uc-infra-crate-split-failure-matrix.md)，把九类失败情形对应到现有证据，它不能代替事前矩阵和 S0 基线。

详细证据见[实施记录](../completed/144-uc-infra-crate-split-record.md)。

## 密钥字段可见性审计（2026-10-04，只读，未改生产代码）

**issue 的原始要求**（§5 依赖切断第 100 行）：
- `MasterKey`/`Kek` 需要跨包可见时，使用不透明类型，字段、`Debug` 和清零策略保持收敛。
- 跨 crate 可见不等于稳定产品接口，禁止 Engine facade 重导出原始密钥访问。
- 优先迁移完整的算法操作，避免新增方便调用方获取原始字节的接口。

验收第 4 项要求 Engine 不新增原始密钥访问。

**拆分中被放宽的密钥类字段**（对照 `54ffafb3` 与 `1dd4cbad`）：

| 类型与字段 | 所在 crate | 拆分前 → 现在 | 字段类型 | `Debug` / 清零 / 序列化 | 跨 crate 使用方 | Engine 可达性 | 实际输出 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `EncryptionPassphraseChangeJournal.kek`（同一结构还有 `keyslot`、`prepared_registration`、`format_version`；`validate()` 也从 `pub(crate)` 变为 `pub`） | `uc-infra-storage`（crate 根 `pub use` 导出） | `pub(crate)` → `pub`（S3） | **原始字节** `Vec<u8>` | 手写 `Debug` 固定输出 `[REDACTED]`；`Drop` 清零 `kek` 与 `prepared_registration`；`Serialize` 只在 storage 内部写入 `Zeroizing` 缓冲，再用 profile 密钥加密落盘，读取时解密进 `Zeroizing`（这几点拆分前就如此） | `uc-infra-profile` 的 `space/encryption_passphrase_change.rs`：构造日志，读取 `kek` 后立即转成不透明的 `Kek::from_bytes` | `uc-engine` 直接依赖 storage，**编译层面可以访问**；Engine 和绑定源码中**没有引用**，facade 也没有重导出 | 未发现。profile 的 `uc_info!`/`uc_warn!` 只记录固定 trigger/stage 与错误类型，不包含日志结构内容 |
| `PersistedContentKeyEntry.key`（以及 `content_key_id`、`epoch`，和外层 `PersistedContentKeyCatalog`） | `uc-infra-crypto` | `pub(super)` → `pub`（S1） | **原始字节** `Vec<u8>` | 没有 derive `Debug`，无法被格式化输出；`ZeroizeOnDrop`；`Serialize` 经 `encode()` 生成内容密钥目录字节 | `uc-infra-security` 的 `session.rs` 直接使用；`access.rs`、`profile_content_key_vault/catalog.rs`、profile 的 `space_control_generation/material.rs` 只调用 `export_`/`import_admission_content_key_catalog` 这类完整操作 | `uc-engine` 不直接依赖 `uc-infra-crypto`，**无法在源码中直接引用** | 未发现 |
| `MlsAdmission` 等结构的 `wrapping_key`（4 处） | `uc-infra-crypto` | `pub(crate)` → `pub`（S1） | **不透明** `MasterKey` | `MasterKey` 的 `Debug` 输出 `[REDACTED]`，`ZeroizeOnDrop`；`MlsAdmission` 的 `Debug` 把该字段写成 `[REDACTED]` | `uc-infra-security` 的 `access.rs`、profile 的 `profile_key_recovery.rs` | 同上，Engine 不直接依赖 crypto | 未发现 |

**结论**：
两个问题分开判断：

**① 接口边界（违背 issue §5）**：
- 三处都是可见性扩大。
- 前两处让任何直接依赖对应 crate 的代码都能读写原始密钥字节，也能通过公开的 serde 实现把它们序列化出来，与 issue §5"使用不透明类型、避免新增方便调用方获取原始字节的接口"不一致。
- 日志结构所在的 storage 被 `uc-engine` 直接依赖，因此 Engine 在编译层面可以拿到原始 KEK 字节，即使目前没有引用。
- 第三处（`wrapping_key: MasterKey`）是不透明类型，符合要求。

**② 实际输出（未发现）**：
- 三处都没有发现密钥被真实输出：没有日志、`Debug` 或错误正文会写出密钥；序列化只用于加密落盘。
- 这只说明目前没有泄漏，**不能**据此认为 ① 不是问题。

**修复**（2026-10-04，[PR #154](https://github.com/UniClipboard/Engine/pull/154)，提交 `157aead89f830892f55f148df11ac0bd53bcfdae`，分支 `hp/uni/t-0161-key-boundary`，基于 main `1dd4cbad`；2026-10-04T16:08:39Z 合并为 main `cdf272ee`，合并时 PR 上的 CI 尚未跑完）：

- **`EncryptionPassphraseChangeJournal`（由 storage 负责）**：
  - 公开类型只保存 `KeySlot`、不透明的 `Kek` 和 `Zeroizing` 包装的 `prepared_registration`。字段全部私有，不实现 serde，`Debug` 仍然脱敏。
  - 接口：`new(keyslot, kek: Kek, prepared_registration)`、`keyslot()`、`kek() -> &Kek`、`prepared_registration()`。
  - 持久化由 storage 私有的 DTO 负责，字段名、顺序和类型与原结构相同，析构时清零。
  - 原公开的 `validate()` 改为内部的完整性检查，条件不变。
  - profile 的口令变更流程迁移了两处生产调用。
  - 现有测试 `protected_journal_completes_after_restart_without_plaintext_passphrase` 里有 3 行对公开类型做 serde 往返，在新接口下无法再编写，已经移除。紧跟其后的保存、加载断言走的是真实持久化格式和校验，覆盖了同一性质。
- **内容密钥目录（由 crypto 负责）**：
  - 新增不透明的 `ContentKeyCatalog`/`ContentKeyCatalogEntry`，条目只保存 `MasterKey`。
  - `decode`/`encode`/`v2`/`push`/`entries`/`version` 都由 crypto 实现；原 `PersistedContentKey*` 降为私有 DTO，原来的公开 `decode`/`encode` 函数也改为私有。
  - security 的 `session.rs` 迁移了四处用法：两处新建目录、一处安装、一处轮换追加。
  - 细节差异：密钥长度改为在 `decode` 时统一校验。错误分类仍是 `key_material_corrupt`，只是检查顺序变了。
- 没有采用"私有字段 + 返回原始字节的 pub 方法"这种表面上的收敛：跨 crate 只能拿到 `Kek`/`MasterKey`，原始字节只出现在所属模块的持久化格式中。
- 不在本次范围内：
  - `wrapping_key: MasterKey` 已经符合要求，不改。
  - `uc-core` 的 `AdmissionContentKeyEntryV1.key: Vec<u8>` 是拆分前就有的准入协议类型。
  - `SpaceKeyMaterial::key_catalog()` 返回的编码后目录字节里包含密钥，这是拆分前就有的 Core 材料设计。
  - 以上两项列为剩余风险。

**修复的证据**（工件在 `<local-artifact-dir>/issue-144-key-boundary/`，含 `SHA256SUMS`，共 45 个文件）：

- **格式兼容（真实跨版本）**：
  - 用 `1dd4cbad` 的真实公开类型，以固定输入写出口令变更日志、V1 和 V2 内容密钥目录。
  - 新代码能解码，内容一致；重新编码，以及用新接口新建，都与旧字节逐字节相同；`Debug` 仍然脱敏。
  - 临时探针没有提交。
- **边界确实收紧**：
  - 从 `uc-engine` 读取 `journal.kek`、要求实现 `Serialize`、用结构体字面量构造，分别得到 `E0616`、`E0277`、`E0560`。
  - 从 `uc-infra-security` 读取 `entry.key`、要求实现 `Serialize`、引用 DTO，分别得到 `E0616`、`E0277`、`E0603`。
  - 同样的访问在 `1dd4cbad` 上能编译通过，说明探针确实能识别问题。
  - 探针没有提交。
- **现有端到端与集成测试**（修复前跑过一次，格式化后又跑了一次）：
  - crypto、security、storage、profile 共 751 项通过，其中口令变更 4 项用真实存储和 access，包括重启后恢复。
  - `host_contract` 的 `key_loss` 全组和 `startup` 共 23 项通过，包括 `passphrase_change_rewraps_recovery_without_replacing_history`，以及丢失钥匙串或解锁密钥后重启。
  - `config_migration_round_trip_e2e` 2 项通过。
  - `space_membership_auto_pairing_e2e` 的 admission、自动连接、移除收敛共 24 项通过。
  - uniffi `public_contract` 30 项通过。
  - `host_contract` 中唯一的超时是 `space_leave::repeated_leaves…`，它在干净的 main 上也稳定超时。
- **交付清单**：metadata、workspace 与 `lan-compat` check、fmt、style、仓库检查、`git diff --check` 都通过。提交内容与测试时的代码树逐字节相同。
- **没有验证的范围**：
  - 完整升级矩阵（旧版本 Engine 写入、新版本读取）没有在本机运行。
  - 已经存在于真实设备上的加密日志和目录文件，没有用真实设备回放。
  - 设备平台没有运行。

## 未闭环事项

1. **Rust coverage 已在 main 上转绿（单次运行）**：修复 `2d693eb0` 已随 PR #153 合并为 main `1dd4cbad`
   （2026-10-04T13:35:53Z）。main run 37206189173 的 8 个 job 全部 success，包括 Rust coverage（job 111447748041）
   和 4 个 Connection recovery job；上一次 run 37196002696 失败。这个失败本身是偶发的，单次通过不能单独证明问题已消失；
   主要证据仍是本地对照（修复前 3/8，修复后 0/32）。coverage 转绿只说明这一项闭环，上表中"部分满足"和"未测"的项目仍未闭环。
2. **`lifecycle_targets.rs:73` 偶发的暂停超时**：根因未查明；t-0176 的 `083fd0aa` 可能相关，未验证。
3. **与 t-0176 的协作去重**：`is_admission_target_stopped` 仍在运行期线程上同步解密。按 2026-10-04 定下的方案 1，PR #153
   保持原范围（当时的 head 是 `d8bcb887`），该 PR 已于 2026-10-04T13:35:53Z 合并。是否随 t-0176 的 iOS 修复带入，由 t-0176 根据真实生命周期复现决定。
4. **密钥字段边界**：修复已随 [PR #154](https://github.com/UniClipboard/Engine/pull/154) 合并为 main `cdf272ee`（2026-10-04T16:08:39Z），见"密钥字段可见性审计"。合并时 PR 上的 CI 还没跑完；要等 main 上这次合并的 CI 通过后才算闭环。
5. **全部收尾后**：再把本计划整体移入 `completed/`。稳定结论已写回 `docs/design-docs/layers/infrastructure.md`：§2.3、§3 是七个 crate 的划分和依赖方向，§13.2.1 是 async 入口不得在运行期线程上同步阻塞。

## 遗留风险 / 下一步必须处理的事项

1. **构建性能 A/B 实验已完成**（结果见实施记录）。冷构建、完整 workspace、release 和其他宿主仍未测。
2. **main `7d4e496d` 的 Rust coverage 失败**（历史，2026-10-04 run 37196002696）：根因与修复见实施记录中"main coverage 失败"一节（PR #153）；已被 main `1dd4cbad` 的 run 37206189173（全部 success）取代，见"未闭环事项"第 1 条。
3. **S2–S5 没有产出 issue 字面要求的事前失败矩阵文档**（2026-10-04 收尾时补写了[事后对照](../completed/144-uc-infra-crate-split-failure-matrix.md)，不能代替事前矩阵）。
4. **切片收尾必须以 CI 的 Engine tests 为准**：本地只跑 `--lib` 会漏掉集成测试与跨 crate 观测测试；
   S2–S5 的回归都是 CI 已报告但被当作偶发失败合入的。偶发失败要逐条与拆分前 `main` 的失败清单对比后才能忽略。
5. **`infrastructure.md` 中与拆分无关的过时内容未处理**：§13.3.1 引用的 `pairing/session.rs` recv-pump
   与 `spawn_supervised` 已不存在，§14.1 提到不存在的 `uc-platform`；拆分前就已过时，不在本计划范围。
6. **`uc-infra-security`/`uc-infra-storage`/`uc-infra-content`/`uc-infra-p2p`/`uc-infra-profile` 的
   `test-util` feature** 各自放宽了若干 `#[cfg(test)]`/`#[cfg(any(test, feature = "test-util"))]`
   方法；继续拆分时如果还有类似的跨 crate 测试脚手架需求，复用同一个 feature 名字，不要新增第二个
   同义 feature。
