# 类型化日志事件与工具链强制

状态：M0、M1、M2、M4 已完成（一次性迁移，2026-09-29）；M3 与 `DiagnosticTaskKind` 声明生成未做；对应 [ADR-030](../../design-docs/decisions/030-typed-log-events-and-enforcement.md)（已采纳）。
基线：`a37be892`（模块日志通道已提交）。

## 动作与责任

- **完整负责人**：`uc-observability-contract` 拥有封装宏、字段目录、值类别与生成的白名单；
  `uc-observability-runtime` 只消费目录，不再手写 `ALLOWED_TEXT_FIELDS`。
- **调用方唯一动作**：用 `uc_warn!` 等封装宏记录，字段名必须在目录内，值必须是该字段声明的类别。
- **成功结果**：字段名或值类别错误在编译期失败；合规调用产生与现有模块日志相同的记录。
- **失败结果**：直接使用 `tracing::{trace,debug,info,warn,error}!` 在 CI 新增违规时失败（棘轮）。
- **重启/重试责任**：无运行期状态；基线文件随迁移进度逐次下调，不允许上调。
- 位置依据：`uc-core` 不依赖 tracing，`uc-application`、`uc-infra`、`uc-engine` 均已依赖 `uc-observability-contract`，
  因此宏放在该 crate，不新增 crate，不给 Engine 增加任何公开面。

## 现状事实（2026-09-29 实测）

- 全 workspace 935 处直接使用 tracing 宏：`uc-application` 409、`uc-infra` 351、`uc-engine` 105、`compatibility` 39、
  `bindings` 13、`uc-observability-runtime` 15、`uc-observability-contract` 3；测试文件仅 19 处。
- 写法分布：`%` 格式化字段 341 处，`?` 格式化字段 41 处，`target:` 39 处，仅消息 175 处，`error` 相关字段 27 处，
  含内插格式串的消息 4 处，带点号的字段名 2 处。
- `%` 字段几乎都是标识：`entry_id` 158、`representation_id` 54、`snapshot_hash` 23、`event_id` 18、`transfer_id` 16、
  `peer` 15、`blob_id` 15。其中 `snapshot_hash`、`hash`、`content_hash`、`code_hash` 是内容派生哈希，
  与[观测文档](../../design-docs/observability.md)“内容派生的哈希不在允许清单内”不一致，是迁移时必须逐个裁决的点。

## 设计

### 调用形态（尽量贴近现有 tracing 写法以降低迁移成本）

```rust
uc_warn!(entry_id = id(&entry_id), error_kind = "peer_scope_unavailable", attempt = attempt, "peer-online resync skipped");
uc_warn!(error = &err as &dyn std::error::Error, "history cleanup failed");
```

- 字段在前、字面量消息在最后；消息不接受格式参数，取值一律进字段。
- 级别宏：`uc_trace!`、`uc_debug!`、`uc_info!`、`uc_warn!`、`uc_error!`；`target:` 只接受目录内常量。
- 展开为 `tracing::event!(Level::X, ..)`，因为试验证明展开为 `warn!` 会被 `disallowed_macros` 报告。

### 字段目录与值类别

- 目录是单一声明，每项是“字段名 → 值类别”：`Literal`（`&'static str` 或实现 `LogEnum` 的固定词表枚举）、
  `Id`（应用生成的随机标识，必须经显式适配器 `id(&x)`）、`Number`、`Bool`、`Error`（`&dyn Error`，走既有错误层渲染）、
  `Redacted`（`Sensitive<T>`，只输出 `<redacted>`，可用于任意字段）。
- 宏把字段解析为目录中的具体项：不在目录内的字段名是编译错误，值类别不匹配是编译错误。
  这直接取代运行期 `<omitted>` 与 `REVIEWED_OMITTED_FIELDS`。
- `Id` 用显式适配器而不是给 Core 的标识类型实现 trait：Core 不得携带观测 trait，孤儿规则也不允许在别处实现。
  适配器只接受 `Display` 的引用，是否真的是随机标识由目录里字段名的准入和评审保证，不由类型系统证明。
- 目录同时生成：运行期白名单、`DiagnosticTaskKind` 一类固定枚举的 `as_str`、schema 快照。不再各自手写。

### 强制

