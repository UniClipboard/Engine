# Findings: Observability Runtime Cleanup

## Requirements

- 用户要求直接整理并优化 `crates/uc-observability-runtime`。
- 保持最简单实现，不做预防性抽象。
- 修改必须测试先行，并更新架构维护记录。
- 保留当前工作区所有无关和并行改动。

## Confirmed Findings

- `runtime.rs` 同时承担全局安装、provider 构造、输出 layer、平台选择、过滤、health、flush/shutdown 和测试捕获。
- 本地日志的文件/容量 owner 在 `local_file.rs`，队列、guard、flush 与 health 却在 `runtime.rs`，完整责任被拆开。
- 当前依赖的非阻塞 writer 的 `flush()` 是空操作；现有 `force_flush`/`shutdown` 不能保证本地队列排空。
- 当前 dropped 计数只覆盖 100 MB 上限丢弃，不覆盖非阻塞队列满导致的丢弃。
- `health` 的 remote/local 状态主要是启动快照，不代表持续可用性。
- deadline helper 超时后留下后台线程继续执行，返回超时不等于操作停止。
- `runtime.rs` 直接知道旧的 admission/membership/storage target；这些是迁移删除项，不应配置化。
- 当前工作区存在与本任务重叠的并行改动，必须逐段合并而非覆盖。
- 并行改动已用自有异步文件 worker 取代 `tracing-appender`，并新增显式 Flush/Shutdown 消息与队列丢弃统计；本任务应在此基础上继续，不能恢复旧依赖。
- 当前 `LocalFileWorker::flush`/`shutdown` 可以在内部等待队列，但公开调用始终由外层 deadline 限制；结合规格中的“后台尽力 flush、直接终止允许丢失”，这里不新增取消机制。
- shutdown 是进程最终退出的一次性动作，超时后仍按“已开始关闭”处理；这符合现有接口的不可复活约束，不把它改造成可重试关闭。
- 并行改动仍把 `LocalFileWorker`、writer、drop counter 分别交给 `runtime.rs` 组装，本地日志的完整 owner 尚未形成。
- 完整测试中的隐私哨兵证明远程层原先只按 target 放行，带未批准字段的同 target 事件仍会发送；远程 owner 需要在运行时再次校验字段集合。
- 严格审查发现文件控制消息使用阻塞发送；当队列已满且 writer 卡住时，外层虽然按时返回，后台 lifecycle 线程会永久滞留，多次 flush 可累积线程。

## Intended Ownership

- Process runtime: 全局安装、同配置复用、句柄与最终生命周期。
- Telemetry pipeline: resource、远程 providers、远程发送结果。
- Local log runtime: 文件、队列、容量、丢弃计数、flush、shutdown。
- Output assembly: 系统输出、过滤和 layer 组合。
- Test support: 仅测试捕获，不留在生产 `runtime.rs`。

## Verification Baseline

- 诊断前同一 HEAD 下 crate 测试 6 项通过。
- observability privacy checker 通过，盘点 1299 个调用点。
- 当前并行 Cargo 改动导致后续 `--locked` 重跑暂时被 lock mismatch 阻断。
