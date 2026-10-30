#!/usr/bin/env bash
# 在独立网络命名空间封锁 UDP，运行已构建的五进程分支恢复测试。
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
if [[ "${1:-}" != --inside ]]; then
  [[ $# == 2 && -x "$1" && ( "$2" == current || "$2" == issuer || "$2" == recipient ) ]] || {
    printf '用法: %s <已构建测试二进制> <current|issuer|recipient>\n' "$0" >&2
    exit 2
  }
  test_binary=$(realpath "$1")
  if [[ "$2" != current ]]; then
    baseline="$repo/target/branch-recovery-frozen-main"
    [[ "$(cat "$baseline/engine-revision.txt")" == 5cbca8fc24a278fa7b01ae611bf4b925914bef13 ]]
    sha256sum --check "$baseline/binary-sha256.txt"
  fi
  out="$repo/target/branch-recovery-relay-$2"
  mkdir -p "$out"
  exec unshare -Urn bash "$0" --inside "$test_binary" "$2" "$out"
fi
test_binary=$2
side=$3
out=$4
ip link set lo up
ip link add recovery0 type dummy
ip addr add 10.234.0.1/24 dev recovery0
ip link set recovery0 up
nft add table inet recovery_test
nft 'add chain inet recovery_test output { type filter hook output priority -50; policy accept; }'
nft add rule inet recovery_test output meta l4proto udp counter drop
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
export UC_BRANCH_RECOVERY_EXPECT_PATH=relay
if [[ "$side" == current ]]; then
  unset UC_BRANCH_RECOVERY_OLD_HOST UC_BRANCH_RECOVERY_OLD_SIDE
else
  export UC_BRANCH_RECOVERY_OLD_HOST="$repo/target/branch-recovery-frozen-main/uc-connectivity-host"
  export UC_BRANCH_RECOVERY_OLD_SIDE="$side"
fi
status=0
timeout --kill-after=5s 600 "$test_binary" --exact separate_processes_restore_selected_membership_branch --nocapture > "$out/test.log" 2>&1 || status=$?
nft -j list table inet recovery_test > "$out/blocked-udp.json"
node --input-type=module - "$out/blocked-udp.json" <<'NODE'
import { readFileSync } from 'node:fs'
import assert from 'node:assert/strict'
const rules = JSON.parse(readFileSync(process.argv[2], 'utf8'))
const packets = rules.nftables.flatMap(row => row.rule?.expr ?? []).reduce((sum, row) => sum + (row.counter?.packets ?? 0), 0)
assert(packets > 0, 'UDP drop rule must actually block candidate paths')
process.stdout.write(`已封锁 UDP 包: ${packets}\n`)
NODE
cat "$out/test.log"
exit "$status"
