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
- Slice 0 初始生成 1,321 个生产调用点的五类 inventory；移动日志旧计划按已完成/由 037 取代归档。最终数量以后续重生成结果为准。

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

### Slice 3

- **Status:** complete
- 两个独立 Agent 分别完成 Space 配对/成员链路和 Apple/Android/HarmonyOS 宿主接入，主线按提交汇合。
- 并行基线提交 `539671ab`、Space 汇合提交 `7e9514b8`、平台汇合提交 `6663080d` 的 schema blob 均为
  `478210ce2ff204212ff80022f649b3e2f79a0a95`，Cargo.lock blob 均为 `c103886bc647f823973aaba556a7970c62dbe17a`。
- 配对四次认证往返使用同一匿名 flow；每次 client/server 同 TraceId 且 parent 正确；旧成员在三设备测试中最终收到新成员更新并可向其发送内容。
- Apple、Android 和 HarmonyOS 在进程启动、暂停、恢复、单 Engine 关闭与最终进程关闭上使用同一生命周期合同。

### Slice 4

- **Status:** complete
- 删除 Engine 远程诊断设置、Core tracing、伪 OTLP、旧 timing/flow 与跨层后台任务 helper；产品分析开关与事件保持不变。
- TaskRegistry 只返回无名称和错误正文的关闭计数，由 Engine runtime owner 记录完整结果。
- JSONL 改为显式有界队列和真实 flush/shutdown；文件名合同唯一，诊断导出前先刷新。
- 远程字段在设备编码前精确收窄，日志正文固定为空，位置/线程/忙闲元数据关闭；Collector 再次收窄并使用稳定事件名。

### Slice 5

- **Status:** complete
- 补齐远程排队丢弃、发送失败、本地丢弃健康计数，以及不可达、拒绝、TLS、慢响应、并发刷新和关闭测试。
- Android 初始化系统证书校验器，AAR 按 Cargo metadata 打包维护库的 Java 组件和混淆规则。
- 本地双设备真实运行后，Jaeger 显示 17 条 trace、27 个 span；同一匿名 flow 下有四棵三层调用树和四条连接建立记录，三层均成功，搜索页与调用树页面均已截图检查。
- 一秒诊断最终报告端到端 `5.338917s`、传输外壳粗估 `0.405162s`、相减值 `4.933755s`；该相减值不是本机耗时上下界，1 秒目标仍如实失败。
- 当前环境没有 PostHog 项目凭据；生产模板静态校验通过，真实项目投递明确记为外部跳过。
- 生产 Collector 全量保留错误 trace、其余保留 10%，本地 Jaeger 继续全量；Collector 0.160.0 实际验证两份配置通过。
- iOS 模拟器应用完成构建、安装和完整生命周期；Android AAR、HarmonyOS HAP/HAR、macOS、Linux 与 Windows 构建通过。实体设备和真实 PostHog 项目记为跳过。
- 最终审查要求把跨重启关联号完全移出 Engine：Application 完整准入 owner 改为开启不透明作用域，Infra 只记录真实网络边界，
  Engine 只装饰完整认证 endpoint；同时删除成员账本与分支恢复子步骤观测。最终全量回归、平台构建和架构门禁均已通过。
- 独立实现与文档复审均无剩余阻断；复审发现的 HarmonyOS 启动参数、超时分类和可重试延期分类已修正并复验。

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
| 3 | 双设备公开配对 + 三设备旧成员更新 | 通过；旧成员最终可向新成员发送 |
| 4 | runtime failure/lifecycle/privacy tests | 通过；含不可达、401、TLS、慢响应、队列满和并发关闭 |
| 5 | 本地 Collector + Jaeger 配对调用树与页面截图 | 17 traces / 27 spans；同一 flow 下 4 棵成功三层树与 4 条连接记录；逐节点一条完成日志 |
| 5 | 指定的一秒诊断测试 | 如实失败：端到端 5.338917s；外壳相减 4.933755s 只作粗估，不记为通过 |
| 5 | 全量回归 | contract 49、runtime 38、Core 360、Application 750、Infra 896、Engine 225、UniFFI 51、OHOS 44，均无失败；Infra 7 项明确跳过 |
| 5 | 平台交付 | iOS 模拟器完整生命周期、Android AAR、HarmonyOS HAP/HAR、HarmonyOS N-API 正常/失败宿主 smoke、Linux/Windows 目标均通过 |

## Performance Samples

- 方法：四种模式各丢弃 1 次预热，再轮换起始顺序交错运行 5 次；每次均完成真实双设备配对。
- 无观测（微秒）：`5227797, 5330650, 5314320, 5744097, 5551913`。
- 本地记录（微秒）：`5462975, 5525091, 5456379, 5356076, 5833397`。
- 健康远程（微秒）：`5166730, 5543259, 5601388, 5233657, 5516288`。
- 不可达远程（微秒）：`5599411, 5093715, 5429277, 5442773, 4966211`。
- p50 分别为 5.330650、5.462975、5.516288、5.429277 秒；五样本 p95 分别为 5.744097、5.833397、5.601388、5.599411 秒；后三组相对基线为 +1.5546%、-2.4844%、-2.5189%。
- 三轮最大常驻内存（bytes）：无观测 `311181312, 311623680, 312885248`；本地记录 `320880640, 312934400, 314769408`；健康远程 `316932096, 308248576, 313671680`。中位数分别为 311,623,680、314,769,408、313,671,680；三样本只作诊断。

## Error Log

| Error | Resolution |
| --- | --- |
| `target` 损坏链接 | 使用独立 CARGO_TARGET_DIR |
| 建链 span 被 Iroh 派生任务延长 | 建链调用不再把底层连接任务放入 span；认证往返由 Infra 的通用 transport span 记录 |
| 移动 binding 测试仍使用 HTTP 地址 | 生产配置只接受 HTTPS；本地 runtime fixture 单独使用 loopback 构造 |
