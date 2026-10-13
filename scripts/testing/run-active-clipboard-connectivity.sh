#!/usr/bin/env bash
# 复用真实宿主与 Relay，在独立命名空间验收恢复通知和完整内容读回。
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
source scripts/build-cache/env.sh
mode=${1:-direct}
[[ "$mode" == direct || "$mode" == relay ]] || { echo 'mode must be direct or relay' >&2; exit 2; }
out="$repo/target/active-clipboard-connectivity-evidence/${UC_ACTIVE_CLIPBOARD_RUN_LABEL:-$mode}"
mkdir -p "$out"
if [[ "${2:-}" != --inside ]]; then
  # 命名空间内只运行已构建二进制；共享 target 与编译缓存始终由原构建用户使用。
  if [[ "$mode" == relay ]]; then cargo build --locked -p uc-connectivity-relay; fi
  cargo test --locked -p uc-connectivity-host --test active_clipboard_connectivity --no-run --message-format=json > "$out/build-artifacts.jsonl"
  test_binary=$(node --input-type=module - "$out/build-artifacts.jsonl" <<'NODE'
import { readFileSync } from 'node:fs'
const rows = readFileSync(process.argv[2], 'utf8').split('\n').filter(Boolean).map(line => JSON.parse(line))
const artifact = rows.find(row => row.reason === 'compiler-artifact' && row.target?.name === 'active_clipboard_connectivity' && row.executable)
if (!artifact) process.exit(1)
process.stdout.write(artifact.executable)
NODE
  )
  sha256sum "$test_binary" target/debug/uc-connectivity-host > "$out/binary-sha256.txt"
  git rev-parse HEAD > "$out/source-revision.txt"
  git diff --binary HEAD > "$out/source-diff.patch"
  # 未跟踪的新实现也属于输入，不能只用 tracked diff 表示工作区源码。
  node --input-type=module - "$out/source-tree-sha256.txt" <<'NODETREE'
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync, lstatSync, readFileSync, writeFileSync } from 'node:fs'
const paths = execFileSync('git', ['ls-files', '--cached', '--others', '--exclude-standard', '-z'], { encoding: 'utf8' }).split('\0').filter(Boolean)
const digest = createHash('sha256')
for (const path of [...new Set(paths)].sort()) {
  if (!existsSync(path) || !lstatSync(path).isFile()) continue
  digest.update(path).update('\0').update(createHash('sha256').update(readFileSync(path)).digest())
}
writeFileSync(process.argv[2], `${digest.digest('hex')}\n`)
NODETREE
  if ((EUID == 0)); then
    unshare -nm bash "$0" "$mode" --inside "$test_binary"
  elif unshare -Urn true 2>/dev/null; then
    # 无需全局 root 权限；rootless user namespace 的 UID 映射保留工件原所有者。
    unshare -Urnm bash "$0" "$mode" --inside "$test_binary"
  else
    # Ubuntu runner 可能禁止非特权 user namespace；仍只在命名空间中运行预建二进制。
    sudo -n -E unshare -nm bash "$0" "$mode" --inside "$test_binary"
  fi
  exit "$?"
fi
ip link set lo up
ip link add active0 type dummy
ip addr add 10.234.0.1/24 dev active0
ip -6 addr add fd23:234::1/64 dev active0
ip link set active0 up
ip -j address > "$out/interfaces.json"
ip -j route > "$out/ipv4-routes.json"
ip -6 -j route > "$out/ipv6-routes.json"
relay_pid=''
cleanup() {
  if [[ -n "$relay_pid" ]]; then kill -INT "$relay_pid" 2>/dev/null || true; wait "$relay_pid" || true; fi
}
trap cleanup EXIT
if [[ "$mode" == relay ]]; then
  nft add table inet active_test
  nft 'add chain inet active_test output { type filter hook output priority -50; policy accept; }'
  nft add rule inet active_test output meta l4proto udp counter drop
  "$repo/target/debug/uc-connectivity-relay" 127.0.0.1:19488 > "$out/relay.log" 2>&1 &
  relay_pid=$!
  for attempt in {1..100}; do
    [[ "$(head -1 "$out/relay.log")" != ready ]] || break
    kill -0 "$relay_pid"
    sleep 0.1
  done
  [[ "$(head -1 "$out/relay.log")" == ready ]]
  # 共用既有进程夹具的本地 relay 配置入口，不另建网络模拟框架。
  export UC_MEMBERSHIP_HISTORY_TEST_RELAY=http://127.0.0.1:19488
else
  unset UC_MEMBERSHIP_HISTORY_TEST_RELAY
fi
export UC_TEST_ARTIFACTS_DIR="$out"
: > "$out/results.tsv"
status=0
for kind in ${UC_ACTIVE_CLIPBOARD_CASES:-text image file directory}; do
  [[ "$kind" =~ ^(text|image|file|directory)$ ]] || exit 2
  started=$(date +%s%N)
  case_status=0
  timeout --kill-after=5s 120 "$3" --exact "restore_missing_${kind}_reads_back_complete_content" --nocapture > "$out/$kind.log" 2>&1 || case_status=$?
  elapsed=$(( ($(date +%s%N) - started) / 1000000 ))
  # 防止拼错过滤器产生零测试的假通过。
  if ((case_status == 0)) && ! rg -q 'test result: ok. 1 passed;' "$out/$kind.log"; then case_status=1; fi
  printf '%s\t%s\t%s\n' "$kind" "$case_status" "$elapsed" >> "$out/results.tsv"
  if ((case_status != 0)); then status=$case_status; break; fi
done
if [[ "$mode" == relay ]]; then
  nft -j list table inet active_test > "$out/blocked-udp.json"
  node --input-type=module - "$out/blocked-udp.json" <<'NODE'
import { readFileSync } from 'node:fs'
import assert from 'node:assert/strict'
const rules = JSON.parse(readFileSync(process.argv[2], 'utf8'))
const packets = rules.nftables.flatMap(row => row.rule?.expr ?? []).reduce((sum, row) => sum + (row.counter?.packets ?? 0), 0)
assert(packets > 0, 'inet UDP drop rule must actually block candidate paths')
NODE
fi
python3 - "$out" "$mode" <<'PYREPORT'
import sys
from pathlib import Path
import xml.etree.ElementTree as E
root=Path(sys.argv[1]); mode=sys.argv[2]
rows={row.split('\t')[0]: row.split('\t')[1:] for row in (root/'results.tsv').read_text().splitlines()}
suite=E.Element('testsuite', name=f'active-clipboard-{mode}', tests='4')
for kind in ['text','image','file','directory']:
 row=rows.get(kind)
 case=E.SubElement(suite,'testcase', name=f'restore-{kind}', time=str(int(row[1])/1000 if row else 0))
 if not row:E.SubElement(case,'skipped', message='not selected or previous case failed')
 elif row[0]!='0': E.SubElement(case,'failure',message=f'process exited {row[0]}').text=(root/f'{kind}.log').read_text()
E.ElementTree(suite).write(root/'junit.xml', encoding='utf-8', xml_declaration=True)
PYREPORT
exit "$status"
