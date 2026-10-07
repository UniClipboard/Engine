#!/usr/bin/env bash
# 把 cargo 产出的 uc-engine-uniffi 动态库整理成 Go 宿主可链接的交付目录，并写入来源清单。
#
#   stage-native.sh <profile: dev|release> <输出目录>
#
# 从仓库根目录的源码构建（`just cargo build -p uc-engine-uniffi --profile <profile> --locked`）后运行。
# 输出目录必须不存在或为空，包含：
#   libuc_engine_uniffi.<dylib|so>   install_name/soname 已改为 @rpath（macOS 重新 ad-hoc 签名）
#   native-manifest.json              来源、构建参数与产物 sha256；Go 侧 native.Verify 逐项核对
#
# 工作区有未提交改动时拒绝运行：清单里的 revision 必须能唯一还原源码。
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$here/../../.." && pwd)"
profile="${1:?usage: stage-native.sh <dev|release> <out dir>}"
out="${2:?usage: stage-native.sh <dev|release> <out dir>}"
source "$here/../generator/PIN.env"

case "$profile" in
  dev) profile_dir=debug ;;
  release) profile_dir=release ;;
  *) echo "profile must be dev or release" >&2; exit 2 ;;
esac
if [[ -e "$out" && -n "$(ls -A "$out")" ]]; then
  echo "output dir must be empty: $out" >&2
  exit 2
fi

cd "$repo_root"
if [[ -n "$(git status --porcelain)" ]]; then
  echo "working tree is not clean; refusing to record provenance" >&2
  exit 1
fi

case "$(uname -s)" in
  Darwin) ext=dylib ;;
  Linux) ext=so ;;
  *) echo "unsupported host: $(uname -s)" >&2; exit 1 ;;
esac
target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"
source_lib="$target_dir/$profile_dir/libuc_engine_uniffi.$ext"
[[ -f "$source_lib" ]] || { echo "missing $source_lib; build first" >&2; exit 1; }

sha256_of() { shasum -a 256 "$1" | cut -d' ' -f1; }
mkdir -p "$out"
staged="$out/libuc_engine_uniffi.$ext"
cp "$source_lib" "$staged"
raw_sha256="$(sha256_of "$staged")"
if [[ "$ext" == dylib ]]; then
  install_name_tool -id "@rpath/libuc_engine_uniffi.dylib" "$staged"
  codesign --force --sign - "$staged"
else
  patchelf --set-soname libuc_engine_uniffi.so "$staged"
fi

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
uniffi_version="$(sed -n 's/^uniffi = "=\(.*\)"/\1/p' bindings/uc-engine-uniffi/Cargo.toml)"
revision="$(git rev-parse HEAD)"

# 库必须由当前提交构建：构建时嵌入的完整 revision 出现在产物中，否则可能是旧提交或其他树的库。
if ! LC_ALL=C grep -a -F -q -- "$revision" "$staged"; then
  echo "library does not embed the current HEAD; rebuild with build-native.sh at this commit" >&2
  exit 1
fi

# 全部值经环境变量交给 python，避免命令输出中的引号或 $ 破坏脚本。
UC_MANIFEST_OUT="$out/native-manifest.json" \
UC_REVISION="$revision" \
UC_VERSION="$version" \
UC_LOCK_SHA256="$(sha256_of Cargo.lock)" \
UC_UNIFFI_VERSION="$uniffi_version" \
UC_TARGET="$(rustc -vV | sed -n 's/^host: //p')" \
UC_PROFILE="$profile" \
UC_RUSTC="$(rustc --version)" \
UC_CARGO="$(cargo --version | tail -1)" \
UC_GENERATOR_REVISION="$UNIFFI_BINDGEN_GO_REVISION" \
UC_GENERATOR_TAG="$UNIFFI_BINDGEN_GO_TAG" \
UC_GENERATOR_LOCK_SHA256="$UNIFFI_BINDGEN_GO_LOCK_SHA256" \
UC_GENERATOR_PATCH_SHA256="$UNIFFI_BINDGEN_GO_PATCH_SHA256" \
UC_GENERATED_SHA256="$(cd bindings/go/uc_engine_uniffi && shasum -a 256 uc_engine_uniffi.go uc_engine_uniffi.h | shasum -a 256 | cut -d' ' -f1)" \
UC_LIBRARY_FILE="libuc_engine_uniffi.$ext" \
UC_LIBRARY_SHA256="$(sha256_of "$staged")" \
UC_LIBRARY_SIZE="$(stat -f%z "$staged" 2>/dev/null || stat -c%s "$staged")" \
UC_RAW_SHA256="$raw_sha256" \
python3 - <<'PY'
import json, os
e = os.environ
manifest = {
    "schema": 1,
    "engine_revision": e["UC_REVISION"],
    "engine_version": e["UC_VERSION"],
    "cargo_lock_sha256": e["UC_LOCK_SHA256"],
    "uniffi_version": e["UC_UNIFFI_VERSION"],
    "target": e["UC_TARGET"],
    "profile": e["UC_PROFILE"],
    "features": [],
    "rustc": e["UC_RUSTC"],
    "cargo": e["UC_CARGO"],
    "generator": {
        "revision": e["UC_GENERATOR_REVISION"],
        "tag": e["UC_GENERATOR_TAG"],
        "lock_sha256": e["UC_GENERATOR_LOCK_SHA256"],
        "patch_sha256": e["UC_GENERATOR_PATCH_SHA256"],
    },
    "generated_sources_sha256": e["UC_GENERATED_SHA256"],
    "library": {
        "file": e["UC_LIBRARY_FILE"],
        "sha256": e["UC_LIBRARY_SHA256"],
        "size": int(e["UC_LIBRARY_SIZE"]),
        "cargo_artifact_sha256": e["UC_RAW_SHA256"],
    },
}
with open(e["UC_MANIFEST_OUT"], "w") as handle:
    handle.write(json.dumps(manifest, indent=2) + "\n")
PY
echo "staged $staged"
