# macOS Engine 基准测试框架

状态：CPU 归一化修正与仓库验证已完成；首次完整的 macOS 无头参考套件已完成并验证。参考批准与 CI 门禁仍需人工进行且尚未建立。负责人：基准测试控制器。Issue：#374。

调用方只运行一条套件命令。成功必须满足真实双进程剪贴板同步、重启后持久内容校验、完整 OS 采样以及清理成功。失败会保留在不可变产物中。控制器负责截止时间与清理；Engine 负责业务重试与持久恢复。运行不会静默重试失败的样本。

复用现有 connectivity host、JSON 管道协议、本地 rendezvous 夹具和 uc-testkit。保留 release profile 与共享构建缓存。首个切片是新启动和一次真实文本传输，随后是超时与崩溃清理验收。仅在该切片通过后，才扩展到四个场景和每个 revision 六次运行。

实现前的失败清单：spawn 失败、提前退出、格式错误/超大响应、管道阻塞、截止时间耗尽、内容错误、历史缺失/重复、重启丢失、采集器退出/API 失败、控制器中断、产物 I/O 失败、资源清理失败、不兼容的二进制来源、竞争基准进程，以及缺少平台支持。

验收：进程级 E2E 用例产生 JSON 证据；实现后不新增单元测试。必须运行必需的仓库检查与 connectivity 回归检查。本阶段仅接受 macOS。Linux、Windows、设备与 CI 调度仍标记为跳过。

## 已验证的设计修正

首次真实文本 E2E 证明 `CaptureCurrentClipboard` 会持久化快照，但仅凭它不会进入自动出站监听流程。基准适配器现在发出既有的公开 `HostClipboardChangeStream` 通知。它只保留一条待处理通知，且不改变生产 API。失败的运行保留在本地证据中。

## 实现与验收回执（2026-11-03）

稳定的 Engine 接口保持不变。`uc-benchmark` 负责监督、采样、正确性检查与比较。`uc-testkit` 提供共享 rendezvous 夹具、有界异步管道客户端、profile 租约与清理报告。现有同步 connectivity 测试保留其基于断言的进程驱动；引入有界异步所有者不要求重写这些测试。

宿主存储保护拒绝跨 worktree 复用 target。因此构建驱动要求为分离的 baseline worktree 及其独立 target 提供显式外部隔离根目录。构建保持串行并共享 MBX。二进制收集后，隔离 worktree 与 target 已移除；候选 target 与共享缓存保留。

固定的 Main 快照与候选 Engine 源码都使用 `aa1b409fda38053d2d1e61e7801f680e2261ea26`。生产 Engine 源码未改变。两者使用相同的适配测试宿主源码。构建来源区分被修改的整体工作树与未改变的 Engine 源码。baseline 清单最初的 dirty 字段歧义在测量前已修正，原始清单保留在本地构建证据中。

已执行的验收：

- 十个进程 E2E 用例通过：短文本、startup、disconnected idle、connected idle、200 条文本同步、timeout、host crash、collector crash、controller interruption，以及 supervisor kill 后的 stale-lease 回收。
- 六个产物/控制 E2E 用例通过：A/A 比较、样本被篡改、样本缺失、清理回执缺失、格式错误响应与超大响应。
- 最终的 baseline 与候选 release 可执行文件各自同步了 200 条文本，并在接收端重启后验证了持久化历史。
- startup ABBA 套件为每个可执行文件产生两次预热和六次测量验收运行。逐次运行的清单与环境一致性已验证。报告明确将这些运行标记为 acceptance-only。
- 现有 active-clipboard connectivity 回归：七项通过，一个既有旧宿主互操作用例被忽略。
- 必需的 metadata、workspace/all-target 检查、格式化、Rust 风格、仓库架构与 diff 检查通过。

