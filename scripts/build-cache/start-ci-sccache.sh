#!/usr/bin/env bash
# CI 编译缓存启动入口：校验固定版本的 sccache，选择唯一缓存后端并在当前 job 内常驻 sccache 服务。
# 取舍、信任边界与失败回退见 docs/design-docs/decisions/029-ci-r2-compile-cache.md。
#
# 由 .github/actions/rust-ci-setup 调用，输入全部来自环境变量：
#   BUILD_CACHE_ACCESS              write | read；只决定 sccache 读写模式，真实权限由 R2 令牌本身限定
#   BUILD_CACHE_R2_ENDPOINT         https://<ACCOUNT_ID>.r2.cloudflarestorage.com（可为空）
#   BUILD_CACHE_R2_ACCESS_KEY_ID    R2 S3 访问密钥 ID（可为空）
#   BUILD_CACHE_R2_SECRET_ACCESS_KEY R2 S3 访问密钥（可为空）
#   BUILD_CACHE_KEY_PREFIX          可选，覆盖默认对象前缀（仅供基准测试使用一次性前缀）
#   SCCACHE_SERVER_PORT             可选，sccache 服务端口（默认 4226），会写入 GITHUB_ENV 供后续编译使用
#
# 后端按顺序尝试，只启用第一个成功者，不叠加多个后端：
#   1. R2：三项凭据齐全且端点格式正确时使用；
#   2. GitHub Actions 缓存：无 R2 凭据（fork、Dependabot）或 R2 启动失败时使用，条目按 GitHub 规则限于当前 ref；
#   3. runner 本地磁盘：前两者都失败时使用，只在本 job 内有效。
# R2 凭据只交给 sccache 服务进程，不写入 GITHUB_ENV，后续步骤与第三方 Action 的环境中不出现凭据。
set -euo pipefail

SCCACHE_VERSION=v0.18.0
R2_BUCKET=uniclipboard-build-cache
# 对象前缀按仓库与平台划分；工具链、目标三元组、编译参数与源码已由 sccache 自身的键哈希区分。
# 代际号 v1 用于整体失效：缓存损坏或键模型变化时改为 v2，旧前缀由生命周期规则回收。
R2_KEY_GENERATION=v1

fail() {
  printf '::error title=Compile cache::%s\n' "$1" >&2
  exit 1
}

warn() {
  printf '::warning title=Compile cache::%s\n' "$1" >&2
}

# 输出 sccache-action 安装的二进制应有的 sha256；取自 mozilla/sccache v0.18.0 发布包解压后的 sccache，
# 发布包本身与发布页附带的 .sha256 一致。版本与摘要须同时更新。
pinned_sccache_sha256() {
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) echo 973cb15f6a986d84ca334bbed3bbe2eb8f1ee8fd81bf9e115b8539a293bf8d59 ;;
    Darwin-arm64) echo ecb9522010419996ff703b929cf000119928cfd3ee4a6ca8346ecc52ddcc8c13 ;;
    *) fail "当前 runner 平台没有固定的 sccache 摘要：$(uname -s)-$(uname -m)" ;;
  esac
}

sha256_of() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

logical_cpus() {
  if command -v nproc >/dev/null; then
    nproc
  else
    sysctl -n hw.logicalcpu
  fi
}

platform_segment() {
  local os arch
  os=$(uname -s | tr '[:upper:]' '[:lower:]')
  arch=$(uname -m)
  case "$arch" in
    x86_64 | amd64) arch=x64 ;;
    aarch64 | arm64) arch=arm64 ;;
  esac
  printf '%s-%s' "$os" "$arch"
}

sccache_bin=${SCCACHE_PATH:-$(command -v sccache || true)}
[[ -n "$sccache_bin" && -x "$sccache_bin" ]] || fail "未找到 sccache；应先运行 mozilla-actions/sccache-action"
[[ "$(sha256_of "$sccache_bin")" == "$(pinned_sccache_sha256)" ]] \
  || fail "sccache 二进制摘要与固定版本 $SCCACHE_VERSION 不一致"

