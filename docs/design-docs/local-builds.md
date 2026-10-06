# 本地多 worktree 构建

本地构建的完整负责人是 [`mbx.sh`](../../scripts/build-cache/mbx.sh)；调用方只需执行
`just mbx <Cargo 参数>` 或 `just mbx-exec <命令>`。失败保留命令退出码，重试仍由调用方发起。
默认 Cargo 和 CI 的编译缓存职责见 [ADR-028](decisions/028-optional-mbx-build-cache.md) 与
[ADR-029](decisions/029-ci-r2-compile-cache.md)。

## 输出、缓存与并行度

每个 worktree 使用自己的 target，同一 worktree 的 Cargo 验证串行执行。多个 worktree 可以共用
MBX 动作缓存，不能共用同一个 Cargo 输出目录：锁等待会串行化构建，mtime 判断也不适合作为跨树来源证明。
MBX 不接管或回收 target。外置路径由本机既有配置或调用方提供，仓库不包含机器绝对路径。
不要改变全局 Cargo、sccache 或系统配置，也不要回收其他工作树的输出。

可选 MBX 入口默认 `CARGO_BUILD_JOBS=4`，已有环境值与命令行 `-j` 可以覆盖；普通 Cargo
继续使用 `.cargo/config.toml` 的 2，CI 已按 runner 核数单独覆盖。

编译并行度是 Cargo 的任务数上限，不是 rustc 内部线程或峰值内存的硬限制。多 worker 的总资源消耗
仍需调用方安排；内存紧张时通过 `CARGO_BUILD_JOBS=2` 收紧，命令行 `-j N` 优先于环境默认。
不要通过关闭必要 features、降低 Infra 优化级别或改变发布 profile 换取无法比较的耗时。

## R2 分布式动作缓存

使用固定 MBX 原生 S3 后端：`just mbx --r2 <Cargo 参数>`。每个 worker 仍有独立 target 和本地
动作缓存；远端对象位于 `uniclipboard-build-cache` 桶的 `engine/mbx/v1/`，与 sccache 前缀隔离。
R2 共享编译结果，不提供跨机器编译锁或调度，也不会把 sccache 对象转换为 MBX 对象。

调用方从安全凭据管理器提供 `BUILD_CACHE_R2_ENDPOINT`、`BUILD_CACHE_R2_ACCESS_KEY_ID` 与
`BUILD_CACHE_R2_SECRET_ACCESS_KEY`，不要在仓库、命令行参数或日志中保存值。端点是账户的 HTTPS R2
S3 端点；访问密钥仅限该桶。缺少任一项时入口报错，不会启动本地构建冒充远端使用。
入口只在当前子进程映射为原生 AWS 凭据，region 为 `auto`，默认 `MBX_REMOTE_MODE=read-only`；
本机和 PR 应使用只读令牌。凭据会进入本次构建的环境，原生 MBX 不是对 build script 的凭据隔离机制。

可信 main 的受保护分支 push CI 才使用写令牌与 `read-write`；写令牌仍放在仅允许 main 的
`engine-build-cache-writer` 环境，PR 使用 `engine-build-cache-reader`。MBX 1.18.0 会将本地、PR、
未保护分支和手工触发收紧为只读，不能靠只写 `read-write` 启用发布，也不得伪造 GitHub 环境。
主线必须真实受保护；接入前应核对实际规则。本机尚需独立配置只读凭据，GitHub secrets 无法读回。
正式 release/tag 不接入共享缓存。主线发布与 PR 只读验收由
[`mbx-r2-cache.yml`](../../.github/workflows/mbx-r2-cache.yml) 运行，工件包含诊断及原生统计；主线还必须
运行独立空 target/动作缓存的消费者并确认下载、命中、无远端错误与零上传。主线分别播种
`check --workspace --all-targets --locked` 与迁移测试构建的 manifest；消费者按同一命令读取，
不能用测试构建的种子代替常用 check 的种子。

R2 诊断使用 `just mbx --r2 --mbx doctor --json`；预取使用
`just mbx --r2 --mbx prefetch check --workspace --all-targets --locked`。空 manifest 不等于连接故障，
但诊断成功也不等于编译命中。用 `MBX_STATS_REPORT` 保存原生统计，并在独立空 target/本地动作缓存
消费者中确认 `downloaded_bytes`、恢复输出与 hits；远端错误和上传失败也必须保留。