现有 Rust 集成测试二进制的可选 release profile 调用在编译 `iroh-util` 时失败：rustc 报告同一批 `n0-error 1.0.1` 类型具有不同身份。Cargo 依赖树中只有一个版本。未清理任何缓存，也未修改生产依赖。默认 profile 的集成回归通过。这不影响单独构建的 release 宿主进程 E2E 结果。

本地证据位于 `target/benchmark-acceptance-02`、`target/benchmark-report-acceptance`、`target/benchmark-final-baseline-text`、`target/benchmark-final-candidate-text`、`target/benchmark-startup-repeatability-final` 与 `target/benchmark-binaries-03`。构建/检查日志保留在 `target/benchmark-*.log`。这些本地产物不提交。

## 剩余验收

该机器当时有活动的 Simulator 和并发构建。由于它们属于其他工作，未被停止。严格运行会拒绝此环境。首次受控参考 baseline、全场景重复性能比较、Linux/Windows/设备验收以及 CI 调度仍未执行。acceptance-only 数据不认证任何数值预算或优化声明。

要完成受控 baseline，请在没有 `--acceptance-only` 的情况下，于空闲的专用 macOS 主机上使用 README 中的构建与套件命令。保留完整的原始产物集合。不要重置已批准的参考，也不要自动提高预算。

## 受控运行启动（2026-11-03）

用户授权停止 Android 模拟器并启动 baseline。经确认的 Android 模拟器进程在 SIGTERM 后退出。没有已启动的 iOS 模拟器设备；仅剩一个空闲的 Simulator 应用窗口。

Preflight 现在检查已启动的 iOS 模拟器设备与 Android 模拟器进程。它不再将空闲的 Simulator 窗口视为活动设备。一项进程 E2E 检查证明，已启动的设备会在宿主启动前阻止准入。此变更后必需的仓库检查通过。

首次正式尝试在第一次预热前被拒绝，因为启动了一个无关的 Cargo 构建。其失败回执保留在 `target/benchmark-baseline-2026-11-03`。另一次尝试位于 `target/benchmark-baseline-2026-11-03-attempt-02`，它会等待 30 秒且不阻塞构建或活动模拟器设备，然后在没有 `--acceptance-only` 的情况下运行全部四个场景。每个场景为每个可执行文件包含一次预热和六次测量运行。驱动独立于交互会话记录完成状态。先前的失败被保留而不是替换。

启动运行不代表批准参考 baseline。在接受测量结果前，请阅读其完成回执、正确性、清理、环境与样本产物。

第二次正式尝试完成了四次 startup 采样，随后另一个外部 Cargo 构建导致下一次准入失败。没有替换任何失败样本。两次尝试均保持完整。当前后台驱动位于 `target/benchmark-baseline-2026-11-03-scheduled`；它要求 300 秒安静时间，并保留独立的完整套件尝试。它只重试发生在宿主启动前且未产生样本目录的环境性拒绝。真实工作负载失败会停止任务。环境等待上限为两小时，完整套件尝试次数上限为三次。读取 `state.json` 了解当前状态，读取 `completion.json` 了解最终结果。

## 受控运行结果（2026-11-03）

调度驱动用尽全部三次套件尝试，并于 Asia/Shanghai 03:10 退出。每次尝试都在较晚的环境性准入拒绝处停止，且被拒绝的调用尚未创建样本目录。不存在完整的四场景 baseline。

尝试 2 完成了 startup 与 disconnected idle：每个可执行文件和场景六次测量运行。全部 24 个测量运行产物通过清单、正确性与清理验证。connected idle 仅完成第一次预热；未到达文本同步。逐进程空闲 RSS 在各次运行间从约 29 MiB 变化到 221 MiB，因此这些部分观察既不能建立稳定的内存参考，也不能证明代码回归。两个 Engine 源码提交完全相同。

旁路部分报告保留在 `target/benchmark-baseline-2026-11-03-scheduled/partial-report.md` 与 `.json`。原始尝试与原始样本保持不变。没有基准进程仍在活动。在批准参考 baseline 前，仍需完整的主机预留与 RSS 变化调查。

