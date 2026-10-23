#!/usr/bin/env bash
# 第七切片默认必需网络门禁；旧库、宿主、隔离拓扑和资料清理由既有工具负责。
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
source scripts/build-cache/env.sh
matrix_root="$repo/target/blob-connectivity-evidence"
root="${UC_BLOB_CONNECTIVITY_EVIDENCE_ROOT:-$matrix_root/runs/$(date -u +%Y%m%dT%H%M%SZ)-$$}"
frozen_main_rev=a461b89444efd60081c7ef63f927946f9dfc072d
historical_anchor=a07
for tool in node python3 ip nft just rg; do command -v "$tool" >/dev/null; done
# 每次调用使用新目录，首次失败的工件不能被后续重跑覆盖。
if [[ -e "$root" ]]; then
  printf 'blob matrix evidence directory already exists\n' >&2
  exit 2
fi
mkdir -p "$(dirname "$root")"
mkdir -m 700 "$root"
current_rev=$(git rev-parse HEAD)
UC_HOST_ENGINE_REV="$current_rev" UC_HOST_ANCHOR=blob-current \
  just cargo build --locked -p uc-connectivity-host -p uc-connectivity-relay > "$root/build-current.log" 2>&1
UC_CONNECTIVITY_BASELINE_REV="$frozen_main_rev" \
UC_CONNECTIVITY_BASELINE_ROOT="$matrix_root/frozen-main" \
UC_CONNECTIVITY_BASELINE_LABEL=blob-baseline \
  bash scripts/testing/build-peer-reachability-main.sh > "$root/build-frozen-main.log" 2>&1
bash scripts/testing/build-upgrade-anchors.sh "$historical_anchor" > "$root/build-history.log" 2>&1
rc15_rev=$(node -e 'const a=JSON.parse(require("fs").readFileSync("tests/upgrade-matrix/anchors.json","utf8")).anchors.find(a=>a.id===process.argv[1]);if(!a)process.exit(2);process.stdout.write(a.engine_rev)' "$historical_anchor")
main_host="$matrix_root/frozen-main/uc-connectivity-host"
rc15_host="$repo/target/upgrade-anchors/$rc15_rev/bin/uc-connectivity-host"
run() {
  local label=$1; shift
  UC_CONNECTION_RECOVERY_EVIDENCE="$root/$label" \
    bash scripts/testing/run-connection-recovery-e2e.sh --suite network --prebuilt --repeat 1 "$@" \
    > "$root/$label.log" 2>&1
  printf '%s\n' "$label" >> "$root/completed-labels.txt"
}
run new-new-direct --mode direct --case E02-file-transfer
run new-new-relay --mode relay --case E02-file-transfer
for anchor in main rc15; do
  if [[ "$anchor" == main ]]; then old="$main_host"; else old="$rc15_host"; fi
  for topology in direct relay; do
    for side in 0 1; do
      run "$anchor-$topology-side$side" --mode "$topology" --case P00-prepaired-upgrade \
        --prepaired-host "$old" --upgrade-side "$side" --blob-baseline
    done
  done
done
python3 - "$root" "$frozen_main_rev" "$rc15_rev" <<'PY'
from pathlib import Path
import hashlib,json,subprocess,sys,tomllib

root=Path(sys.argv[1])
head=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
labels=(root/'completed-labels.txt').read_text().splitlines()
expected={'new-new-direct','new-new-relay'} | {
    f'{anchor}-{mode}-side{side}'
    for anchor in ['main','rc15'] for mode in ['direct','relay'] for side in [0,1]
}
assert len(labels)==10 and set(labels)==expected, 'all ten independent cells are required'
cases=[]
for label in labels:
    files=list((root/label).glob('*.json'))
    assert len(files)==1, 'each cell must retain its real runner evidence'
    result=json.loads(files[0].read_text())
    assert result['failed'] is False and result['cleaned'] is True and result['plaintext_clean'] is True
    records={row['id']:row for row in result['records']}
    legacy=not label.startswith('new-new-')
    row=records['P00-prepaired-upgrade' if legacy else 'E02-file-transfer']
    assert row['outcome']=='passed'
    proof=row['proof']
    current_hosts=[host for host in proof['hosts'] if host['anchor']=='blob-current']
    assert len(current_hosts)==(1 if legacy else 2)
    assert all(host['engine_rev']==head for host in current_hosts), 'stale current host binary'
    assert proof['negotiated_protocol']=='blob' and proof['actual_handshake_observed'] is True
    mode='relay' if '-relay' in label else 'direct'
    assert proof['paths'] and all(path['selected_path']==mode for path in proof['paths'])
    if legacy:
        original=records['P00-prepaired-upgrade-old-pairing']
        assert original['outcome']=='passed' and original['proof']['exact_bidirectional_files'] is True
        assert original['proof']['old_pairing'] is True and original['proof']['old_profile_reopen'] is True
        assert proof['original_received_files_readable'] is True and proof['exact_bidirectional_files'] is True
        assert proof['independent_original_copies'] is True and len(proof['retained'])==2
        assert all(node[key] is True for node in proof['retained']
                   for key in ['identity','transport_identity','members','content'])
        old_revision=sys.argv[2] if label.startswith('main-') else sys.argv[3]
        assert any(host['engine_rev']==old_revision for host in proof['hosts'])
    else:
        assert proof['exact_bytes_verified'] is True and proof['direction_count']==2
    cases.append({'case':label,'passed':True,'topology':mode,'exact_bidirectional_files':True,
                  'original_received_files_readable':legacy,'actual_blob_handshake_observed':True,
                  'cleaned':True,'plaintext_clean':True,
                  'binaries':result['binaries'],
                  'evidence_sha256':hashlib.sha256(files[0].read_bytes()).hexdigest()})
packages=tomllib.loads(Path('Cargo.lock').read_text())['package']
sdk=next(package for package in packages if package['name']=='iroh-blobs')
summary={'head':head,
         'frozen_main':sys.argv[2],'historical_engine':sys.argv[3],
         'sdk_version':sdk['version'],'sdk_revision':sdk.get('source','').partition('#')[2],
         'completed_cases':len(cases),'cases':cases}
(root/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print('blob compatibility matrix: ten cells passed')
PY
