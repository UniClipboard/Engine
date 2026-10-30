#!/usr/bin/env bash
# 切片8生产矩阵：复用原隔离harness与冻结旧端，不补丁旧Engine库。
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
source scripts/build-cache/env.sh
umask 077
root="${UC_PROGRESS_CONNECTIVITY_EVIDENCE_ROOT:-$repo/target/progress-connectivity-evidence/runs/$(date -u +%Y%m%dT%H%M%SZ)-$$}"
[[ ! -e "$root" ]] || { printf 'progress evidence directory already exists\n' >&2; exit 2; }
mkdir -p "$root"
head=$(git rev-parse HEAD)
state=clean
if [[ -n $(git status --porcelain) ]]; then state=modified; fi
UC_ENGINE_SOURCE_COMMIT="$head" UC_ENGINE_SOURCE_STATE="$state" UC_HOST_ENGINE_REV="$head" UC_HOST_ANCHOR=progress-current \
  just cargo build --locked -p uc-connectivity-host -p uc-connectivity-relay > "$root/build-current.log" 2>&1
# 原四格旧旧基线及其严格校验仍实际执行，当前bin由原构建器监督还原。
UC_PROGRESS_LEGACY_EVIDENCE_ROOT="$root/old-old" bash scripts/testing/run-progress-legacy-baseline.sh > "$root/old-old.log" 2>&1
python3 - "$root" "$head" "$state" <<'PY'
from pathlib import Path
import hashlib,json,subprocess,sys
root=Path(sys.argv[1]); baseline=json.loads((root/'old-old/sources.json').read_text())
anchors={'main':'5cbca8fc24a278fa7b01ae611bf4b925914bef13','history':'f6f305d9689e4e79e7ab6d0e4921061f9416e4a6'}
assert {row['revision'] for row in baseline['anchors']}==set(anchors.values())
paths=set(subprocess.check_output(['git','ls-files','--cached','--others','--exclude-standard','-z']).decode().split('\0'))
files={name:hashlib.sha256(Path(name).read_bytes()).hexdigest() for name in sorted(paths) if name and Path(name).is_file()}
(root/'sources.json').write_text(json.dumps({'head':sys.argv[2],'state':sys.argv[3],'anchors':anchors,
 'current_binary_sha256':hashlib.sha256(Path('target/debug/uc-connectivity-host').read_bytes()).hexdigest(),
 'driver_files_sha256':files},indent=2)+'\n')
PY
run() {
  local label=$1; shift
  UC_CONNECTION_RECOVERY_EVIDENCE="$root/$label" \
    bash scripts/testing/run-connection-recovery-e2e.sh --suite network --prebuilt --repeat 1 --progress-baseline "$@" > "$root/$label.log" 2>&1
  printf '%s\n' "$label" >> "$root/completed-labels.txt"
}
run new-new-direct --mode direct --case E02-file-transfer
run new-new-relay --mode relay --case E02-file-transfer
for anchor in main history; do
  if [[ "$anchor" == main ]]; then old="$root/old-old/frozen-main/uc-connectivity-host"
  else old="$repo/target/upgrade-anchors/f6f305d9689e4e79e7ab6d0e4921061f9416e4a6/bin/uc-connectivity-host"; fi
  for mode in direct relay; do
    for side in 0 1; do
      run "$anchor-$mode-side$side" --mode "$mode" --case P00-prepaired-upgrade --prepaired-host "$old" --upgrade-side "$side" --blob-baseline
    done
  done
done
node scripts/testing/verify-progress-connectivity.mjs "$root"