## 空闲保留诊断与后续设计

在进一步实现前，重放诊断复现了现有空闲窗口内 192 MiB 的下降。一次实时 disconnected Engine 运行复现了相同症状。经脱敏的 `vmmap -summary` 证据显示 `Malloc Large (empty)` 中为 192 MiB，而实时分配堆仍约为 5.3 MiB。回收后，同一进程的 footprint 从约 205 MiB 降至 12 MiB。生产 KDF 默认使用 128 MiB 进行 KEK 派生，使用 64 MiB 用于 OPAQUE 准入。这表明 macOS 分配器暂时保留了已释放的初始化分配；不需要 Engine 优化或削弱 KDF。

控制器仍是完整所有者。将工作负载版本改为 2，并在空闲/文本测量前使用固定的 90 秒 post-setup 预热。保留每个初始化与预热样本，标记预热边界，并将完整进程峰值与稳定工作指标分开报告。实时重放诊断必须在固定的工作窗口内保持稳定，同时仍保留初始化峰值。这是测量方法修正，不是资源优化。

当环境 preflight 繁忙时，在正式运行之间增加有界等待。保留成功的不可变运行与 ABBA 顺序。没有运行目录且确认环境繁忙的拒绝调用属于准入延迟，单独记录。启动宿主或因其他原因失败的调用仍算失败运行，并停止套件。将聚合环境等待上限设为 1 小时。失败时检查点保存进度与部分摘要。

首次完整参考测量可以选择单个固定的 Engine 可执行文件，因为 baseline 与候选当前共享同一生产提交。现有的配对比较仍可用。reference-only 模式为每个场景执行一次预热加六次测量运行，且不虚构比较列。

实现前失败清单：不稳定的空闲窗口；初始化样本被丢弃；比较不同工作负载版本；环境在探测与启动之间变忙；环境等待耗尽；真实宿主失败被错误重试；后续拒绝后部分结果丢失；无效/被篡改的二进制来源；以及 reference-only 模式中误导性的比较输出。通过真实进程 E2E 与保留产物验证这些内容，不新增单元测试。

## 版本 2 验证与参考启动

实时 disconnected 诊断在固定 90 秒预热下通过。在 60 秒工作窗口内，端点 RSS 跨度分别为 65,536 字节与 49,152 字节。完整初始化样本仍可用。独立重放还验证了修正后的偶数样本中位数。

真实进程准入验收通过：环境等待保留已完成的样本，reference-only 模式保留六次测量 startup 运行，注入的 host crash 会以已完成的清理停止套件，而不是重试失败的工作负载。connected idle 与 200 条文本同步分别通过。文本接收端重启验证了全部持久化条目。

版本 2 变更后，必需的锁定 metadata、workspace/all-target 检查、格式化、Rust 风格、仓库架构与 diff 检查均通过。未改变任何生产 Engine 源码、KDF 参数、release profile 或共享缓存设置。

完整的 reference-only 套件正在 `target/benchmark-reference-v2-2026-11-03` 下运行。其配置固定了 Engine 提交、可执行文件哈希、工作负载版本、四个场景以及每个场景六次测量运行。运行器使用正式准入，而非 acceptance-only 模式。它保留进度，并在运行之间等待竞争工作负载。宿主未预留以阻止之后启动的无关工具；在批准参考前需要审查环境快照。没有参考被自动批准。

版本 2 验证证据：`target/benchmark-idle-retention-fixed`、`target/benchmark-idle-retention-fixed-verified`、`target/benchmark-v2-admission-final`、`target/benchmark-v2-connected` 与 `target/benchmark-v2-text`。检查日志为 `target/benchmark-v2-*.log`。

## CPU 单位修正

