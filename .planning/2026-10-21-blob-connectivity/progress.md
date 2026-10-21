# 进度

- 2026-10-21：读取 implement、uni-rust-development、planning-with-files 与仓库约束；fetch 并新开分支，保留已有改动。
- 尚未实现生产代码或执行阶段 0 技术验收。

- SDK 源码：Remote 私有 get_blob_ranges_impl 在网络 write 错误时 try_join 提前退出，丢弃 import rx；store import 在独立运行时执行。wait_idle 是整个 store 的全局等待，不能视作单项停止。
- 新增独立集成探针：真实 QUIC + FsStore，通过测试 subscriber 暂停 import worker，再注入网络重置，检查 SDK 错误返回是否等待 worker。
- 首轮编译失败：let_underscore_lock 拒绝未绑定 MutexGuard；改用具名 guard，首轮日志保留 probe.log。
- 第二轮探针已到退出清理，但重复关闭 provider store 返回 Sender closed；Router 的 BlobsProtocol 已关闭同一 store。删除重复 shutdown，失败日志保留 probe-fixed.log。
- 停止探针修正后通过（1 项，0.13 秒）；扩展同连接 sibling 内容读回时出现 SDK error 3，尚在定位测试是否混入了额外完成语义问题，日志 probe-sibling.log 保留。
- sibling 扩展固定失败在 SDK get 的 stream reset error 3，原因未证明。该扩展不属于最小停止复现，撤回扩展并保留失败记录；不能宣称同连接双流或内容读回通过，相关验收保持未执行。

- 最终最小探针通过：1 passed（0.14 秒）。根 metadata、workspace all-targets check、fmt、Rust style、Engine repository preflight、文档相对链接和差异检查通过。
- workspace 分组完整回归运行中（4306 项）；成员全集、升级矩阵、设备和线上 CI 未执行。
- 手工审查确认无生产接口、协议、持久格式和依赖来源变化，测试全局 subscriber 仅在独立探针进程使用；code-review 技能未安装，不假称已调用。

- 完整 workspace 分组结束：555.896 秒，4305 passed、1 failed、27 skipped，退出码 100；新增探针 0.145 秒通过。
- 唯一失败 peer_address_read_diagnostics.rs:270 的符号状态 unresolved／captured；对应生产与测试来源均未改动。独立运行既有二进制仍失败（退出 101），保留 diagnostic-existing-binary.log，不放宽断言，不声称整轮通过。
- 一次用 workspace Cargo 命令复核诊断项触发无必要的全目标重建，已终止该命令组（143），确认其无存活进程，改用既有二进制复核；原日志 diagnostic-rerun.log 保留。
- 本次未创建外置 target、临时旧库或真实资料副本；只保留共享编译缓存。阶段 0 仍阻塞，后续生产阶段未启动。

- 续接：已重新核对分支／工作区／goal，上一 goal 相关工作留下可复现证据；本次继续验证底层 SDK 的完成等待权，不把高层 API 失败扩大为全部 SDK 不可用。

- 已验证固定 SDK 底层 get/import 与 provider/export 原型（managed-provider-probe.log，1 passed，0.44 秒）：暂停同次本地工作时取消不提前完成，兄弟流内容正确；未新增 SDK 依赖。
- 已定位旧 sibling error 3：Raw blob 使用 GetRequest::all 错当 HashSeq；改为 blob 后同连接读回通过，原失败保留。
- Uni 原型首轮编译发现 SDK pair.get_request 的 &self future 要求半流 Sync，改用当前生产未开启 SDK 进度追踪的 RequestTracker::NONE，避免将 Sync 限制扩大到 Application；Blob 新协议分类须在组更新 wire 中显式拒绝，已补齐。

- 真实 Uni 原型首次已通过（0.17 秒）；补充无效 reset／stop code 的稳定分类、source 与连接保留后再通过（0.09 秒）。固定 SDK 接收角色的普通与空 blob 对替代 provider 也通过（0.40 秒）。
- 续接相关回归全部通过：Uni 29、原生 connectivity 3、替代接收生命周期 6、Application transport 20；根 metadata、workspace all-targets check、fmt、Rust style、repository preflight、相对文档链接与差异检查通过。日志见 resume-validation.log 与对应命令日志。
- 本轮未重跑整套 workspace／成员全集／旧端矩阵和平台 CI，不沿用上一检查点的完整回归结果冒充当前 head。完整 spec 完成前仍须完成其全部必需门禁。
- 手工差异审查：通用半流变更仅扩展传输无关能力，稳定 Engine／绑定未扩大；I/O 分离后的连接与许可保留至实际本地工作退出。所有 SDK 替代流程仍为测试原型，生产 blob 路径未切换。
- 下一步：原始接收项材料与身份重建、旧端当前授权及资料角色、协议／资源基线前置；原型采用时迁入唯一 Infra 实现并删除测试副本，再推进调度和生产接线。
