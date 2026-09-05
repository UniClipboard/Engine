# Task Plan: Implement Spec 037

## Goal

完成规格 037 的真实 tracing/log/OTLP 架构、跨设备传播、平台输出、clean cutover、验收与文档收口，并提交经过验证的原子变更。

## Next Step

无。037 已完成；一秒性能目标由 038 等待用户根据本次验证结果决定是否实施。

## Current Phase

Completed

## Phases

### Slice 0: Baseline truth and privacy gate

- [x] 记录 dirty worktree 归属
- [x] 删除/重写 037 原型，保留无关用户改动
- [x] 建立隐私红测并隔离旧记录输出
- [x] 校准移动日志计划与 inventory
- [x] 取得绿色基线
- **Status:** complete

### Slice 1: Single-process OTLP tracer bullet

- [x] 冻结 diagnostics schema 与 crate 版本组
- [x] TDD 实现进程级 runtime、trace/log bridge、JSONL、flush/shutdown
- [x] 用 storage upgrade 贯通真实 OTLP fixture
- [x] 本地 Collector + Jaeger 验收
- **Status:** complete

### Slice 2: Clipboard cross-device tracer bullet

- [x] TDD 实现认证后 W3C context propagation
- [x] Engine 负责完整 capability tracing
- [x] 删除 Core/Application timing 泄露
- [x] 双 endpoint trace/log 验收
- **Status:** complete

### Slice 3: Parallel migration

- [x] 冻结 schema/Cargo/文件 allowlist
- [x] Agent A 完成 Space admission/membership
- [x] Agent B 完成 Apple/Android/Harmony outputs
- [x] 主线合并后统一验证
- **Status:** complete

### Slice 4: Clean cutover

- [x] 删除 uc_otlp/stages/TraceMetadata/旧 flow/timing
- [x] 删除 Engine telemetry setting，保留 analytics setting
- [x] TaskRegistry 返回纯关闭报告
- [x] 统一 JSONL retention/export
- **Status:** complete

### Slice 5: Final verification and documentation

- [x] 强化架构门禁与 CI
- [x] 性能、过载、生命周期、隐私和平台矩阵
- [x] PostHog 真实能力验证或记录外部凭据阻塞边界
- [x] 更新稳定文档并移动 037 到 completed
- [x] 严格代码审查、原子提交
- **Status:** complete

## Key Decisions

| Decision | Rationale |
| --- | --- |
| 本地 Collector + Jaeger | 用户确认 |
| 生产优先 PostHog，客户端只接 Collector | 用户确认且保持供应商解耦 |
| remote diagnostics permission 只归宿主 | 用户确认，删除 Engine 同名开关 |
| JSONL 7 天、100,000,000 bytes | 用户确认 |
| TDD + tracer bullet | 仓库与技能硬约束 |
| Slice 3 才并行写代码 | 规格要求前置 schema/传播冻结 |

## Dirty Worktree Boundary

- `crates/uc-infra/src/security/profile_storage_upgrade/target.rs`：用户/其他工作，禁止回退。
- 037 研究文档、代码和索引：本任务保留并持续更新。
- admission/membership/sync_engine/session supervisor/Cargo 的未提交关联代码：037 原型，Slice 0 分类后删除或重写。
- `AGENTS.md` 的步骤泄露硬约束属于先前独立修改，本提交不纳入。
- 两套 Cargo 构建目录维护 planning、对应检查脚本，以及主检查脚本中的调用属于独立维护，本提交不纳入。
- 架构圣经中的 Cargo 构建目录与 Alpha 旧资料兼容记录属于独立维护，本提交不纳入。

## Errors Encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
| 仓库 `target` 初期是损坏链接 | 1 | 初期证据使用隔离目录；后续由独立维护改为外置共享目录，最终检查统一复用仓库 `target` |
