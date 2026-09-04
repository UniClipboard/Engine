# Progress: Spec 037 Implementation

## 2026-09-04

### Slice 0

- **Status:** complete
- 已读取实现、TDD、文件规划技能和规格 037。
- 已启动三个只读并行审计：diff 归属、官方 OTel API、敏感日志 inventory。
- 已建立实施完成标准、dirty worktree 边界和分片状态。
- 已恢复并审计未完成原型，保留 035 已验证装饰器和无关 profile upgrade 改动。
- Apple/Android 系统层和文件层改为只接受审核目标；真实文件哨兵证明普通模块的路径不会写入。
- 新增隐私门禁，自测覆盖注释、测试模块、各日志/span 宏、shorthand、原始错误和敏感字段。
- 生成 1,321 个生产调用点的五类 inventory；移动日志旧计划按已完成/由 037 取代归档。

### Slice 1

- **Status:** complete
- 新增进程级共同运行时，真实输出 traces/logs，共用同一进程资源信息并支持有界 flush/shutdown。
- 本地 JSONL 固定 7 天与 100,000,000 bytes，只管理严格命名的文件；目录失败降级。
- 可解码 HTTP fixture 证明 trace/log 各一条、关联号一致、事件不在 span 内重复。
- Docker Collector + Jaeger 真实运行两次，Jaeger API 与 Collector 输出显示同一 TraceId/SpanId。
- Rust 1.95 下 iOS、Android、HarmonyOS 三目标 runtime 编译通过。

### Slice 2

- **Status:** complete
- Clipboard 业务头、Core result 和 Application 输入已删除旧 flow/timing 字段与伪 OTLP 阶段记录。
- Infra 私有 W3C context 只在已认证连接后接受；消息前置标记使新旧布局双向明确不兼容。
- Engine 只装饰完整 dispatch port；接收 server span 由已认证 Infra endpoint owner 创建，没有向 Engine/Application/Core增加步骤接口。
- 两个真实本机 Iroh endpoint 证明 client/server 同 TraceId、正确 parent，接收日志关联 server SpanId；未知 peer 不建立 remote server span。

## Verification Log

| Slice | Command / Evidence | Result |
| --- | --- | --- |
| 0 | `cargo test -p uc-engine-uniffi file_log::tests --locked` | 2 passed，非零测试 |
| 0 | `cargo check --workspace --all-targets --locked` | 通过；仅既有 warning |
| 0 | `node scripts/architecture/check-engine-repository.mjs` | 通过；6 个 OpenMLS tests 与全部负向 fixture |
| 0 | privacy self-test / repository scan / metadata / fmt / diff | 通过 |
| 1 | `cargo test -p uc-observability-contract -p uc-observability-runtime --locked` | 通过；含真实 OTLP HTTP fixture |
| 1 | 本地 Collector 0.160.0 + Jaeger 2.20.0 | `uc-engine` trace 与关联 log 实际到达 |
| 1 | iOS / Android / OHOS target `cargo check` | 三项通过 |
| 2 | `cargo test -p uc-application clipboard:: --locked` | 399 passed，0 failed |
| 2 | Clipboard wire / receiver / Engine decorator focused tests | 10 + 6 + 1 passed |

## Error Log

| Error | Resolution |
| --- | --- |
| `target` 损坏链接 | 使用独立 CARGO_TARGET_DIR |
