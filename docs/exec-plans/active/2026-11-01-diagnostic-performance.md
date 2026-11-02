# 进程性能与 OTLP 离线诊断

## 完整负责人

`uc-observability-runtime` 拥有采样、有界历史与 OTLP 记录。
Desktop daemon 启动采样，并在用户导出时冻结记录。
Engine 组装层连接现有资料加密适配器。调用方不接触密钥。
采样覆盖 daemon 和通过当前资料本地认证登记的 GUI、Quick Panel。
登记绑定内核创建标识与内存中的可执行文件路径。
macOS 使用微秒启动时间，Linux 使用内核启动时钟 tick，Windows 使用创建 FILETIME。
这些标识只在内存中使用，不依赖 sysinfo 的秒级启动时间。
Wails 公开 API 未提供 WebView PID，无法确认归属时保留 unknown。
不能将这些进程的数据称为全应用独占资源占用。

## 数据与存储

指标、已批准日志和已完成 span 使用 OTLP 类型。
`opentelemetry-proto 0.32.0` 负责 JSON 编码。
三类信号分别导出 JSONL。每行是完整 OTLP 请求。
CPU 累计值为总 CPU 秒数。使用率为单逻辑核心比例。
两者使用 `uc.process.cpu.*`，避免伪造标准 CPU 模式或核心归一化口径。
内存指标使用 `process.memory.usage`，单位为字节。
首样本与调度缺口省略 CPU 使用率。

内存历史保留 30 分钟，最多 8 MiB。
加密历史保存至日志目录的 `otel-history/history.enc`。
持久化前使用现有资料内容加密适配器，绑定固定 AAD。
密钥不可用时保留内存历史，并报告失败状态。
重启后仅通过同一资料加密适配器读取历史。
持久历史的明文输入最多 512 KiB，密文文件最多 8 MiB。
旧进程保留原 Resource 与原始时间戳。

## 验证与剩余工作

已执行 `cargo check -p uc-engine --all-targets --locked`。
该检查只证明编译，不证明产品导出或后端导入。
Desktop 的验收记录负责真实 ZIP、Collector、后端查询与跨平台状态。
GUI、Quick Panel 的新增登记仍需在 Desktop 原生验收中核对。
WebView 只接受宿主明确拥有的 PID，不通过名称或父子关系扫描推断。

## 审查回归：关联白名单与历史恢复

修改前的失败场景：同一真实诊断 ZIP 的 OTLP 日志缺少原始顶层关联值；破坏 history.enc 后，重新启动仍无法持久化。

验收先写入 Desktop 的既有 diagnostics_acceptance_run.py：逐条匹配原日志和 OTLP，保留真实 trace/span 与 LocalRecordingState.run_id；用隔离合成资料验证损坏、历史密钥缺失、锁定、超限、权限错误和重启重载。不得新增实现后单元测试。

顶层关联属性只取明确白名单。密文打开错误在拥有 BlobCipherError/ContentProtectionError 类型信息的 assembly 边界分类并保留 source。缺失 V3 或旧版历史密钥不能解释为文件损坏。

确定不可读的历史在新历史加密成功后，保存到单个有界密文证据槽，再原子更新。锁定和可重试错误不修改文件。恢复原因、证据原大小与裁剪状态写入新的加密历史。单个证据槽最多 8 MiB，不新增明文文件或文件密钥。
