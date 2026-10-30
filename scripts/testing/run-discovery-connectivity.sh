#!/usr/bin/env bash
# Cloud保持开启；隔离公网并以旧双栈端口阻断和mDNS开关建立真实对照。
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
out="$repo/target/discovery-connectivity-evidence/${UC_DISCOVERY_RUN_LABEL:-cloud-outage}"
mkdir -p "$out"
if [[ "${1:-}" != --inside ]]; then
  targets=(-p uc-connectivity-host --test discovery_connectivity)
  case "${UC_TEST_BUILD_SCOPE:-group}" in
    group) ;;
    workspace) targets=(--workspace --all-targets) ;;
    *) echo 'UC_TEST_BUILD_SCOPE must be group or workspace' >&2; exit 2 ;;
  esac
  just cargo test --locked "${targets[@]}" --no-run --message-format=json > "$out/build-artifacts.jsonl"
  binary=$(python3 - "$out/build-artifacts.jsonl" <<'PY'
import json,sys
rows=[json.loads(line) for line in open(sys.argv[1]) if line.startswith('{')]
print(next(row['executable'] for row in rows if row.get('target',{}).get('name')=='discovery_connectivity' and row.get('executable')))
PY
)
  sha256sum "$binary" target/debug/uc-connectivity-host > "$out/binary-sha256.txt"
  git diff --binary HEAD > "$out/source-diff.patch"
  python3 - "$out/source-inputs.json" <<'PYINPUTS'
import hashlib,json,pathlib,subprocess,sys
paths=subprocess.check_output(['git','ls-files','--cached','--others','--exclude-standard','-z']).decode().split('\0')
inputs={p:hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest() for p in sorted(set(paths)) if p and pathlib.Path(p).is_file()}
pathlib.Path(sys.argv[1]).write_text(json.dumps(inputs,indent=2)+'\n')
PYINPUTS
  if ((EUID == 0)); then
    unshare -nm bash "$0" --inside "$binary"
  elif unshare -Urn true 2>/dev/null; then
    unshare -Urnm bash "$0" --inside "$binary"
  else
    sudo -n -E unshare -nm bash "$0" --inside "$binary"
  fi
  exit "$?"
fi
mount --make-rprivate /
ip link set lo up
ip link add discovery0 type dummy
ip addr add 10.235.0.1/24 dev discovery0
ip -6 addr add fd23:235::1/64 dev discovery0 nodad
ip link set discovery0 multicast on
ip link set discovery0 up
ip -j address > "$out/interfaces.json"
ip -j route > "$out/ipv4-routes.json"
ip -6 -j route > "$out/ipv6-routes.json"
nft add table inet discovery_test
nft 'add chain inet discovery_test output { type filter hook output priority -50; policy accept; }'
nft add chain inet discovery_test mdns
nft add rule inet discovery_test output jump mdns
export UC_MDNS_DISCOVERY_ISOLATED=1 UC_TEST_CLOUD_DISCOVERY=1 UC_TEST_ARTIFACTS_DIR="$out"
unset UC_MEMBERSHIP_HISTORY_TEST_RELAY
status=0
timeout --kill-after=5s 180 "$2" --ignored --exact cloud_outage_uses_only_mdns_after_both_processes_restart --nocapture > "$out/discovery.log" 2>&1 || status=$?
nft -j list table inet discovery_test > "$out/firewall.json"
if ((status == 0)) && ! rg -qF 'test result: ok. 1 passed;' "$out/discovery.log"; then status=1; fi
printf '%s\n' "$status" > "$out/exit-code.txt"
cat "$out/discovery.log"
exit "$status"
