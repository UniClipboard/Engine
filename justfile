# 仓库级开发命令。所有 Rust 命令仍从仓库根目录运行；这里只收录需要固定参数或工具版本的入口。

set positional-arguments

# 列出可用命令
default:
    @just --list

# 默认 Cargo 入口：构建、检查与测试统一使用 MBX
cargo *args:
    @scripts/build-cache/bin/cargo "$@"

# 经由 mbx 跨 worktree 编译缓存运行一条 Cargo 命令，例如：just mbx check --workspace --all-targets --locked
mbx *args:
    @bash scripts/build-cache/mbx.sh "$@"

# 让命令内部的 cargo 调用也经由 mbx，例如：just mbx-exec bash scripts/testing/run-test-group.sh evidence
mbx-exec *args:
    @bash scripts/build-cache/mbx.sh --exec "$@"

# 以同一配置运行 mbx 自身命令，例如：just mbx-tool cache stats
mbx-tool *args:
    @bash scripts/build-cache/mbx.sh --mbx "$@"

# 下载并校验仓库固定的 mbx 版本
mbx-install:
    @bash scripts/build-cache/mbx.sh --install
