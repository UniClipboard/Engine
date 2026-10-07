# ADR-028：本地构建可选使用 mbx 跨 worktree 编译缓存

- **状态**：已被 [ADR-033](033-unified-mbx-rust-builds.md) 取代；下文保留历史决定
- **日期**：2026-09-27
- **范围**：本地开发构建入口；不改变 CI、发布构建、全局 Cargo 配置或默认的 `cargo` 行为
- **相关文件**：[`justfile`](../../../justfile)、[`scripts/build-cache/mbx.sh`](../../../scripts/build-cache/mbx.sh)

## 背景

Engine 开发普遍采用“一个任务一个 worktree”。现有本地与 CI 方案都用 sccache（`RUSTC_WRAPPER=sccache`、
`CARGO_INCREMENTAL=0`）。实测在 macOS 上，sccache 对“同一提交的新 worktree”帮助很小：

- 新检出的 `cargo check --workspace --all-targets` 仍有 504 次未命中、260 次不可缓存，只比冷构建快约 16%；
- 测试构建（`nextest --no-run`）只快约 8%。

未命中主要来自路径相关的编译键；不可缓存的主要是 sccache 不处理的链接产物（测试二进制、proc-macro、可执行文件）。

[mbx（mr-boxington）](https://mr-boxington.jdx.dev/) 同样在 `RUSTC_WRAPPER` 层按内容缓存编译结果，但会把工作区路径
映射为占位符，并缓存测试二进制、proc-macro 链接和 build script 中的 C 编译。

## 实测（macOS arm64，Engine `987c5eed`，Rust 1.95.0，`CARGO_BUILD_JOBS=4`，外置 APFS SSD）

| 场景 | sccache | mbx 默认 | mbx（本决定的配置） |
|---|---|---|---|
| 新检出 `check --workspace --all-targets`（3 次中位数） | 76.8 s | 30.1 s | 23.9 s |
| 新检出测试构建（1 次） | 244 s | 77 s | — |
| 同一检出改动 `uc-core` 常量后重建（3 次中位数） | 44.4 s | 51.2 s | 45.4 s |
| 撤销改动后重建（3 次中位数） | 25.0 s | 20.7 s | 2.5 s |
| 冷缓存首次填充 `check` / 测试构建 | 91.9 s / 264 s | 86.0 s / 338 s | 85.7 s / — |
| 已恢复产物上运行 `run-test-group.sh evidence` | — | — | 18/18 通过；构建与运行 32 s（填充时 296 s） |

另外两项正确性检查：
- `MBX_VERIFY=1` 在 `uc-core` 上比对 52 次编译，0 处不一致；
- 在恢复产物上运行 `process` 测试组，30/30 通过。

mbx 默认开启的 learned incremental 会让改动后的重建变慢，缓存每次增长约 1.8 GiB；关闭后改动重建与 sccache 持平。

## 决定

1. **可选入口，默认行为不变。** 提供 `just mbx …` / `just mbx-exec …`，只在该命令的进程环境中改用 mbx；
   默认的 `cargo`、sccache 和 CI 保持不变。
2. **版本与完整性。** 固定 mbx 版本，并为每个支持平台固定发布包 sha256。首次使用时下载到用户工具缓存（默认
   `~/.cache/uniclipboard-engine/tools`，可用 `UC_TOOLS_DIR` 覆盖），校验失败即停止。
3. **仓库脚本固定的配置：**
   - 在该命令内移除 `RUSTC_WRAPPER`：mbx 遇到其他 wrapper 会原样转交，自己不做缓存。
   - `MBX_TARGET_VIEWS=0`、`MBX_TARGET_SEED=0`：mbx 不接管、移动或回收 target。target 位置仍由 Cargo 和本机
     既有配置决定，本地构建目录规则不受影响。
   - `MBX_LEARNED_INCREMENTAL=0`。
   - 动作缓存上限默认 20 GiB（`MBX_GC_MAX_SIZE`，可覆盖）。
4. **缓存位置属于机器配置。** 由开发者环境中的 `MBX_CACHE_DIR` 指定，仓库不写入机器路径。已设置但上级目录
   不存在（例如外置盘未挂载）时脚本拒绝运行，不让缓存悄悄落到其他磁盘。
5. **同一 worktree 只走一条路线。** mbx 与 sccache 对产物的编译键不同，同一 target 在两条路线间切换会重编
   工作区 crate（实测单向约 55 s）。新 worktree 选定 `just mbx` 后应持续使用。

## 备选方案

- **维持现状，只用 sccache。** 新 worktree 的首次构建仍接近冷构建。
- **全局改用 mbx（`mbx setup` 或全局 `RUSTC_WRAPPER`）。** 影响所有仓库和编辑器，且 mbx 默认会接管并自动
  回收 target 目录。项目只维护 5 周、单一主要作者、发布频繁，不适合作为全局默认。
- **沿用 mbx 默认配置。** 改动重建慢 15–58%，缓存增长过快。
- **CI 改用 mbx。** CI 已由 rust-cache 恢复 target；收益需要 mbx GitHub Action 或远端缓存，未实测，不在本决定范围。

## 后果与未验证项

- 回滚：删除 `justfile` 中的 `mbx*` 命令和 `scripts/build-cache/`，再删除本机工具缓存与 `MBX_CACHE_DIR` 目录。
  mbx 的部分缓存目录是只读的，删除前需要先加回写权限。
- 升级 mbx 时需要同时更新版本号和各平台 sha256，并按上述场景复测。缓存格式可丢弃，读不懂的条目按未命中处理。
- 未验证：Linux 与 WSL 的实测、Windows、移动端产物构建、CI、多个构建同时共用一个缓存，以及 1.18.0 之后的版本。


## 本地有限并行度补充（2026-10-07）

可选入口在未设置 `CARGO_BUILD_JOBS` 时默认使用 4；已有环境值与 Cargo `-j` 保持优先。
普通 Cargo 的保守默认 2、缓存参数和 CI 保持原样。当前 HEAD 下空缓存检查与注释微改重建
分别缩短 47.9% 和 26.0%，热构建基本不变；机器、样本限制和可重复入口见
[本地构建指南](../local-builds.md) 与 [基准快照](../../generated/local-build-benchmark.md)。
这不是多个 worker 的 CPU/内存配额保证，也不是其他平台或 CI 的收益结论。

## R2 分布式缓存补充（2026-10-07）

用户明确选择可信 main CI 写入、本地 worker 与 PR 只读。`mbx.sh --r2` 使用 MBX 原生 S3 后端，
复用现有 R2 桶但使用 `engine/mbx/v1/` 独立对象命名空间。远端设置与密钥仍属于调用方环境，
不写 `.mbx.toml` 或全局配置。MBX 的发布策略要求真实的受保护分支 push；现有 writer 环境的 main
限制不能代替该条件。接入配置与发布条件、诊断和独立消费者证明见[本地构建指南](../local-builds.md#r2-分布式动作缓存)。
本补充不把原本地并行度结果解释为 R2 收益，也不改变正式发布配置。