R2 的桶级令牌不能按前缀隔离信任；只有可信构建可以拿到写令牌。MBX 原生内容校验、请求期限、
读失败后本地编译与异步上传负责缓存失败恢复，不另实现网络缓存层。R2 的真实共享命中及耗时须另行
验收，先前的 2/4 任务基准不是远端缓存收益。配置语义见
[MBX 固定版本远端缓存文档](https://github.com/jdx/mr-boxington/blob/v1.18.0/docs/remote-cache.md)
与 [Cloudflare R2 S3 接入](https://developers.cloudflare.com/r2/get-started/s3/)。

## 可重复测量

从仓库根目录运行，`--scratch` 指向已存在的外置可再生目录，`--output` 必须是新的日志目录：

```bash
python3 scripts/build-cache/benchmark-local.py \
  --scratch "$BUILD_SCRATCH" --output "$BENCHMARK_OUTPUT" --jobs 2 4 --repeats 3
```

脚本为每个并行度创建独立空 target 和 MBX 动作缓存，固定执行
`check --workspace --all-targets --locked --timings`。依次记录一次冷构建、多次热构建、
`uc-core` 注释微改后的重建、恢复源码后的重建，以及同一 target 中两条命令的锁竞争。
微改不改变业务行为，正常退出与异常退出时恢复原始字节；运行期间不得有其他进程编辑该文件。
单次构建默认限时 3600 秒，可用 `--timeout` 覆盖；工具信息采集限时 30 秒。
超时记录 `timed_out=true` 与退出码 124，先终止本次独立构建进程组（含 Cargo/rustc）再恢复源码。
两个锁竞争样本分别记录 `lock_wait_observed` 与匹配原文；未实际等待时不能据标签宣称竞争。

工件包含源码 HEAD/状态、机器和工具版本、非敏感构建环境、完整命令/耗时/退出码、原始日志、
Cargo timing HTML 和中位数表。冷构建只指空 target/动作缓存，不包含清空 Cargo 下载或 OS 页面缓存。
顺序运行会有系统负载与页面缓存偏差；首次冷构建的单样本不能解释为稳定的机器普遍收益。
macOS 的 `/usr/bin/time -l` 同时记录 CPU 时间与最大 RSS，不能把其单个最高进程 RSS 当作整棵进程树的峰值。
脚本不删除调用方的目录；完成后由当前任务负责人确认无活动构建进程，再回收日志中列出的专用目录。

完整二进制构建与真实 E2E 验收单独执行，不能用 `cargo check` 代替运行正确性。
来源验收应断言已链接产物中的完整 HEAD 与源码状态，不能在运行时读取所在目录的 git 来伪造证明。

实测输入、结果与未验证边界见 [基准快照](../generated/local-build-benchmark.md)。

## 文章候选的适用范围

[Infrastructure for Agentic Rust](https://blog.brokk.ai/infrastructure-for-agentic-rust/)
讨论 MBX、输出空间、链接器和把构建分散到多机器。这里只采用与本仓证据对应的部分：

- MBX 已固定版本与校验值；保留独立 target 和动作缓存，而不是再增加一种缓存工具。
- 现有 `line-tables-only` 已缩减开发调试信息，发布配置维持独立的完整性要求。
- MBX 的 `share_workspace_root` 会改变 `file!()`、panic 与调试信息中的路径，本次不开启。
- Apple 产物继续使用 Apple ld；文章的 Linux 链接器比较不能直接证明 macOS 收益。
- 不重格式化磁盘、不安装 ZFS、不迁移编译宿主；当前问题先用已有 Cargo 任务并行度解决。
- CI 已按逻辑核数设置并行度，并使用依赖缓存与 R2 sccache；本地实测不代表 CI 提速。

MBX 参数语义以 [固定版本文档](https://github.com/jdx/mr-boxington/blob/v1.18.0/docs/configuration.md)
为准；Cargo 并行度优先级见 [Cargo configuration](https://doc.rust-lang.org/cargo/reference/config.html#buildjobs)。
