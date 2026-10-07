#!/usr/bin/env bash
# 构建 Go 宿主使用的 uc-engine-uniffi 动态库（当前主机目标）。
#
#   build-native.sh <dev|release>
#
# release 与移动包使用同一发布配置（panic=abort、LTO、符号剥离）并重映射构建机路径，
# 构建后断言产物不含本机路径。随后用 stage-native.sh 整理交付目录并写入来源清单。
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
profile="${1:?usage: build-native.sh <dev|release>}"
# 当前脚本与嵌套 Cargo 调用统一经由 MBX。
source "$here/../../../scripts/build-cache/env.sh"
REPO_ROOT="$(cd "$here/../../.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
BUILD_PROFILE="$profile"
case "$profile" in
  dev) profile_dir=debug ;;
  release) profile_dir=release ;;
  *) echo "profile must be dev or release" >&2; exit 2 ;;
esac

cd "$REPO_ROOT"
source "$REPO_ROOT/bindings/uc-engine-uniffi/scripts/release-path-remap.sh"
export CARGO_TARGET_DIR="$TARGET_DIR"
with_release_path_remap cargo build -p uc-engine-uniffi --profile "$profile" --lib --locked

case "$(uname -s)" in
  Darwin) library="$TARGET_DIR/$profile_dir/libuc_engine_uniffi.dylib" ;;
  Linux) library="$TARGET_DIR/$profile_dir/libuc_engine_uniffi.so" ;;
  *) echo "unsupported host: $(uname -s)" >&2; exit 1 ;;
esac
[[ -f "$library" ]] || { echo "missing $library" >&2; exit 1; }
if ! (verify_release_paths "$library") 2>/dev/null; then
  # 构建机路径不是秘密；列出命中的前缀与样例，便于定位新的路径来源。
  for prefix in ${RELEASE_PATH_REMAP_PREFIXES[@]+"${RELEASE_PATH_REMAP_PREFIXES[@]}"}; do
    if LC_ALL=C grep -a -F -q -- "$prefix" "$library"; then
      echo "leaked prefix: $prefix" >&2
      LC_ALL=C strings -a "$library" | grep -F -- "$prefix" | sort | uniq -c | sort -rn | head -5 >&2
    fi
  done
  exit 1
fi
echo "built $library"