源码审查发现 `ri_user_time` 与 `ri_system_time` 保留 Mach 绝对时间单位。初始采集器将其和错误标记为纳秒。在此 Apple M4 主机上，`mach_timebase_info` 报告 125/3 比率。因此原始 CPU 摘要低估了实际消耗的 CPU 时间。内存样本与业务正确性证据不受影响。

实现修正前，失败清单为：非 1:1 timebase 导致静默低估；转换期间整数溢出；无效 timebase 信息；归一化期间丢失原始计数器；以及将旧的错误标记报告复用为归一化比较。通过独立子进程 `resource.getrusage` CPU 测量验证采集器。保留修正前的一次失败运行与修正后的一次通过运行。不要在正在进行的正式测量期间运行此 CPU 工作负载。

保持原始正式运行产物不可变。其原始 `ri_user_time` 与 `ri_system_time` 计数器允许使用记录的主机 timebase 进行确定性的修正分析。派生回执必须标明此修正，并忽略旧版 `cpu_time_ns` 字段。保留原始哈希与 timebase 证据。旧套件的 CPU 列无效；使用修正后的回执。未来的采集器必须显式存储 timebase 与归一化纳秒。

主要来源：Apple XNU 中 `osfmk/kern/bsd_kern.c` 的 `fill_task_rusage` 将 task power/recount 计数器直接赋给 rusage CPU 字段。`task_power_info` 保留 Mach 绝对时间单位。转换使用 `mach_timebase_info`。

## 续接检查点：部分测量与存储阻塞

版本 2 套件完成了六次测量 startup 运行、六次测量 disconnected idle 运行以及一次测量 connected idle 运行。包括三次预热在内的十六个总产物通过清单与清理验证。剩余的 connected idle 运行与文本套件尚未完成。运行器保留所有已完成的运行，并在工作负载之间等待；当出现其他构建时它不会重置套件。

修正后的部分回执为 `target/benchmark-reference-v2-2026-11-03/partial-verified-report.md`，其旁附有机器可读结果与 SHA-256 验证清单。部分快照不可变。两个端点的 startup 中位数约为 262–269 ms。disconnected idle 工作窗口 RSS 中位数约为 28.9 MiB；观察到的初始化峰值仍约为 221 MiB。某些工作窗口仍包含分配器页面回收。这些观察不批准稳定的资源门禁。

一次真实的一秒子进程 CPU 工作负载复现了旧采集器 97.6% 的低估。使用测得的 125/3 timebase 重放其保留的原始计数器，与独立的 `resource.getrusage` CPU 时钟在约 0.0072% 内一致。修正后的部分回执与最终分析使用这些原始计数器，而不是旧版错误标记字段。原始逐次运行产物与旧报告保持不变。新采集器记录转换比率与归一化纳秒；新比较会拒绝旧版 CPU 格式。

下一次 release 构建在格式化后受阻，当时本地存储保护尝试更新 `.uni-last-used`。原生堆栈样本定位到 `setattrlistat` 内部的 `os.utime`。在配置的 target 与外部卷根目录中分别执行的五秒写入探测也超时。读取成功；APFS 卷以读写方式挂载，可用空间约 228 GiB。未使用替代 target、缓存重置、强制卸载或终止无关进程。仅终止了本任务受阻的构建/检查进程。探测子进程已退出。随后确认两个生成的探测路径均不存在。

CPU 修正后，Rust 风格与 Python 语法检查通过，`git diff --check` 也通过。新的 release 编译、修正后采集器的实时验收、workspace/all-target 检查、format-check 完成以及仓库架构检查中的 OpenMLS 执行仍被外部存储阻塞。早先的版本 2 检查在此 CPU 修正前通过；不要将那些早期结果表述为对该修正的验证。

正式运行器仍位于 `target/benchmark-reference-v2-2026-11-03`。它会等待无关的 Cargo 构建完成。完成监视器仅在整个套件通过后运行修正分析并写入 `completion.json`。批准仍需人工进行。此本地套件测量无头 connectivity 宿主，而非 iOS、Android 或原生守护进程。

