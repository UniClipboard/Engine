# 本地 MBX 编译并行度基准

生成日期：2026-10-07。来源：真实 Engine 工作树执行日志与 Cargo timing；复跑入口见
[本地构建指南](../design-docs/local-builds.md)。此快照不代表其他机器、并行 worker 的总吞吐或 CI 收益。

## 环境与固定输入

- 业务源码 HEAD：`0b471c4f14a0196b8ed747f48a96b31667f5d1c0`，开始时工作区干净。
- Apple M4，10 个逻辑核，24 GiB RAM；外置 APFS SSD。
- Rust 1.95.0 (`59807616e`)，Cargo 1.95.0 (`f2d3ce0bd`)，LLVM 22.1.2。
- Apple ld 1328.2；仓库固定 MBX 1.18.0。
- `CARGO_INCREMENTAL=0`，仓库 `dev` profile、默认 features 的 workspace 统一、本机 `aarch64-apple-darwin`。
- 命令：`bash scripts/build-cache/mbx.sh check --workspace --all-targets --locked --timings`。
- 唯一候选变量：`CARGO_BUILD_JOBS=2` 或 `4`；每组独立空 target 与空 MBX 动作缓存。
- 下载缓存与 OS 页面缓存保留；两组顺序执行，冷构建每组只有一个样本。
- 微改仅在 `uc-core/src/lib.rs` 追加不同中文注释，结束后恢复原始字节。

## 结果

| 场景 | 2 个任务 | 4 个任务 | 观测 |
| --- | --- | --- | --- |
| 空 target/动作缓存全工作区检查，1 次 | 177.394 s | 92.347 s | 缩短 47.9% |
| 热构建，3 次中位数 | 0.620 s | 0.625 s | 基本不变，不宣称提升 |
| 注释微改后重建，3 次中位数 | 54.972 s | 40.682 s | 缩短 26.0% |

两组 Cargo timing 中的 917 个编译单元与 features 完全一致。优化后的仓库复跑脚本
再次从空输出/动作缓存执行 4 任务检查，得到 93.376 秒，作为可重复性补充，不混入原始对照中位数。

冷构建主要耗时来自 Rust 编译检查与构建脚本，而不是最终 Engine 可执行文件链接。
2 任务的最长单元包含 `uc-application` 检查测试目标 13.37 秒、`zstd-sys` 构建脚本 13.00 秒、
`uc-engine` 检查测试目标 12.34 秒。提高 Cargo 任务并行度与该瓶颈一致；不修改链接器。

额外的 4 任务“恢复源码”样本遇到仓库检查器内部 OpenMLS 测试的锁等待，并切换 wrapper，
因此不纳入收益表。其日志保留，不能当作热构建、缓存恢复或优化失败数据。

## 采纳与边界

仅在可选 MBX 入口将未设置的 `CARGO_BUILD_JOBS` 默认设为 4；普通 Cargo 的保守默认 2、
CI 核数设置、features、Infra 优化级别、发布 profile 与来源链接方式均未改变。
已有环境值和 Cargo `-j` 可以覆盖；内存紧张或多个 worker 同时构建时可收紧到 2。

原始工件包含每次完整日志、CPU/内存计时、Cargo timing HTML、环境与源文章快照；
当前任务的交付报告提供本地归档。单进程最大 RSS 不等于所有编译进程的物理内存总量。
Linux/Windows、移动设备、正式发布构建及 CI 性能对比未执行，不能据此宣称提速。
