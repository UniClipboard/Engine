# 类型化日志事件与工具链强制

状态：实施中（M0 起）；对应 [ADR-030](../../design-docs/decisions/030-typed-log-events-and-enforcement.md)（已采纳）。
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

- workspace clippy 使用独立配置禁止五个 tracing 宏（不含 `tracing::event`，见 ADR 试验）；
  观测三个 crate 与测试文件通过配置豁免或 `cfg_attr(test, allow(..))`。
- 新增 CI 步骤只统计 `clippy::disallowed_macros`，与整体 clippy 是否通过无关（当前整体不干净，且 CI 不跑 clippy）。
- 基线文件按文件记录违规数，脚本对比：任何文件的计数只许下降，下降后基线必须同步下调。“被修改的文件清零”不作为门禁（会让一行改动被迫迁移几十处），
  而是 M2 逐模块迁移的目标。
- `check-rust-style.mjs` 增加：拒绝 `tracing::event!` 直接使用、拒绝 `uc_*!` 消息中的内插与位置参数。

## 阶段

- [x] **M0 宏与目录（2026-09-29 完成）**：`uc_observability_contract::{log_fields, log_event}`；五个级别宏、`target:`、`error =` 分支；
  目录种子 `entry_id`（`Identifier(random)`）、`error_kind`（`Literal`）、`io_error_kind`（`IoKind`）；
  运行期文本字段白名单由目录并上过渡清单 `LEGACY_TEXT_FIELDS` 得出，测试冻结迁移前的 69 个名字、保证集合不变；
  `warn_on_error!` 改为 `event!` 形式；`check-rust-style.mjs` 同时读取目录，并把 `$crate::` 视为宏卫生路径；
  `trybuild` 用例覆盖未登记字段、`String` 入固定词表字段、标识未经 `id()`、内插消息、`Identifier` 缺少确认记号；
  第一个真实调用点是 `clipboard/sync/active_state/fanout.rs` 的两处 `warn!`，端到端测试确认落盘字段、错误链与源码位置不变。
  验证：`cargo check --workspace --all-targets --locked`、`fmt --check`、两个架构脚本、脚本测试、契约与运行期测试、`cargo audit` 均通过。
  实施中定下的细节：字段名不含点号（仓库 2 处带点字段改名，不进目录）；`Literal` 类别只接受 `&'static str` 或为自己实现
  `Accept<Literal>` 的封闭枚举；类别约束放在调用点（`Accept::<fields::名::Class>::accept`），`#[diagnostic::on_unimplemented]`
  给出稳定报错，避免每次新增字段都改写 `trybuild` 的期望输出；`target:` 暂接受任意表达式，M2 再收紧到目录常量。
- [x] **M1 强制与基线（2026-09-29 完成）**：`scripts/architecture/log-macro-clippy/clippy.toml`（只经 `CLIPPY_CONF_DIR` 启用，
  日常 clippy 不受影响）、`check-log-macro-ratchet.mjs` 与其测试、`log-macro-baseline.json`、PR Check 步骤、
  `check-rust-style.mjs` 拒绝观测 crate 之外直接使用 `tracing::event!`。
  实测基线 945 处 / 199 个文件：默认特性 932，加 `uc-engine/lan-compat` 后 945（多 13 处），所以脚本跑两轮并取并集。
  用 `--cap-lints warn -A clippy::all -W clippy::disallowed_macros` 避开仓库已有的 clippy 错误，只统计目标 lint。
  端到端验证：故意新增一处 `tracing::warn!` 使脚本以 1 退出并指出文件与行号，撤销后通过。
  未采用的原方案：观测 crate 与测试文件不设豁免，其现有违规照常计入基线，避免维护第二套豁免清单。
- [ ] **M2 逐 crate 迁移**：顺序 `uc-application`（409）→ `uc-engine`（105）→ `uc-infra`（351）→ 其余；
  每个提交只迁一个模块，基线同步下调。内容派生哈希字段逐个裁决：改成不记录，或改用 `Sensitive`。
  补日志任务的 55 条缺口在 M0、M1 完成后直接使用新宏写入，不先写旧式再迁移。
- [ ] **M3 错误分类（ADR 第 3 步）**：分类 trait 与 `error_kind` 词表进目录；随触碰的错误类型渐进实现；
  `log_safe_errors!` 全部覆盖后删除并更新 `check-module-log-errors.mjs`。
- [ ] **M4 收尾**：删除旧白名单与 `REVIEWED_OMITTED_FIELDS`、旧启发式检查；更新 `observability.md` 与
  `error-handling.md`；把 ADR-030 改为已采纳并记录最终偏离。

## 验证

- 每个阶段先写失败测试再实现；编译失败用例放入 `trybuild` 或等价机制（是否新增依赖需确认，见开放问题）。
- Cargo 由单一负责人使用共享 `target` 串行执行；`-p uc-infra --features lan-compat` 单独覆盖。
- 交付前检查沿用仓库清单；设备矩阵未执行项记为“跳过”。

## 已裁决问题（2026-09-29，用户）

1. 编译失败测试采用 `trybuild` 作为开发依赖，不用 `compile_fail` 文档测试。
2. `#[instrument]` 约 130 处不纳入本轮 lint，M2 完成后单独处理。
3. 目录中每个 `Id` 字段必须标注“已确认随机生成”，并在评审清单中列出。

## 仍开放

- `bindings`（13 处，只能依赖 `uc-engine`）与 `compatibility`（39 处，独立发布线）能否直接使用宏，M2 前核对；若需经 `uc-engine` 再导出，先论证不属于为观测扩大 facade。
- 基线文件与 `RUST_STYLE_BASE_SHA` 差异检查并存时的冲突处理，M1 内定。
