#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"
artifact_dir="$repo_root/.artifacts/held-redelivery"
mkdir -p "$artifact_dir"
rm -f "$artifact_dir/manifest.json"
run_scenario() {
  local label="$1" scenario="$2"
  export UC_HELD_REDELIVERY_EVIDENCE="$artifact_dir/evidence-$label.json"
  rm -f "$UC_HELD_REDELIVERY_EVIDENCE"
  just cargo test -p uc-engine --features dev-tools --lib --locked "$scenario" \
    -- --nocapture 2>&1 | tee "$artifact_dir/candidate-$label.log"
}
run_scenario both real_engine_reopened_completed_redelivery_reports_sender_completion_without_download
run_scenario sender real_engine_reopened_sender_completed_redelivery_gets_fresh_completion_without_download
python3 - "$artifact_dir" <<'PY'
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import sys
root = Path(sys.argv[1])
files = []
for label in ['both', 'sender']:
    evidence = json.loads((root / f'evidence-{label}.json').read_text())
    assert evidence['redeliveries'] == 2
    assert evidence['reopened']
    assert evidence['receiver_reopened'] == (label == 'both')
    assert evidence['sender_completed'] == 3 and evidence['sender_settled']
    assert evidence['receiver_attempt_unchanged'] and evidence['saved_files_unchanged']
    assert evidence['exports_before_repeat'] == evidence['exports_after_repeat'] == 4
    files.extend([f'candidate-{label}.log', f'evidence-{label}.json'])
receipt = {
    'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'source_state': 'modified' if subprocess.check_output(['git', 'status', '--porcelain']) else 'clean',
    'diff_sha256': hashlib.sha256(subprocess.check_output(['git', 'diff', 'HEAD'])).hexdigest(),
    'lock_sha256': hashlib.sha256(Path('Cargo.lock').read_bytes()).hexdigest(),
    'runner_sha256': hashlib.sha256(Path('scripts/testing/run-held-redelivery-e2e.sh').read_bytes()).hexdigest(),
    'platform': platform.platform(),
    'command': 'bash scripts/testing/run-held-redelivery-e2e.sh',
    'features': ['dev-tools'],
    'files': {name: hashlib.sha256((root / name).read_bytes()).hexdigest() for name in files},
    'device_matrix': 'skipped',
}
(root / 'manifest.json').write_text(json.dumps(receipt, indent=2) + '\n')
print(root / 'manifest.json')
PY
