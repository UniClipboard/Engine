#!/usr/bin/env bash
# 统一 Rust 编译缓存负责人：当前进程树经由 MBX，不修改全局 Cargo 配置。
# 取舍与实测依据见 docs/design-docs/decisions/028-optional-mbx-build-cache.md。
#
#   mbx.sh <cargo 参数...>      例：mbx.sh check --workspace --all-targets --locked
#   mbx.sh --cargo <参数...>    保留原生 Cargo shim 的命令/工具查询语义
#   mbx.sh --exec <命令...>     让命令内部的 cargo 调用也经由 mbx（如 scripts/testing/run-test-group.sh）
#   mbx.sh --mbx <子命令...>    以同一配置运行 mbx 自身命令（如 cache stats、gc --dry-run）
#   mbx.sh --install            只下载并校验固定版本
#   mbx.sh --r2 <上述参数...>    使用 R2 分布式动作缓存（凭据由调用方提供）
#
# 缓存位置由调用方环境的 MBX_CACHE_DIR 决定（未设置时使用 mbx 平台默认位置）；仓库不写入机器路径。
set -euo pipefail

# 同时支持 shell Cargo 与原生 Node/Python 子进程；配置只在构建进程树加载。
if [[ "${1:-}" != --install ]]; then
  if [[ -n "${UC_ENGINE_MBX_ENV_FILE:-}" ]]; then
    # shellcheck source=/dev/null
    source "$UC_ENGINE_MBX_ENV_FILE"
  fi
  if [[ -n "${UC_ENGINE_MBX_STATS_DIR:-}" && -z "${MBX_STATS_REPORT:-}" ]]; then
    mkdir -p "$UC_ENGINE_MBX_STATS_DIR"
    report_dir=$(mktemp -d "$UC_ENGINE_MBX_STATS_DIR/session.XXXXXX")
    export MBX_STATS_REPORT="$report_dir/stats.json"
  fi
  if [[ "${1:-}" != --r2 && -n "${BUILD_CACHE_R2_ENDPOINT:-}" ]]; then
    set -- --r2 "$@"
  fi
fi

# 仅当前命令配置原生 S3 后端；本地与 PR 的写入限制继续由 mbx 和桶级权限决定。
if [[ "${1:-}" == --r2 ]]; then
  shift
  for required in BUILD_CACHE_R2_ENDPOINT BUILD_CACHE_R2_ACCESS_KEY_ID BUILD_CACHE_R2_SECRET_ACCESS_KEY; do
    if [[ -z "${!required:-}" ]]; then
      printf 'mbx: R2 缺少 %s；未启动构建\n' "$required" >&2
      exit 1
    fi
  done
  export MBX_REMOTE_URL=s3://uniclipboard-build-cache
  export MBX_REMOTE_NAMESPACE=engine/mbx
  export MBX_REMOTE_S3_ENDPOINT="$BUILD_CACHE_R2_ENDPOINT"
  export MBX_REMOTE_S3_REGION=auto
  export MBX_REMOTE_MODE="${MBX_REMOTE_MODE:-read-only}"
  export AWS_ACCESS_KEY_ID="$BUILD_CACHE_R2_ACCESS_KEY_ID"
  export AWS_SECRET_ACCESS_KEY="$BUILD_CACHE_R2_SECRET_ACCESS_KEY"
  # R2 固定访问密钥不用会话令牌，不能继承另一套 AWS 身份的令牌。
  unset AWS_SESSION_TOKEN MBX_REMOTE_TOKEN MBX_REMOTE_TOKEN_FILE MBX_REMOTE_OIDC_AUDIENCE
fi

MBX_VERSION=1.18.0

fail() {
  printf 'mbx: %s\n' "$1" >&2
  exit 1
}

# 输出 “<target triple> <发布包 sha256>”；sha256 取自 v1.18.0 的 SHA256SUMS，并经 GitHub 构建来源证明校验。
# 用 case 而非关联数组，兼容 macOS 自带的 bash 3.2。
pinned_release() {
  case "$(uname -s)-$(uname -m)" in
    MINGW* | MSYS* | CYGWIN*)
      # 查询原生系统架构，避免 ARM64 Windows 上 x64 Git Bash 误选包。
      case "$(powershell.exe -NoProfile -NonInteractive -Command 'Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Architecture' | tr -d '\r\n')" in
        9) echo x86_64-pc-windows-msvc 6f3e761a18946b7d17ab54d4021773144ff7b9203b99b2df1ccddbebed6e1828 ;;
        12) echo aarch64-pc-windows-msvc 1208ec2f61af53d76c892c1ad4603294f49e62ad4c50ad1f8401aa5f3567b1b8 ;;
        *) fail "无法识别 Windows 原生架构" ;;
      esac ;;

    Darwin-arm64) echo aarch64-apple-darwin 70ab23933b3745205125174120bd930c0116c1ef3ac38786122e5651636b9aea ;;
    Linux-x86_64) echo x86_64-unknown-linux-gnu 92833c87261ea0c65fee52898ea67605b152b516fc0a3d8dee5a276601a8924c ;;
    Linux-aarch64 | Linux-arm64)
      echo aarch64-unknown-linux-gnu ab0f4af3e98295bc35f448d453cfe15931bb02342d48163b2ede94583f937cdc ;;
    *) fail "当前平台没有固定的 mbx 版本：$(uname -s)-$(uname -m)" ;;
  esac
}

