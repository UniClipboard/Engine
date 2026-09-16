# Task Plan: UniClipboardEngine 可观测性架构规格

## Goal

基于当前 Core/Application/Infra/Engine/Bindings 分层和官方 OpenTelemetry 规范，产出一份可直接实施、可分片验证、明确多 Agent 并行边界的 clean-cutover 执行计划。

## Next Step

已完成，交付规格与验证结果。

## Current Phase

Phase 5

## Phases

### Phase 1: Requirements & Discovery

- [x] 固定用户目标和分层硬约束
- [x] 审计现有 tracing、日志、OTLP 与 analytics 链路
- [x] 核对官方 OpenTelemetry/Rust 能力
- [x] 审计既有计划重叠与当前未提交改动
- **Status:** complete

### Phase 2: Architecture Design

- [x] 定义各层唯一责任与接口
- [x] 定义 trace/log/resource/privacy schema
- [x] 定义跨设备、重试和重启关联模型
- [x] 定义 clean-cutover 删除清单
- **Status:** complete

### Phase 3: Execution Slicing

- [x] 设计最小端到端分片
- [x] 标注可多 Agent 并行分片及文件所有权
- [x] 固定每片入口、出口、依赖和验收门
- **Status:** complete

### Phase 4: Spec Authoring

- [x] 写入 docs/exec-plans/active 正式规格
- [x] 更新 active index 与 architecture bible 维护记录；PLANS 为总入口无需逐项登记
- **Status:** complete

### Phase 5: Verification

- [x] 核对 11 节规格结构和路径
- [x] 运行文档最低交付检查
- [x] 复核 dirty worktree 边界
- **Status:** complete

## Key Questions

1. Application 如何提供业务含义而不把步骤控制泄露给 Engine？
2. 谁安装真实 OTLP exporter，谁拥有 flush/backpressure/shutdown？
3. 跨设备在线 trace 与跨重启 durable flow 如何分开表达？
4. 哪些分片可并行且不会同时改同一接口或装配文件？

## Decisions Made

| Decision | Rationale |
| --- | --- |
| 本轮只写规格，不实施生产代码 | 用户明确要求研究后撰写执行计划 |
| 不复用或覆盖 `.planning/.active_plan` | 现有指针属于其他计划，避免破坏并行工作 |
| 最终方案必须 clean cutover | 仓库不接受长期新旧两套观测入口 |
| 远程诊断开关由宿主启动配置唯一拥有 | 宿主拥有用户许可和 exporter；删除未接线 Engine 字段比增加跨层控制 port 更清晰 |
| 单机样例使用 profile storage upgrade | 无网络变量，可先证明真实 trace、log、OTLP 和生命周期 |
| 第一条跨设备样例使用 Clipboard | 已有 flow/wire/timing，可同时删除当前 Application/Core 观测泄露 |
| Slice 3A 与 3B 可双 Agent 并行 | 前置契约已冻结，且配对与平台输出具有不重叠文件所有权 |

## Errors Encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
| `docs/exec-plans/active/037-*` glob 无匹配导致 zsh 报错 | 1 | 改用 `rg --files` 和显式文件读取，不重复使用裸 glob |
| findings 占位上下文不存在 | 1 | 读取实际文件后按真实段落追加 |
