# ADR-030：日志编写改为类型化事件并由工具链强制

- **状态**：已采纳；第 1 步已于 2026-09-29 一次性完成，第 2 步部分完成，第 3 步未开始
- **日期**：2026-09-29
- **范围**：`uc-application`、`uc-infra`、`uc-engine` 中的 `tracing` 调用点，`uc-observability-contract` 的任务与字段词表，
  `scripts/architecture/check-rust-style.mjs` 与 workspace clippy 配置；不改变分层责任、远程遥测合同与业务记录准入标准
- **实施计划**：[类型化日志事件与工具链强制](../../exec-plans/active/2026-09-29-typed-log-events.md)
- **相关文件**：[运行期观测](../observability.md)、[错误处理](../error-handling.md)、
  [`module_log_fields.rs`](../../../crates/uc-observability-runtime/src/module_log_fields.rs)、
  [`diagnostics/mod.rs`](../../../crates/uc-observability-contract/src/diagnostics/mod.rs)

## 背景

当前日志规则正确但主要靠人和文本检查守住，违规的表现是静默降级而不是编译失败：

1. 字符串字段不在 `ALLOWED_TEXT_FIELDS` 时运行期记为 `<omitted>`，编译与测试都不报错。
2. 错误类型未用 `log_safe_errors!` 登记时该层渲染为 `<opaque>`。仓库约 325 个错误类型，登记入口约 15 处。
   稳定版 Rust 只能对具体类型 `downcast`，anyhow 的 context 层与第三方错误无法覆盖。
3. 新增一个 `DiagnosticTaskKind` 变体要同步枚举、`as_str`、schema 快照与隐私测试四处。
4. 仓库约 900 个日志调用点直接使用 `tracing::{info,warn,error,debug}!`。`check-rust-style.mjs` 对新增行做文本启发式检查，
   文档承认需要人工复核；它只看相对基线新增的行，因此存量缺口只能靠专项扫描发现。

2026-09-29 的全仓扫描（[findings](../../../.planning/2026-09-29-observability-gap-fixes/findings.md)）印证了这一点：
15 条 P0 缺口多数不是“缺一行 `warn!`”，而是缺少合适的记录形态（无对应任务类别、错误未登记、来源在 `map_err` 处已丢失）。

## 决定

分三步。第 1 步已一次性完成，不留新旧两套写法；第 2、3 步单独推进。

1. **收口写入口，并用编译工具链禁止绕过（已完成，2026-09-29）。**
   - 日志一律使用 `uc_trace!`、`uc_debug!`、`uc_info!`、`uc_warn!`、`uc_error!`（位于 `uc-observability-contract`，
     绑定经 `uc_engine::observability` 再导出，不新增层）。字段名解析为目录中的真实路径，未登记的名字与值类别不符都是编译错误；
     `target:` 只接受字面量，消息只接受字面量。
   - 文本类值类别：`Literal`（`&'static str`）、`Identifier(random)`（应用生成的随机标识，经 `log_id(&x)` 显式适配）、
     `Vocabulary(reviewed)`（已审定的词表类文本，字面量直接接受，其余经 `log_vocab` / `log_vocab_debug` 显式适配）、`IoKind`；
     数值与布尔为 `Scalar`。`Identifier` 与 `Vocabulary` 的适配器与 ADR 起草时设想的密封 `LogSafe` trait 不同：
     Core 不能携带观测 trait，孤儿规则也不允许在别处实现，所以由调用点显式断言、字段名在目录里带确认记号
     （`random`、`reviewed`，漏写是编译错误），信任模型是“字段名审定 + 调用点断言”，不由类型证明。
   - 已审定为“不落盘”的字段（`peer`、`path`、各类哈希、`session_id`、`conn` 等，运行期取值恒为 `<omitted>`）
     在调用点直接删除，不进入目录；因此这些名字无法再出现在 `uc_*!` 里。
   - workspace 内 936 处直接使用 `tracing` 日志宏的调用点全部迁移，基线为零，不设基线文件：
     `check-direct-log-macros.mjs` 在默认特性与 `lan-compat` 下各跑一轮 clippy 的 `disallowed_macros`，任何一处都失败。
     clippy 只认 crate 级 allow，故意保留原始 tracing 的文件（观测运行期验证未登记字段处理的三个集成测试）在文件顶部
     用 `#![allow(clippy::disallowed_macros)]` 并写明理由。`tracing::event!` 只有观测 crate 自己可以直接使用。
2. **事件与字段词表单点声明并生成（部分完成）。** 日志字段目录与运行期白名单已是单一声明；
   `DiagnosticTaskKind` 由声明生成、`error_kind` 词表与 schema 快照仍未做，作为独立后续。
3. **错误改为固定分类，取代逐层 downcast 登记（未开始）。**
   - 仓库自有错误类型实现分类 trait（返回固定 `ErrorKind` 枚举）。日志边界取分类与 `io_error_kind`，
     不再依赖 `log_safe_errors!` 逐层渲染；第三方与 anyhow 层继续保守渲染为 `<opaque>`。
   - 迁移随触碰的错误类型渐进进行，`log_safe_errors!` 在覆盖完成前保留，之后删除，不长期并存两套入口。

## 先后关系