- workspace clippy 使用独立配置禁止五个 tracing 宏（不含 `tracing::event`，见 ADR 试验），只经 `CLIPPY_CONF_DIR` 启用。
- 检查只统计 `clippy::disallowed_macros`，与整体 clippy 是否通过无关（当前整体不干净，且 CI 不跑整体 clippy）；任何一处都失败，没有基线。
- clippy 只认 crate 级 allow：故意保留原始 tracing 的文件在文件顶部用 `#![allow(clippy::disallowed_macros)]` 并写明理由。
- `check-rust-style.mjs` 拒绝直接使用日志宏与观测 crate 之外的 `tracing::event!`；`uc_*!` 宏自身只接受字面量消息，内插在编译期失败。

## 阶段

- [x] **M0 宏与目录**：`uc_observability_contract::{log_fields, log_event}`；五个级别宏、`target:`（仅字面量）、`error =` 分支；
  值类别 `Literal`、`Identifier(random)`、`Vocabulary(reviewed)`、`IoKind`、`Scalar` 与适配器 `log_id`、`log_vocab`、`log_vocab_debug`；
  `warn_on_error!` 改为 `event!` 形式；`trybuild` 用例覆盖未登记字段、`String` 入固定词表字段、标识与词表未经适配器、
  内插消息、`Identifier` 缺少确认记号、已删除字段不在目录里。
- [x] **M1 强制**：`scripts/architecture/log-macro-clippy/clippy.toml`（只经 `CLIPPY_CONF_DIR` 启用）、
  `check-direct-log-macros.mjs` 与其测试、PR Check 步骤；`check-rust-style.mjs` 拒绝直接使用日志宏与观测 crate 之外的 `tracing::event!`。
  用 `--cap-lints warn -A clippy::all -W clippy::disallowed_macros` 避开仓库已有的 clippy 错误；默认特性与 `uc-engine/lan-compat`
  各一轮取并集（后者多 13 处）。起草时的“基线棘轮”随一次性迁移退化为零容忍检查，不再有基线文件。
- [x] **M2 一次性迁移**：用确定性 codemod（解析每个调用点的参数与偏移，按字段类别改写取值、展开简写、整理导入）加编译器逐轮纠错，
  一次改完 936 处调用点（198 个文件）。分类表见 `.planning/2026-09-29-observability-gap-fixes/field-classification.md`。
  用户裁决：已放行的文本字段用 `Vocabulary(reviewed)` 适配器保持落盘文本不变；已审定为不落盘的字段在调用点直接删除；
  绑定经 `uc_engine::observability` 再导出宏与适配器。
  手工处理：`sql`、`relay_url` 与 `blobs.rs` 的连接路径标签删除；`error.type` 改为 `error_kind`；绑定的 `error_kind = ?error`
  改成变体名的固定映射（原来写入 Debug 输出）；因字段删除而失去用途的变量、参数与死函数一并清理。
  故意保留原始 tracing 的三个观测运行期集成测试用 crate 级 allow。
- [x] **M3 错误分类（ADR 第 3 步，2026-09-30）**：`uc_core::error_class::ErrorClass` 与 `error_class`/`source_class` 字段；
  已登记的 12 个类型一次性迁移，`log_safe_errors!` 及登记入口、`warn_on_error!`、`check-module-log-errors.mjs` 全部删除。
- [x] **M4 收尾**：删除 `LEGACY_TEXT_FIELDS`、`REVIEWED_OMITTED_FIELDS` 与 `check-rust-style.mjs` 里读它们的字段审定逻辑；
  运行期白名单直接由目录得出，并有测试冻结“迁移前 69 个名字（去掉 `error.type`）加 12 个已批准新增”的集合；
  更新 `observability.md`、`engine-repository-checks.md`、`uc-engine-interface.md`、`AGENTS.md` 与 ADR-030。
- [x] **`DiagnosticTaskKind` 由声明生成（2026-09-30）**：`diagnostic_task_kinds!` 一处声明，合同测试核对取值与 `observability.md`。

## 验证

- 每个阶段先写失败测试再实现；编译失败用例放入 `trybuild` 或等价机制（是否新增依赖需确认，见开放问题）。
- Cargo 由单一负责人使用共享 `target` 串行执行；`-p uc-infra --features lan-compat` 单独覆盖。
- 交付前检查沿用仓库清单；设备矩阵未执行项记为“跳过”。

## 已裁决问题（2026-09-29，用户）

1. 编译失败测试采用 `trybuild` 作为开发依赖，不用 `compile_fail` 文档测试。
2. `#[instrument]` 约 130 处不纳入本轮 lint，M2 完成后单独处理。
3. 目录中每个 `Id` 字段必须标注“已确认随机生成”，并在评审清单中列出。

## 仍开放

- Windows 与 Android 专属代码里迁移过的调用点没有在对应平台上编译。
- `#[instrument]` 约 130 处不受 lint 影响，单独处理。
