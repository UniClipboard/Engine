---
name: uni-runtime-resilience-audit
description: "独立审查 UniClipboardEngine 的异步任务生命周期、取消、超时、背压、关闭、并发竞态与过载行为。用于定时运行期可靠性审查，或 spawn、channel、锁、重试和生命周期代码变更审查；不用于实现修复，也不调用其他 skill。"
---

# Uni 运行期韧性审查

## 目的

验证后台任务和并发资源有明确 owner，系统在取消、关闭、过载、panic 和重启场景下行为可预测。仅看到 `spawn`、channel 或锁不构成 finding。

## 独立运行约束

- 一个 session 只运行本 skill，不调用或模拟其他审查 skill。
- 默认只读；除调用方明确指定的报告输出路径外，不修改仓库、Issue 或 PR。
- 开始前完整读取共享的 [finding contract](../uni-audit-report/references/finding-contract.md)。
- 只报告运行期并发和生命周期问题；其他维度写入 `handoff`。

## 事实来源

先读取根目录和目标目录最近的 `AGENTS.md`，再读取：

- `docs/RELIABILITY.md`
- `docs/design-docs/core-beliefs.md`
- `docs/design-docs/observability.md`
- `docs/design-docs/error-handling.md`
- `ARCHITECTURE.md`
- `docs/PLANS.md`，仅用于识别已声明技术债

## 工作流

1. 按共享契约确定范围并记录基线。
2. 建立 task、`JoinHandle`、channel、锁、timer、取消令牌和 shutdown signal 的 owner/lifetime 图。
3. 对候选任务追踪启动、正常完成、失败、取消、关闭和重启六种路径；不能只看创建点。
4. 检查：
   - task handle 是否被持有、等待、取消或由明确 supervisor 接管；
   - task panic、join failure 和提前退出是否对 owner 可见；
   - shutdown 是否向下传播，阻塞操作和外部 I/O 是否有退出边界；
   - channel 是否有容量与背压策略，关闭和 send/recv 失败是否被正确解释；
   - timeout 是否表达领域截止时间，超时后底层操作是否仍泄漏或继续副作用；
   - retry 是否可能形成风暴，是否有预算、退避、抖动和单一责任方；
   - 锁是否跨 `await` 持有，锁顺序、回调重入和状态检查是否可能产生死锁或竞态；
   - 并发写入、重复消息和重放是否满足幂等或冲突规则；
   - 平台回调、线程亲和性与 runtime 边界是否明确。
5. 通过测试、明确状态机或可复现时序证明风险。理论上可能但无可达路径的情况标为 `suspected`，不得提升为确认问题。
6. 已有明确计划和 owner 的同一问题标为 `existing_declared_debt`。
7. 按共享契约输出本 lane JSON；未指定路径时输出等价 Markdown。

## 建议检索

使用 `rg` 搜索 `spawn`、`spawn_blocking`、`JoinHandle`、`select!`、`timeout`、`CancellationToken`、`mpsc`、`watch`、`broadcast`、`Mutex`、`RwLock`、循环与 retry。始终继续阅读 owner 和 shutdown 路径。

## 完成条件

- 关键后台任务均能回答谁启动、谁停止、谁观察失败。
- 每条 finding 包含可达时序和用户或系统影响。
- 没有把异步原语的存在或 channel 容量大小单独当作缺陷。
- 覆盖范围和未执行的压力/设备验证如实标为 `skipped`。