- 模块日志通道（`2026-09-29-module-log-channel`）已先行提交，本 ADR 的第 1 步建立在它之上。
- 补日志（`2026-09-29-observability-gap-fixes` 的 55 条缺口）在第 1 步之后直接使用 `uc_*!` 宏编写，不先写旧式再迁移；
  它需要新增的任务类别与字段按目录声明方式登记。
- 补日志与本 ADR 互不替代：前者补缺失记录，后者保证以后新写的记录不再静默出错。

## 备选方案

- **维持现状，只加强文本检查。** 成本最低，但启发式检查无法区分字面量与插值，违规仍然静默，长期复杂度继续增长。
- **整体迁移到 OpenTelemetry Weaver 等外部注册表工具链。** 可借鉴其“声明式清单生成代码、文档与校验”的思路，
  但引入外部工具链与本仓的隐私词表、固定枚举模型不完全对应，先在仓内用最小生成器验证，再决定是否引入。
- **逐模块渐进迁移，用基线计数棘轮约束。** 起草时的方案。用户要求一次性迁移且不留中间状态，
  实测可行（机械改写加编译器逐轮纠错），所以改为一次完成并把基线直接定为零；棘轮脚本随之退化为零容忍检查。
- **严格枚举 `as_str` 取代 `Vocabulary(reviewed)`。** 编译期能证明取值封闭，但落盘文本可能从 Debug 写法变成 `as_str` 写法，
  涉及几十个枚举与 Core 改动；本次未采用，可作为 `Vocabulary` 字段的后续收紧方向。
- **依赖 `Error::provide` 通用成员访问。** 据当前了解仍属不稳定特性，不作为基础。

## 影响

- 行为变化（已由测试冻结）：运行期文本字段白名单去掉带点号的 `error.type`（两处调用点改用 `error_kind`），
  新增 12 个可见文本字段——`cause`、`context`、`dependency`、`emitter`、`event`、`file_paths_source`、`issue`、`plan`、
  `storage_generation`、`task`、`trigger` 只接受 `&'static str`，`existing_status` 是文件传输状态的已审定词表；
  约 195 处原先恒为 `<omitted>` 的键被删除；`blobs.rs` 的连接路径标签、`sql` 文本等只为已删除字段计算的值一并移除。
- 绑定与宿主经 `uc_engine::observability` 再导出宏与适配器，属于既有观测入口的扩充，不暴露 Application 或 Core 的内部阶段。
- 新增字段必须先在目录登记类别；`Vocabulary` 的适配器是评审点，需要审查者确认取值不含用户数据。
- 直接使用 `tracing` 日志宏的检查需要 clippy，约两分钟，只在 PR Check 中运行。

## 起草期试验结果（2026-09-29）

用 `CLIPPY_CONF_DIR` 指向临时配置，对 `uc-application` lib 启用 `disallowed_macros`（禁止 `tracing::{trace,debug,info,warn,error}`），
另用独立小 crate 验证封装宏：

- 直接调用被稳定识别：423 处，涉及 79 个文件；`#[instrument]` 展开不误报。
- 禁止列表里不能包含 `tracing::event`：`warn!` 等宏内部展开为 `event!`，会额外产生 501 条指向 tracing 源码的重复报告。
- 封装宏若展开为 `tracing::warn!`，调用处仍被报告，在展开内加 `#[allow(clippy::disallowed_macros)]` 无效。
  封装宏展开为 `tracing::event!(Level::X, ..)` 则不被报告，直接调用仍被报告。
  因此封装宏走 `event!`，`tracing::event!` 的直接使用由 `check-rust-style.mjs` 单独拒绝（该写法在仓库中极少，文本检查即可）。
- 全 workspace（`--workspace --all-targets`，默认特性）共 935 处唯一违规：`uc-application` 409、`uc-infra` 351、`uc-engine` 105、
  `compatibility` 39、`bindings` 13、`uc-observability-runtime` 15、`uc-observability-contract` 3；
  按级别 `warn` 400、`debug` 233、`info` 220、`error` 71、`trace` 11；位于测试文件的仅 19 处。冷缓存下整轮约 1 分半。
- `LogSafe` 可用 `macro_rules!` 表达：字符串类字段值经密封 trait 约束，`String` 与非 `'static` 的 `&str` 编译失败，
  字面量与 `u64` 通过；消息位置只接受字面量。数值、布尔与 `error = &e as &dyn Error` 需各自单独的匹配分支。
- `#[allow(clippy::disallowed_macros)]` 写在语句、块、函数或模块上都不能压住这条 lint，只有 crate 级（含每个集成测试文件顶部）的 allow 有效。
- 当前工作区 `cargo clippy` 并不干净：`uc-application` 有约 190 条其他 lint 与 1 个 deny 级错误
  （`async_yields_async`，`application/shutdown.rs:70`），`tests/hosts/uc-mobile-probe-core` 另有 2 个 `never_loop` 错误。CI 目前不运行 clippy。棘轮必须只统计 `clippy::disallowed_macros`，
  并用独立的 clippy 步骤，不能依赖“clippy 整体通过”。

## 未验证项

- 检查在 CI 上的真实耗时（本地冷缓存两轮约 2 分钟）需要一次真实 PR 运行确认。
- Windows 与 Android 专属代码里迁移过的调用点（例如 `hidden_path.rs` 的两处字面量消息）没有在对应平台上编译。
- `#[instrument]` 约 130 处不受 lint 影响，其 `fields(..)` 与 `err` 参数未纳入目录，需要单独方案。
