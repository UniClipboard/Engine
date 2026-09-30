# ADR-030：日志编写改为类型化事件并由工具链强制

- **状态**：已采纳（2026-09-29；模块日志通道已提交，见“先后关系”；开放问题的裁决见实施计划）
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

分三步，每步独立可回退，顺序不可调换。

1. **收口写入口，并用编译工具链禁止绕过。**
   - 提供唯一的日志封装宏（位于既有观测 crate，不新增层）。字符串类字段的值类型必须实现密封 trait `LogSafe`
     （固定词表枚举、`&'static str` 字面量、应用生成的随机标识、`Sensitive<T>`）；裸 `String`、`&str` 变量、路径、地址无法通过编译。
   - workspace clippy 启用 `disallowed_macros`，禁止业务 crate 直接使用 `tracing::{trace,debug,info,warn,error}!`。
     封装宏所在 crate 显式豁免。工具链固定为 1.95.0，该 lint 可用。
   - 存量调用点用基线计数棘轮迁移：CI 比较 clippy 报告的违规数与已提交基线，只许减少不许增加；
     被修改的文件必须清零。不做一次性全量迁移。
2. **事件与字段词表单点声明并生成。**
   - `DiagnosticTaskKind`、日志字段名与允许取值、`error_kind` 词表用一份声明生成枚举、`as_str`、白名单与 schema 快照。
   - 新增变体只改声明；生成结果由快照测试守护，声明与生成物不一致时测试失败。
   - 字段名不在词表内时封装宏在编译期报错，取代运行期 `<omitted>`。
3. **错误改为固定分类，取代逐层 downcast 登记。**
   - 仓库自有错误类型实现分类 trait（返回固定 `ErrorKind` 枚举）。日志边界取分类与 `io_error_kind`，
     不再依赖 `log_safe_errors!` 逐层渲染；第三方与 anyhow 层继续保守渲染为 `<opaque>`，但不再是仓库自有错误的常态。
   - 迁移随触碰的错误类型渐进进行，`log_safe_errors!` 在覆盖完成前保留，之后删除，不长期并存两套入口。

## 先后关系

- 本 ADR 与 `2026-09-29-module-log-channel` 直接冲突：该任务正在改写登记宏、白名单与模块日志层，工作区含大量未提交改动。
  必须先提交并冻结那个任务的 P0，再开始第 1 步。
- `2026-09-29-observability-gap-fixes` 的 P-A（新增 `DiagnosticTaskKind`、登记类型）可在本 ADR 采纳前先行，
  但每个新增项要按第 2、3 步的声明方式书写，避免生成后再返工。
- 补日志（55 条缺口）与本 ADR 互不替代：前者补缺失记录，后者保证以后新写的记录不再静默出错。

## 备选方案

- **维持现状，只加强文本检查。** 成本最低，但启发式检查无法区分字面量与插值，违规仍然静默，长期复杂度继续增长。
- **整体迁移到 OpenTelemetry Weaver 等外部注册表工具链。** 可借鉴其“声明式清单生成代码、文档与校验”的思路，
  但引入外部工具链与本仓的隐私词表、固定枚举模型不完全对应，先在仓内用最小生成器验证，再决定是否引入。
- **一次性全量迁移约 900 个调用点。** 与业务功能分支冲突面过大，且不符合“不长期保留新旧两套实现”之外的可交付节奏；
  用基线棘轮达成同样终态。
- **依赖 `Error::provide` 通用成员访问。** 据当前了解仍属不稳定特性，不作为基础。

## 影响

- 调用点写法变化，需更新 [运行期观测](../observability.md) 的“模块日志”一节与 `check-rust-style.mjs` 规则，
  并把 `log_safe_errors!` 相关文字标记为过渡。
- 生成器与封装宏成为观测合同的一部分，新增公共类型需保持 `uc-engine` 之外无新增公开面（不扩大 Engine facade）。
- 短期增加编译与维护成本，换取违规从运行期降级变为编译或 CI 失败。

## 试验结果（2026-09-29，未改仓库文件）

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
- 当前工作区 `cargo clippy` 并不干净：`uc-application` 有约 190 条其他 lint 与 1 个 deny 级错误
  （`async_yields_async`，`application/shutdown.rs:70`），`tests/hosts/uc-mobile-probe-core` 另有 2 个 `never_loop` 错误。CI 目前不运行 clippy。棘轮必须只统计 `clippy::disallowed_macros`，
  并用独立的 clippy 步骤，不能依赖“clippy 整体通过”。

## 未验证项

- `lan-compat` 等非默认特性下的违规数；上面的全量计数只覆盖默认特性。
- 棘轮基线的载体与 CI 耗时；需要新增一个只报告 `disallowed_macros` 的 clippy 步骤。
- 分类 trait 能否覆盖跨 crate 的 `#[source]` 链而不引入新的循环依赖。
- 封装宏对 span 字段（`#[instrument(fields(..))]`）的覆盖方式；`#[instrument]` 约 130 处不受 lint 影响，需要单独方案。
- 封装宏需要同时覆盖 `error = ..` 错误字段、`%`/`?` 格式化字段与可变字段个数；原型只验证了 `key = value` 形式。
- 迁移期间基线文件与 `RUST_STYLE_BASE_SHA` 比较起点的配合。
