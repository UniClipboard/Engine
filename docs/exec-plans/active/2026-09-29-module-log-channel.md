# 模块日志通道与错误链

状态：实施中（P0、P1 门禁、P2 部分、P3 已完成本地验证；开关已按用户决定删除）。来源：交接文档 `engine-observability-framework-handoff.md`
（SHA256 `2db2f0db9b2e622dfb1a17f518bf56fc56ea40897df3ab3128ff44206f07a76a`），基线 `c7a821b4`。

## 动作与责任

- **完整负责人**：`uc-observability-runtime` 拥有模块日志层、限额与导出计数；各业务 crate 只依赖 `tracing` 与
  `uc-observability-contract` 中的 `Sensitive` 及错误层登记。
- **调用方唯一动作**：用普通 `tracing` 宏记录；失败在返回或结算它的负责人处写
  `error = &e as &dyn std::error::Error`。
- **成功结果**：记录写入既有本地 JSONL（`source = "engine_module"`），随诊断导出一并带出。
- **失败结果**：限速、预算、裁剪与拒绝均计数并随 `LocalDiagnosticExportReport.module_logs` 给出；不承诺无限无损。
- **重启/重试责任**：无重试；进程重启后预算与限速状态重置，文件保留策略沿用既有 100 MB / 7 天。

## 已确认决定

| 项 | 决定 |
| --- | --- |
| A 未知外部错误层 | `<opaque>` 占位，不输出 `Display` |
| B 标准级别 | INFO 及以上；Detailed 采集窗口内 DEBUG |
| C 诊断导出 | 默认包含（与合同记录同文件） |
| D release 启用 | 原为审计前仅开发/测试构建；后经用户决定删除开关，所有构建启用 |
| E 类型识别 | 增量登记 + lint：spike 表明稳定版 `&dyn Error` 只能对已知类型 downcast，anyhow context 层不可识别 |

## 设计要点

- 独立 `Layer` 加入 `all_layers`，不改 `engine_layer` 的 `local_sink_enabled`，故不流向远程与系统日志层；
  `host_metadata_enabled` 的排除规则保持不变（共用 `is_engine_source`）。
- 关联：模块 span 创建时记下最近 OpenTelemetry 祖先的 `trace_id/span_id`，事件优先取当前 OTel 上下文。
- 位置：crate 相对 `file:line`，绝对路径丢弃；`spans` 是 span 名路径，`error.chain` 是 `Error::source` 链；
  三者都不是 backtrace。
- 不新增 `LocalDiagnosticSource` 变体（该枚举被 uniffi 绑定穷尽匹配），模块日志计数作为导出报告的附加字段。
- 错误层渲染：`io::Error` 与 `serde_json::Error` 内置结构化提取；仓库类型由所属 crate 用 `log_safe_errors!`
  登记；其余（含 anyhow context）记 `<opaque>` 并累计 `opaque_error_layers`。
- 不登记 `sponsor` 专用合同通道；固定拒绝原因以普通错误类型的 `#[source]` 表达（`AdmissionRefusal`）。

## 阶段

- P0 通道与错误链（含 `sponsor_state_load` 端到端）。
- P1 审计计数、包装敏感值、lint；完成后再决定 release 启用。
- P2 移除 `LocalCompletionDetail` 本地链字段与 `error.call_path` 重复；迁移 `instrument(err)`。
- P3 多 zip 时间线脚本；改写 `observability.md` 与 `error-handling.md`；关闭本计划。

## 进展与遗留

- P0：模块日志层、`Sensitive`、`log_safe_errors!`、独立预算与限速、`sponsor_state_load` 固定拒绝原因（`AdmissionRefusal`）已完成，
  端到端测试经 nextest 通过（日志文件 → 诊断导出 → 完整链，无路径与原始标识）。
- P1：字段与消息正文审计已完成（存量）：全部 886 处日志宏、338 个字段名逐名归类，落在
  `crates/uc-observability-runtime/src/module_log_fields.rs`（`ALLOWED_TEXT_FIELDS` 与 `REVIEWED_OMITTED_FIELDS`）；
  自由文本字段默认省略为 `<omitted>`；5 处消息正文内插已改为字面量 + 字段（其中一处原先把 diesel 错误正文写进消息）；
  `check-rust-style.mjs` 对新增行强制：字段名已归类、消息为字面量、`#[instrument]` 带 `skip_all`/`fields(..)`、
  `#[error]` 不内插自由文本；`check-module-log-errors.mjs` 拒绝已登记类型内插自由文本（该检查与登记机制已随 ADR-030 第 3 步于 2026-09-30 删除）。
  45 个内插自由文本的 `#[error]` 类型未登记，链上只出 `<opaque>`。用户已决定删除发布构建开关，所有构建启用（2026-09-29，用户原话：“删除这个开关， 都启用”）。
- P2：12 处 `instrument(err)` 已迁移为 `warn_on_error!`（完整链）。按用户决定删除 `LocalCompletionDetail` 的本地
  `error.chain`/`error.call_path`（`source_chain()` 及其 `error.call_path` 重复）；`error.phase`/`error.reason` 保留。
  `address_record` 的 `error.chain`（地址读取诊断的另一条固定 token 链）不属于 `LocalCompletionDetail`，未动，作为遗留项记录。
- P3：`scripts/diagnostics/merge-diagnostic-timeline.mjs` 已完成（时钟偏移需显式给出，不做自动估计）；
  `observability.md` 与 `error-handling.md` 已改写。

## 验收

- 日志文件 → 诊断导出的端到端测试：
  - `crates/uc-infra/tests/space_admission_state/sponsor.rs`（固定拒绝原因的完整链、与合同记录同 `trace_id`）；
  - `crates/uc-observability-runtime/tests/module_log_channel.rs`（预先列出 8 种失败方式：错误链逐层渲染与 opaque/io/serde、
    自由文本字段默认省略、Detailed 窗口 DEBUG 门、超长裁剪、限速与 `suppressed`、字节预算耗尽、导出计数与文件行数一致、
    无绝对路径）。该场景发现并修复了 `#[source] Box<T>` 层无法识别的缺陷。
- 读取层与上层重复记录的所有权：12 处 store 读取层的 `warn_on_error!` 是这些失败唯一带类型化 source 链的记录；
  Application 处理器用 `?` 原样返回，Engine 装饰器只写合同记录（固定分类），二者不写链，因此不重复模块日志。
- 未执行的平台或真机项一律记为“跳过”。
