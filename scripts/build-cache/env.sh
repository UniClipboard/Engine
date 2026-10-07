#!/usr/bin/env bash
# 只激活当前进程树的仓库 Cargo 入口，不修改全局工具或配置。
uc_engine_cargo_bin="$(cd "$(dirname "${BASH_SOURCE[0]}")/bin" && pwd)"
case ":$PATH:" in
  *":$uc_engine_cargo_bin:"*) ;;
  *) export PATH="$uc_engine_cargo_bin:$PATH" ;;
esac
unset uc_engine_cargo_bin
