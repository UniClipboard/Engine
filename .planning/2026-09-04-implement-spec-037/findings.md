# Findings: Spec 037 Implementation

## Baseline

- HEAD `df9ab44b`; branch `main`。
- 规格 035 已完成 Space port decorator，不重建第二入口。
- 当前没有真实 OTel SDK/exporter；`uc_otlp` 只是 tracing event。
- Apple/Android 有 OSLog/Logcat + 文本文件；Harmony 无对等安装。
- 当前 dirty worktree 有 037 原型和无关 profile upgrade 修改。

## Invariants

- Core 不感知 OTel；Application 不手工持续计时；Infra 只做具体实现和认证后传播；Engine 只装饰完整能力；Host 拥有 provider/sinks。
- 不为观测新增业务步骤查询、状态接口或持久格式解析。
- 所有业务失败 source chain 保留，但日志/remote records 不写原始正文。
- 远程失败永不改变业务；第一版不持久化 telemetry queue。

## External Decisions

- 本地 Jaeger；生产优先 PostHog。
- 宿主拥有 remote diagnostics permission。
- 本地日志保留 7 天、十进制 100 MB。

## Research Sources

- OpenTelemetry Rust official repository and OTLP examples.
- W3C Trace Context via official OpenTelemetry propagator.
- Spec 037 and completed Spec 035.

## Open External State

- 未发现可用 PostHog 生产凭据；实现可完成 Collector contract 和本地 Jaeger，真实 PostHog project 验收需检查环境/用户已有配置。
