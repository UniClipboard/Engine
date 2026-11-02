# 本地进程采集与 OTLP 导出

## 范围与完整负责人

`uc-observability-runtime` 拥有进程采样、有界内存记录和 OTLP 序列化。
Desktop daemon 启动采样，并在用户导出时取得快照。
Desktop 负责 ZIP 组装。导出的 OTLP JSONL 可直接读取，导出不触发上传。

采样覆盖 daemon 和通过当前资料本地认证登记的 GUI、Quick Panel。
登记在内存中绑定 PID、内核创建标识和可执行文件路径。
macOS 使用微秒启动时间，Linux 使用内核启动时钟 tick，Windows 使用创建 FILETIME。
无法确认的 WebView 归属保持 unknown。进程覆盖不等于全应用独占资源占用。

## 数据与生命周期

指标、经过隐私过滤的日志和已完成 span 使用 OTLP 类型。
`opentelemetry-proto 0.32.0` 负责 JSON 编码。三类信号分别导出 JSONL。
CPU 累计值和单逻辑核心使用率使用 `uc.process.cpu.*`；内存使用 `process.memory.usage`。
首样本与调度缺口省略 CPU 使用率。
顶层关联字段仅使用明确白名单，保留原始作用域、trace/span 和时间戳。

内存记录最多保留 30 分钟、8 MiB。程序退出后丢弃，重启从空记录开始。
本功能不写诊断历史文件，不接入资料加密、不使用 AAD、不加载旧密文，也不执行隔离或恢复。
已有普通日志和独立网络日志的行为不变。旧诊断密文不自动读取、导出或删除。
快照在采样线程请求最新样本，等待上限为 1 秒；超时如实报告。

## 验收与已移除路径

先在 Desktop 现有 `diagnostics_acceptance_run.py` 中定义 `--memory-only` 场景，再移除实现。
失败场景包括：自动生成历史文件、ZIP 附带密文、丢失可读指标、重启加载旧记录、离线导出误用旧密文及测试进程未退出。
使用隔离资料验证实时导出、文件校验和、重启边界及 Rust/Go 两个离线归档入口，保留 JSON 回执与二进制哈希。
旧加密存储、资料密钥回调、密文错误分类和损坏恢复路径全部删除，不保留并行实现。

远端 CI、Windows/Linux 原生、真实休眠、同 PID 复用及采集开销仍按实际执行结果分别报告。
