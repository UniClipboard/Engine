#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"
artifact_dir="$repo_root/.artifacts/receive-item-progress"
mkdir -p "$artifact_dir"
just cargo test -p uc-engine --features dev-tools --lib --locked --no-run \
  --message-format=json > "$artifact_dir/build.jsonl"
producer="$(python3 - "$artifact_dir/build.jsonl" <<'PY'
import json
import sys
for line in open(sys.argv[1]):
    try:
        value = json.loads(line)
    except json.JSONDecodeError:
        continue
    if value.get('reason') == 'compiler-artifact' and value.get('executable') and value.get('target', {}).get('name') == 'uc_engine':
        print(value['executable'])
PY
)"
test -x "$producer"
export UC_RECEIVE_PROGRESS_PRODUCER="$producer"
export UC_RECEIVE_PROGRESS_EVIDENCE="$artifact_dir/evidence.json"
"$producer" receive_retry --nocapture 2>&1 | tee "$artifact_dir/engine-e2e.log"
just cargo test -p uc-engine-uniffi -p uc-ohos-napi --lib --locked \
  real_receive_evidence -- --ignored --nocapture 2>&1 | tee "$artifact_dir/binding-e2e.log"
python3 - "$artifact_dir" <<'PY'
from pathlib import Path
import hashlib
import json
import sys
root = Path(sys.argv[1])
files = sorted(root.glob('evidence*.json'))
assert files, 'E2E evidence missing'
manifest = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in files}
(root / 'sha256-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
print('Verified evidence:', root / 'sha256-manifest.json')
PY
