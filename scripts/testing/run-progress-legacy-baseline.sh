#!/usr/bin/env bash
# 切片8的固定旧端旧旧内容基线，复用既有真实配对/重开/文件读回和隔离拓扑。
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
umask 077
matrix_root="$repo/target/progress-legacy-evidence"
root="${UC_PROGRESS_LEGACY_EVIDENCE_ROOT:-$matrix_root/runs/$(date -u +%Y%m%dT%H%M%SZ)-$$}"
main_rev=5cbca8fc24a278fa7b01ae611bf4b925914bef13
history_rev=f6f305d9689e4e79e7ab6d0e4921061f9416e4a6
selected=
if (($#)); then
  [[ "$#" == 2 && "$1" == --case ]] || { printf 'usage: %s [--case main-direct|main-relay|history-direct|history-relay]\n' "$0" >&2; exit 2; }
  selected=$2
  case "$selected" in
    main-direct|main-relay|history-direct|history-relay) ;;
    *) printf 'unknown fixed legacy cell\n' >&2; exit 2 ;;
  esac
fi
[[ ! -e "$root" ]] || { printf 'legacy evidence directory already exists\n' >&2; exit 2; }
mkdir -p "$root"
python3 - "$root" "$main_rev" "$history_rev" "$selected" <<'PY'
import hashlib,json,subprocess,sys
from pathlib import Path
root=Path(sys.argv[1]); sources=[]
for rev,path in [(sys.argv[2],'crates/uc-sync-protocol/src/transfer_progress.rs'),
                 (sys.argv[3],'crates/uc-infra/src/network/iroh/transfer_progress_wire.rs')]:
    source=subprocess.check_output(['git','show',f'{rev}:{path}'])
    digest=hashlib.sha256(source).hexdigest()
    assert digest=='4f8130692441496a436be085f09691cd60236b9c3cc509803a56c504c6d61d0b', 'frozen legacy codec changed'
    sources.append({'revision':rev,'codec_path':path,'codec_sha256':digest})
anchors=json.loads(Path('tests/upgrade-matrix/anchors.json').read_text())['anchors']
assert next(a for a in anchors if a['id']=='a07')['engine_rev']==sys.argv[3]
inputs=['Cargo.lock','scripts/testing/run-progress-legacy-baseline.sh',
        'scripts/testing/run-connection-recovery-e2e.sh','scripts/testing/connection-recovery-network.mjs',
        'crates/uc-sync-protocol/tests/fixtures/transfer-progress-v1.txt']
(root/'sources.json').write_text(json.dumps({
    'driver_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
    'driver_tree':subprocess.check_output(['git','rev-parse','HEAD^{tree}'],text=True).strip(),
    'driver_state':'modified' if subprocess.check_output(['git','status','--porcelain']) else 'clean',
    'selected_case':sys.argv[4] or None,
    'driver_inputs_sha256':{p:hashlib.sha256(Path(p).read_bytes()).hexdigest() for p in inputs},
    'anchors':sources},indent=2)+'\n')
PY
if [[ -z "$selected" || "$selected" == main-* ]]; then
  UC_CONNECTIVITY_BASELINE_REV="$main_rev" UC_CONNECTIVITY_BASELINE_ROOT="$root/frozen-main" \
  UC_CONNECTIVITY_BASELINE_LABEL=progress-legacy-main \
    bash scripts/testing/build-peer-reachability-main.sh > "$root/build-main.log" 2>&1
fi
if [[ -z "$selected" || "$selected" == history-* ]]; then
  CARGO_TARGET_DIR="$repo/target" bash scripts/testing/build-upgrade-anchors.sh a07 > "$root/build-history.log" 2>&1
  cp "target/upgrade-anchors/$history_rev/host.json" "$root/history-host.json"
fi
just cargo build --locked -p uc-connectivity-relay > "$root/build-relay.log" 2>&1
for anchor in main history; do
  if [[ "$anchor" == main ]]; then old="$root/frozen-main/uc-connectivity-host"
  else old="$repo/target/upgrade-anchors/$history_rev/bin/uc-connectivity-host"; fi
  for topology in direct relay; do
    label="$anchor-$topology"
    if [[ -n "$selected" && "$label" != "$selected" ]]; then continue; fi
    UC_CONNECTION_RECOVERY_EVIDENCE="$root/$label" \
      bash scripts/testing/run-connection-recovery-e2e.sh --suite network --prebuilt --repeat 1 \
        --mode "$topology" --case P00-prepaired-upgrade --host "$old" \
        --prepaired-host "$old" --upgrade-side 0 --blob-baseline --old-pair-only \
      > "$root/$label.log" 2>&1
    printf '%s\n' "$label" >> "$root/completed-labels.txt"
  done
done
python3 - "$root" "$main_rev" "$history_rev" "$selected" <<'PY'
import hashlib,json,sys
from pathlib import Path
root=Path(sys.argv[1]); labels=(root/'completed-labels.txt').read_text().splitlines()
all_cases={'main-direct','main-relay','history-direct','history-relay'}
expected={sys.argv[4]} if sys.argv[4] else all_cases
assert len(labels)==len(expected) and set(labels)==expected
cases=[]
for label in labels:
    files=list((root/label).glob('*.json')); assert len(files)==1
    result=json.loads(files[0].read_text())
    assert result['failed'] is False and result['cleaned'] is True and result['plaintext_clean'] is True
    assert result['old_pair_only'] is True
    records={r['id']:r for r in result['records']}
    assert set(records)=={'P00-prepaired-upgrade-old-pairing'}, 'an old-only cell executed an upgrade'
    row=records['P00-prepaired-upgrade-old-pairing']; assert row['outcome']=='passed'
    proof=row['proof']; mode=label.split('-')[1]; rev=sys.argv[2] if label.startswith('main-') else sys.argv[3]
    assert all(proof[key] is True for key in ['old_pairing','old_profile_reopen','exact_bidirectional_content','exact_bidirectional_files'])
    assert len(proof['hosts'])==2 and all(host['engine_rev']==rev for host in proof['hosts'])
    assert len(proof['paths'])==2 and all(path['selected_path']==mode for path in proof['paths'])
    if mode=='relay':
        faults=[f for f in result['faults'] if f['action']=='direct_paths_blocked']
        assert {f['node'] for f in faults}=={'A','B'}
        assert all(f['ipv4_drop_packets']>0 and f['ipv6_drop_packets']>0 for f in faults)
    cases.append({'case':label,'engine_revision':rev,'passed':True,'paths':proof['paths'],
        'binaries':result['binaries'],'evidence_sha256':hashlib.sha256(files[0].read_bytes()).hexdigest()})
(root/'summary.json').write_text(json.dumps({'scope':'selected-fixed-old-pair-reproduction' if sys.argv[4] else 'fixed-old-pairs-before-progress-migration',
    'full_matrix':not bool(sys.argv[4]),
    'skipped_cases':sorted(all_cases-expected),
    'sources':json.loads((root/'sources.json').read_text()),'cases':cases},indent=2)+'\n')
if sys.argv[4]:
    print(f'legacy progress reproduction: {sys.argv[4]} exact-content cell passed; full matrix not run')
else:
    print('legacy progress baseline: four exact-content cells passed')
PY