# 从 stdin 计算，避免 GNU sha256sum 为含反斜杠的 Windows 文件名添加转义前缀。
sha256_of() {
  if command -v sha256sum >/dev/null; then
    sha256sum < "$1" | cut -d' ' -f1
  else
    shasum -a 256 < "$1" | cut -d' ' -f1
  fi
}

# 固定版本安装在用户工具缓存中，多个 worktree 共用一份；已存在时只做存在性检查。
install_mbx() {
  local triple expected root archive staging binary extension
  binary=mbx
  extension=tar.gz
  read -r triple expected <<<"$(pinned_release)"
  [[ -n "${expected:-}" ]] || exit 1
  if [[ "$triple" == *-pc-windows-msvc ]]; then
    binary=mbx.exe
    extension=zip
  fi
  root="${UC_TOOLS_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/uniclipboard-engine/tools}/mbx/$MBX_VERSION/$triple"
  if [[ ! -x "$root/$binary" ]]; then
    mkdir -p "$root"
    staging=$(mktemp -d "$root.staging.XXXXXX")
    archive="$staging/mbx-$triple.$extension"
    curl -fsSL --retry 3 -o "$archive" \
      "https://github.com/jdx/mr-boxington/releases/download/v$MBX_VERSION/mbx-$triple.$extension"
    if [[ "$(sha256_of "$archive")" != "$expected" ]]; then
      rm -rf "$staging"
      fail "下载的 mbx $MBX_VERSION 校验失败，已停止"
    fi
    if [[ "$extension" == zip ]]; then
      # shellcheck disable=SC2016 # PowerShell 在子进程中展开环境变量。
      UC_MBX_ARCHIVE="$(cygpath -w "$archive")" UC_MBX_STAGING="$(cygpath -w "$staging")" \
        powershell.exe -NoProfile -NonInteractive -Command \
        '$ErrorActionPreference = "Stop"; Expand-Archive -LiteralPath $env:UC_MBX_ARCHIVE -DestinationPath $env:UC_MBX_STAGING -Force'
    else
      tar -xzf "$archive" -C "$staging" mbx
    fi
    mv "$staging/$binary" "$root/$binary"
    rm -rf "$staging"
  fi
  mkdir -p "$root/shim"
  if [[ "$binary" == mbx.exe ]]; then
    # 原生子进程需要 cargo.exe；复制不要求管理员权限或 Developer Mode。
    [[ -f "$root/shim/cargo.exe" ]] || cp "$root/$binary" "$root/shim/cargo.exe"
    MBX_CARGO="$root/shim/cargo.exe"
  else
    ln -sfn "$root/$binary" "$root/shim/cargo"
    MBX_CARGO="$root/shim/cargo"
  fi
  MBX_BIN="$root/$binary"
  MBX_ROOT=$root
}

install_mbx

if [[ -n "${MBX_CACHE_DIR:-}" && ! -d "$(dirname "$MBX_CACHE_DIR")" ]]; then
  # 例如外置盘未挂载：不自动建出上级目录，避免缓存悄悄落到其他磁盘。
  fail "MBX_CACHE_DIR 的上级目录不存在：$(dirname "$MBX_CACHE_DIR")"
fi

# mbx 遇到其他 RUSTC_WRAPPER（如 sccache）会原样转交而不缓存，因此仅在本命令内移除。
unset RUSTC_WRAPPER CARGO_BUILD_RUSTC_WRAPPER
# 不接管、移动或回收 target 目录；target 位置仍由 Cargo 及本机既有配置决定。
export MBX_TARGET_VIEWS=0 MBX_TARGET_SEED=0
# 默认的 learned incremental 在 Engine 上使改动后重建变慢，并使缓存每次增长约 1.8 GiB。
export MBX_LEARNED_INCREMENTAL=0
export CARGO_INCREMENTAL=0
export MBX_GC_MAX_SIZE="${MBX_GC_MAX_SIZE:-20GiB}"
export MBX_DISPLAY="${MBX_DISPLAY:-plain}"
# 统一入口在本地默认四个编译任务；CI 由 runner 装配数字并行度。
# 保留调用方的环境值，Cargo 命令行 -j 仍具有最高优先级；实测与资源边界见本地构建指南。
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
# CI 的 shell Cargo 入口不能留在原生 shim 后面，否则真实 Cargo 解析会再次进入本包装器。
script_bin="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/bin"
remaining=""
IFS=: read -r -a entries <<< "$PATH"
for entry in "${entries[@]}"; do
  if [[ "$entry" != "$script_bin" && "$entry" != "${UC_ENGINE_MBX_BIN:-}" ]]; then
    remaining="${remaining:+$remaining:}$entry"
  fi
done
export PATH="$remaining"
# cargo -> mbx 垫片放在 PATH 最前，mbx 再调用其后的 cargo。
# Windows cargo.exe 按 PATH 查找 mbx.exe，必须先于用户另一版本。
export PATH="$MBX_ROOT/shim:$MBX_ROOT:$PATH"

case "${1:-}" in
  --install) printf 'mbx %s: %s\n' "$MBX_VERSION" "$MBX_BIN" ;;
  --cargo) shift; exec "$MBX_CARGO" "$@" ;;
  --exec) shift; exec "$@" ;;
  --mbx) shift; exec "$MBX_BIN" "$@" ;;
  "" | -h | --help) sed -n '2,12p' "$0" ;;
  *) exec "$MBX_BIN" "$@" ;;
esac
