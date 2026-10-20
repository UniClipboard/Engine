#!/usr/bin/env bash
# 构建开工时冻结的 main；同一宿主源码，旧 Engine 库不打补丁，共享 target 串行执行。
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
source scripts/build-cache/env.sh
rev=b6fa0ce6b2efd1c1764e76c0a76537bdeead4e1b
root="$repo/target/peer-reachability-connectivity-evidence/frozen-main"
mkdir -p "$root" "${XDG_CACHE_HOME:-$HOME/.cache}"
source_dir=$(mktemp -d "${XDG_CACHE_HOME:-$HOME/.cache}/uc-peer-reachability-source.XXXXXX")
backup="$root/current-host-backup-$$"
cleanup() {
  if [[ -f "$backup" ]]; then
    cp "$backup" "$repo/target/debug/uc-connectivity-host"
    rm -f -- "$backup"
  fi
  rm -rf -- "$source_dir"
}
trap cleanup EXIT
if [[ -f target/debug/uc-connectivity-host ]]; then cp target/debug/uc-connectivity-host "$backup"; fi
git archive "$rev" | tar -x -C "$source_dir"
# 与既有固定历史门禁相同：时间戳只用于 fresh 判定，不修改冻结源码字节。
find "$source_dir" -type f -name '*.rs' -exec touch {} +
# 宿主只使用已冻结版本支持的公开操作，源码与当前驱动一致。
cp tests/hosts/connectivity/src/*.rs "$source_dir/tests/hosts/connectivity/src/"
cp tests/hosts/connectivity/Cargo.toml "$source_dir/tests/hosts/connectivity/Cargo.toml"
# 仅为同一宿主补 URL 解析依赖；冻结旧 Engine 的依赖版本不变。
python3 - "$source_dir/Cargo.lock" <<'PYLOCK'
from pathlib import Path
import sys
p=Path(sys.argv[1]); text=p.read_text()
a=text.index('name = "uc-connectivity-host"'); b=text.index('\n[[package]]', a)
block=text[a:b]
if ' "url",' not in block: block=block.replace(' "uc-engine",', ' "uc-engine",\n "url",')
p.write_text(text[:a]+block+text[b:])
PYLOCK
sha256sum "$source_dir/tests/hosts/connectivity/Cargo.toml" "$source_dir/Cargo.lock" > "$root/driver-build-inputs-sha256.txt"
sha256sum tests/hosts/connectivity/src/*.rs > "$root/driver-sha256.txt"
sha256sum tests/hosts/connectivity/tests/active_clipboard_connectivity.rs tests/hosts/connectivity/tests/common/process.rs tests/hosts/connectivity/tests/fixtures/* > "$root/matrix-driver-sha256.txt"
printf '%s\n' 'features=current-engine,uc-engine/dev-tools' 'profile=dev,debug=0' > "$root/build-profile.txt"
printf '%s\n' "$rev" > "$root/engine-revision.txt"
git archive "$rev" | sha256sum > "$root/engine-archive-sha256.txt"
CARGO_TARGET_DIR="$repo/target" CARGO_PROFILE_DEV_DEBUG=0 UC_ENGINE_SOURCE_COMMIT="$rev" UC_ENGINE_SOURCE_STATE=clean \
UC_HOST_ENGINE_REV="$rev" UC_HOST_ANCHOR=peer-reachability-baseline \
  cargo build --locked --manifest-path "$source_dir/Cargo.toml" -p uc-connectivity-host
cp target/debug/uc-connectivity-host "$root/uc-connectivity-host"
sha256sum "$root/uc-connectivity-host" > "$root/binary-sha256.txt"
if [[ -f "$backup" ]]; then cp "$backup" target/debug/uc-connectivity-host; fi