access=${BUILD_CACHE_ACCESS:-read}
case "$access" in
  write) rw_mode=READ_WRITE ;;
  read) rw_mode=READ_ONLY ;;
  *) fail "BUILD_CACHE_ACCESS 只能是 write 或 read" ;;
esac

key_prefix=${BUILD_CACHE_KEY_PREFIX:-engine/sccache/$R2_KEY_GENERATION/$(platform_segment)}
[[ "$key_prefix" =~ ^engine/[a-z0-9/_-]+$ ]] || fail "对象前缀必须位于 engine/ 下且只含小写字母、数字、/、_ 与 -"

runner_temp=${RUNNER_TEMP:-${TMPDIR:-/tmp}}
server_log="$runner_temp/sccache-server.log"
: >"$server_log"
server_port=${SCCACHE_SERVER_PORT:-4226}
export SCCACHE_SERVER_PORT=$server_port
# 单次后端尝试等待服务就绪的上限（秒）；BUILD_CACHE_STARTUP_TIMEOUT 仅供故障演练缩短等待。
startup_timeout=${BUILD_CACHE_STARTUP_TIMEOUT:-30}
[[ "$startup_timeout" =~ ^[1-9][0-9]*$ ]] || fail "BUILD_CACHE_STARTUP_TIMEOUT 必须是正整数秒"
server_pid=

port_open() {
  (exec 3<>"/dev/tcp/127.0.0.1/$server_port") 2>/dev/null
}

# 只停止本脚本启动并记录 PID 的服务：先 TERM，2 秒后仍存活则 KILL，确认退出后端口即释放。
kill_own_server() {
  [[ -n "$server_pid" ]] || return 0
  kill "$server_pid" 2>/dev/null || true
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    kill -0 "$server_pid" 2>/dev/null || break
    sleep 0.2
  done
  if kill -0 "$server_pid" 2>/dev/null; then
    kill -9 "$server_pid" 2>/dev/null || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      kill -0 "$server_pid" 2>/dev/null || break
      sleep 0.2
    done
  fi
  server_pid=
}

# 以前台模式（SCCACHE_NO_DAEMON=1）启动服务并记录其 PID；超时或失败时直接结束该进程，
# 不会留下稍后才完成启动、与后备服务争用端口的迟到服务。服务在命令替换的子 shell 中启动，
# 由 init 收养，不进入本脚本的作业表。sccache 只在存储读探测通过后才监听端口，
# 因此端口可连接即表示该后端已就绪。服务在本 job 内常驻（IDLE_TIMEOUT=0），长时间只跑测试后再次编译时
# 不会由客户端以不同配置重启服务。每次尝试都从干净环境启动，保证同时只启用一个后端。
# 服务的 stderr 写入 runner 临时目录，其中的存储错误可能含端点，只供本脚本匹配，不回显、不上传。
start_server() {
  kill_own_server
  if port_open; then
    warn "端口 $server_port 已有其他 sccache 服务，拒绝复用未知配置的服务"
    return 1
  fi
  server_pid=$(env -u SCCACHE_GHA_ENABLED -u SCCACHE_BUCKET -u SCCACHE_ENDPOINT -u SCCACHE_REGION \
    -u SCCACHE_S3_KEY_PREFIX -u SCCACHE_S3_RW_MODE -u AWS_ACCESS_KEY_ID -u AWS_SECRET_ACCESS_KEY \
    -u AWS_SESSION_TOKEN -u AWS_PROFILE -u SCCACHE_STARTUP_NOTIFY \
    SCCACHE_START_SERVER=1 SCCACHE_NO_DAEMON=1 SCCACHE_IDLE_TIMEOUT=0 "$@" \
    "$sccache_bin" </dev/null >/dev/null 2>>"$server_log" &
    echo $!)
  local waited=0
  while ((waited < startup_timeout * 5)); do
    port_open && return 0
    kill -0 "$server_pid" 2>/dev/null || break
    sleep 0.2
    waited=$((waited + 1))
  done
  kill_own_server
  return 1
}

