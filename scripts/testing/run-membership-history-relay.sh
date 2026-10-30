#!/usr/bin/env bash
# 在独立网络命名空间封锁全部 UDP，验证真实三进程成员历史跨轮复用。
set -euo pipefail
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/../build-cache/env.sh"
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
out="${UC_MEMBERSHIP_HISTORY_RELAY_EVIDENCE_ROOT:-$repo/target/membership-history-relay}"
mkdir -p "$out"
if [[ "${1:-}" != --inside ]]; then
  # 后续同样按全部工作区目标取得测试二进制，普通relay不能先缩回单包特性图。
  cargo build --locked --workspace --all-targets
  # 全部编译在普通构建用户下完成；命名空间内只执行已构建二进制，不切换缓存用户。
  cargo test --workspace --all-targets --locked --no-run --message-format=json > "$out/build-artifacts.jsonl"
  test_binary=$(node --input-type=module - "$out/build-artifacts.jsonl" <<'NODE'
import { readFileSync } from 'node:fs'
const rows = readFileSync(process.argv[2], 'utf8').split('\n').filter(Boolean).map(line => JSON.parse(line))
const artifact = rows.find(row => row.reason === 'compiler-artifact' && row.target?.name === 'space_device_update_recovery' && row.executable)
if (!artifact) process.exit(1)
process.stdout.write(artifact.executable)
NODE
  )
  runner=(unshare -nm bash "$0" --inside "$test_binary")
  if ((EUID != 0)); then runner=(sudo -- "${runner[@]}"); fi
  status=0
  "${runner[@]}" || status=$?
  if ((EUID != 0)); then sudo chown -R -- "$(id -u):$(id -g)" "$out"; fi
  exit "$status"
fi
ip link set lo up
# 提供与现有隔离 harness 一致的非 loopback 接口，实际 UDP 仍全部封锁。
ip link add history0 type dummy
ip addr add 10.234.0.1/24 dev history0
ip link set history0 up
nft add table inet history_test
nft 'add chain inet history_test output { type filter hook output priority -50; policy accept; }'
nft add rule inet history_test output meta l4proto udp counter drop
"$repo/target/debug/uc-connectivity-relay" 127.0.0.1:19487 > "$out/relay.log" 2>&1 &
relay_pid=$!
cleanup() {
  kill -INT "$relay_pid" 2>/dev/null || true
  wait "$relay_pid" || true
}
trap cleanup EXIT
for attempt in {1..100}; do
  if [[ "$(head -1 "$out/relay.log")" == ready ]]; then break; fi
  kill -0 "$relay_pid"
  sleep 0.1
done
[[ "$(head -1 "$out/relay.log")" == ready ]]
export UC_MEMBERSHIP_HISTORY_TEST_RELAY=http://127.0.0.1:19487
export UC_TEST_ARTIFACTS_DIR="$out"
timeout --kill-after=5s 120 "$2" --exact separate_process_membership_history_reuses_connections_across_rounds --nocapture
nft -j list table inet history_test > "$out/blocked-udp.json"
node --input-type=module - "$out/blocked-udp.json" <<'NODE'
import { readFileSync } from 'node:fs'
import assert from 'node:assert/strict'
const rules = JSON.parse(readFileSync(process.argv[2], 'utf8'))
const packets = rules.nftables.flatMap(row => row.rule?.expr ?? []).reduce((sum, row) => sum + (row.counter?.packets ?? 0), 0)
assert(packets > 0, 'UDP drop rule must actually block candidate paths')
NODE
