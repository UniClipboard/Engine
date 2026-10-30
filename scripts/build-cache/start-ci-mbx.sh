#!/usr/bin/env bash
# CI 统一本地 MBX 装配；后续 Cargo 入口不持有远端配置或凭据。
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
: "${GITHUB_ENV:?GITHUB_ENV is required}"
: "${GITHUB_PATH:?GITHUB_PATH is required}"
bash "$root/scripts/build-cache/mbx.sh" --install
# 凭据仅属于快照传输步骤，不落盘、不传给后续编译。
backend=local-snapshot
mkdir -p "$RUNNER_TEMP/mbx-evidence"
{
  echo "UC_ENGINE_MBX_LOCAL_ONLY=1"
  # 历史源码中的旧入口也不能继承 self-hosted 身份或全局远端配置。
  for variable in UC_ENGINE_MBX_ENV_FILE BUILD_CACHE_R2_ENDPOINT BUILD_CACHE_R2_ACCESS_KEY_ID BUILD_CACHE_R2_SECRET_ACCESS_KEY MBX_REMOTE_URL MBX_REMOTE_NAMESPACE MBX_REMOTE_S3_ENDPOINT MBX_REMOTE_S3_REGION MBX_REMOTE_TOKEN MBX_REMOTE_TOKEN_FILE MBX_REMOTE_OIDC_AUDIENCE ${!AWS_@}; do
    echo "$variable="
  done
  echo "UC_ENGINE_MBX_STATS_DIR=$RUNNER_TEMP/mbx-evidence"
  echo "MBX_CACHE_DIR=$(mktemp -d "$RUNNER_TEMP/engine-mbx-cache.XXXXXX")"
  echo "MBX_GC_MAX_SIZE=${MBX_GC_MAX_SIZE:-20GiB}"
  echo 'CARGO_INCREMENTAL=0'
  if [[ $(uname -s) == Darwin ]]; then cpus=$(sysctl -n hw.logicalcpu); else cpus=$(getconf _NPROCESSORS_ONLN); fi
  echo "CARGO_BUILD_JOBS=$cpus"
  echo "UC_COMPILE_CACHE_BACKEND=$backend"
} >> "$GITHUB_ENV"
# 保留本次 workflow 的工具快照，后续 checkout 历史源码不能将缓存入口退回旧实现。
tools=$(mktemp -d "$RUNNER_TEMP/engine-mbx-tools.XXXXXX")
mkdir -p "$tools/scripts/build-cache/bin"
cp "$root/scripts/build-cache/mbx.sh" "$root/scripts/build-cache/env.sh" "$tools/scripts/build-cache/"
cp "$root/scripts/build-cache/bin/cargo" "$tools/scripts/build-cache/bin/"
echo "UC_ENGINE_MBX_BIN=$tools/scripts/build-cache/bin" >> "$GITHUB_ENV"
printf '%s\n' "$tools/scripts/build-cache/bin" >> "$GITHUB_PATH"
printf 'MBX cache backend: %s; compilation has no native remote backend\n' "$backend"