cache_location() {
  "$sccache_bin" --show-stats 2>/dev/null | sed -n 's/^Cache location[[:space:]]*//p'
}

backend=
endpoint=${BUILD_CACHE_R2_ENDPOINT:-}
key_id=${BUILD_CACHE_R2_ACCESS_KEY_ID:-}
secret=${BUILD_CACHE_R2_SECRET_ACCESS_KEY:-}

if [[ -n "$endpoint" && -n "$key_id" && -n "$secret" ]]; then
  # 只接受 R2 的 HTTPS 账户端点（含 eu / fedramp 管辖区）；不符合时不回显端点内容。
  if [[ ! "$endpoint" =~ ^https://[0-9a-f]{32}(\.eu|\.fedramp)?\.r2\.cloudflarestorage\.com/?$ ]]; then
    warn "R2 端点格式不是 https://<ACCOUNT_ID>.r2.cloudflarestorage.com，改用后备缓存"
  elif start_server \
    SCCACHE_BUCKET="$R2_BUCKET" SCCACHE_ENDPOINT="${endpoint%/}" SCCACHE_REGION=auto \
    SCCACHE_S3_KEY_PREFIX="$key_prefix" SCCACHE_S3_RW_MODE="$rw_mode" \
    AWS_ACCESS_KEY_ID="$key_id" AWS_SECRET_ACCESS_KEY="$secret" \
    && [[ "$(cache_location)" == s3* ]]; then
    backend="r2-$access"
    # 可写模式下 sccache 在写入探测失败时静默降为只读；据服务日志提示令牌权限不足。
    if [[ "$access" == write ]] && grep -q 'storage write check failed' "$server_log"; then
      warn "R2 写入探测失败，本次按只读使用；检查写入令牌是否具备 Object Read & Write 且作用于 $R2_BUCKET"
      backend=r2-read
    fi
  else
    warn "R2 缓存服务启动失败（端点不可达、${startup_timeout} 秒超时或凭据无效），改用后备缓存"
  fi
fi

if [[ -z "$backend" ]]; then
  if [[ -n "${ACTIONS_RESULTS_URL:-}" && -n "${ACTIONS_RUNTIME_TOKEN:-}" ]] \
    && start_server SCCACHE_GHA_ENABLED=true && [[ "$(cache_location)" == gha* ]]; then
    backend=gha
  elif start_server && [[ "$(cache_location)" == "Local disk"* ]]; then
    backend=local
  else
    kill_own_server
    fail "sccache 服务无法以任何后端启动"
  fi
fi
unset endpoint key_id secret

# sccache 不缓存增量编译产物。客户端与服务通信失败时回退为直接编译，不让缓存故障中断构建。
# 仓库 .cargo/config.toml 为本地开发限制为 2 个并行任务；CI runner 专用于本次构建，恢复为全部逻辑核。
# 必须写成数字：cargo-llvm-cov 自行解析 CARGO_BUILD_JOBS，不接受 cargo 的 "default"。
{
  echo "RUSTC_WRAPPER=$sccache_bin"
  echo "SCCACHE_SERVER_PORT=$server_port"
  echo "SCCACHE_IGNORE_SERVER_IO_ERROR=1"
  echo "CARGO_INCREMENTAL=0"
  echo "CARGO_BUILD_JOBS=$(logical_cpus)"
  echo "UC_COMPILE_CACHE_BACKEND=$backend"
} >>"${GITHUB_ENV:-/dev/null}"

printf 'Compile cache backend: %s (sccache %s, prefix %s)\n' "$backend" "$SCCACHE_VERSION" "$key_prefix"
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  printf '编译缓存后端：`%s`（sccache %s）\n' "$backend" "$SCCACHE_VERSION" >>"$GITHUB_STEP_SUMMARY"
fi
