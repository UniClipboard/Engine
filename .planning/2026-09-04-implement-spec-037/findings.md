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

## Final Findings

- OpenTelemetry 0.32 的官方 batch processor 不公开运行中队列丢弃和后台发送失败。最终保留官方 processor，并在外层用同容量原子门和 exporter wrapper 提供精确计数，不复制批处理线程或重试实现。
- 原始 OTLP 默认会携带源码位置、线程、busy/idle 和源码行事件名；设备侧现已关闭或稳定化这些元数据，并在 processor 边界再次检查完整字段集合和空正文。
- Space 建链 span 曾被 Iroh 派生任务延长到 2.4-3.4 秒，而完成事件显示真实调用只有 76/2/3/3 毫秒。建链调用不再把派生连接任务放入 span；认证消息往返由 Infra 的通用 `network_transport` 记录，未向 Engine 泄露业务步骤。
- 最终一秒诊断的端到端耗时为 5.338917 秒，传输外壳粗估为 0.405162 秒，相减值为 4.933755 秒。该外壳既包含部分本机工作又漏掉初始网络等待，不能作为本机耗时上界或下界。代码审计识别出准入加密状态的重复读写、维护重载和会话切换等候选，实际占比与顺序由规格 038 先测量再决定。
- 生产 Collector 不能沿用本地全采样：最终策略是设备保持完整父子关系，Collector 全量保留错误 trace，其余固定保留 10%，日志不采样。
- 相同 release 设置下，UniFFI 动态库相对 037 前增加 204,928 bytes（1.2088%）；三轮双设备进程峰值中位数显示本地记录与健康远程相对无观测分别增加 3,145,728 bytes（1.0095%）和 2,048,000 bytes（0.6572%）。
- 认证前没有可信父关系，失败只发送一条无 TraceId/SpanId 的真实耗时日志；认证成功后的四棵三层树逐节点恰有一条完成日志。
- 新旧布局提示在首次交换可稳定拒绝，后续状态只保存阻塞并公开 Pending/Active 提示；提示出现、清除或明确拒绝后只发一次通用刷新，对端上线立即重放，旧 V1 待交换记录严格兼容且不接受尾随或其他截断状态。
- 最终独立审查补出三处运行期边界：HarmonyOS 宿主必须使用固定发布渠道值；SDK 已明确报告的超时必须继续公开为超时；可重试的准入传输延期不能记成失败。三项均以真实宿主 smoke 或先红后绿定向测试修正。
