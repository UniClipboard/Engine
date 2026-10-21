#!/usr/bin/env bash
# 在未经修改的 rc.15 库上编译独立验证 example；仅添加 example 源码，不改 Engine／SDK。
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
rev=f6f305d9689e4e79e7ab6d0e4921061f9416e4a6
root="$repo/target/blob-connectivity-evidence"
mkdir -p "$root" "${XDG_CACHE_HOME:-$HOME/.cache}"
source_dir=$(mktemp -d "${XDG_CACHE_HOME:-$HOME/.cache}/uc-blob-vector-source.XXXXXX")
cleanup() {
  # 正常构建已等待 Cargo 退出；若外部中断留下工作进程，则保留现场供本次执行者先终止再回收。
  if pgrep -f -- "$source_dir" >/dev/null; then
    echo 'frozen vector build still has active processes' >&2
    return 1
  fi
  rm -rf -- "$source_dir"
}
trap cleanup EXIT
git archive "$rev" | tar -x -C "$source_dir"
mkdir -p "$source_dir/crates/uc-infra/examples/support"
cp crates/uc-infra-p2p/tests/support/blob_protocol_vectors.rs "$source_dir/crates/uc-infra/examples/support/blob_protocol_vectors.rs"
cat > "$source_dir/crates/uc-infra/examples/emit_blob_vectors.rs" <<'RUST'
#[path = "support/blob_protocol_vectors.rs"]
mod vectors;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let target = std::env::var_os("UC_BLOB_VECTOR_OUTPUT").expect("vector output");
    std::fs::write(target, serde_json::to_vec_pretty(&vectors::generate().await?)?)?;
    Ok(())
}
RUST
CARGO_TARGET_DIR="$repo/target" CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 \
UC_BLOB_VECTOR_OUTPUT="$root/protocol-vectors-rc15.json" \
  just cargo run --locked --offline --manifest-path "$source_dir/Cargo.toml" -p uc-infra --example emit_blob_vectors
mkdir -p crates/uc-infra-p2p/tests/fixtures
cp "$root/protocol-vectors-rc15.json" crates/uc-infra-p2p/tests/fixtures/blob-connectivity-rc15-vectors.json
sha256sum "$root/protocol-vectors-rc15.json" > "$root/protocol-vectors-rc15.sha256"
printf '%s\n' "$rev" > "$root/protocol-vectors-rc15-source.txt"
