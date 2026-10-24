# 本地多 worktree 构建

本地构建的完整负责人是 [`mbx.sh`](../../scripts/build-cache/mbx.sh)；调用方只需执行
`just cargo <Cargo 参数>`；仓库测试/平台构建脚本自动接入。当前 shell 直接执行 Cargo 前先
`source scripts/build-cache/env.sh`，任意外部命令可用 `just mbx-exec <命令>`。失败保留命令退出码，重试仍由调用方发起。
本地、CI、E2E 与发布的唯一缓存决定见 [ADR-033](decisions/033-unified-mbx-rust-builds.md)。

## 输出、缓存与并行度

每个 worktree 使用自己的 target，同一 worktree 的 Cargo 验证串行执行。多个 worktree 可以共用
MBX 动作缓存，不能共用同一个 Cargo 输出目录：锁等待会串行化构建，mtime 判断也不适合作为跨树来源证明。
MBX 不接管或回收 target。外置路径由本机既有配置或调用方提供，仓库不包含机器绝对路径。
不要改变全局 Cargo、sccache 或系统配置，也不要回收其他工作树的输出。

统一 MBX 入口默认 `CARGO_BUILD_JOBS=4`，已有环境值与命令行 `-j` 可以覆盖；普通 Cargo
继续使用 `.cargo/config.toml` 的 2，CI 已按 runner 核数单独覆盖。

编译并行度是 Cargo 的任务数上限，不是 rustc 内部线程或峰值内存的硬限制。多 worker 的总资源消耗
仍需调用方安排；内存紧张时通过 `CARGO_BUILD_JOBS=2` 收紧，命令行 `-j N` 优先于环境默认。
不要通过关闭必要 features、降低 Infra 优化级别或改变发布 profile 换取无法比较的耗时。

## CI 压缩快照与本地 R2 入口

CI 的唯一远端编译缓存是外层 CAS 压缩快照，格式、兼容键、信任边界及验证契约见
[ADR-033](decisions/033-unified-mbx-rust-builds.md#2026-10-24-修订ci-仅使用外层压缩-cas-快照)。
无凭据、缺失或损坏归档只进行本地冷编译；任何 CI 编译的 MBX 原生上传/下载均应为零。
`mbx-r2-cache.yml` 保留完整 workspace check 与迁移 E2E，并在可信 main 发布后用独立空
CAS/target 验证归档恢复。原生 hits、测试实际执行和外层下载分别记录，不能混为一谈。

开发机仍可显式使用 `just mbx --r2 <Cargo 参数>`，调用方从安全凭据管理器提供三项
`BUILD_CACHE_R2_*` 凭据，默认原生只读。该路径不属于 CI，也不共享新的快照格式；CI 即使
收到 `--r2` 也强制本地编译。不要在仓库、命令行或日志保存凭据，不伪造 CI 身份取得写权限。

本地聚焦验证：

```bash
python3 scripts/build-cache/mbx-snapshot.test.py
python3 scripts/build-cache/mbx-snapshot.integration.py
python3 scripts/build-cache/verify-r2-build.test.py
python3 scripts/build-cache/check-snapshot-routing.py
```

integration 使用仓库 target 下独占的短期 fixture，并在子进程结束后回收；不清理已有 target。
R2 专用验收继续保留磁盘与 macOS 资源计数，但采样不是峰值，也不证明托管快照收益。

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
- CI 按逻辑核数设置并行度，编译动作统一经 MBX；本地实测不代表 CI 提速。

MBX 参数语义以 [固定版本文档](https://github.com/jdx/mr-boxington/blob/v1.24.0/docs/configuration.md)
为准；Cargo 并行度优先级见 [Cargo configuration](https://doc.rust-lang.org/cargo/reference/config.html#buildjobs)。

## 测试与命中边界

`just cargo test`、`just cargo nextest` 与各测试分组脚本都经过 MBX。MBX 只恢复编译产物；测试、网络交互与工件断言仍实际执行。metadata/fmt/审计不是可命中的 rustc 动作，第一次编译和不支持的原生 bypass 也不能记为命中。

CI 在 `mbx-evidence` 保存原生编译统计与独立快照传输报告。凭据仅进入传输步骤；fork 无凭据只进行本地编译。`compile-cache-benchmark.yml` 只读兼容快照，使用两个独立空 target 和本次本地 CAS 验证恢复，不模拟主线发布。
