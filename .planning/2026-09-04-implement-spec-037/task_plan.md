# Task Plan: Implement Spec 037

## Goal

完成规格 037 的真实 tracing/log/OTLP 架构、跨设备传播、平台输出、clean cutover、验收与文档收口，并提交经过验证的原子变更。

## Next Step

冻结 Slice 3 的 schema/Cargo/文件清单，启动 Space 与平台输出两个独立并行分片。

## Current Phase

Slice 3

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

- [ ] 冻结 schema/Cargo/文件 allowlist
- [ ] Agent A 完成 Space admission/membership
- [ ] Agent B 完成 Apple/Android/Harmony outputs
- [ ] 主线合并后统一验证
- **Status:** in_progress

### Slice 4: Clean cutover

- [ ] 删除 uc_otlp/stages/TraceMetadata/旧 flow/timing
- [ ] 删除 Engine telemetry setting，保留 analytics setting
- [ ] TaskRegistry 返回纯关闭报告
- [ ] 统一 JSONL retention/export
- **Status:** pending

### Slice 5: Final verification and documentation

- [ ] 强化架构门禁与 CI
- [ ] 性能、过载、生命周期、隐私和平台矩阵
- [ ] PostHog 真实能力验证或记录外部凭据阻塞边界
- [ ] 更新稳定文档并移动 037 到 completed
- [ ] 严格代码审查、原子提交
- **Status:** pending

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
- 037 研究文档与索引：本任务保留并持续更新。
- admission/membership/sync_engine/session supervisor/Cargo 的未提交关联代码：037 原型，Slice 0 分类后删除或重写。
- `AGENTS.md` 的步骤泄露硬约束：本任务保留。

## Errors Encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
| 仓库 `target` 是损坏链接 | 1 | 所有 Cargo 命令使用独立 `CARGO_TARGET_DIR` |