最终锁定 metadata 尝试也超过了 30 秒截止时间。仅终止了其私有进程组。部分清单在全部 25 个列出文件上重新验证。准入延迟期间没有基准宿主进程仍在活动。

## 外部存储重试与已验证的 CPU 修正

用户要求进行外部存储重试。创建、时间戳更新与删除在原始配置的构建 target 中均成功。单独的外部卷根目录探测返回权限拒绝；基准不要求写入卷根目录。未改变任何存储配置、target 位置、release 设置或缓存包装器。

release 基准可执行文件重新构建成功。实时采集器验收与独立的一秒子进程 CPU 时钟在约 0.018% 内一致。排队准入、六次真实 startup 重复以及失败不重试验收也随修正后的可执行文件通过。锁定 metadata、workspace/all-target 检查、格式化、Rust 风格、包括 OpenMLS 执行在内的仓库架构以及 diff 检查均在重试时通过。

较早的正式套件随后在用尽聚合一小时环境等待后结束。它保留了六次测量 startup 运行、六次测量 disconnected idle 运行以及两次测量 connected idle 运行。没有已启动工作负载的失败；未到达文本。原始套件及其失败保持不变。

位于 `target/benchmark-reference-v2-cpu-fixed-2026-11-03` 的新套件使用已验证可执行文件与运行器的冻结副本。它测量全部四个场景，每个场景一次预热和六次测量运行。现在每次运行都记录归一化 CPU 格式与 Mach timebase。保留的 CPU 验收、二进制来源与套件配置标识这些输入。`completion.json` 记录最终的套件与分析完成；只有在全部 28 个运行清单、清理回执与 CPU 转换通过后才会生成 `verified-report.md`。这是正在进行的测量，不是已完成或已批准的 baseline。

验证回执：`target/benchmark-cpu-units-fixed-retry`、`target/benchmark-cpu-fixed-admission-retry`，以及 `target/benchmark-cpu-fixed-*-retry.log` / `.json`。

## 完整的归一化 CPU 参考结果

新套件于 2026-11-03 11:53 Asia/Shanghai 完成，耗时约 53 分钟。全部四个场景各完成六次测量运行加一次预热。全部 28 个清单、归一化 CPU 转换、正确性结果与清理回执均通过。六次测量文本运行每次都同步了 200 个条目，并在接收端重启后验证了持久化历史。一次准入延迟为竞争构建等待了约 31 秒；没有过滤任何失败样本。

证据：`target/benchmark-reference-v2-cpu-fixed-2026-11-03/completion.json`、`verified-report.md`、`verified-summary.json`、`verification-manifest.json`，以及 `results/` 下不可变的逐次运行产物。报告完成前，最终验证清单与所有清理/文本结果均已重新检查。

各端点 startup 中位数约为 263 与 272 ms。disconnected idle RSS 中位数约为 29 MiB；connected idle 约为 33–35 MiB；文本工作约为 34–36 MiB。观察到的完整进程峰值达到约 228 MiB，包含初始化。这些是固定 Engine 提交并在来源中记录测试宿主 overlay 的无头 connectivity 宿主测量。它们不是原生守护进程或设备测量。不隐含任何已批准的参考、数值门禁或优化声明。

## PR 交付验证

最终可执行文件在交付前重复了真实双文本同步与接收端重启验收。六个产物/控制用例再次通过：A/A 验收比较、样本被篡改、样本缺失、清理回执缺失、格式错误响应与超大响应。证据位于 `target/benchmark-pr-acceptance` 与 `target/benchmark-pr-report-acceptance`。完整参考验证清单中的全部 37 个文件均已重新检查。

被测 Engine revision 仍为 `aa1b409fda38053d2d1e61e7801f680e2261ea26`。之后的主分支变更不在该测量范围内。本地原始产物被保留，不由 CI 提交或上传。首次参考不能证明后续 Engine revision 的性能等价。
