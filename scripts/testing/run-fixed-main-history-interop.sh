#!/usr/bin/env bash
# 固定未修改旧源码，使用同一 target 串行构建并验收双向成员历史兼容。
set -euo pipefail
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/../build-cache/env.sh"
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
rev=edf0a296118c071a97d4c14ae27b09a61931bbae
root="$repo/target/fixed-main-history"
mkdir -p "${XDG_CACHE_HOME:-$HOME/.cache}"
source_dir=$(mktemp -d "${XDG_CACHE_HOME:-$HOME/.cache}/uc-history-source.XXXXXX")
current_backup="$root/current-host-backup-$$"
cleanup() {
  if [[ -f "$current_backup" ]]; then
    cp "$current_backup" "$repo/target/debug/uc-connectivity-host"
    rm -f -- "$current_backup"
  fi
  rm -rf -- "$source_dir"
}
trap cleanup EXIT
mkdir -p "$root"
if [[ -f "$repo/target/debug/uc-connectivity-host" ]]; then
  cp "$repo/target/debug/uc-connectivity-host" "$current_backup"
fi
if ! git cat-file -e "$rev^{commit}"; then git fetch --no-tags origin "$rev"; fi
git archive "$rev" | tar -x -C "$source_dir"
# 修改文件时间戳不修改源码，避免共享 target 将其他旧快照误判 fresh。
find "$source_dir" -type f -name '*.rs' -exec touch {} +
# 通过不同 debug profile 指纹避免旧/新相同 crate 路径的 mtime 误判；编译缓存继续启用。
CARGO_TARGET_DIR="$repo/target" CARGO_PROFILE_DEV_DEBUG=0 \
UC_ENGINE_SOURCE_COMMIT="$rev" UC_ENGINE_SOURCE_STATE=clean \
  cargo build --locked --manifest-path "$source_dir/Cargo.toml" -p uc-connectivity-host
cp "$repo/target/debug/uc-connectivity-host" "$root/uc-connectivity-host"
if [[ -f "$current_backup" ]]; then cp "$current_backup" "$repo/target/debug/uc-connectivity-host"; fi
printf '%s\n' "$rev" > "$root/source-revision.txt"
shasum -a 256 "$root/uc-connectivity-host" > "$root/binary-sha256.txt"
UC_GROUP_UPDATE_OLD_HOST="$root/uc-connectivity-host" \
  cargo nextest run --profile ci --locked -p uc-connectivity-host \
  --test space_device_update_recovery --run-ignored only -E 'test(fixed_main_group_update)'
