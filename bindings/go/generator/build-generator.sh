#!/usr/bin/env bash
# 从固定 revision 取得 uniffi-bindgen-go，核对上游 Cargo.lock，应用名称卫生补丁并构建。
#
#   build-generator.sh <外置可再生构建目录>
#
# 目录必须不存在或为空；脚本不删除它，调用方在确认无活动构建进程后回收。输出：
# <目录>/bin/uniffi-bindgen-go 与 <目录>/generator-build.json（来源与哈希）。
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$here/../../.." && pwd)"
work="${1:?usage: build-generator.sh <external build dir>}"
source "$here/PIN.env"
source "$repo_root/scripts/build-cache/env.sh"

if [[ -e "$work" && -n "$(ls -A "$work")" ]]; then
  echo "build dir must be empty: $work" >&2
  exit 2
fi
mkdir -p "$work"
work="$(cd "$work" && pwd)"

sha256_of() { shasum -a 256 "$1" | cut -d' ' -f1; }

[[ "$(sha256_of "$here/$UNIFFI_BINDGEN_GO_PATCH")" == "$UNIFFI_BINDGEN_GO_PATCH_SHA256" ]] \
  || { echo "patch hash differs from PIN.env" >&2; exit 1; }

git clone --quiet "$UNIFFI_BINDGEN_GO_URL" "$work/src"
git -C "$work/src" checkout --quiet "$UNIFFI_BINDGEN_GO_REVISION"
[[ "$(git -C "$work/src" rev-parse HEAD)" == "$UNIFFI_BINDGEN_GO_REVISION" ]]
[[ "$(sha256_of "$work/src/Cargo.lock")" == "$UNIFFI_BINDGEN_GO_LOCK_SHA256" ]] \
  || { echo "upstream Cargo.lock differs from PIN.env" >&2; exit 1; }
git -C "$work/src" apply --check "$here/$UNIFFI_BINDGEN_GO_PATCH"
git -C "$work/src" apply "$here/$UNIFFI_BINDGEN_GO_PATCH"

# 上游自带 rust-toolchain.toml（1.87）；统一使用本仓固定工具链，记录在构建清单中。
toolchain_version="$(cd "$repo_root" && rustc --version)"
(cd "$repo_root" && cargo build --manifest-path "$work/src/Cargo.toml" -p uniffi-bindgen-go \
  --release --locked --target-dir "$work/target")

mkdir -p "$work/bin"
cp "$work/target/release/uniffi-bindgen-go" "$work/bin/uniffi-bindgen-go"
cp "$work/src/LICENSE" "$work/LICENSE"
printf '{\n  "revision": "%s",\n  "tag": "%s",\n  "lock_sha256": "%s",\n  "patch": "%s",\n  "patch_sha256": "%s",\n  "binary_sha256": "%s",\n  "rustc": "%s"\n}\n' \
  "$UNIFFI_BINDGEN_GO_REVISION" "$UNIFFI_BINDGEN_GO_TAG" "$UNIFFI_BINDGEN_GO_LOCK_SHA256" \
  "$UNIFFI_BINDGEN_GO_PATCH" "$UNIFFI_BINDGEN_GO_PATCH_SHA256" \
  "$(sha256_of "$work/bin/uniffi-bindgen-go")" "$toolchain_version" > "$work/generator-build.json"
echo "generator ready: $work/bin/uniffi-bindgen-go"
