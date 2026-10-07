#!/usr/bin/env bash
# CI 统一 MBX 装配；只将受限配置文件路径传给后续 Cargo 入口。
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
: "${GITHUB_ENV:?GITHUB_ENV is required}"
: "${GITHUB_PATH:?GITHUB_PATH is required}"
bash "$root/scripts/build-cache/mbx.sh" --install
umask 077
config=$(mktemp "$RUNNER_TEMP/engine-mbx-env.XXXXXX")
backend=local
endpoint=${BUILD_CACHE_R2_ENDPOINT:-}
key_id=${BUILD_CACHE_R2_ACCESS_KEY_ID:-}
secret=${BUILD_CACHE_R2_SECRET_ACCESS_KEY:-}
if [[ -n "$endpoint" && -n "$key_id" && -n "$secret" ]]; then
  [[ "$endpoint" == https://* ]] || { echo 'MBX R2 endpoint must use HTTPS' >&2; exit 1; }
  mode=read-only
  if [[ "${BUILD_CACHE_ACCESS:-read}" == write ]]; then mode=read-write; fi
  {
    printf 'export BUILD_CACHE_R2_ENDPOINT=%q\n' "$endpoint"
    printf 'export BUILD_CACHE_R2_ACCESS_KEY_ID=%q\n' "$key_id"
    printf 'export BUILD_CACHE_R2_SECRET_ACCESS_KEY=%q\n' "$secret"
    printf 'export MBX_REMOTE_MODE=%q\n' "$mode"
  } > "$config"
  backend=r2
elif [[ -n "$endpoint$key_id$secret" ]]; then
  echo '::error::Incomplete MBX R2 credentials; refusing to misreport the backend' >&2
  exit 1
else
  # 无凭据 job 不继承 self-hosted runner 的另一套 R2 身份。
  printf 'unset BUILD_CACHE_R2_ENDPOINT BUILD_CACHE_R2_ACCESS_KEY_ID BUILD_CACHE_R2_SECRET_ACCESS_KEY\n' > "$config"
fi
unset endpoint key_id secret
mkdir -p "$RUNNER_TEMP/mbx-evidence"
{
  echo "UC_ENGINE_MBX_ENV_FILE=$config"
  echo "UC_ENGINE_MBX_STATS_DIR=$RUNNER_TEMP/mbx-evidence"
  echo "MBX_CACHE_DIR=$RUNNER_TEMP/engine-mbx-cache"
  if [[ $backend == r2 ]]; then echo 'MBX_GC_MAX_SIZE=20GiB'; else echo 'MBX_GC_MAX_SIZE=2GiB'; fi
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
printf 'MBX cache backend: %s; native protected-push policy determines effective write access\n' "$backend"
